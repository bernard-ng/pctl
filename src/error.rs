use std::{
    io,
    path::{Path, PathBuf},
};
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

    /// Every problem found in one validation pass, so they can be fixed together.
    #[error("{}", describe(.0))]
    Validation(Vec<String>),
}

fn describe(problems: &[String]) -> String {
    match problems {
        [only] => only.clone(),
        _ => {
            let mut text = format!("{} problems found:", problems.len());
            for problem in problems {
                text.push_str("\n  - ");
                text.push_str(problem);
            }
            text
        }
    }
}

impl Error {
    /// `map_err` adapter naming the failed action and path. The message is only
    /// formatted when an error actually occurs.
    pub fn io<'a>(action: &'static str, path: &'a Path) -> impl FnOnce(io::Error) -> Self + 'a {
        move |source| Self::Io {
            context: format!("{action} {}", path.display()),
            source,
        }
    }
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

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Message(value.to_string())
    }
}
