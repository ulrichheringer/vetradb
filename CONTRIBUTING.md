# Contributing to VetraDB

VetraDB is at the foundation stage. The Rust workspace and verification tooling build and run; no usable database engine exists yet. Use the [development guide](docs/development.md) and run `python3 tools/verify.py`. Start from the [roadmap](docs/roadmap.md), [issue index](docs/planning/issue-index.md) and architecture documents.

## Work selection

Choose an open task with satisfied dependencies. Discuss ownership in its issue before a large implementation. Epics track milestone gates; do not close an epic merely because its design document exists. Initial issues are intentionally open for implementation/ratification work even when a proposed design is already documented.

An implementation issue must contain a clear contract, scoped behavior, failure cases, dependencies, meaningful verification and evidence needed to finish. Split work that cannot be reviewed coherently; preserve the parent acceptance criteria. New features that change storage, isolation, compatibility or external APIs require an RFC/ADR before their implementation is merged.

## Review requirements

- Keep the engine's single transaction/durability boundary intact.
- Update the specification, supported-feature matrix and negative cases alongside behavior changes.
- Provide reproducible tests appropriate to the behavior: real persistence/concurrency oracles for guarantees that a mock cannot prove.
- Make changes reviewable; avoid unrelated refactors and broad unexplained dependencies.
- Document unsafe Rust, on-disk compatibility and new dependency/license risks.
- Keep secrets, credentials, copied production payloads and unsafe default configurations out of the repository and logs.

## Definition of done

An issue is complete when every acceptance criterion is demonstrated, dependencies are satisfied, required fixtures and behavioral tests pass, public contracts/docs are updated and review evidence is linked. A design issue finishes with an accepted RFC/ADR and concrete follow-up work. A test issue finishes with a runnable oracle and demonstrated failure detection, not a test that merely duplicates implementation structure.

Workspace/MSRV, CI, formatting/linting and unit/integration/documentation gates now exist; commands are documented in the [development guide](docs/development.md). Real database performance budgets and artifact packaging remain their own milestone work. Do not present future engine campaigns as maintained workflows.

## Planning source

[backlog.json](docs/planning/backlog.json) is a versioned snapshot of the initial issue plan, including stable IDs and published links. GitHub issues are authoritative for later discussion and status. If the plan changes, update the affected docs/manifest deliberately; do not overwrite live issue discussions with stale generated bodies. No issue synchronization bot exists yet.

Contributions are licensed under Apache-2.0. A DCO/sign-off policy, if adopted, must be documented through governance review; the initial plan does not require an invented CLA.
