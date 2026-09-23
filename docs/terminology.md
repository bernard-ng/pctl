# Terminology

- **Project**: a repository with an operations manifest.
- **Task**: a named operation with declared inputs and dependencies.
- **Parameter**: an explicit, typed input to a task invocation.
- **Profile**: a named execution context used to select task availability and configuration requirements.
- **Contract**: declared requirements for an operational input or output.
- **Plan**: an ordered description of resolved tasks for an invocation.
- **Execution**: one attempt to run a plan.
- **Artifact**: an output retained for later consumption or inspection.
- **Policy**: a rule determining whether an operation is permitted.
- **Capability**: an external facility needed to execute an operation.
- **Adapter**: an implementation at a seam between pctl and an external facility.

Artifacts, capabilities, and broader policies are design vocabulary for subsequent versions; the initial schema does not implement them as standalone records.
