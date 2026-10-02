use pctl::{model::Tool, process::Cancellation, tools::probe};
use std::{collections::BTreeMap, time::Instant};

fn tool(command: &[&str]) -> Tool {
    Tool {
        command: command.iter().map(|part| (*part).into()).collect(),
        version_contains: None,
    }
}

fn environment() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())])
}

#[test]
fn probes_run_concurrently_and_deduplicate_names() {
    let tools = BTreeMap::from([
        ("a".into(), tool(&["sh", "-c", "sleep 1; echo a-1"])),
        ("b".into(), tool(&["sh", "-c", "sleep 1; echo b-2"])),
        ("c".into(), tool(&["sh", "-c", "sleep 1; echo c-3"])),
    ]);
    let names = ["a", "b", "c", "a"].map(String::from);
    let started = Instant::now();
    let versions = probe(
        &tools,
        names,
        &std::env::temp_dir(),
        &environment(),
        &Cancellation::default(),
    )
    .unwrap();
    assert!(
        started.elapsed().as_millis() < 2500,
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        versions,
        BTreeMap::from([
            ("a".into(), "a-1".into()),
            ("b".into(), "b-2".into()),
            ("c".into(), "c-3".into()),
        ])
    );
}

#[test]
fn failures_report_the_cause_and_unknown_names_fail_before_running() {
    let marker = tempfile::tempdir().unwrap();
    let touched = marker.path().join("ran");
    let tools = BTreeMap::from([
        ("missing".into(), tool(&["definitely-not-a-real-binary"])),
        (
            "side-effect".into(),
            tool(&["sh", "-c", &format!("touch {}", touched.display())]),
        ),
    ]);
    let run = |names: &[&str]| {
        probe(
            &tools,
            names.iter().map(|name| (*name).to_owned()),
            marker.path(),
            &environment(),
            &Cancellation::default(),
        )
        .unwrap_err()
        .to_string()
    };

    let error = run(&["missing"]);
    assert!(
        error.contains("missing: unable to execute version probe"),
        "{error}"
    );
    assert!(error.contains("No such file"), "{error}");

    let error = run(&["side-effect", "unknown"]);
    assert!(error.contains("Unknown capability unknown"), "{error}");
    assert!(
        !touched.exists(),
        "no probe may start for an invalid request"
    );
}
