# pctl

pctl plans and runs repository operations from TOML. The first implementation provides a Rust library and a CLI for task discovery, dependency planning, sequential process execution, and environment contract validation.

## Install

Install the latest statically linked Linux release on an `x86_64` or `aarch64` host:

```sh
curl -fsSL https://raw.githubusercontent.com/bernard-ng/pctl/main/deploy/install.sh | sudo bash
```

Containers that already run as root do not need `sudo`. Set `PCTL_INSTALL_DIR` to choose another destination:

```sh
curl -fsSL https://raw.githubusercontent.com/bernard-ng/pctl/main/deploy/install.sh | PCTL_INSTALL_DIR=/opt/bin bash
```

Remove the executable while preserving project manifests and local state:

```sh
curl -fsSL https://raw.githubusercontent.com/bernard-ng/pctl/main/deploy/uninstall.sh | sudo bash -s -- --yes
```

## Start

```sh
cargo build --locked
cargo run -- list
cargo run -- doctor
cargo run -- plan check --json
cargo run -- run check
```

Run from the project root or select a manifest explicitly:

```sh
pctl --manifest /path/to/project/pctl.toml list
pctl --profile ci plan api.test
```

No parent-directory discovery or automatic tool installation occurs. Rust is needed to build pctl; consumers of the binary only need the tools their tasks invoke.

## Declare tasks

```toml
schema_version = 1
include = [".pctl/tasks.toml"]

[tasks."api.test"]
description = "Run API unit tests"
working_directory = "api"
command = ["php", "vendor/bin/phpunit", "--filter", "{param:filter}"]
profiles = ["local", "ci"]

[tasks."api.test".parameters.filter]
type = "string"
default = ".*"
```

Quote task names containing dots. Every included file declares `schema_version = 1`; include paths are relative to the root manifest directory. Duplicate task or variable definitions, repeated includes, unknown fields, and dependency cycles are errors. There are no implicit overlays. Loading reports every structural problem it finds in one pass, including `{param:name}` references to undeclared parameters, rather than stopping at the first.

```sh
pctl plan api.test --param filter=Login --json
pctl run api.test --param filter=Login
```

Parameters support `string`, `boolean`, `integer`, and `enum`. Defaults and CLI values are strings; booleans accept exactly `true` or `false`. Enums declare `values = ["unit", "functional"]`. Parameters apply to the requested task; dependencies use their own defaults. Unknown, repeated, missing, or invalid parameters fail before execution.

An argument such as `{param:filter}` is replaced as one complete argument. Partial interpolation is rejected. Commands run directly, with no shell expansion. Invoke `sh` or another shell explicitly if required.

A task can declare `depends_on = ["format", "test"]`. Dependencies run in declaration order, once per plan. An aggregate task may omit `command`. Every dependency must permit the selected profile. An omitted `profiles` list permits all profiles, so restrict operational tasks explicitly.

`destructive = true` requires `pctl run TASK --allow-destructive`. The runner checks every working directory and destructive flag before launching the first process. Working directories and manifest includes must resolve inside the project, including through symlinks. These checks do not sandbox the commands themselves.

## Configuration contracts

```toml
[variables.PUBLIC_ENVIRONMENT]
type = "enum"
values = ["staging", "production"]
visibility = "public"
consumers = ["main", "admin"]
required_in = ["staging", "production"]
browser_exposed = true

[variables.DATABASE_URL]
type = "string"
visibility = "secret"
consumers = ["api"]
required_in = ["production"]
```

```sh
pctl config check
pctl --profile production config check --values
```

The first command checks declarations without requiring credentials. `--values` additionally checks the current process environment. Diagnostics name the variable without echoing its value. Only public variables may declare browser exposure. URL validation and application-specific semantics remain outside this initial validator.

Task `environment` maps are literal, non-secret overrides. Declared secrets cannot be stored there. Tasks do not inherit the caller's environment: a child receives only basic OS variables (`PATH`, `HOME`, `TMPDIR`, `LANG`, and similar), anything listed in the task's `pass_environment`, and the declared variables whose `consumers` overlap the task's own `consumers`.

For a declared variable a task consumes, the value comes from, in order of precedence: the task's own `environment` override, the process environment, then the profile's `[profiles.NAME.environment]` default. `config check --values` validates the same precedence. Profile values for variables a task does not consume are ignored, so one profile serves every task; a task override of a variable it does not consume is an error. Profile entries that are not declared variables apply to every task. `--profile` must name a profile the manifest mentions (declared, in a `required_in`, or in a task's `profiles`); `local` is the always-valid default.

No `.env` file is automatically loaded; pass one explicitly with `--env-file`. It accepts `NAME=value` lines, an optional leading `export`, `#` comment lines, trailing ` # comments` after a value, and single or double quotes around a value (removed, with no escapes or expansion). The real process environment takes precedence over the file, and errors name the line, never the value.

Do not pass secrets as task parameters or command literals: JSON plans contain resolved arguments and literal overrides. Values of declared `secret` variables found in the process environment are masked as `[REDACTED]` in child stdout/stderr, including when split across read chunks. Redaction matches exact values only; it cannot hide transformed copies (encoded, truncated, or reformatted).

## Execution and exit codes

`plan` is read-only and does not launch tasks. `run` executes dependencies in order, one task at a time unless `--jobs N` allows independent tasks to overlap (tasks sharing an `exclusive` name never do), and stops launching new work after the first failure, preserving its exit code. CLI usage and configuration errors return 2, so a task that itself exits 2 is indistinguishable by code alone; use `--format json` for the failing task's id. On Unix a child terminated by a signal maps to `128 + signal`, and a task exceeding `timeout_seconds` returns 124.

Each task runs in its own process group with no stdin. Ctrl-C or SIGTERM is forwarded to the group (escalating to SIGKILL after one second), stops scheduling, and exits `128 + signal`. `cleanup` commands (and the `down` step of `compose` tasks) run after the task whether it succeeded, failed, timed out, or was cancelled, each limited to 30 seconds; a failing cleanup fails an otherwise successful task. Programs that deliberately start a new session or process group escape supervision, and SIGKILL or host failure of pctl itself cannot run cleanup. Process supervision requires Unix.

`run --format` selects the report written to stdout (child output always goes to stderr): `terminal` (default), `json`, `junit`, or `ndjson`. `ndjson` streams one JSON object per line while the run is in progress: a `{"event":"task_finished","task":{...}}` line as each task finishes (cached and failed tasks included, in completion order), then one for each task that never started because of an earlier failure or cancellation (`"status":"skipped"`), and finally `{"event":"run_finished","exit_code":N}`. A consumer can therefore follow progress live and treat `run_finished` as the end marker; if it is missing, pctl itself was killed.

`doctor` validates manifests and working directories and runs each declared tool's version probe (concurrently, 10 s each). It reports every failing directory and tool together.

Cache `inputs` and `outputs` may be files or directories. The declared path itself must not pass through a symlink, but symlinks found inside a declared directory are fingerprinted by their target path and never followed, so trees such as `node_modules` can be cached. A task whose dependency is not itself cached always runs uncached.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

See [architecture](docs/architecture.md), [terminology](docs/terminology.md), and the [changelog](CHANGELOG.md).
