use crate::Result;
use crate::model::{Manifest, Visibility};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("Cannot open manifest: {e}"))?;
        let root = path
            .parent()
            .ok_or("Manifest has no parent directory")?
            .to_owned();
        let mut visited = BTreeSet::new();
        let manifest = load_fragment(&path, &root, &mut visited)?;
        validate(&manifest)?;
        Ok(Self { root, manifest })
    }
}

fn load_fragment(path: &Path, root: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<Manifest> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve include: {e}"))?;
    if !path.starts_with(root) {
        return Err("Manifest includes must stay inside the project".into());
    }
    if !visited.insert(path.clone()) {
        return Err(format!("Repeated or cyclic include: {}", path.display()));
    }
    let source =
        std::fs::read_to_string(&path).map_err(|e| format!("Cannot read manifest: {e}"))?;
    // Parser errors can include the input line, so keep them out of diagnostics.
    let mut manifest: Manifest = toml::from_str(&source).map_err(|_| {
        format!(
            "Invalid TOML or unknown manifest field in {}",
            path.display()
        )
    })?;
    if manifest.schema_version != 1 {
        return Err(format!("Unsupported schema version in {}", path.display()));
    }
    for include in std::mem::take(&mut manifest.include) {
        let fragment = load_fragment(&root.join(include), root, visited)?;
        for (id, task) in fragment.tasks {
            if manifest.tasks.insert(id.clone(), task).is_some() {
                return Err(format!("Duplicate task: {id}"));
            }
        }
        for (id, variable) in fragment.variables {
            if manifest.variables.insert(id.clone(), variable).is_some() {
                return Err(format!("Duplicate variable: {id}"));
            }
        }
    }
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<()> {
    for (name, variable) in &manifest.variables {
        if variable.browser_exposed && variable.visibility != Visibility::Public {
            return Err(format!(
                "{name}: browser exposure requires public visibility"
            ));
        }
        if variable.consumers.is_empty() {
            return Err(format!("{name}: declare at least one consumer"));
        }
    }
    for (id, task) in &manifest.tasks {
        if task.command.is_empty() && task.depends_on.is_empty() {
            return Err(format!("{id}: task needs a command or dependencies"));
        }
        if task
            .command
            .first()
            .is_some_and(|program| program.is_empty())
        {
            return Err(format!("{id}: command executable cannot be empty"));
        }
        for dependency in &task.depends_on {
            if !manifest.tasks.contains_key(dependency) {
                return Err(format!("{id}: unknown dependency {dependency}"));
            }
        }
        for (name, parameter) in &task.parameters {
            if let Some(default) = &parameter.default
                && !parameter.kind.accepts(default, &parameter.values)
            {
                return Err(format!("{id}: invalid default for parameter {name}"));
            }
        }
        for name in task.environment.keys() {
            if manifest
                .variables
                .get(name)
                .is_some_and(|variable| variable.visibility == Visibility::Secret)
            {
                return Err(format!(
                    "{id}: secret {name} must come from the process environment"
                ));
            }
        }
    }
    let mut done = BTreeSet::new();
    for id in manifest.tasks.keys() {
        check_cycle(id, manifest, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

fn check_cycle(
    id: &str,
    manifest: &Manifest,
    active: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
) -> Result<()> {
    if done.contains(id) {
        return Ok(());
    }
    if !active.insert(id.into()) {
        return Err(format!("Task dependency cycle at {id}"));
    }
    for dependency in &manifest.tasks[id].depends_on {
        check_cycle(dependency, manifest, active, done)?;
    }
    active.remove(id);
    done.insert(id.into());
    Ok(())
}

/// Validate supplied values without returning them in diagnostics.
pub fn validate_environment(
    manifest: &Manifest,
    profile: &str,
    values: &BTreeMap<String, String>,
) -> Result<()> {
    for (name, contract) in &manifest.variables {
        match values.get(name) {
            None if contract.required_in.iter().any(|p| p == profile) => {
                return Err(format!("{name}: required for profile {profile}"));
            }
            Some(value) if !contract.kind.accepts(value, &contract.values) => {
                return Err(format!("{name}: invalid value for declared type"));
            }
            _ => {}
        }
    }
    Ok(())
}
