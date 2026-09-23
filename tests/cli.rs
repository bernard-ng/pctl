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
