use thiserror::Error;

#[derive(Error, Debug)]
pub enum MakoError {
    #[error("VM error: {0}")]
    Vm(String),

    #[error("VM not running")]
    VmNotRunning,

    #[error("VM already running")]
    VmAlreadyRunning,

    #[error("vsock connection failed: {0}")]
    Vsock(String),

    #[error("Docker engine error: {0}")]
    Docker(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("File sharing error: {0}")]
    FileShare(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, MakoError>;
