use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("semtech udp server_runtime error: {0}")]
    ServerRuntime(#[from] semtech_udp::server_runtime::Error),
    #[error("semtech udp client_runtime error: {0}")]
    ClientRuntime(#[from] semtech_udp::client_runtime::Error),
    #[error("error parsing socket address: {0}")]
    AddrParse(#[from] std::net::AddrParseError),
    #[error("join error: {0}")]
    Join(#[from] tokio::task::JoinError),
}
