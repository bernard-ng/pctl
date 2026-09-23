# pctl

pctl plans and runs repository operations from TOML. The first implementation provides a Rust library and a CLI for task discovery, dependency planning, sequential process execution, and environment contract validation.

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

Quote task names containing dots. Every included file declares `schema_version = 1`; include paths are relative to the root manifest directory. Duplicate task or variable definitions, repeated includes, unknown fields, and dependency cycles are errors. There are no implicit overlays.

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

Task `environment` maps are literal, non-secret overrides. Declared secrets cannot be stored there. Children inherit the caller's environment; pctl does not yet filter it by `consumers`. Consumer metadata records intended ownership only. No `.env` file is automatically loaded.

Do not pass secrets as task parameters or command literals: JSON plans contain resolved arguments and literal overrides. Child stdout/stderr pass through unchanged and are not redacted.

## Execution and exit codes

`plan` is read-only and does not launch tasks. `run` executes sequentially and stops at the first failure, preserving its exit code. CLI usage and configuration errors return 2. On Unix a child terminated by a signal maps to `128 + signal`.

This initial process adapter uses ordinary foreground process execution. Dedicated process-group supervision, runner-directed cancellation, timeouts, and guaranteed cleanup are pending. Use existing lifecycle scripts for container workflows until those capabilities are implemented. SIGKILL and host failure cannot guarantee in-process cleanup in any implementation.

`doctor` validates manifests and working directories. It does not yet probe tool versions or capabilities.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

See [architecture](docs/architecture.md), [terminology](docs/terminology.md), and the [changelog](CHANGELOG.md).
