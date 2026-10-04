# Documentation

All capabilities in these documents are planned unless a released support matrix explicitly records them as tested. The foundation workspace and verification tooling exist; database behavior remains unimplemented.

| Document | Purpose |
| --- | --- |
| [Product contract](product.md) | Target workloads, scope, examples and non-goals |
| [Architecture](architecture.md) | Modules, transaction boundary, commit protocol and deployment modes |
| [Foundation contract](specs/foundation.md) | Release vocabulary, boundaries and journey-to-task trace |
| [Module policy](specs/modules.md) | Acyclic Rust ownership, MSRV/targets and dependency review |
| [Format v1 candidate](specs/persistent-format-v1.md) | Byte layouts, WAL/envelope framing and recovery boundaries |
| [M00 completion](specs/m00-completion.md) | Foundation exit evidence and governance/disclosure verification |
| [Development](development.md) | Maintained workspace/CI commands and deferred workflows |
| [Conformance contract](specs/postgresql-conformance.md) | Feature inventory, negative wire cases and driver/ORM pinning |
| [Fault/oracle contract](specs/fault-oracles.md) | Evidence levels and independent bounded models/oracles |
| [Bootstrap evidence](specs/bootstrap-evidence.md) | Actual #17–#19 results and platform limits |
| [Foundation evidence](specs/foundation-evidence.md) | Runnable fixture checks, hand-worked recovery traces and accepted designs |
| [Storage](storage.md) | Page/B+Tree organization, WAL, recovery and format invariants |
| [SQL](sql.md) | Relational semantics, catalog, constraints, types and execution |
| [Compatibility](compatibility.md) | PostgreSQL reference baseline, stages and client evidence |
| [Temporal](temporal.md) | Versioning, AS OF, history, metadata, retention and snapshots |
| [Work](work.md) | Queue/job lifecycle, claims, retries, cron and atomicity |
| [Events](events.md) | Pub/sub, CDC cursors, initial snapshot handoff and realtime |
| [Security](security.md) | Trust boundaries, grants, secret handling and threat model |
| [Operations](operations.md) | Deployment, backup/PITR, maintenance, upgrades and resource limits |
| [Verification](verification.md) | Fault model, concurrency checks, differential tests and release gates |
| [Replication](replication.md) | Future consensus, fencing, snapshots and failover contracts |
| [Roadmap](roadmap.md) | Milestones, dependencies and evidence-based release scope |
| [Risks](risks.md) | Tradeoffs, research questions, decisions and mitigations |
| [ADRs](adr/README.md) | Initial architectural decisions and change process |
| [References](references.md) | Primary technical references and how they inform this design |
| [Issue index](planning/issue-index.md) | Published epics, implementation tasks and dependency links |
| [Backlog manifest](planning/backlog.json) | Machine-readable snapshot of the initial issue plan |

Normative terms: MUST is an acceptance requirement; SHOULD permits a documented and reviewed exception; MAY is optional. Proposed SQL extensions and API names are design examples, not executable or released interfaces. Each implementation issue must finalize its API before shipping it.
