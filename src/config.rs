use crate::{
    Result,
    error::Error,
    model::{Manifest, Task, ValueType, Visibility, parameter_reference},
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Profile used when `--profile` is not given; always considered valid.
pub const DEFAULT_PROFILE: &str = "local";

pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path.canonicalize().map_err(|source| Error::Io {
            context: "Cannot open manifest".into(),
            source,
        })?;
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
    let path = path.canonicalize().map_err(|source| Error::Io {
        context: "Cannot resolve include".into(),
        source,
    })?;
    if !path.starts_with(root) {
        return Err("Manifest includes must stay inside the project".into());
    }
    if !visited.insert(path.clone()) {
        return Err(format!("Repeated or cyclic include: {}", path.display()).into());
    }
    let source = std::fs::read_to_string(&path).map_err(|source| Error::Io {
        context: "Cannot read manifest".into(),
        source,
    })?;
    let deserializer = toml::Deserializer::parse(&source).map_err(|source| Error::InvalidToml {
        path: path.clone(),
        source,
    })?;
    let mut manifest: Manifest =
        serde_path_to_error::deserialize(deserializer).map_err(|error| Error::InvalidManifest {
            path: path.clone(),
            location: error.path().to_string(),
        })?;
    if manifest.schema_version != 1 {
        return Err(format!("Unsupported schema version in {}", path.display()).into());
    }
    for include in std::mem::take(&mut manifest.include) {
        let fragment = load_fragment(&root.join(include), root, visited)?;
        merge(&mut manifest.tasks, fragment.tasks, "task")?;
        merge(&mut manifest.variables, fragment.variables, "variable")?;
        merge(&mut manifest.profiles, fragment.profiles, "profile")?;
        merge(&mut manifest.tools, fragment.tools, "tool")?;
    }
    Ok(manifest)
}

/// Fold a fragment's definitions in; includes never overlay one another.
fn merge<T>(into: &mut BTreeMap<String, T>, from: BTreeMap<String, T>, kind: &str) -> Result<()> {
    for (id, value) in from {
        if into.insert(id.clone(), value).is_some() {
            return Err(format!("Duplicate {kind}: {id}").into());
        }
    }
    Ok(())
}

/// Check every structural rule and report all violations together.
fn validate(manifest: &Manifest) -> Result<()> {
    let mut problems = Vec::new();
    validate_tools(manifest, &mut problems);
    validate_profiles(manifest, &mut problems);
    validate_variables(manifest, &mut problems);
    for (id, task) in &manifest.tasks {
        validate_task(manifest, id, task, &mut problems);
    }
    let mut done = BTreeSet::new();
    for id in manifest.tasks.keys() {
        if let Err(error) = check_cycle(id, manifest, &mut BTreeSet::new(), &mut done) {
            problems.push(error.to_string());
            break;
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation(problems))
    }
}

fn has_executable(command: &[String]) -> bool {
    command.first().is_some_and(|program| !program.is_empty())
}

fn validate_tools(manifest: &Manifest, problems: &mut Vec<String>) {
    for (id, tool) in &manifest.tools {
        if !has_executable(&tool.command) {
            problems.push(format!("{id}: tool needs a version probe command"));
        }
    }
}

fn validate_profiles(manifest: &Manifest, problems: &mut Vec<String>) {
    for profile in manifest.profiles.values() {
        for name in profile.environment.keys() {
            if is_secret(manifest, name) {
                problems.push(format!("{name}: profile must not contain secret values"));
            }
        }
    }
}

fn validate_variables(manifest: &Manifest, problems: &mut Vec<String>) {
    for (name, variable) in &manifest.variables {
        if !crate::environment::valid_name(name) {
            problems.push(format!("Invalid environment variable name: {name}"));
        }
        if matches!(variable.kind, ValueType::Enum) && variable.values.is_empty() {
            problems.push(format!("{name}: enum requires values"));
        }
        if variable.browser_exposed && variable.visibility != Visibility::Public {
            problems.push(format!(
                "{name}: browser exposure requires public visibility"
            ));
        }
        if variable.consumers.is_empty() {
            problems.push(format!("{name}: declare at least one consumer"));
        }
    }
}

fn is_secret(manifest: &Manifest, name: &str) -> bool {
    manifest
        .variables
        .get(name)
        .is_some_and(|variable| variable.visibility == Visibility::Secret)
}

fn validate_task(manifest: &Manifest, id: &str, task: &Task, problems: &mut Vec<String>) {
    if task.command.is_empty() && task.depends_on.is_empty() && task.compose.is_none() {
        problems.push(format!("{id}: task needs a command or dependencies"));
    }
    if task.compose.is_some() && !task.command.is_empty() {
        problems.push(format!("{id}: choose command or compose"));
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
        problems.push(format!("{id}: invalid Compose service or project"));
    }
    if task.cleanup.iter().any(|command| !has_executable(command)) {
        problems.push(format!("{id}: cleanup commands must have an executable"));
    }
    for name in &task.requires {
        if !manifest.tools.contains_key(name) {
            problems.push(format!("{id}: unknown tool {name}"));
        }
    }
    if task.timeout_seconds == Some(0) {
        problems.push(format!("{id}: timeout must be positive"));
    }
    if task.cache.is_some() {
        validate_cache(manifest, id, task, problems);
    }
    for name in &task.pass_environment {
        if manifest.variables.contains_key(name) {
            problems.push(format!(
                "{id}: use consumers for contracted environment variable {name}"
            ));
        }
    }
    if !task.command.is_empty() && !has_executable(&task.command) {
        problems.push(format!("{id}: command executable cannot be empty"));
    }
    for argument in &task.command {
        match parameter_reference(argument) {
            Some(name) if !task.parameters.contains_key(name) => {
                problems.push(format!("{id}: unknown parameter reference {name}"));
            }
            None if argument.contains("{param:") => problems.push(format!(
                "{id}: parameter references must occupy an entire argument"
            )),
            _ => {}
        }
    }
    for dependency in &task.depends_on {
        if !manifest.tasks.contains_key(dependency) {
            problems.push(format!("{id}: unknown dependency {dependency}"));
        }
    }
    for (name, parameter) in &task.parameters {
        if matches!(parameter.kind, ValueType::Enum) && parameter.values.is_empty() {
            problems.push(format!("{id}: enum parameter {name} requires values"));
        }
        if let Some(default) = &parameter.default
            && !parameter.kind.accepts(default, &parameter.values)
        {
            problems.push(format!("{id}: invalid default for parameter {name}"));
        }
    }
    for name in task.environment.keys() {
        if !crate::environment::valid_name(name) {
            problems.push(format!("{id}: invalid environment name"));
        }
        if is_secret(manifest, name) {
            problems.push(format!(
                "{id}: secret {name} must come from the process environment"
            ));
        }
    }
}

fn validate_cache(manifest: &Manifest, id: &str, task: &Task, problems: &mut Vec<String>) {
    let Some(cache) = &task.cache else { return };
    if cache.inputs.is_empty()
        || cache.outputs.is_empty()
        || task.destructive
        || task.compose.is_some()
        || task.command.is_empty()
        || !task.cleanup.is_empty()
    {
        problems.push(format!(
            "{id}: cache requires inputs/outputs and a non-destructive process task"
        ));
    }
    let secret_consumer = manifest.variables.values().any(|variable| {
        variable.visibility == Visibility::Secret
            && variable
                .consumers
                .iter()
                .any(|consumer| task.consumers.contains(consumer))
    });
    // Fingerprints ignore OS variables, so a declared variable with such a name
    // would change a task's behaviour without invalidating its cache.
    for (name, variable) in &manifest.variables {
        if crate::environment::OS_VARIABLES.contains(&name.as_str())
            && variable
                .consumers
                .iter()
                .any(|consumer| task.consumers.contains(consumer))
        {
            problems.push(format!(
                "{id}: cached tasks cannot consume {name}; the name is reserved for OS pass-through and is not fingerprinted"
            ));
        }
    }
    let unclassified = task.pass_environment.iter().any(|name| {
        manifest
            .variables
            .get(name)
            .is_none_or(|variable| variable.visibility == Visibility::Secret)
    });
    if secret_consumer || unclassified {
        problems.push(format!(
            "{id}: secret or unclassified environment inputs cannot be cached"
        ));
    }
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
        return Err(format!("Task dependency cycle at {id}").into());
    }

    // Unknown dependencies are reported by `validate_task`; skip them here.
    if let Some(task) = manifest.tasks.get(id) {
        for dependency in &task.depends_on {
            check_cycle(dependency, manifest, active, done)?;
        }
    }

    active.remove(id);
    done.insert(id.into());
    Ok(())
}

/// Reject a profile name the manifest never mentions, which is almost always a
/// typo (`--profile prod` for `production`) that would otherwise silently run
/// with no profile settings. Profiles may be declared implicitly through
/// `required_in` and task `profiles`, so those count as mentions, and the CLI
/// default `local` is always accepted. A manifest that mentions no profile at all
/// has nothing to compare against and accepts any name.
pub fn validate_profile(manifest: &Manifest, profile: &str) -> Result<()> {
    let known: BTreeSet<&str> = manifest
        .profiles
        .keys()
        .map(String::as_str)
        .chain(
            manifest
                .variables
                .values()
                .flat_map(|variable| variable.required_in.iter().map(String::as_str)),
        )
        .chain(
            manifest
                .tasks
                .values()
                .flat_map(|task| task.profiles.iter().map(String::as_str)),
        )
        .collect();
    if profile == DEFAULT_PROFILE || known.is_empty() || known.contains(profile) {
        return Ok(());
    }
    Err(format!(
        "Unknown profile {profile}; the manifest mentions: {}",
        known.into_iter().collect::<Vec<_>>().join(", ")
    )
    .into())
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
                return Err(format!("{name}: required for profile {profile}").into());
            }
            Some(value) if !contract.kind.accepts(value, &contract.values) => {
                return Err(format!("{name}: invalid value for declared type").into());
            }
            _ => {}
        }
    }
    Ok(())
}
