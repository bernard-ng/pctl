use std::{io, path::PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },

    #[error("Invalid TOML in {path}: {source}")]
    InvalidToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Invalid manifest in {path} at {location}")]
    InvalidManifest { path: PathBuf, location: String },
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Message(value.to_string())
    }
}

impl From<toml::de::Error> for Error {
    fn from(value: toml::de::Error) -> Self {
        Self::Message(value.to_string())
    }
}
