use std::fmt;

#[derive(Debug)]
pub enum NetError {
    Io(std::io::Error),
    Connect(String),
    Timeout,
    Protocol(String),
    Tls(String),
    Http(String),
    Unsupported(String),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e)             => write!(f, "io: {e}"),
            Self::Connect(msg)      => write!(f, "connect: {msg}"),
            Self::Timeout           => write!(f, "connection timed out"),
            Self::Protocol(msg)     => write!(f, "protocol error: {msg}"),
            Self::Tls(msg)          => write!(f, "tls error: {msg}"),
            Self::Http(msg)         => write!(f, "http error: {msg}"),
            Self::Unsupported(msg)  => write!(f, "unsupported: {msg}"),
        }
    }
}

impl std::error::Error for NetError {}
impl From<std::io::Error> for NetError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
