use std::fmt;

#[derive(Debug)]
pub enum CacheError {
    /// Another prune, clear or manifest write holds the store lock.
    Busy,
    /// Agents are active in the project, so the store is not cleared without `force`.
    NotIdle,
    Invalid(String),
    Io(String),
}

pub type Result<T> = std::result::Result<T, CacheError>;

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(
                f,
                "the cache store is busy: another prune or clear is running"
            ),
            Self::NotIdle => write!(
                f,
                "agents are active in this project; retry when idle or use --force"
            ),
            Self::Invalid(message) | Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CacheError {}

impl CacheError {
    pub(crate) fn io(context: impl fmt::Display, error: std::io::Error) -> Self {
        Self::Io(format!("{context}: {error}"))
    }
}
