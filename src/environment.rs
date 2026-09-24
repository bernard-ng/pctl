//! Explicit environment resolution and consumer-scoped injection.
use crate::{
    Result,
    model::{Manifest, PlannedTask, Visibility},
};
use std::{collections::BTreeMap, path::Path};

pub type Environment = BTreeMap<String, String>;

pub fn load(file: Option<&Path>) -> Result<Environment> {
    let mut result = Environment::new();
    if let Some(file) = file {
        let source = std::fs::read_to_string(file).map_err(|_| "Cannot read environment file")?;
        for (index, line) in source.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (name, value) = line.split_once('=').ok_or_else(|| {
                crate::error::Error::from(format!(
                    "Invalid environment assignment on line {}",
                    index + 1
                ))
            })?;
            let name = name.trim();
            if !valid_name(name) {
                return Err(format!("Invalid environment name on line {}", index + 1).into());
            }
            let value = value.trim();
            let value = if value.starts_with('"') || value.starts_with('\'') {
                if value.len() < 2 || value.chars().next() != value.chars().last() {
                    return Err(format!("Unclosed quote on line {}", index + 1).into());
                }
                &value[1..value.len() - 1]
            } else {
                value
            };
            if result.insert(name.into(), value.into()).is_some() {
                return Err(format!("Duplicate environment name {name}").into());
            }
        }
    }
    result.extend(
        std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    );
    Ok(result)
}

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn resolve(
    manifest: &Manifest,
    task: &PlannedTask,
    profile: &str,
    source: &Environment,
) -> Result<Environment> {
    let mut environment = Environment::new();
    // OS facilities needed by subprocesses; application values are explicitly selected.
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LC_ALL",
        "SYSTEMROOT",
        "WINDIR",
    ] {
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
    for (name, variable) in &manifest.variables {
        if !variable
            .consumers
            .iter()
            .any(|consumer| task.consumers.contains(consumer))
        {
            continue;
        }
        let value = task.environment.get(name).or_else(|| source.get(name));
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
        if let Some(variable) = manifest.variables.get(name)
            && !variable
                .consumers
                .iter()
                .any(|c| task.consumers.contains(c))
        {
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
