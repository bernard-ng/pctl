use pctl::{
    environment,
    execution::{ProcessExecutor, ProcessRequest, RunOptions, execute_with},
    model::{Cache, Compose, Manifest, Plan, PlannedTask},
};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Condvar, Mutex},
    time::Duration,
};

#[derive(Debug, Clone)]
struct Call {
    command: Vec<String>,
    directory: PathBuf,
    environment: BTreeMap<String, String>,
    timeout: Option<Duration>,
    cancellation: i32,
    secrets: Vec<String>,
}

#[derive(Default)]
struct FakeExecutor {
    calls: Mutex<Vec<Call>>,
    outcomes: BTreeMap<String, pctl::Result<i32>>,
    write_output: bool,
    cancel_primary: bool,
}

impl ProcessExecutor for FakeExecutor {
    fn execute(&self, request: ProcessRequest<'_>) -> pctl::Result<i32> {
        self.calls.lock().unwrap().push(Call {
            command: request.command.to_vec(),
            directory: request.directory.to_path_buf(),
            environment: request.environment.clone(),
            timeout: request.timeout,
            cancellation: request.cancellation.signal(),
            secrets: request.secrets.to_vec(),
        });
        let name = request.command.join(" ");
        if self.cancel_primary && name == "primary" {
            request.cancellation.cancel(15);
        }
        if self.write_output && name == "build" {
            let input = fs::read(request.directory.join("input.txt")).unwrap();
            fs::write(request.directory.join("output.txt"), input).unwrap();
        }
        self.outcomes.get(&name).cloned().unwrap_or(Ok(0))
    }
}

fn task(id: &str) -> PlannedTask {
    PlannedTask {
        id: id.into(),
        command: vec![id.into()],
        working_directory: ".".into(),
        environment: BTreeMap::new(),
        destructive: false,
        depends_on: vec![],
        cleanup: vec![],
        timeout_seconds: None,
        exclusive: vec![],
        requires: vec![],
        consumers: vec![],
        pass_environment: vec![],
        compose: None,
        cache: None,
    }
}

fn plan(tasks: Vec<PlannedTask>) -> Plan {
    Plan {
        profile: "ci".into(),
        tasks,
        tools: BTreeMap::new(),
    }
}

#[test]
fn cleanup_runs_after_exit_failure_and_spawn_error_and_dependents_are_skipped() {
    for (outcome, expected_code, expected_message) in [
        (Ok(42), 42, None),
        (
            Err("cannot spawn primary".into()),
            2,
            Some("cannot spawn primary"),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut primary = task("primary");
        primary.cleanup = vec![vec!["cleanup-fails".into()], vec!["cleanup-last".into()]];
        let mut dependent = task("dependent");
        dependent.depends_on = vec!["primary".into()];
        let executor = FakeExecutor {
            outcomes: BTreeMap::from([
                ("primary".into(), outcome),
                ("cleanup-fails".into(), Ok(9)),
            ]),
            ..Default::default()
        };
        let report = execute_with(
            &plan(vec![primary, dependent]),
            root.path(),
            &RunOptions::default(),
            &executor,
        )
        .unwrap();
        assert_eq!(report.exit_code, expected_code);
        assert_eq!(report.failed.as_deref(), Some("primary"));
        assert!(report.completed.is_empty());
        assert_eq!(report.tasks[0].status, "failed");
        assert_eq!(report.tasks[0].cleanup_errors, 1);
        assert_eq!(report.tasks[0].message.as_deref(), expected_message);
        assert_eq!(report.tasks[1].status, "skipped");
        let calls = executor.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|c| c.command[0].as_str())
                .collect::<Vec<_>>(),
            ["primary", "cleanup-fails", "cleanup-last"]
        );
    }
}

#[test]
fn cleanup_failure_turns_success_into_failure_for_exit_and_spawn_errors() {
    for outcome in [Ok(7), Err("cannot spawn cleanup".into())] {
        let root = tempfile::tempdir().unwrap();
        let mut primary = task("primary");
        primary.cleanup = vec![vec!["cleanup".into()]];
        let executor = FakeExecutor {
            outcomes: BTreeMap::from([("cleanup".into(), outcome)]),
            ..Default::default()
        };
        let report = execute_with(
            &plan(vec![primary]),
            root.path(),
            &RunOptions::default(),
            &executor,
        )
        .unwrap();
        assert_ne!(report.exit_code, 0);
        assert_eq!(report.failed.as_deref(), Some("primary"));
        assert!(report.completed.is_empty());
        assert_eq!(report.tasks[0].status, "failed");
        assert_eq!(report.tasks[0].cleanup_errors, 1);
        assert_eq!(executor.calls.lock().unwrap().len(), 2);
    }
}

#[test]
fn run_options_reach_processes_and_cleanup_uses_fresh_cancellation() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("work")).unwrap();
    let mut primary = task("primary");
    primary.working_directory = "work".into();
    primary.timeout_seconds = Some(17);
    primary
        .environment
        .insert("SOURCE".into(), "literal".into());
    primary.cleanup = vec![vec!["cleanup".into()]];
    let resolved = BTreeMap::from([("SOURCE".into(), "resolved".into())]);
    let options = RunOptions {
        environments: BTreeMap::from([("primary".into(), resolved.clone())]),
        secrets: vec!["redaction-sentinel".into()],
        ..Default::default()
    };
    let executor = FakeExecutor {
        cancel_primary: true,
        outcomes: BTreeMap::from([("primary".into(), Ok(143))]),
        ..Default::default()
    };
    let report = execute_with(&plan(vec![primary]), root.path(), &options, &executor).unwrap();
    assert_eq!(report.exit_code, 143);
    assert_eq!(options.cancellation.signal(), 15);
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].timeout, Some(Duration::from_secs(17)));
    assert_eq!(calls[1].timeout, Some(Duration::from_secs(30)));
    for call in calls.iter() {
        assert_eq!(
            call.directory,
            root.path().join("work").canonicalize().unwrap()
        );
        assert_eq!(call.environment, resolved);
        assert_eq!(call.secrets, options.secrets);
        assert_eq!(call.cancellation, 0);
    }
}

#[derive(Default)]
struct ConcurrencyState {
    active: usize,
    maximum: usize,
    started: usize,
    completed: usize,
}

struct ConcurrentExecutor {
    state: Mutex<ConcurrencyState>,
    changed: Condvar,
    rendezvous: bool,
}

impl ProcessExecutor for ConcurrentExecutor {
    fn execute(&self, request: ProcessRequest<'_>) -> pctl::Result<i32> {
        let mut state = self.state.lock().unwrap();
        if request.command[0] == "join" {
            return if state.completed == 3 && state.active == 0 {
                Ok(0)
            } else {
                Err("join started before its dependencies finished".into())
            };
        }
        state.active += 1;
        state.started += 1;
        state.maximum = state.maximum.max(state.active);
        self.changed.notify_all();
        if self.rendezvous && state.started < 2 {
            let (next, timeout) = self
                .changed
                .wait_timeout_while(state, Duration::from_secs(3), |s| s.started < 2)
                .unwrap();
            state = next;
            if timeout.timed_out() && state.started < 2 {
                state.active -= 1;
                return Err("two independent tasks never overlapped".into());
            }
        }
        drop(state);
        std::thread::sleep(Duration::from_millis(40));
        let mut state = self.state.lock().unwrap();
        state.active -= 1;
        state.completed += 1;
        Ok(0)
    }
}

fn check_concurrency(exclusive: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut tasks = vec![task("left"), task("right"), task("third")];
    if exclusive {
        for task in &mut tasks {
            task.exclusive = vec!["shared-database".into()];
        }
    }
    let mut join = task("join");
    join.depends_on = tasks.iter().map(|t| t.id.clone()).collect();
    tasks.push(join);
    let executor = ConcurrentExecutor {
        state: Mutex::default(),
        changed: Condvar::new(),
        rendezvous: !exclusive,
    };
    let options = RunOptions {
        jobs: 2,
        ..Default::default()
    };
    let report = execute_with(&plan(tasks), root.path(), &options, &executor).unwrap();
    assert_eq!(report.exit_code, 0, "{report:?}");
    assert_eq!(report.completed, ["left", "right", "third", "join"]);
    let state = executor.state.lock().unwrap();
    assert_eq!(state.maximum, if exclusive { 1 } else { 2 });
    assert_eq!(state.started, 3);
    assert_eq!(state.completed, 3);
}

#[test]
fn jobs_two_overlaps_independent_tasks_and_waits_for_dependencies() {
    check_concurrency(false);
}

#[test]
fn shared_exclusive_resource_serializes_tasks_with_jobs_two() {
    check_concurrency(true);
}

#[test]
fn compose_down_runs_after_up_failure_spawn_error_and_setup_failure() {
    let up = "docker compose --project-name runtime-test --file compose.yaml up --build --abort-on-container-exit --exit-code-from tests tests";
    let down = "docker compose --project-name runtime-test --file compose.yaml down";
    for (command, outcome, code) in [
        (up, Ok(23), 23),
        (up, Err("docker unavailable".into()), 2),
        ("setup", Ok(19), 19),
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("compose.yaml"), "").unwrap();
        let mut compose_task = task("setup");
        compose_task.compose = Some(Compose {
            file: "compose.yaml".into(),
            service: "tests".into(),
            project: "runtime-test".into(),
            build: true,
        });
        compose_task.cleanup = vec![vec!["extra-cleanup".into()]];
        let executor = FakeExecutor {
            outcomes: BTreeMap::from([(command.into(), outcome)]),
            ..Default::default()
        };
        let report = execute_with(
            &plan(vec![compose_task]),
            root.path(),
            &RunOptions::default(),
            &executor,
        )
        .unwrap();
        assert_eq!(report.exit_code, code);
        let calls = executor.calls.lock().unwrap();
        let actual: Vec<_> = calls.iter().map(|c| c.command.join(" ")).collect();
        let expected = if command == "setup" {
            vec!["setup", down, "extra-cleanup"]
        } else {
            vec!["setup", up, down, "extra-cleanup"]
        };
        assert_eq!(actual, expected);
        assert_eq!(report.tasks[0].cleanup_errors, 0);
    }
}

#[test]
fn cache_verifies_inputs_and_outputs_and_force_bypasses_hit() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("input.txt"), "first").unwrap();
    let mut build = task("build");
    build.cache = Some(Cache {
        inputs: vec!["input.txt".into()],
        outputs: vec!["output.txt".into()],
    });
    let plan = plan(vec![build]);
    let executor = FakeExecutor {
        write_output: true,
        ..Default::default()
    };
    let mut options = RunOptions::default();
    for (step, status, calls) in [
        ("initial", "success", 1),
        ("unchanged", "cached", 1),
        ("input", "success", 2),
        ("output", "success", 3),
        ("missing-output", "success", 4),
        ("warm", "cached", 4),
        ("force", "success", 5),
    ] {
        match step {
            "input" => fs::write(root.path().join("input.txt"), "second").unwrap(),
            "output" => fs::write(root.path().join("output.txt"), "tampered").unwrap(),
            "missing-output" => fs::remove_file(root.path().join("output.txt")).unwrap(),
            "force" => options.force = true,
            _ => {}
        }
        let report = execute_with(&plan, root.path(), &options, &executor).unwrap();
        assert_eq!(report.exit_code, 0, "{step}: {report:?}");
        assert_eq!(report.tasks[0].status, status, "{step}");
        assert_eq!(report.completed, ["build"]);
        assert_eq!(executor.calls.lock().unwrap().len(), calls, "{step}");
        assert_eq!(
            fs::read(root.path().join("output.txt")).unwrap(),
            fs::read(root.path().join("input.txt")).unwrap()
        );
    }
}

#[test]
fn resolved_environments_only_reach_their_declared_consumers() {
    let root = tempfile::tempdir().unwrap();
    let manifest: Manifest = toml::from_str(
        r#"
schema_version = 1
[variables.API_TOKEN]
type = "string"
visibility = "secret"
consumers = ["api"]
required_in = ["ci"]
[variables.PUBLIC_URL]
type = "string"
visibility = "public"
consumers = ["web"]
required_in = ["ci"]
"#,
    )
    .unwrap();
    let source = BTreeMap::from([
        ("API_TOKEN".into(), "private-sentinel".into()),
        ("PUBLIC_URL".into(), "https://example.test".into()),
        ("UNRELATED_SECRET".into(), "must-not-leak".into()),
        ("EXPLICIT".into(), "passed".into()),
    ]);
    let mut api = task("api");
    api.consumers = vec!["api".into()];
    let mut web = task("web");
    web.consumers = vec!["web".into()];
    web.pass_environment = vec!["EXPLICIT".into()];
    let plan = plan(vec![api, web]);
    let environments = plan
        .tasks
        .iter()
        .map(|task| {
            (
                task.id.clone(),
                environment::resolve(&manifest, task, "ci", &source).unwrap(),
            )
        })
        .collect();
    let options = RunOptions {
        environments,
        ..Default::default()
    };
    let executor = FakeExecutor::default();
    let report = execute_with(&plan, root.path(), &options, &executor).unwrap();
    assert_eq!(report.exit_code, 0);
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].environment,
        BTreeMap::from([("API_TOKEN".into(), "private-sentinel".into())])
    );
    assert_eq!(
        calls[1].environment,
        BTreeMap::from([
            ("PUBLIC_URL".into(), "https://example.test".into()),
            ("EXPLICIT".into(), "passed".into()),
        ])
    );
    let mut missing = source.clone();
    missing.remove("API_TOKEN");
    assert!(environment::resolve(&manifest, &plan.tasks[0], "ci", &missing).is_err());
    assert!(environment::resolve(&manifest, &plan.tasks[1], "ci", &missing).is_ok());
    let mut unauthorized = plan.tasks[1].clone();
    unauthorized
        .environment
        .insert("API_TOKEN".into(), "literal".into());
    assert!(environment::resolve(&manifest, &unauthorized, "ci", &source).is_err());
}
