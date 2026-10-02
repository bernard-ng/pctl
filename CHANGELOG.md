# Changelog

## 0.1.3

- Fix: cache fingerprints no longer include the host's ambient `PATH`, `HOME`, `TMPDIR` and similar variables, which made caches miss between runners that differed only in those. Tasks can opt back in with `pass_environment`.
- Stream child output with `poll(2)` instead of a fixed sleep (about 7x faster on 100 MB of output) and skip redaction work when no secrets are declared.
- Hash cache inputs by streaming, canonicalize the project root once, and fingerprint symlinks inside cached directories by target instead of failing.
- Throttle scheduler lock retries, run tool version probes concurrently, and shrink release binaries (thin LTO, stripped).
- Report every manifest validation problem in one pass, validate `{param:name}` references at load time, and aggregate `doctor` findings.
- Include the underlying cause in version-probe failures, and add path context to I/O errors.
- Accept `export NAME=value` and trailing comments in `--env-file`.
- Use typed `TaskStatus` and `Format` values instead of strings.
- Fix: a profile variable no longer fails every task that does not consume it ("not a consumer of X").
- Fix: `config check --values` and `run` now share one precedence (task override, process environment, profile default); previously `run` let the profile beat the process environment.
- Fix: an unknown `--profile` is rejected instead of silently running with no profile settings.
- Fix: `pctl list | head` and similar closed-pipe cases no longer panic.
- Stream `--format ndjson` events as each task finishes instead of printing them all at the end. `RunOptions::on_task_finished` exposes the same hook to library users.
- Kill a task's leftover process group before reaping its leader, so a recycled pid can never be signalled.
- Remove the unused `completions` subcommand and the `clap_complete` dependency.
- Correct the README and architecture notes on environment injection, redaction, supervision and cleanup.

## 0.1.2

- Add scripts for installing the latest Linux release on hosts and containers and removing the executable.
- Build static Linux release binaries that do not depend on the host's glibc version.
- Complete the typed error migration across runtime code and tests.
- Add GitHub Actions quality checks and tag-driven Linux release artifacts for `pctl`.
- Add a Rust library and CLI for declarative repository operations.
- Load modular TOML manifests with strict schema validation and duplicate detection.
- Plan dependencies deterministically with profile restrictions and typed parameters.
- Execute processes sequentially with preflight checks and child exit-code preservation.
- Validate environment contracts and browser exposure declarations without echoing values.
- Add self-hosted development tasks, interface tests, and architecture documentation.
