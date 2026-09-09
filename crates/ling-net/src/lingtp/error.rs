#[derive(Debug, thiserror::Error)]
pub enum LingtpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("lingtp protocol error: {0}")]
    Protocol(String),
    #[error("lingtp crypto error: {0}")]
    Crypto(&'static str),
    #[error(
        "host key for {0} does not match the one on file in .lingtp/known_hosts — \
         refusing to continue (possible MITM); remove the stale entry only if you're sure"
    )]
    HostKeyMismatch(String),
    #[error("lingtp envelope (de)serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
