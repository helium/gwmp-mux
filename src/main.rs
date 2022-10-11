use semtech_udp::{
    client_runtime::{self, Event as ClientEvent},
    push_data,
    server_runtime::{self, Event as ServerEvent, UdpRuntime},
    tx_ack, MacAddress,
};
use slog::{self, debug, error, info, o, warn, Drain, Logger};
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::str::FromStr;
use structopt::StructOpt;
use tokio::{io::AsyncReadExt, signal, time::Duration};

mod gwmp_mux;
use gwmp_mux::*;

mod errors;
use errors::Error;

pub type Result<T = ()> = std::result::Result<T, Error>;

#[derive(Debug, StructOpt)]
#[structopt(name = "gwmp-mux", about = "Multiplexer for Semtech's GWMP over UDP")]
pub struct Opt {
    /// port to host the service on
    #[structopt(long, default_value = "1681")]
    pub host: u16,
    /// addresses to be clients to (eg: 127.0.0.1:1680)
    /// WARNING: all addresses will receive all ACKs for transmits
    #[structopt(long, default_value = "127.0.0.1:1680")]
    pub client: Vec<String>,

    /// Log level to show (default info)
    #[structopt(parse(from_str = parse_log), default_value = "info")]
    pub log_level: slog::Level,

    /// Disable timestamp from logs
    #[structopt(long)]
    pub disable_timestamp: bool,
}

fn main() {
    let cli = Opt::from_args();
    let logger = mk_logger(cli.log_level, cli.disable_timestamp);
    let scope_guard = slog_scope::set_global_logger(logger);
    let logger = slog_scope::logger().new(o!());
    slog_stdlog::init().unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let (shutdown_trigger, shutdown_signal) = triggered::trigger();

        let handle = tokio::spawn(async move {
            let logger = slog_scope::logger().new(o!());
            if let Err(e) = host_and_mux(cli, shutdown_signal).await {
                error!(&logger, "host_and_mux error: {e}")
            }
        });

        watch_for_shutdown().await;
        info!(&logger, "Triggering gwmp-mux shutdown");
        shutdown_trigger.trigger();
        handle
            .await
            .expect("Error awaiting host_and_mux_sup shutdown");
        info!(&logger, "Shutdown complete");
    });
    runtime.shutdown_timeout(Duration::from_secs(0));
    drop(scope_guard);
}

async fn watch_for_shutdown() {
    let mut in_buf = [0u8; 64];
    let mut stdin = tokio::io::stdin();
    loop {
        tokio::select!(
            _ = signal::ctrl_c() => return,
            read = stdin.read(&mut in_buf) => if let Ok(0) = read { return },
        )
    }
}

/// An empty timestamp function for when timestamp should not be included in
/// the output.
fn timestamp_none(_io: &mut dyn io::Write) -> io::Result<()> {
    Ok(())
}

fn mk_logger(log_level: slog::Level, disable_timestamp: bool) -> Logger {
    let decorator = slog_term::PlainDecorator::new(io::stdout());
    let timestamp = if !disable_timestamp {
        slog_term::timestamp_local
    } else {
        timestamp_none
    };
    let drain = slog_term::FullFormat::new(decorator)
        .use_custom_timestamp(timestamp)
        .build()
        .fuse();
    let async_drain = slog_async::Async::new(drain)
        .build()
        .filter_level(log_level)
        .fuse();
    slog::Logger::root(async_drain, o!())
}

fn parse_log(src: &str) -> slog::Level {
    src.parse().unwrap()
}
