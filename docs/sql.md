# Relational SQL and execution contract

## Core language

The first usable SQL milestone covers schemas/tables, transactional DDL, INSERT/UPDATE/DELETE, RETURNING, SELECT with filtering/projection/order/limit, joins, grouping/aggregates, NULL logic, parameters, transactions and constraints. The 1.0 milestone adds common subqueries, non-recursive CTEs, views, upsert, sequences, generated/default values, supported JSON operations, COPY and common window functions. Each construct gets a compatibility entry; syntax acceptance alone is insufficient.

Initial scalar types: boolean, int2/int4/int8, float4/float8, text/varchar, bytea, UUID, exact numeric, date/time/timestamp/timestamptz, JSON/JSONB. Precision, overflow, NaN ordering, UTF-8, timezone conversion and text/binary codecs must be explicit. Arrays and user-defined types are deferred unless their own complete conformance issues are added. Initial collation is deterministic bytewise/C behavior; locale-aware collation requires versioned providers and index invalidation rules.

## Catalog and DDL

Stable object IDs and schema versions let old row values remain interpretable after rename, drop or type migration. DDL uses the same transaction/WAL path as data. Catalog locks prevent reads/plans from mixing incompatible schema epochs. Historical objects remain resolvable by stable identity within retained history.

DDL dependencies cover foreign keys, views, indexes, default expressions, publications and subscriptions. Unsupported CASCADE behavior fails before any mutation. Constraints and authorization are checked for all write paths, including internal work APIs and bulk import. Catalog introspection exposes accurate supported metadata, not invented PostgreSQL objects.

## Semantics

SQL uses three-valued logic and SQL aggregate/NULL behavior. Errors have typed internal codes mapped to PostgreSQL SQLSTATE, including unique violation, foreign-key violation, serialization failure, deadlock, cancellation and feature not supported. A failed statement inside an explicit transaction enters the failed state until rollback or rollback-to-savepoint as applicable.

Primary/unique indexes protect concurrent inserts and updates. Foreign keys are validated under locks against concurrent delete/update. CHECK expressions cannot access volatile external state. Deferred constraints are a separate scope item; initially unsupported deferrability is rejected explicitly.

Sequences allocate unique values under a documented PostgreSQL-like non-rollback contract: gaps are allowed and aborted allocation is not a committed user-row version. Identity/default expression results used by a committed transaction are recorded in its logical envelope. Sequence allocator durability is an internal recovery concern and must not imply that a rolled-back transaction appears in committed history.

## Query pipeline

Parse -> bind names/types/parameters to a versioned catalog -> normalized logical plan -> safe rewrites -> physical plan -> execution under a transaction snapshot and resource budget.

Initial operators use a simple iterator/batch model with scans, filters, projections, sort, limit, joins and aggregation. Index access, cost estimates, join ordering, hash/merge joins, spill and plan caching grow in the optimizer milestone. Correct baseline scans must be available when an index or optimization cannot preserve visibility or SQL semantics.

Temporal reads are explicit plan properties. Optimizations must preserve the selected schema/commit basis, authorization and visibility; a current-state index cannot answer a historical predicate without a valid historical access strategy. Realtime plans use a restricted eligibility checker instead of treating arbitrary SQL as incrementally maintainable.

EXPLAIN exposes physical operators, estimates, chosen indexes, temporal basis and resource assumptions. EXPLAIN ANALYZE executes a statement with normal effects unless explicitly rolled back by the caller, and must document that behavior. Cancellation reaches scans, waits, spill and network output.

## Proposed extension surface

SQL extensions live in a `vetradb` namespace through functions or table-valued functions so ordinary PostgreSQL syntax remains useful. Proposed names include `vetradb.history`, `vetradb.transactions`, `vetradb.enqueue` and `vetradb.publish`. SQL AS OF syntax versus a session snapshot-basis interface is finalized in the temporal API issue; until then examples describe behavior without claiming a stable grammar.

No arbitrary SQL callback, HTTP request, worker handler or extension hook executes inside the engine commit path.
