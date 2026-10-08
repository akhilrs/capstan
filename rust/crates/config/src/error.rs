use std::fmt;

/// Why a configuration did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// One of the Node loader's own `ConfigError` texts, worded exactly as it words it.
    Invalid(String),
    /// The text is not TOML the Rust parser reads (or holds something this port defers). The Node loader words this
    /// with a line and column the Rust parser cannot reproduce, so a caller hands the command to Node.
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
