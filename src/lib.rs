//! Repository operations with explicit planning and execution seams.
pub mod config;
pub mod execution;
pub mod model;
pub mod planning;

pub type Result<T> = std::result::Result<T, String>;
