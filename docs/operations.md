# Operations and lifecycle

## Configuration and supported platforms

Versioned configuration defines data/WAL/archive paths, listen endpoints, TLS/authentication, memory and spill budgets, transaction timeouts, checkpoint policy, retention, background services and resource quotas. Unknown keys and unsafe combinations fail early. Secrets use providers/files with restrictive permissions and are never printed in effective-config output.

1.0 production targets are Linux x86_64/aarch64 with tested local filesystems and documented sync guarantees. macOS and Windows receive support only at the level proven by their platform-specific fault/restore tests. Network filesystem shared ownership is not supported initially. Data directories have an explicit format, database identity, timeline and exclusive lock.

## Startup and shutdown

Lifecycle: validate -> exclusive lock -> recovery -> integrity checks -> read services -> optional workers/dispatch -> ready. Readiness is false until recovery finishes and the database can honor its durability mode. Liveness is not a promise of writable storage.

Graceful shutdown stops new requests/claims, drains bounded in-flight transactions, cancels remaining work after a deadline, flushes required WAL and closes handles. A forced kill remains recoverable. Recovery, backup and migrations expose progress and cancellation only where cancellation is safe.

## Backup and restore

Online backups capture a consistent physical basis and versioned manifest: database/timeline, page/catalog/ledger roots, checksums, file identities, checkpoint/required WAL range, format and feature versions. Register a backup retention pin before selecting the basis. Stream required pages/WAL, verify them, then mark the backup complete atomically. A directory copy while the engine runs is not a supported backup.

Restore writes into a fresh directory, checks every manifest/file, applies required WAL and verifies data/history/service state at the same commit basis. Archived WAL enables PITR to a transaction or timestamp under documented recovery rules. Targets earlier than the backup's recoverable basis are rejected; historical SQL queries are a separate way to inspect older retained data. Recovery time and recoverable data loss are measured, not assumed. A missing archive segment fails restoration precisely.

A restore fork receives a new timeline. Workers, scheduler and outbound dispatch start paused by default, because a restored job/event state can repeat external effects. The operator chooses a resume policy and consumer bootstrap procedure. Query snapshots and backup snapshots are different objects.

## Maintenance, retention and disk pressure

Expose checkpoint progress, vacuum/horizon state, oldest history basis, replay/archive/replica pins, WAL growth and estimated free space. An operator can inspect a pin's owner and expiry. Expiring a pin has an explicit effect such as snapshot invalidation or consumer resnapshot; silently dropping a guarantee is prohibited.

Disk pressure reserves enough capacity for abort/CLR/recovery operations, stops or throttles new writes predictably and produces actionable diagnostics. A flush failure puts the write path into a failed state until reopen/recovery. Never keep acknowledging commits after a failed durability operation.

## Observability

Structured redacted logs and metrics cover throughput/latency by class, WAL flush/group size, dirty/pinned pages, checkpoints, recovery, locks/deadlocks, aborted/unknown-outcome transactions, query/spill budgets, oldest snapshots, history size, queue age/leases/retries, scheduler lag, CDC/realtime lag and consumer expiry. Tracing IDs may be linked to safe transaction metadata; raw data is opt-in and governed by redaction.

Diagnostics include EXPLAIN, lock/pin inspection, integrity verification, format version, capability matrix and effective safe configuration. High-cardinality row/job IDs do not become unbounded metric labels.

## Distribution and upgrades

Plan signed/tagged releases, Rust embedded package, server/CLI binaries, checksummed artifacts, an SBOM and a non-root container with persistent volumes and termination behavior. Production operators get a minimal deployment guide, capacity-sizing method, backup automation examples and incident runbooks.

Publish file-format/readable-version ranges, export/import guarantees and offline/online upgrade constraints. Validate interrupted migration, rollback feasibility and restoring a pre-upgrade backup. A binary version change must not accidentally rewrite storage before compatibility validation.

## Future HA

Single-node backups are not HA. Replication/consensus/failover add independent operational gates in [replication.md](replication.md), including lag, quorum loss, leader fencing and schedule/lease ownership.
