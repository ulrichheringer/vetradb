# Replication and high availability: future phase

These features start after single-node correctness and the 1.0 gate. They are planned separately from PostgreSQL wire compatibility and native CDC. No replication or HA capability exists yet.

## Initial replication target

One writable leader per database, a quorum-replicated logical mutation log, and followers applying deterministic committed envelopes to their own page/WAL stores. The exact consensus implementation is selected in a dedicated RFC; a reviewed Raft library is the first candidate. Do not build bespoke consensus without a documented reason and verification strategy.

The log carries stable object IDs, generated values, schema changes, history, job/queue mutations, events and offsets. Followers do not rerun SQL, cron expressions, clock reads, random jitter or application handlers. Consensus order determines committed sequence order. A leader publishes/acknowledges only after the selected quorum durability guarantee is satisfied; local page checkpoints may lag.

## Decisions to ratify

Define logical log framing/versioning, CSN allocation across leader terms, cluster/database/timeline identities, commit/apply watermarks and how the local WAL transaction maps to the consensus index. Migration from a single node must not renumber already committed history. Rollout/rollback rules prevent a node with an incompatible envelope or page format from joining.

Separate asynchronous read-replica mode from quorum HA. Read replicas expose a committed applied-basis watermark and documented stale reads. Linearizable reads use a consensus read barrier; read-your-writes requires an explicit minimum basis. Disconnected followers never serve a claim of fresh data without proof.

## Snapshots and membership

Bootstrap uses a verified physical/logical snapshot plus subsequent replicated log, including ledger/history and service state. Install is crash-safe. Log compaction waits for replica catch-up or expires/rebuilds the replica explicitly. Membership changes use the consensus protocol, authenticated peers and compatible format ranges. Snapshot transfer is bounded, resumable and integrity checked.

## Leadership and backend services

Use a term/epoch in addition to job fencing generation. A stale leader must not acknowledge writes, issue valid job claims, advance schedules or commit offsets. Only the current service owner materializes cron occurrences and dispatch leases; occurrence uniqueness and replicated transitions remain authoritative.

Failover may repeat external side effects or deliveries because acknowledgments can be lost. Job idempotency and at-least-once event semantics continue to apply. A restore into a new cluster/timeline invalidates old tokens and consumer positions unless an explicit migration contract proves continuity.

## Failure and operations gates

Test partitions, leader crash after quorum commit before response, delayed messages, snapshot interruption, clock skew, disk failure, minority isolation and membership change under load. Verify no split-brain acknowledgments, no partial cross-feature transaction, committed-prefix preservation, bounded retention and accurate read consistency.

Publish quorum configuration, recovery procedures, RPO/RTO measurements, backup/archive integration and failover drills. Operator tooling covers bootstrap, join/remove, lag, leadership, draining and forced-recovery warnings. Multi-region latency, sharding and distributed SQL remain separate future RFCs.
