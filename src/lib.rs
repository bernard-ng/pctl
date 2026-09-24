//! Repository operations with explicit planning and execution seams.
pub mod cache;
pub mod compose;
pub mod config;
pub mod environment;
pub mod error;
pub mod execution;
pub mod generation;
pub mod model;
pub mod paths;
pub mod planning;
pub mod process;
pub mod reporting;
pub mod tools;

pub type Result<T> = std::result::Result<T, error::Error>;
