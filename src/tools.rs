//! Bounded capability probes. Version output contributes to cache identity.
use crate::{
    Result,
    environment::Environment,
    model::Tool,
    process::{self, Cancellation},
};
use std::{collections::BTreeMap, path::Path, time::Duration};

pub fn probe(
    tools: &BTreeMap<String, Tool>,
    names: impl IntoIterator<Item = String>,
    root: &Path,
    source: &Environment,
    cancellation: &Cancellation,
) -> Result<BTreeMap<String, String>> {
    let mut versions = BTreeMap::new();
    let environment: Environment = ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"]
        .iter()
        .filter_map(|name| source.get(*name).map(|v| (name.to_string(), v.clone())))
        .collect();
    for name in names {
        if versions.contains_key(&name) {
            continue;
        }
        let tool = tools
            .get(&name)
            .ok_or_else(|| format!("Unknown capability {name}"))?;
        let (code, output) = process::capture(
            &tool.command,
            root,
            &environment,
            Some(Duration::from_secs(10)),
            cancellation,
        )
        .map_err(|_| format!("{name}: unable to execute version probe"))?;
        if code != 0 {
            return Err(format!("{name}: version probe failed ({code})"));
        }
        if tool
            .version_contains
            .as_ref()
            .is_some_and(|required| !output.contains(required))
        {
            return Err(format!(
                "{name}: installed version does not match version_contains"
            ));
        }
        versions.insert(name, output.trim().into());
    }
    Ok(versions)
}
