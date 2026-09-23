use crate::Result;
use crate::model::Plan;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct ProcessRequest<'a> {
    pub command: &'a [String],
    pub directory: &'a Path,
    pub environment: &'a BTreeMap<String, String>,
}

/// Process effects are replaceable; planning and scheduling remain ordinary Rust.
pub trait ProcessExecutor {
    fn execute(&mut self, request: ProcessRequest<'_>) -> Result<i32>;
}

pub struct LocalProcessExecutor;

impl ProcessExecutor for LocalProcessExecutor {
    fn execute(&mut self, request: ProcessRequest<'_>) -> Result<i32> {
        let status = Command::new(&request.command[0])
            .args(&request.command[1..])
            .current_dir(request.directory)
            .envs(request.environment)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("Unable to start task process: {e}"))?;
        if let Some(code) = status.code() {
            return Ok(code);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            Ok(128 + status.signal().unwrap_or(1))
        }
        #[cfg(not(unix))]
        {
            Ok(1)
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ExecutionReport {
    pub exit_code: i32,
    pub completed: Vec<String>,
    pub failed: Option<String>,
}

pub fn execute(
    plan: &Plan,
    root: &Path,
    allow_destructive: bool,
    executor: &mut impl ProcessExecutor,
) -> Result<ExecutionReport> {
    // Preflight every task before starting any process.
    let directories = preflight(plan, root, allow_destructive)?;
    let mut report = ExecutionReport {
        exit_code: 0,
        completed: Vec::new(),
        failed: None,
    };
    for (task, directory) in plan.tasks.iter().zip(directories) {
        if !task.command.is_empty() {
            let code = executor.execute(ProcessRequest {
                command: &task.command,
                directory: &directory,
                environment: &task.environment,
            })?;
            if code != 0 {
                report.exit_code = code;
                report.failed = Some(task.id.clone());
                return Ok(report);
            }
        }
        report.completed.push(task.id.clone());
    }
    Ok(report)
}

pub fn preflight(plan: &Plan, root: &Path, allow_destructive: bool) -> Result<Vec<PathBuf>> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("Cannot resolve project directory: {e}"))?;
    plan.tasks
        .iter()
        .map(|task| {
            if task.destructive && !allow_destructive {
                return Err(format!("{}: requires --allow-destructive", task.id));
            }
            let directory = root
                .join(&task.working_directory)
                .canonicalize()
                .map_err(|_| format!("{}: working directory does not exist", task.id))?;
            if !directory.starts_with(&root) || !directory.is_dir() {
                return Err(format!(
                    "{}: working directory must be inside project",
                    task.id
                ));
            }
            Ok(directory)
        })
        .collect()
}
