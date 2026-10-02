//! Bounded capability probes. Version output contributes to cache identity.
use crate::{
    Result,
    environment::Environment,
    error::Error,
    model::Tool,
    process::{self, Cancellation},
};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// Probe each distinct tool once, concurrently; each probe is bounded to 10 s.
/// Unknown names fail before any process starts. When several probes fail, the
/// first failure in the order the names were supplied is reported.
pub fn probe(
    tools: &BTreeMap<String, Tool>,
    names: impl IntoIterator<Item = String>,
    root: &Path,
    source: &Environment,
    cancellation: &Cancellation,
) -> Result<BTreeMap<String, String>> {
    probe_all(tools, names, root, source, cancellation)?
        .into_iter()
        .map(|(name, output)| Ok((name, output?)))
        .collect()
}

/// Like [`probe`], but keeps each tool's outcome so callers can report every
/// failing tool. Only an unknown name fails the call as a whole.
pub fn probe_all(
    tools: &BTreeMap<String, Tool>,
    names: impl IntoIterator<Item = String>,
    root: &Path,
    source: &Environment,
    cancellation: &Cancellation,
) -> Result<Vec<(String, Result<String>)>> {
    let environment: Environment = ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"]
        .iter()
        .filter_map(|name| source.get(*name).map(|v| (name.to_string(), v.clone())))
        .collect();

    let mut declared: Vec<(String, &Tool)> = Vec::new();
    for name in names {
        if declared.iter().any(|(seen, _)| *seen == name) {
            continue;
        }
        let tool = tools
            .get(&name)
            .ok_or_else(|| format!("Unknown capability {name}"))?;
        declared.push((name, tool));
    }

    let outputs: Vec<Result<String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = declared
            .iter()
            .map(|(name, tool)| {
                let environment = &environment;
                scope.spawn(move || probe_one(name, tool, root, environment, cancellation))
            })
            .collect();
        workers
            .into_iter()
            .zip(&declared)
            .map(|(worker, (name, _))| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err(format!("{name}: version probe panicked").into()))
            })
            .collect()
    });

    Ok(declared
        .into_iter()
        .zip(outputs)
        .map(|((name, _), output)| (name, output))
        .collect())
}

fn probe_one(
    name: &str,
    tool: &Tool,
    root: &Path,
    environment: &Environment,
    cancellation: &Cancellation,
) -> Result<String> {
    let (code, output) = process::capture(
        &tool.command,
        root,
        environment,
        Some(Duration::from_secs(10)),
        cancellation,
    )
    .map_err(|error| Error::from(format!("{name}: unable to execute version probe: {error}")))?;
    if code != 0 {
        return Err(format!("{name}: version probe failed ({code})").into());
    }
    if tool
        .version_contains
        .as_ref()
        .is_some_and(|required| !output.contains(required))
    {
        return Err(format!("{name}: installed version does not match version_contains").into());
    }
    Ok(output.trim().into())
}
