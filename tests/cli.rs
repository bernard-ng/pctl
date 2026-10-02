use std::process::Command;

#[test]
fn cli_loads_fragments_and_emits_a_plan_without_execution() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("pctl.toml");
    std::fs::write(&manifest, "schema_version = 1\ninclude = ['tasks.toml']").unwrap();
    std::fs::write(directory.path().join("tasks.toml"), "schema_version = 1\n[tasks.hello]\ndescription = 'Hello'\ncommand = ['nonexistent-command']").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pctl"))
        .args([
            "--manifest",
            manifest.to_str().unwrap(),
            "plan",
            "hello",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["tasks"][0]["id"], "hello");
}

#[cfg(unix)]
#[test]
fn cli_preserves_actual_child_exit_code() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("pctl.toml");
    std::fs::write(
        &manifest,
        "schema_version = 1\n[tasks.fail]\ndescription = 'Fail'\ncommand = ['sh', '-c', 'exit 42']",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pctl"))
        .args(["--manifest", manifest.to_str().unwrap(), "run", "fail"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42));
}

#[cfg(unix)]
mod profiles_and_pipes {
    use std::{
        path::Path,
        process::{Command, Output, Stdio},
    };

    const MANIFEST: &str = r#"
schema_version = 1
[variables.API_URL]
type = "url"
visibility = "public"
consumers = ["web"]
required_in = ["production"]
[profiles.production.environment]
API_URL = "http://profile.example"
[tasks.lint]
description = "Does not consume API_URL"
command = ["sh", "-c", "echo lint-ran >&2"]
consumers = ["tools"]
[tasks.web]
description = "Consumes API_URL"
command = ["sh", "-c", "echo url=$API_URL >&2"]
consumers = ["web"]
"#;

    fn project() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("pctl.toml"), MANIFEST).unwrap();
        directory
    }

    fn pctl(directory: &Path, environment: &[(&str, &str)], args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pctl"))
            .current_dir(directory)
            .env_remove("API_URL")
            .envs(environment.iter().copied())
            .args(args)
            .output()
            .unwrap()
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn a_profile_variable_does_not_break_tasks_that_do_not_consume_it() {
        let directory = project();
        let output = pctl(
            directory.path(),
            &[],
            &["--profile", "production", "run", "lint"],
        );
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(text(&output.stderr).contains("lint-ran"));
    }

    #[test]
    fn process_environment_beats_the_profile_and_check_agrees_with_run() {
        let directory = project();
        let run = |environment: &[(&str, &str)]| {
            pctl(
                directory.path(),
                environment,
                &["--profile", "production", "run", "web"],
            )
        };
        assert!(text(&run(&[]).stderr).contains("url=http://profile.example"));
        let output = run(&[("API_URL", "http://process.example")]);
        assert!(text(&output.stderr).contains("url=http://process.example"));

        // The value both commands see is the process one, so an invalid one fails both.
        let invalid = [("API_URL", "not a url")];
        assert_eq!(run(&invalid).status.code(), Some(2));
        let check = pctl(
            directory.path(),
            &invalid,
            &["--profile", "production", "config", "check", "--values"],
        );
        assert_eq!(check.status.code(), Some(2));
    }

    #[test]
    fn unknown_profiles_are_rejected_but_implicit_and_default_ones_work() {
        let directory = project();
        for command in [
            ["plan", "lint"].as_slice(),
            ["run", "lint"].as_slice(),
            ["config", "generate"].as_slice(),
            ["config", "check", "--values"].as_slice(),
        ] {
            let mut args = vec!["--profile", "prod"];
            args.extend(command);
            let output = pctl(directory.path(), &[], &args);
            assert_eq!(output.status.code(), Some(2), "{command:?}");
            let error = text(&output.stderr);
            assert!(error.contains("Unknown profile prod"), "{error}");
            assert!(error.contains("production"), "{error}");
        }
        // `production` is only mentioned by the variable and `[profiles.production]`;
        // `local` is the built-in default and always valid.
        for profile in ["production", "local"] {
            let output = pctl(
                directory.path(),
                &[],
                &["--profile", profile, "plan", "lint"],
            );
            assert!(
                output.status.success(),
                "{profile}: {}",
                text(&output.stderr)
            );
        }
    }

    #[test]
    fn a_closed_stdout_pipe_ends_output_quietly_instead_of_panicking() {
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = String::from("schema_version = 1\n");
        for index in 0..4000 {
            manifest.push_str(&format!(
                "[tasks.task-number-{index}]\ndescription = 'A reasonably long description'\ncommand = ['true']\n"
            ));
        }
        std::fs::write(directory.path().join("pctl.toml"), manifest).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_pctl"))
            .current_dir(directory.path())
            .arg("list")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Output exceeds the pipe buffer, so pctl must write after the reader is gone.
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        let error = text(&output.stderr);
        assert!(!error.contains("panicked"), "{error}");
        assert!(output.status.success(), "{:?}: {error}", output.status);
    }
}

#[cfg(unix)]
mod ndjson {
    use serde_json::Value;
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };

    fn run(manifest: &str, args: &[&str]) -> (std::process::Child, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("pctl.toml"), manifest).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_pctl"))
            .current_dir(directory.path())
            .args(["run"])
            .args(args)
            .args(["--format", "ndjson"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        (child, directory)
    }

    fn event(line: &str) -> Value {
        serde_json::from_str(line.trim()).unwrap()
    }

    #[test]
    fn events_arrive_while_the_run_is_still_in_progress() {
        let (mut child, _directory) = run(
            r#"
schema_version = 1
[tasks.fast]
description = "Finishes immediately"
command = ["true"]
[tasks.slow]
description = "Keeps the run open"
command = ["sleep", "2"]
[tasks.all]
description = "Both"
depends_on = ["fast", "slow"]
"#,
            &["all", "--jobs", "2"],
        );
        let started = std::time::Instant::now();
        let mut lines = BufReader::new(child.stdout.take().unwrap());
        let mut first = String::new();
        lines.read_line(&mut first).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1500),
            "first event took {:?}; it must not wait for the 2 s task",
            started.elapsed()
        );
        let first = event(&first);
        assert_eq!(first["event"], "task_finished");
        assert_eq!(first["task"]["id"], "fast");
        assert!(
            child.try_wait().unwrap().is_none(),
            "the first event must be written before the run finishes"
        );

        let rest: Vec<Value> = lines.lines().map(|line| event(&line.unwrap())).collect();
        let ids: Vec<_> = rest
            .iter()
            .map(|event| event["task"]["id"].as_str().unwrap_or("-"))
            .collect();
        assert_eq!(ids, ["slow", "all", "-"]);
        assert_eq!(rest[2]["event"], "run_finished");
        assert_eq!(rest[2]["exit_code"], 0);
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn tasks_that_never_started_are_reported_as_skipped_before_the_final_event() {
        let (child, _directory) = run(
            r#"
schema_version = 1
[tasks.broken]
description = "Fails"
command = ["sh", "-c", "exit 42"]
[tasks.after]
description = "Depends on the failure"
command = ["true"]
depends_on = ["broken"]
"#,
            &["after"],
        );
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(42));
        let events: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(event)
            .collect();
        let summary: Vec<_> = events
            .iter()
            .map(|event| {
                (
                    event["event"].as_str().unwrap(),
                    event["task"]["id"].as_str(),
                    event["task"]["status"].as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("task_finished", Some("broken"), Some("failed")),
                ("task_finished", Some("after"), Some("skipped")),
                ("run_finished", None, None),
            ]
        );
        assert_eq!(events[2]["exit_code"], 42);
    }
}
