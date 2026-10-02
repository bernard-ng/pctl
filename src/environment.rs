//! Explicit environment resolution and consumer-scoped injection.
use crate::{
    Result,
    error::Error,
    model::{Manifest, PlannedTask, Visibility},
};
use std::{collections::BTreeMap, path::Path};

pub type Environment = BTreeMap<String, String>;

/// Ambient variables every task process receives so that ordinary programs work.
/// They describe the host, not the task, so fingerprints ignore them (tool
/// identity comes from `requires`); a task opts one back in via `pass_environment`.
pub const OS_VARIABLES: [&str; 9] = [
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "SYSTEMROOT",
    "WINDIR",
];

pub fn load(file: Option<&Path>) -> Result<Environment> {
    let mut result = match file {
        Some(file) => parse(
            &std::fs::read_to_string(file)
                .map_err(Error::io("Cannot read environment file", file))?,
        )?,
        None => Environment::new(),
    };
    result.extend(
        std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    );
    Ok(result)
}

/// Parse dotenv text: `NAME=value` lines, an optional leading `export`, `#`
/// comment lines, and trailing ` # comments` after values. Values may be wrapped
/// in single or double quotes, which are removed; no escapes or expansion are
/// applied. Diagnostics name the line but never include values.
pub fn parse(source: &str) -> Result<Environment> {
    let mut result = Environment::new();
    for (index, line) in source.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = match line.strip_prefix("export") {
            Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
            _ => line,
        };
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("Invalid environment assignment on line {number}"))?;
        let name = name.trim();
        if !valid_name(name) {
            return Err(format!("Invalid environment name on line {number}").into());
        }
        let value = parse_value(value.trim(), number)?;
        if result.insert(name.into(), value.into()).is_some() {
            return Err(format!("Duplicate environment name {name}").into());
        }
    }
    Ok(result)
}

fn parse_value(value: &str, line: usize) -> Result<&str> {
    let Some(quote) = value.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
        // An unquoted value ends at whitespace followed by `#`.
        let end = value
            .match_indices('#')
            .find(|(at, _)| value[..*at].ends_with([' ', '\t']))
            .map_or(value.len(), |(at, _)| at);
        return Ok(value[..end].trim_end());
    };
    let inner = &value[1..];
    if let Some(close) = inner.find(quote) {
        let rest = inner[close + 1..].trim_start();
        if rest.is_empty() || rest.starts_with('#') {
            return Ok(&inner[..close]);
        }
    }
    // Historical leniency: a value that merely ends with the opening quote keeps
    // everything between the outer quotes, e.g. `"a"b"` is `a"b`.
    if inner.ends_with(quote) {
        return Ok(&inner[..inner.len() - 1]);
    }
    Err(format!("Unclosed quote on line {line}").into())
}

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Build the environment a task's process receives. Precedence for a declared
/// variable the task consumes: the task's own override, then the process
/// environment, then the profile's default. Profile values for variables the task
/// does not consume are ignored, so one profile can serve every task; an explicit
/// task override of a variable it does not consume is an error.
pub fn resolve(
    manifest: &Manifest,
    task: &PlannedTask,
    profile: &str,
    source: &Environment,
) -> Result<Environment> {
    let consumes = |name: &str| {
        manifest.variables.get(name).is_none_or(|variable| {
            variable
                .consumers
                .iter()
                .any(|consumer| task.consumers.contains(consumer))
        })
    };
    let defaults = manifest.profiles.get(profile).map(|p| &p.environment);
    let mut environment = Environment::new();
    // OS facilities needed by subprocesses; application values are explicitly selected.
    for key in OS_VARIABLES {
        if !manifest.variables.contains_key(key)
            && let Some(value) = source.get(key)
        {
            environment.insert(key.into(), value.clone());
        }
    }
    for key in &task.pass_environment {
        if let Some(value) = source.get(key) {
            environment.insert(key.clone(), value.clone());
        }
    }
    // Profile entries that are not declared variables have no consumers to
    // scope them, so they apply to every task.
    for (name, value) in defaults.into_iter().flatten() {
        if !manifest.variables.contains_key(name) {
            environment.insert(name.clone(), value.clone());
        }
    }
    for (name, variable) in &manifest.variables {
        if !consumes(name) {
            continue;
        }
        let value = task
            .environment
            .get(name)
            .or_else(|| source.get(name))
            .or_else(|| defaults.and_then(|defaults| defaults.get(name)));
        if let Some(value) = value {
            if !variable.kind.accepts(value, &variable.values) {
                return Err(format!("{name}: invalid value for task {}", task.id).into());
            }
            environment.insert(name.clone(), value.clone());
        } else if variable.required_in.iter().any(|p| p == profile) {
            return Err(
                format!("{name}: required by task {} in profile {profile}", task.id).into(),
            );
        }
    }
    for (name, value) in &task.environment {
        if !consumes(name) {
            return Err(format!("{}: not a consumer of {name}", task.id).into());
        }
        environment.insert(name.clone(), value.clone());
    }
    Ok(environment)
}

pub fn secrets(manifest: &Manifest, source: &Environment) -> Vec<String> {
    manifest
        .variables
        .iter()
        .filter(|(_, v)| v.visibility == Visibility::Secret)
        .filter_map(|(name, _)| source.get(name))
        .filter(|value| !value.is_empty())
        .cloned()
        .collect()
}
