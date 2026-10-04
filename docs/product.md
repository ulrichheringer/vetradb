# Product contract

## Objective

Build an open-source Rust engine for application backends that need relational correctness, explainable state changes and durable asynchronous work. Data and backend primitives share one transaction boundary and one authoritative committed history.

Initial target: a single database on a single host, embedded in one Rust process or served over a PostgreSQL-compatible endpoint. A server may manage multiple independent databases, but transactions, ordering, queues, topics, snapshots and retention are scoped to one database. Cross-database transactions are outside 1.0.

## Representative journeys

1. **Order and work:** atomically create an order, decrement stock, enqueue fulfillment and publish an order event. A rollback leaves none of them committed. After a restart, the order, ledger and ready work agree.
2. **Explain an update:** join a row's change history to trusted actor identity and caller-supplied request metadata. A point-in-time query reconstructs both the row and its schema at the selected commit.
3. **Worker crash:** claim a job, lose the worker, reclaim after the lease expires, and reject the stale worker's acknowledgment using a fencing token.
4. **Reconnect a consumer:** resume after a durable cursor, replay duplicates safely and receive a specific resnapshot error if retention has removed the required range.
5. **Bootstrap realtime:** read an authorized consistent snapshot, then stream every later matching committed change without a race between snapshot and subscription registration.
6. **Recover a host:** restore a verified backup and archived WAL into a new data directory, recover to a selected commit, then start with workers and dispatch paused until the operator approves delivery.
7. **Use an existing client:** connect with a version-pinned PostgreSQL driver, bind typed parameters, commit/rollback, cancel a long query and receive accurate SQLSTATE and transaction-state responses.

## Scope

All user tables are versioned. User schema changes and supported internal work/event state changes have history. Current reads remain ordinary relational queries. History retention, publication retention and recoverable WAL retention are separate policies.

Transaction time records when the engine committed information, not when the business event occurred. Business valid-time columns can be modeled by applications. Native bitemporal correction is future work.

Durable queue claims, scheduler occurrences, acknowledgments, event publication and consumer offsets are database mutations. User code runs in application workers, never as arbitrary native code inside the storage process.

## 1.0 boundaries

- Disk-backed single node; Linux x86_64 and aarch64 on documented local filesystems are the initial production platforms. macOS is a development/embedded validation target; Windows requires its own durability evidence before support.
- Own storage engine; dependencies for parsing, cryptography, checksums and runtime support may be used after review. SQLite, PostgreSQL or RocksDB are not the authoritative store.
- PostgreSQL 17 SQL/behavior reference, protocol 3.0 baseline, selected clients and a published compatibility subset. Subsequent reference versions get separate evidence.
- Durable historical queries and complete retained transaction history, not a claim of legal non-repudiation or tamper-proof storage against a privileged host attacker.
- No distributed transactions, sharding, cross-node execution, PostgreSQL extensions, PL/pgSQL, arbitrary stored code, or transparent drop-in PostgreSQL replacement in the initial release.

## Readiness

Production readiness requires passing the [verification gates](verification.md), operational restore drills, reviewed format and upgrade policies, security evidence and a supported compatibility matrix. Feature availability alone cannot satisfy this requirement. Capacity and latency limits will be published from measured workloads; no performance number is promised before measurement.
