# Architecture

The first release is one Cargo package with a library and a CLI. Modules provide separation while the implementation is small. Separate crates are justified when independent consumers, compile boundaries, or release cycles emerge.

## Ownership

| Module | Owns | Dependencies |
| --- | --- | --- |
| `model` | Task, parameter, variable, plan, and value validation | Serde for transport shapes |
| `config` | Manifest loading, includes, structural validation, environment contract validation | Model and filesystem |
| `planning` | Pure dependency resolution and argument binding | Model |
| `execution` | Sequential scheduling, preflight, stop-on-failure behavior, process adapter | Model and process/filesystem facilities |
| binary | CLI parsing, command dispatch, rendering, exit status | Library and Clap |

The planner accepts a manifest and returns a plan without I/O. The execution module accepts a plan and an executor. `ProcessExecutor` is a real substitution seam: production starts OS processes, tests record requests and control outcomes. Filesystem behavior is tested using temporary directories instead of introducing a filesystem trait prematurely.

SOLID is applied through narrow responsibilities, composition, and substitutable execution behavior. There is no inheritance hierarchy, global context registry, or trait for every data type. The library returns errors; only the binary prints and exits.

## Contracts

Manifest schema version 1 is explicit. Includes merge disjoint definitions; collisions fail. Plans preserve dependency declaration order and execute shared dependencies once. Root task parameters never leak into dependencies. Profiles restrict task availability rather than merging arbitrary configuration.

The model currently uses public data structures and a typed error boundary rendered as concise CLI diagnostics. Manifest parsing preserves field paths, while structural invariants are enforced on manifest loading and planning. The executor additionally checks execution-specific invariants before side effects. Stable diagnostic codes and opaque validated plan types are candidates for the next interface revision, before external library compatibility is promised.

Commands use argument vectors. Parameters are resolved as whole arguments. Environment validation uses supplied values and never includes values in errors. A declaration's `consumers` field is metadata in this version; it is not a sandbox or an injection allowlist.

## Extension sequence

1. Add a normal task in TOML. Use existing Composer, Bun, Docker, and Ansible interfaces.
2. Add process supervision with cancellation, timeouts, and tested cleanup semantics.
3. Add a Compose adapter only when it can replace and preserve existing lifecycle scripts.
4. Add generated environment contracts with byte-for-byte drift checks and an explicit source of truth.
5. Add bounded concurrency and resource locks, then cache fingerprints with output verification.
6. Introduce executable extensions with a versioned protocol only when a real external consumer needs them.

The initial release intentionally excludes dynamic plugins, remote execution, cache skipping, deployment policy, configuration generation, and task result caching. Dependencies and test runtimes continue using their existing caches.

Fingerprints must account for declared inputs, relevant non-secret configuration, tool versions, and task definitions. Excluding a secret alone cannot make a cache safe: secret-dependent operations should be uncacheable unless a safe invalidation model exists. A matching fingerprint also requires valid outputs before skipping work.

## Tests

Integration tests use the public library interfaces for planning, profile restrictions, parameter binding, secret-safe diagnostics, path checks, destructive preflight, and failure propagation. CLI tests execute the actual binary and verify JSON output and child exit status. They do not need Docker, PHP, Bun, or production credentials.

Before adopting pctl as the sole E2E runner, add real subprocess tests for cancellation between tasks, process trees, timeout escalation, cleanup errors, and interrupted initialization. Existing lifecycle scripts remain the executable implementations until those tests pass.
