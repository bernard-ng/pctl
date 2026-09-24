use crate::Result;
use crate::model::{Manifest, Plan, PlannedTask};
use std::collections::{BTreeMap, BTreeSet};

/// Resolve a deterministic plan without accessing processes or the filesystem.
pub fn build(
    manifest: &Manifest,
    target: &str,
    profile: &str,
    parameters: &BTreeMap<String, String>,
) -> Result<Plan> {
    let mut plan = Plan {
        profile: profile.into(),
        tasks: Vec::new(),
        tools: manifest.tools.clone(),
    };

    visit(
        manifest,
        target,
        profile,
        parameters,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
        &mut plan,
    )?;
    Ok(plan)
}

fn visit(
    manifest: &Manifest,
    id: &str,
    profile: &str,
    supplied: &BTreeMap<String, String>,
    active: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
    plan: &mut Plan,
) -> Result<()> {
    if done.contains(id) {
        return Ok(());
    }
    if !active.insert(id.into()) {
        return Err(format!("Task dependency cycle at {id}").into());
    }
    let task = manifest
        .tasks
        .get(id)
        .ok_or_else(|| format!("Unknown task: {id}"))?;
    if !task.profiles.is_empty() && !task.profiles.iter().any(|p| p == profile) {
        return Err(format!("{id}: unavailable in profile {profile}").into());
    }
    for name in supplied.keys() {
        if !task.parameters.contains_key(name) {
            return Err(format!("{id}: unknown parameter {name}").into());
        }
    }
    let mut parameters = BTreeMap::new();
    for (name, definition) in &task.parameters {
        let value = supplied
            .get(name)
            .or(definition.default.as_ref())
            .ok_or_else(|| format!("{id}: missing parameter {name}"))?;
        if !definition.kind.accepts(value, &definition.values) {
            return Err(format!("{id}: invalid parameter {name}").into());
        }
        parameters.insert(name.clone(), value.clone());
    }
    for dependency in &task.depends_on {
        visit(
            manifest,
            dependency,
            profile,
            &BTreeMap::new(),
            active,
            done,
            plan,
        )?;
    }
    let command = task
        .command
        .iter()
        .map(|argument| -> Result<String> {
            if let Some(name) = argument
                .strip_prefix("{param:")
                .and_then(|s| s.strip_suffix('}'))
            {
                parameters.get(name).cloned().ok_or_else(|| {
                    crate::error::Error::from(format!("{id}: unknown parameter reference {name}"))
                })
            } else if argument.contains("{param:") {
                Err(format!("{id}: parameter references must occupy an entire argument").into())
            } else {
                Ok(argument.clone())
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let mut environment = manifest
        .profiles
        .get(profile)
        .map(|p| p.environment.clone())
        .unwrap_or_default();
    environment.extend(task.environment.clone());
    let mut exclusive = task.exclusive.clone();
    if let Some(compose) = &task.compose {
        exclusive.push(format!("compose:{}", compose.project));
    }
    plan.tasks.push(PlannedTask {
        id: id.into(),
        command,
        working_directory: task.working_directory.clone(),
        environment,
        destructive: task.destructive,
        depends_on: task.depends_on.clone(),
        cleanup: task.cleanup.clone(),
        timeout_seconds: task.timeout_seconds,
        exclusive,
        requires: task.requires.clone(),
        consumers: task.consumers.clone(),
        pass_environment: task.pass_environment.clone(),
        compose: task.compose.clone(),
        cache: task.cache.clone(),
    });
    active.remove(id);
    done.insert(id.into());
    Ok(())
}
