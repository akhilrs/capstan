use std::fmt;

/// Why a configuration did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// One of the Node loader's own `ConfigError` texts, worded exactly as it words it.
    Invalid(String),
    /// A text the loader could not read. The loader reads TOML itself, the way `smol-toml` does, and words a refusal as an
    /// `Invalid` text with the line and column Node gives, so nothing produces this any more; it stays for callers that
    /// still match on it.
    Parse(ParseError),
    /// The file could not be read for a reason the Node loader would word as the operating system does.
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub detail: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Invalid(text) | ConfigError::Unreadable(text) => f.write_str(text),
            ConfigError::Parse(error) => {
                write!(f, "capstan.toml is not valid TOML ({})", error.detail)
            }
        }
    }
}

impl std::error::Error for ConfigError {}

pub type Result<T> = std::result::Result<T, ConfigError>;

pub(crate) fn invalid<T>(text: impl Into<String>) -> Result<T> {
    Err(ConfigError::Invalid(text.into()))
}
