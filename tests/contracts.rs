use pctl::{
    config::{Project, validate_environment},
    execution::{self, ProcessExecutor, ProcessRequest},
    planning,
};
use std::collections::BTreeMap;

fn project(source: &str) -> (tempfile::TempDir, Project) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pctl.toml");
    std::fs::write(&path, source).unwrap();
    let project = Project::load(&path).unwrap();
    (directory, project)
}

const TASKS: &str = r#"
schema_version = 1
[tasks.prepare]
description = "Prepare"
command = ["example", "prepare"]
[tasks.left]
description = "Left"
command = ["example", "left"]
depends_on = ["prepare"]
[tasks.right]
description = "Right"
command = ["example", "right"]
depends_on = ["prepare"]
[tasks.all]
description = "All"
depends_on = ["left", "right"]
"#;

#[test]
fn shared_dependencies_execute_once_in_declared_order() {
    let (_directory, project) = project(TASKS);
    let plan = planning::build(&project.manifest, "all", "local", &BTreeMap::new()).unwrap();
    assert_eq!(
        plan.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["prepare", "left", "right", "all"]
    );
}

#[test]
fn validates_parameters_and_keeps_shell_characters_in_one_argument() {
    let (_directory, project) = project(
        r#"
schema_version = 1
[tasks.echo]
description = "Echo"
command = ["echo", "{param:message}"]
profiles = ["local"]
[tasks.echo.parameters.message]
type = "string"
"#,
    );
    let input = "hello; $(touch /tmp/never)";
    let parameters = BTreeMap::from([("message".into(), input.into())]);
    let plan = planning::build(&project.manifest, "echo", "local", &parameters).unwrap();
    assert_eq!(plan.tasks[0].command, ["echo", input]);
    assert!(planning::build(&project.manifest, "echo", "production", &parameters).is_err());
    assert!(planning::build(&project.manifest, "echo", "local", &BTreeMap::new()).is_err());
}

#[test]
fn rejects_cycles_duplicates_unknown_fields_and_browser_secrets() {
    for source in [
        TASKS.replace("command = [\"example\", \"prepare\"]", "depends_on = [\"all\"]"),
        "schema_version = 1\nunknown = true".into(),
        "schema_version = 1\n[variables.TOKEN]\ntype = 'string'\nvisibility = 'secret'\nconsumers = ['api']\nbrowser_exposed = true".into(),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pctl.toml");
        std::fs::write(&path, source).unwrap();
        assert!(Project::load(&path).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("tasks.toml"), TASKS).unwrap();
    std::fs::write(
        directory.path().join("pctl.toml"),
        TASKS.replace(
            "schema_version = 1",
            "schema_version = 1\ninclude = ['tasks.toml']",
        ),
    )
    .unwrap();
    assert!(Project::load(&directory.path().join("pctl.toml")).is_err());
}

#[test]
fn environment_errors_never_echo_values() {
    let (_directory, project) = project(
        r#"
schema_version = 1
[variables.TOKEN]
type = "integer"
visibility = "secret"
consumers = ["api"]
required_in = ["production"]
"#,
    );
    assert!(validate_environment(&project.manifest, "local", &BTreeMap::new()).is_ok());
    assert!(validate_environment(&project.manifest, "production", &BTreeMap::new()).is_err());
    let error = validate_environment(
        &project.manifest,
        "production",
        &BTreeMap::from([("TOKEN".into(), "private-value".into())]),
    )
    .unwrap_err();
    assert!(error.contains("TOKEN"));
    assert!(!error.contains("private-value"));
}

#[derive(Default)]
struct RecordingExecutor {
    calls: Vec<Vec<String>>,
    fail_at: Option<usize>,
}
impl ProcessExecutor for RecordingExecutor {
    fn execute(&mut self, request: ProcessRequest<'_>) -> pctl::Result<i32> {
        self.calls.push(request.command.to_vec());
        Ok(if self.fail_at == Some(self.calls.len()) {
            42
        } else {
            0
        })
    }
}

#[test]
fn stops_on_failure_and_preserves_exit_code() {
    let (_directory, project) = project(TASKS);
    let plan = planning::build(&project.manifest, "all", "local", &BTreeMap::new()).unwrap();
    let mut executor = RecordingExecutor {
        fail_at: Some(2),
        ..Default::default()
    };
    let report = execution::execute(&plan, &project.root, false, &mut executor).unwrap();
    assert_eq!(report.exit_code, 42);
    assert_eq!(report.completed, ["prepare"]);
    assert_eq!(executor.calls.len(), 2);
}

#[test]
fn preflights_entire_plan_before_any_side_effect() {
    let (_directory, project) = project(TASKS);
    let mut plan = planning::build(&project.manifest, "all", "local", &BTreeMap::new()).unwrap();
    plan.tasks.last_mut().unwrap().destructive = true;
    let mut executor = RecordingExecutor::default();
    assert!(execution::execute(&plan, &project.root, false, &mut executor).is_err());
    assert!(executor.calls.is_empty());
    plan.tasks.last_mut().unwrap().working_directory = "..".into();
    assert!(execution::execute(&plan, &project.root, true, &mut executor).is_err());
    assert!(executor.calls.is_empty());
}
