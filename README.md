# VetraDB

**A Rust database engine for modern backends: relational SQL, complete transaction history, and transactional backend primitives in one durability boundary.**

> Foundation stage. This repository contains accepted designs, a Rust workspace and runnable verification tooling. There is no usable database implementation or supported release yet. Production readiness is a release gate, not a current claim.

VetraDB is designed to combine an independent page-based storage engine with progressively tested PostgreSQL compatibility. Applications should be able to change relational data, enqueue work, and publish durable events atomically, then inspect how that state evolved.

## Planned capabilities

| Area | Target |
| --- | --- |
| Storage | Native Rust engine, checksummed pages, B+Trees, buffer pool, WAL, checkpoints, crash recovery |
| Transactions | ACID, MVCC, read committed, repeatable read, serializable, constraints and indexes |
| SQL | Relational SQL, joins, aggregation, prepared statements, query planner and cost-based optimization |
| PostgreSQL | Progressive SQL, type, catalog and frontend/backend protocol compatibility; explicit support matrix |
| History | Immutable committed transaction ledger, complete row and schema versioning, transaction metadata, audit queries |
| Time travel | Transaction-basis and timestamp AS OF reads, history queries, pinned read snapshots |
| Work | Transactional queues, background and delayed jobs, fenced leases, retries, priorities, dead letters, cron |
| Events | Transactional publish, durable pub/sub, CDC, resumable realtime subscriptions |
| Deployment | Embedded Rust library and server sharing the same engine; single node first |
| Operations | Authentication, authorization, resource limits, observability, verified backups, PITR and upgrades |
| Future | Replication, read replicas, consensus and high availability after the single-node release |

The history model is inspired by Datomic's treatment of transactions and time. VetraDB's planned data model is relational SQL; it does not promise Datomic API or storage compatibility. See the [design references](docs/references.md).

## Design commitments

- One transaction manager and commit path for user tables, history, queue state, job scheduling, durable events and consumer offsets.
- Acknowledged commits survive process crashes and power loss on the documented storage platform. Aborted transactions never appear in committed history or event streams; a lost commit response has an explicitly unknown outcome until checked.
- Immutable means committed logical history cannot be rewritten by ordinary SQL. Physical compaction can rewrite pages without changing historical facts.
- Complete history retention is the default. Expiration or administrative redaction requires an explicit policy, an audit record and an observable reduction in the available historical range.
- PostgreSQL compatibility grows through reproducible client and semantic tests. Unsupported features fail explicitly.
- Worker execution and network delivery are at least once. External side effects require application idempotency; they cannot join the database commit.

## Start here

- [Documentation map](docs/index.md)
- [Architecture and commit protocol](docs/architecture.md)
- [Storage, WAL and recovery](docs/storage.md)
- [Temporal and history contract](docs/temporal.md)
- [Queues, jobs and scheduler](docs/work.md)
- [Events, CDC and realtime](docs/events.md)
- [PostgreSQL compatibility](docs/compatibility.md)
- [Roadmap and milestone gates](docs/roadmap.md)
- [Development issue index](docs/planning/issue-index.md)
- [Workspace setup and verification](docs/development.md)
- [Foundation implementation evidence](docs/specs/bootstrap-evidence.md)

## Contributing

Start with [CONTRIBUTING.md](CONTRIBUTING.md). Development is sequenced by milestone and explicit issue dependencies. Architectural changes need an RFC or ADR; correctness claims need reproducible evidence. The foundation workspace and CI are bootstrapped; most engine crates currently declare module boundaries. Run `python3 tools/verify.py` for the maintained checks. Server/storage/SQL behavior remains unimplemented.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
