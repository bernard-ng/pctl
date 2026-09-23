#![cfg(unix)]

use pctl::process::{Cancellation, SignalGuard, capture, run};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

fn shell(script: &str, timeout: Option<Duration>, cancellation: &Cancellation) -> i32 {
    run(
        &["/bin/sh".into(), "-c".into(), script.into()],
        Path::new("/"),
        &BTreeMap::new(),
        timeout,
        cancellation,
        &[],
    )
    .unwrap()
}

#[test]
fn preserves_normal_exit_and_has_noninteractive_stdin() {
    assert_eq!(
        shell(
            "read value && exit 99; exit 7",
            None,
            &Cancellation::default()
        ),
        7
    );
}

#[test]
fn signal_registrations_can_be_installed_and_dropped() {
    let guard = SignalGuard::install(Cancellation::default()).unwrap();
    drop(guard);
}

#[test]
fn only_supplied_environment_reaches_child() {
    let environment = BTreeMap::from([("SUPPLIED".into(), "present".into())]);
    let code = run(
        &[
            "/bin/sh".into(),
            "-c".into(),
            "test \"$SUPPLIED\" = present && test -z \"${HOME+x}\"".into(),
        ],
        Path::new("/"),
        &environment,
        None,
        &Cancellation::default(),
        &[],
    )
    .unwrap();
    assert_eq!(code, 0);
}

#[test]
fn timeout_escalates_when_term_is_ignored() {
    let started = Instant::now();
    assert_eq!(
        shell(
            "trap '' TERM; while :; do sleep 1; done",
            Some(Duration::from_millis(100)),
            &Cancellation::default()
        ),
        124
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn cancellation_is_shared_and_sticky() {
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let worker =
        std::thread::spawn(move || shell("while :; do sleep 1; done", None, &worker_cancel));
    std::thread::sleep(Duration::from_millis(100));
    cancel.cancel(libc::SIGINT);
    cancel.cancel(libc::SIGTERM);
    assert_eq!(cancel.signal(), libc::SIGINT);
    assert_eq!(worker.join().unwrap(), 128 + libc::SIGINT);
}

#[test]
fn pre_cancelled_command_is_not_spawned() {
    let cancel = Cancellation::default();
    cancel.cancel(libc::SIGTERM);
    assert_eq!(
        run(
            &["/does/not/exist".into()],
            Path::new("/"),
            &BTreeMap::new(),
            None,
            &cancel,
            &[]
        )
        .unwrap(),
        128 + libc::SIGTERM
    );
}

#[test]
fn remaining_descendant_is_killed_and_its_pipe_does_not_hang() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("survived");
    // Pass the path as an argument, never interpolate shell source.
    let command = vec![
        "/bin/sh".into(),
        "-c".into(),
        "(trap '' TERM; sleep 1; printf alive > \"$1\") & exit 0".into(),
        "test".into(),
        marker.to_string_lossy().into_owned(),
    ];
    let started = Instant::now();
    assert_eq!(
        run(
            &command,
            directory.path(),
            &BTreeMap::new(),
            None,
            &Cancellation::default(),
            &[]
        )
        .unwrap(),
        0
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        !marker.exists(),
        "descendant survived process-group cleanup"
    );
}

#[test]
fn empty_command_and_spawn_failure_are_errors() {
    let cancel = Cancellation::default();
    assert!(run(&[], Path::new("/"), &BTreeMap::new(), None, &cancel, &[]).is_err());
    assert!(
        run(
            &["/does/not/exist".into()],
            Path::new("/"),
            &BTreeMap::new(),
            None,
            &cancel,
            &[]
        )
        .is_err()
    );
}

#[test]
fn capture_collects_both_streams_and_exit_code() {
    let (code, output) = capture(
        &[
            "/bin/sh".into(),
            "-c".into(),
            "printf version-1; printf warning >&2; exit 3".into(),
        ],
        Path::new("/"),
        &BTreeMap::new(),
        None,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(code, 3);
    assert!(output.contains("version-1"));
    assert!(output.contains("warning"));
}

#[test]
fn capture_errors_on_overflow_and_stops_the_process() {
    let started = Instant::now();
    let error = capture(
        &[
            "/bin/sh".into(),
            "-c".into(),
            "while :; do printf '%01000d' 0; done".into(),
        ],
        Path::new("/"),
        &BTreeMap::new(),
        Some(Duration::from_secs(5)),
        &Cancellation::default(),
    )
    .unwrap_err();
    assert!(error.contains("1 MiB"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
