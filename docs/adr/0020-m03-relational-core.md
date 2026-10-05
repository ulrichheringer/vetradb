# ADR 0020: bounded relational SQL on the shared native core

Status: implemented experimental native contract; merge/release ratification separate.

## Decision

Use the pinned PostgreSQL AST grammar and a static owned binder with explicit catalog epoch and statement basis. Keep SQL types in the runtime-free types crate, catalog ownership above transactions, and execution above the planner. All DDL/DML/index effects use the M02 typed mutation/envelope/physical journal boundary. Networking remains outside embedded.

Use conservative catalog and SQL writer locking first. Every statement freezes RC visibility across operators, uses a private savepoint and rolls back all staging on error. SQL RR writers also validate a common constraint epoch; extra retries and serialized writes are preferable to unproved concurrent uniqueness/FK behavior. A later optimizer/finer locking change needs equivalence fixtures.

Store each SQL scalar as `VSQLV001` followed by bounded UTF-8 JSON of the explicitly tagged `Scalar` enum. Exact numeric stores its decimal string; floats store IEEE u64 bits, including NaN/infinity/signed zero. Date stores Unix-epoch days; time/timestamps store microseconds, timezone-bearing timestamps are UTC instants. SQL values are carried as the existing envelope Bytes value under stable column IDs; the native v1 tag/frame/checksum contract is unchanged.

Store catalog snapshots as `VCAT0001` followed by bounded UTF-8 JSON of `Catalog`, `Table`, `Column`, `Index`, `ForeignKey` and `View` definitions. Unknown fields/variants fail; stable nonzero IDs, references, uniqueness and resource bounds are validated. Each DDL snapshot has a fresh schema epoch; old complete snapshots remain in committed ledger history. Bounded serde JSON is a versioned internal codec, not a PostgreSQL on-disk format or blanket public extension format.

Internal Offset allocator transactions reserve global identities and per-column identity sequences durably before user publication. IDs are never reused and user abort permits gaps. These are explicit system control operations in the ledger, distinct from user-row commits. Generated/default values are stored in original user images; replay does not rerun expressions.

Serving secondary entries are ordinary native physical row/version records in the protected index namespace. Composite keys concatenate prefix-free ordered values and add stable row identity for nonunique/NULL-distinct entries. Unique entries and row/constraint changes publish atomically. Equality access validates a visible pointer to the physical user row; baseline scans remain the oracle and fallback.

## Evidence and tradeoffs

The [M03 evidence](../specs/m03-sql-evidence.md) maps every task to code and tests. The reference corpus was actually executed against pinned PostgreSQL 17.10 and compares rows/NULL/OIDs/SQLSTATE. Native fault/concurrency tests exercise actual physical row/index publication. Resident operator/catalog limits, explicit unsupported grammar and timestamp/casting deviations are documented. Finer write parallelism, spilling, blanket SQL/client compatibility and production qualification are separate work.
