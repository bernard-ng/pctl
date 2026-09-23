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
        return Err(format!("Task dependency cycle at {id}"));
    }
    let task = manifest
        .tasks
        .get(id)
        .ok_or_else(|| format!("Unknown task: {id}"))?;
    if !task.profiles.is_empty() && !task.profiles.iter().any(|p| p == profile) {
        return Err(format!("{id}: unavailable in profile {profile}"));
    }
    for name in supplied.keys() {
        if !task.parameters.contains_key(name) {
            return Err(format!("{id}: unknown parameter {name}"));
        }
    }
    let mut parameters = BTreeMap::new();
    for (name, definition) in &task.parameters {
        let value = supplied
            .get(name)
            .or(definition.default.as_ref())
            .ok_or_else(|| format!("{id}: missing parameter {name}"))?;
        if !definition.kind.accepts(value, &definition.values) {
            return Err(format!("{id}: invalid parameter {name}"));
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
        .map(|argument| {
            if let Some(name) = argument
                .strip_prefix("{param:")
                .and_then(|s| s.strip_suffix('}'))
            {
                parameters
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("{id}: unknown parameter reference {name}"))
            } else if argument.contains("{param:") {
                Err(format!(
                    "{id}: parameter references must occupy an entire argument"
                ))
            } else {
                Ok(argument.clone())
            }
        })
        .collect::<Result<Vec<_>>>()?;
    plan.tasks.push(PlannedTask {
        id: id.into(),
        command,
        working_directory: task.working_directory.clone(),
        environment: task.environment.clone(),
        destructive: task.destructive,
    });
    active.remove(id);
    done.insert(id.into());
    Ok(())
}
