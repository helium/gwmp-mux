use super::*;

struct Client {
    shutdown_trigger: triggered::Trigger,
    clients: Vec<(
        client_runtime::ClientTx,
        tokio::task::JoinHandle<std::result::Result<(), Error>>,
    )>,
}

impl Client {
    async fn create(
        mac: MacAddress,
        client_tx: &server_runtime::ClientTx,
        client_list: &[String],
    ) -> Result<Client> {
        let logger = slog_scope::logger().new(o!());
        let mut clients = Vec::new();
        let (shutdown_trigger, shutdown_signal) = triggered::trigger();
        for address in client_list {
            let socket = SocketAddr::from_str(address)?;
            let (sender, receiver, udp_runtime) =
                client_runtime::UdpRuntime::new(mac, socket).await?;
            info!(logger, "Connecting to server {socket} on behalf of {mac}",);
            let handle = tokio::spawn(run_client_instance(
                shutdown_signal.clone(),
                udp_runtime,
                client_tx.clone(),
                receiver,
                mac,
            ));
            clients.push((sender, handle));
        }
        Ok(Client {
            shutdown_trigger,
            clients,
        })
    }

    async fn shutdown(self) -> Result {
        let logger = slog_scope::logger().new(o!());
        self.shutdown_trigger.trigger();
        for (_client, handle) in self.clients {
            if let Err(e) = handle.await {
                error!(logger, "Error awaiting client instance shutdown: {e}")
            }
        }
        Ok(())
    }
}

pub async fn host_and_mux(cli: Opt, shutdown_signal: triggered::Listener) -> Result {
    let logger = slog_scope::logger().new(o!());
    let addr = SocketAddr::from(([0, 0, 0, 0], cli.host));
    info!(&logger, "Starting server: {addr}");
    let (mut client_rx, client_tx) = UdpRuntime::new(addr).await?.split();

    let mut mux: HashMap<MacAddress, Client> = HashMap::new();
    info!(&logger, "Ready for clients");

    loop {
        let shutdown_signal = shutdown_signal.clone();
        tokio::select!(
             _ = shutdown_signal => {
                info!(&logger, "Awaiting mux-client instances shutdown");
                for (_, client) in mux.into_iter() {
                    client.shutdown().await?;
                }
                info!(&logger, "host_and_mux shutdown complete");
                return Ok(());
            },
            server_event = client_rx.recv() => {
                let mut to_send = None;
                match server_event {
                    ServerEvent::UnableToParseUdpFrame(error, buf) => {
                        error!(logger, "Semtech UDP Parsing Error: {error}");
                        error!(logger, "UDP data: {buf:?}");
                    }
                    ServerEvent::NewClient((mac, addr)) => {
                        info!(logger, "New packet forwarder client: {mac}, {addr}");
                        let client = Client::create(mac, &client_tx, &cli.client).await?;
                        mux.insert(mac, client);
                    }
                    ServerEvent::UpdateClient((mac, addr)) => {
                        info!(logger, "Mac existed, but IP updated: {mac}, {addr}");
                    }
                    ServerEvent::PacketReceived(rxpk, mac) => {
                        info!(logger, "From {mac} received uplink: {rxpk}");
                        to_send = Some(push_data::Packet::from_rxpk(mac, rxpk));
                    }
                    ServerEvent::StatReceived(stat, mac) => {
                        info!(logger, "From {mac} received stat: {stat:?}");
                        to_send = Some(push_data::Packet::from_stat(mac, stat));
                    }
                    ServerEvent::NoClientWithMac(_packet, mac) => {
                        warn!(logger, "Downlink sent but unknown mac: {mac}");
                    }
                    ServerEvent::ClientDisconnected((mac, addr)) => {
                        info!(logger, "{mac} disconnected from {addr}");
                        if let Some(client) = mux.remove(&mac) {
                            client.shutdown().await?;
                        }
                    }
                }
                if let Some(packet) = to_send {
                    if let Some(client) = mux.get_mut(&packet.gateway_mac) {
                        for (sender, _handle) in &client.clients {
                            debug!(logger, "Forwarding Uplink");
                            let logger = logger.clone();
                            let sender = sender.clone();
                            let packet = packet.clone();
                            tokio::spawn ( async move {
                                if let Err(e) = sender.send(packet).await {
                                    error!(logger, "Error sending uplink: {e}")
                                }
                            });
                        }
                    }
                }
            }
        );
    }
}

async fn run_client_instance(
    shutdown_signal: triggered::Listener,
    udp_runtime: client_runtime::UdpRuntime,
    client_tx: server_runtime::ClientTx,
    receiver: client_runtime::ClientRx,
    mac: MacAddress,
) -> Result {
    let logger = slog_scope::logger().new(o!());

    let runtime = tokio::spawn(udp_runtime.run(shutdown_signal.clone()));
    let receive = tokio::spawn(run_client_instance_handle_downlink(
        mac, receiver, client_tx,
    ));
    tokio::select!(
        _ = shutdown_signal =>
            info!(&logger, "Shutting down client instance {mac}"),
        resp = runtime => if let Err(e) = resp {
            error!(&logger, "Error in client instance {mac} udp_runtime: {e}")
        },
        resp = receive => if let Err(e) = resp {
            error!(&logger, "Error in client instance {mac} receiver: {e}")
        }
    );

    Ok(())
}

async fn run_client_instance_handle_downlink(
    mac: MacAddress,
    mut receiver: client_runtime::ClientRx,
    mut client_tx: server_runtime::ClientTx,
) -> Result {
    let logger = slog_scope::logger().new(o!());

    while let Some(client_event) = receiver.recv().await {
        match client_event {
            ClientEvent::DownlinkRequest(downlink_request) => {
                let prepared_send =
                    client_tx.prepare_downlink(Some(downlink_request.txpk().clone()), mac);
                let logger = logger.clone();
                tokio::spawn(async move {
                    if let Err(e) = match prepared_send
                        .dispatch(Some(Duration::from_secs(15)))
                        .await
                    {
                        Err(server_runtime::Error::Ack(e)) => {
                            error!(&logger, "Error Downlinking to {mac}: {:?}", e);
                            downlink_request.nack(e).await
                        }
                        Err(server_runtime::Error::SendTimeout) => {
                            warn!(
                                &logger,
                                "Gateway {mac} did not ACK or NACK. Packet forward may not be connected?"
                            );
                            downlink_request.nack(tx_ack::Error::SendFail).await
                        }
                        Ok(tmst) => {
                            if let Some(tmst) = tmst {
                                info!(&logger, "Downlink to {mac} successful @ tmst: {tmst}");
                            } else {
                                info!(&logger, "Downlink to {mac} successful");
                            }
                            downlink_request.ack().await
                        }
                        Err(e) => {
                            error!(&logger, "Unhandled downlink error: {:?}", e);
                            Ok(())
                        }
                    } {
                        error!(&logger, "Error sending downlink to {mac}: {e}");
                    }
                });
            }
            ClientEvent::UnableToParseUdpFrame(parse_error, buffer) => {
                error!(
                    &logger,
                    "Error parsing frame from {mac}: {parse_error}, {buffer:?}"
                );
            }
            ClientEvent::LostConnection => {
                warn!(
                    &logger,
                    "Lost connection to GWMP client {mac}. Dropping frames."
                )
            }
            ClientEvent::Reconnected => {
                warn!(&logger, "Reconnected to GWMP client {mac}")
            }
        }
    }
    Ok(())
}
