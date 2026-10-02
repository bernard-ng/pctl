//! Dependency scheduling, resource ownership, and failure reporting.
use crate::{
    Result, cache, compose,
    error::Error,
    model::{Plan, PlannedTask},
    paths,
    process::{self, Cancellation},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(100);

pub struct ProcessRequest<'a> {
    pub command: &'a [String],
    pub directory: &'a Path,
    pub environment: &'a BTreeMap<String, String>,
    pub timeout: Option<Duration>,
    pub cancellation: &'a Cancellation,
    pub secrets: &'a [String],
}

pub trait ProcessExecutor: Sync {
    fn execute(&self, request: ProcessRequest<'_>) -> Result<i32>;
}

pub struct LocalProcessExecutor;

impl ProcessExecutor for LocalProcessExecutor {
    fn execute(&self, r: ProcessRequest<'_>) -> Result<i32> {
        process::run(
            r.command,
            r.directory,
            r.environment,
            r.timeout,
            r.cancellation,
            r.secrets,
        )
    }
}

/// Called on the scheduling thread each time a task finishes (including cached
/// and failed tasks), in completion order and before the run ends. Tasks that
/// never start because of an earlier failure or cancellation are not reported.
pub type TaskObserver = Arc<dyn Fn(&TaskReport) + Send + Sync>;

#[derive(Default)]
pub struct RunOptions {
    pub jobs: usize,
    pub allow_destructive: bool,
    pub force: bool,
    pub cancellation: Cancellation,
    pub environments: BTreeMap<String, BTreeMap<String, String>>,
    pub tool_versions: BTreeMap<String, String>,
    pub secrets: Vec<String>,
    pub on_task_finished: Option<TaskObserver>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Success,
    Cached,
    Failed,
    Skipped,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Success => "success",
            Self::Cached => "cached",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        })
    }
}

#[derive(Debug, Serialize)]
pub struct TaskReport {
    pub id: String,
    pub status: TaskStatus,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub cleanup_errors: usize,
    pub message: Option<String>,
    #[serde(skip)]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExecutionReport {
    pub exit_code: i32,
    pub completed: Vec<String>,
    pub failed: Option<String>,
    pub tasks: Vec<TaskReport>,
}

pub fn execute(
    plan: &Plan,
    root: &Path,
    allow_destructive: bool,
    executor: &impl ProcessExecutor,
) -> Result<ExecutionReport> {
    execute_with(
        plan,
        root,
        &RunOptions {
            allow_destructive,
            jobs: 1,
            ..Default::default()
        },
        executor,
    )
}

pub fn execute_with(
    plan: &Plan,
    root: &Path,
    options: &RunOptions,
    executor: &impl ProcessExecutor,
) -> Result<ExecutionReport> {
    let directories = preflight(plan, root, options.allow_destructive)?;
    let lock_dir = paths::state_dir(root, "locks")?;
    let mut results = BTreeMap::<String, TaskReport>::new();
    let mut pending: BTreeSet<usize> = (0..plan.tasks.len()).collect();
    let mut running = 0;
    let mut first_failure = None;
    // Scheduling is re-attempted when a task finishes. Locks held by other pctl
    // processes can be released at any time, so lock-blocked tasks are retried on
    // a timer rather than on every poll of the cancellation flag.
    let mut changed = true;
    let mut next_lock_retry = Instant::now();
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| -> Result<()> {
        loop {
            if first_failure.is_none()
                && options.cancellation.signal() == 0
                && (changed || Instant::now() >= next_lock_retry)
            {
                changed = false;
                for index in pending.iter().copied().collect::<Vec<_>>() {
                    if running >= options.jobs.max(1) {
                        break;
                    }
                    let task = &plan.tasks[index];
                    if !task
                        .depends_on
                        .iter()
                        .all(|id| results.get(id).is_some_and(|r| r.exit_code == 0))
                    {
                        continue;
                    }
                    let Some(locks) = acquire(&lock_dir, task)? else {
                        next_lock_retry = Instant::now() + LOCK_RETRY_INTERVAL;
                        continue;
                    };
                    let directory = &directories[index];
                    let sender = sender.clone();
                    // A cached task's identity includes its dependencies' fingerprints.
                    // A dependency without one (it is not cached, or it ran without a
                    // cache declaration) makes the result unknowable, so this task runs
                    // uncached rather than risk a stale hit.
                    let stamps = task
                        .depends_on
                        .iter()
                        .map(|id| results[id].fingerprint.clone())
                        .collect::<Option<Vec<_>>>();
                    scope.spawn(move || {
                        let _locks = locks;
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            run_task(
                                task,
                                root,
                                directory,
                                &plan.profile,
                                stamps.as_deref(),
                                options,
                                executor,
                            )
                        }))
                        .unwrap_or_else(|_| TaskReport {
                            id: task.id.clone(),
                            status: TaskStatus::Failed,
                            exit_code: 2,
                            duration_ms: 0,
                            cleanup_errors: 0,
                            message: Some("Task worker panicked".into()),
                            fingerprint: None,
                        });
                        let _ = sender.send(result);
                    });
                    running += 1;
                    pending.remove(&index);
                }
            }
            if running == 0
                && (pending.is_empty()
                    || first_failure.is_some()
                    || options.cancellation.signal() != 0)
            {
                break;
            }
            if running > 0 {
                if let Ok(result) = receiver.recv_timeout(Duration::from_millis(20)) {
                    if result.exit_code != 0 && first_failure.is_none() {
                        first_failure = Some((result.id.clone(), result.exit_code));
                    }
                    if let Some(observer) = &options.on_task_finished {
                        observer(&result);
                    }
                    results.insert(result.id.clone(), result);
                    running -= 1;
                    changed = true;
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(())
    })?;
    let mut tasks = Vec::new();
    for task in &plan.tasks {
        tasks.push(results.remove(&task.id).unwrap_or_else(|| TaskReport {
            id: task.id.clone(),
            status: TaskStatus::Skipped,
            exit_code: 0,
            duration_ms: 0,
            cleanup_errors: 0,
            message: Some("Dependency failure or cancellation".into()),
            fingerprint: None,
        }));
    }
    let exit_code = first_failure
        .as_ref()
        .map(|(_, code)| *code)
        .unwrap_or_else(|| {
            if options.cancellation.signal() != 0 {
                128 + options.cancellation.signal()
            } else {
                0
            }
        });
    Ok(ExecutionReport {
        exit_code,
        completed: tasks
            .iter()
            .filter(|r| matches!(r.status, TaskStatus::Success | TaskStatus::Cached))
            .map(|r| r.id.clone())
            .collect(),
        failed: first_failure.map(|(id, _)| id),
        tasks,
    })
}

fn acquire(directory: &Path, task: &PlannedTask) -> Result<Option<Vec<File>>> {
    let mut names = task.exclusive.clone();
    if task.cache.is_some() {
        names.push(format!("cache:{}", task.id));
    }
    names.sort();
    names.dedup();
    let mut locks = Vec::new();
    for name in names {
        let path = directory.join(cache::hash(name.as_bytes()));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(Error::io("Cannot open lock file", &path))?;
        if file.try_lock().is_err() {
            return Ok(None);
        }
        locks.push(file);
    }
    Ok(Some(locks))
}

fn run_task(
    task: &PlannedTask,
    root: &Path,
    directory: &Path,
    profile: &str,
    stamps: Option<&[String]>,
    options: &RunOptions,
    executor: &impl ProcessExecutor,
) -> TaskReport {
    let start = Instant::now();
    let mut report = TaskReport {
        id: task.id.clone(),
        status: TaskStatus::Success,
        exit_code: 0,
        duration_ms: 0,
        cleanup_errors: 0,
        message: None,
        fingerprint: None,
    };
    let environment = options
        .environments
        .get(&task.id)
        .unwrap_or(&task.environment);
    let mut commands = if task.command.is_empty() {
        Vec::new()
    } else {
        vec![task.command.clone()]
    };
    let mut cleanup = task.cleanup.clone();
    if let Some(compose) = &task.compose {
        let (up, down) = compose::commands(compose);
        commands.push(up);
        cleanup.insert(0, down);
    }
    let result = (|| -> Result<i32> {
        if task.cache.is_some()
            && let Some(stamps) = stamps
        {
            let fingerprint = cache::fingerprint(
                root,
                task,
                profile,
                environment,
                &options.tool_versions,
                stamps,
            )?;
            report.fingerprint = Some(fingerprint.clone());
            if !options.force && cache::hit(root, task, &fingerprint)? {
                report.status = TaskStatus::Cached;
                return Ok(0);
            }
        }
        let mut code = 0;
        let mut process_error = None;
        for command in &commands {
            match executor.execute(ProcessRequest {
                command,
                directory,
                environment,
                timeout: task.timeout_seconds.map(Duration::from_secs),
                cancellation: &options.cancellation,
                secrets: &options.secrets,
            }) {
                Ok(exit) => code = exit,
                Err(error) => {
                    process_error = Some(error);
                    code = 2;
                }
            }
            if code != 0 {
                break;
            }
        }
        for command in &cleanup {
            let cleanup_cancel = Cancellation::default();
            match executor.execute(ProcessRequest {
                command,
                directory,
                environment,
                timeout: Some(Duration::from_secs(30)),
                cancellation: &cleanup_cancel,
                secrets: &options.secrets,
            }) {
                Ok(0) => {}
                _ => {
                    report.cleanup_errors += 1;
                    if code == 0 {
                        code = 1;
                    }
                }
            }
        }
        if let Some(error) = process_error {
            return Err(error);
        }
        if code == 0
            && let Some(fingerprint) = &report.fingerprint
        {
            cache::save(root, task, fingerprint)?;
        }
        Ok(code)
    })();
    match result {
        Ok(code) => report.exit_code = code,
        Err(error) => {
            report.exit_code = 2;
            report.message = Some(error.to_string());
        }
    }
    if report.exit_code != 0 {
        report.status = TaskStatus::Failed;
        report.fingerprint = None;
    }
    report.duration_ms = start.elapsed().as_millis();
    report
}

pub fn preflight(plan: &Plan, root: &Path, allow_destructive: bool) -> Result<Vec<PathBuf>> {
    let root = root
        .canonicalize()
        .map_err(Error::io("Cannot resolve", root))?;
    let mut seen = BTreeSet::new();
    plan.tasks
        .iter()
        .map(|task| {
            if !task
                .depends_on
                .iter()
                .all(|dependency| seen.contains(dependency))
                || !seen.insert(task.id.clone())
            {
                return Err("Plan must be topologically ordered with unique tasks".into());
            }
            if task.destructive && !allow_destructive {
                return Err(format!("{}: requires --allow-destructive", task.id).into());
            }
            if task.command.first().is_some_and(|s| s.is_empty()) {
                return Err("Empty task executable".into());
            }
            let directory = paths::inside(&root, Path::new(&task.working_directory))?;
            if !directory.is_dir() {
                return Err(format!("{}: missing working directory", task.id).into());
            }
            if let Some(compose) = &task.compose
                && !paths::inside(&directory, Path::new(&compose.file))?.is_file()
            {
                return Err(format!("{}: missing Compose file", task.id).into());
            }
            if let Some(cache) = &task.cache {
                for path in cache.inputs.iter().chain(&cache.outputs) {
                    paths::inside(&root, Path::new(path))?;
                }
            }
            Ok(directory)
        })
        .collect()
}
