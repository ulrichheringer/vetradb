# Verification and release evidence

Production-grade means the engine has evidence for its stated contracts under failure and concurrency. The Rust foundation workspace, native M01 storage, and independent reference models/oracles now run. See [M01 implementation evidence](specs/m01-storage-evidence.md) for real page/tree tests and their volatile recovery boundary. The [M02 native core](specs/m02-transaction-evidence.md) now runs real WAL/recovery, transaction, independent isolation and process-kill tests. No SQL server, database benchmark or production qualification exists yet. The [foundation fixture checker](specs/foundation-evidence.md) verifies proposed byte examples and module dependencies; it does not qualify storage/recovery behavior.

Maintained checks and CI are documented in [development.md](development.md); [fault-oracles.md](specs/fault-oracles.md) defines evidence levels and remaining fault-provider requirements. [Bootstrap evidence](specs/bootstrap-evidence.md) records actual local results and remote-platform limits.

## Layers

| Layer | Required evidence |
| --- | --- |
| Encodings/pages | Golden format fixtures, round trips, bounds checks, malformed-data fuzzing |
| B+Trees | Random operation sequences against a reference map, structural validation, crash during split/merge/root changes |
| WAL/recovery | Short/torn writes, failed sync, ENOSPC, reorder model, crash after every durable boundary, repeated crash during undo |
| MVCC/constraints | Dirty/nonrepeatable/phantom reads, lost updates, write skew, unique/FK races, lock ordering and deadlocks |
| SQL | sqllogictest-style corpus and differential results/errors against pinned PostgreSQL for the declared subset |
| PostgreSQL wire | Real drivers/pools/ORM journeys, auth/TLS, binary codecs, portals, Sync recovery, COPY and cancellation |
| Temporal | Golden commit histories with DDL changes, AS OF boundaries, row diff, savepoint exclusion, retention and pins |
| Work | Fake clock plus real restart, fenced stale completion, duplicate claims, retry budgets, cron DST/misfire cases |
| Events | Committed-only order, initial-snapshot handoff, duplicates, cursor expiry, slow-consumer limits and grant changes |
| Operations | Verified online backup/PITR, corrupt/missing files, interrupted upgrade, shutdown and disk-pressure drills |
| Security | Protocol/parser fuzzing, secret-redaction checks, authorization matrix, dependency/unsafe review |
| Future replication | Network partitions, reordered/duplicated messages, stale leader, quorum loss, failover with active jobs/streams |

## Durability oracle

Run workloads with an external oracle recording successful acknowledgments and transaction identities. After a modeled crash/power failure, every acknowledged durable commit must exist with matching relational state, ledger, indexes, work and events. Transactions with an unknown client outcome may be fully committed or fully absent; partial state is always a failure. Explicitly aborted transactions must be absent from committed history and deliveries.

A process kill only tests process-crash behavior. The I/O simulator must model loss of unsynced writes and torn/reordered persistence; power-loss claims also require representative platform/storage evidence. Publish what a test actually proves.

## Concurrency oracle

Maintain small reference histories and verify outcomes against the advertised isolation mode. Serializable histories must admit a valid serial order; repeatable read must preserve its snapshot while documenting allowed write skew; read committed refreshes statement snapshots and rechecks locked rows as specified. Durable queue claims and acknowledgments are checked as atomic state transitions. Model-check small lock, lease, scheduler and commit-publication state machines.

## Resource and performance evaluation

Use fixed reproducible workload definitions: indexed CRUD, mixed read/write, joins/aggregation, hot-key contention, temporal scans/diffs, queue bursts/delays, CDC/realtime fanout, large values/transactions and backup under load. Report hardware, filesystem, configuration, data size, retained history, driver versions, warm/cold state, percentiles, memory, disk/WAL amplification and recovery time. Compare only matching durability/isolation settings.

Numeric SLOs and capacity limits are set from measurements at M08 and frozen for the release candidate. Long-running tests include reader/consumer pins and an adversarial slow client so boundedness is verified. Performance improvements cannot change logical results or weaken durability.

## 1.0 release gate

- All M00-M09 acceptance work complete; M10 qualification evidence reviewed.
- Every supported production platform passes crash/recovery and restore coverage; unsupported platforms are labeled accurately.
- Declared SQL/driver/type matrix passes against the pinned reference and has documented deviations.
- ACID, serializable isolation, temporal reconstruction and cross-feature atomicity pass their independent oracles.
- Security review has no unresolved release-blocking issue; disclosure, dependencies and unsafe code are reviewed.
- Backup/PITR and supported upgrades work on release artifacts; operators can diagnose disk pressure and stuck pins.
- Resource limits and measured capacity/SLOs are documented; a sustained mixed workload passes its published soak window, initially proposed as at least 72 hours.
- Crates/binaries/container are reproducible and accompanied by checksums, SBOM, release notes and a support policy.

The qualification milestone must choose exact test budgets/hardware and approve the soak requirement before the release candidate. No fixed calendar date bypasses these gates.
