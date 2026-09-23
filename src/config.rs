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
        for (id, profile) in fragment.profiles {
            if manifest.profiles.insert(id.clone(), profile).is_some() {
                return Err(format!("Duplicate profile: {id}"));
            }
        }
        for (id, tool) in fragment.tools {
            if manifest.tools.insert(id.clone(), tool).is_some() {
                return Err(format!("Duplicate tool: {id}"));
            }
        }
    }
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<()> {
    for (id, tool) in &manifest.tools {
        if tool.command.is_empty() || tool.command[0].is_empty() {
            return Err(format!("{id}: tool needs a version probe command"));
        }
    }

    for profile in manifest.profiles.values() {
        for name in profile.environment.keys() {
            if manifest
                .variables
                .get(name)
                .is_some_and(|v| v.visibility == Visibility::Secret)
            {
                return Err(format!("{name}: profile must not contain secret values"));
            }
        }
    }

    for (name, variable) in &manifest.variables {
        if !crate::environment::valid_name(name) {
            return Err(format!("Invalid environment variable name: {name}"));
        }
        if matches!(variable.kind, crate::model::ValueType::Enum) && variable.values.is_empty() {
            return Err(format!("{name}: enum requires values"));
        }
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
        if task.command.is_empty() && task.depends_on.is_empty() && task.compose.is_none() {
            return Err(format!("{id}: task needs a command or dependencies"));
        }

        if task.compose.is_some() && !task.command.is_empty() {
            return Err(format!("{id}: choose command or compose"));
        }

        if let Some(compose) = &task.compose
            && (compose.service.is_empty()
                || compose.service.starts_with('-')
                || compose.project.is_empty()
                || !compose
                    .project
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'))
        {
            return Err(format!("{id}: invalid Compose service or project"));
        }

        if task
            .cleanup
            .iter()
            .any(|command| command.is_empty() || command[0].is_empty())
        {
            return Err(format!("{id}: cleanup commands must have an executable"));
        }

        for name in &task.requires {
            if !manifest.tools.contains_key(name) {
                return Err(format!("{id}: unknown tool {name}"));
            }
        }

        if task.timeout_seconds == Some(0) {
            return Err(format!("{id}: timeout must be positive"));
        }

        if let Some(cache) = &task.cache {
            if cache.inputs.is_empty()
                || cache.outputs.is_empty()
                || task.destructive
                || task.compose.is_some()
                || task.command.is_empty()
                || !task.cleanup.is_empty()
            {
                return Err(format!(
                    "{id}: cache requires inputs/outputs and a non-destructive process task"
                ));
            }
            if manifest.variables.values().any(|variable| {
                variable.visibility == Visibility::Secret
                    && variable
                        .consumers
                        .iter()
                        .any(|c| task.consumers.contains(c))
            }) || task.pass_environment.iter().any(|name| {
                manifest
                    .variables
                    .get(name)
                    .is_none_or(|v| v.visibility == Visibility::Secret)
            }) {
                return Err(format!(
                    "{id}: secret or unclassified environment inputs cannot be cached"
                ));
            }
        }

        for name in &task.pass_environment {
            if manifest.variables.contains_key(name) {
                return Err(format!(
                    "{id}: use consumers for contracted environment variable {name}"
                ));
            }
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
            if matches!(parameter.kind, crate::model::ValueType::Enum)
                && parameter.values.is_empty()
            {
                return Err(format!("{id}: enum parameter {name} requires values"));
            }
            if let Some(default) = &parameter.default
                && !parameter.kind.accepts(default, &parameter.values)
            {
                return Err(format!("{id}: invalid default for parameter {name}"));
            }
        }

        for name in task.environment.keys() {
            if !crate::environment::valid_name(name) {
                return Err(format!("{id}: invalid environment name"));
            }
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
