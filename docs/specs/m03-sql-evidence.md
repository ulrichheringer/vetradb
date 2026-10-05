# M03 native relational SQL evidence

This implementation connects owned SQL sessions, typed values, catalog binding and relational execution to the M02 transaction core. SQL has one statement basis, one rollback boundary and the same commit/envelope/physical row journal as other native participants. It is an experimental bounded single-node subset; there is no claim of blanket PostgreSQL compatibility.

## Acceptance mapping

| Task | Implementation | Qualification |
| --- | --- | --- |
| SQL-001 #42 | `catalog::Catalog`: database schema/table/column/index/view identities, transactional immutable schema snapshots, retained old definitions through the ledger, catalog epoch locks | DDL rollback/reopen; drop/recreate identity changes; prepared epoch invalidation; current and historical lossless value codecs |
| SQL-002 #43 | Pinned PostgreSQL `sqlparser` AST, 1 MiB input, 128 statements, grammar recursion/nesting 16 and AST depth 32, 4096 expression nodes; syntax diagnostics retain tokenizer locations | Quoted identifiers, literal/comment semicolons, malformed/deep inputs, unsupported arrays/stored expressions/functions; parser errors before user effects |
| SQL-003 #44 | `types::sql::{Scalar,Type}`: lossless values, exact decimal, half-away numeric typmods, SQL comparisons, C byte ordering, ordered composite keys, date/time/microsecond timestamps | All scalar codecs and negative/NULL/NaN/decimal ordered-key comparisons; overflow/UTF-8/precision/timezone/OID cases; PostgreSQL boundary fixtures |
| SQL-004 #45 | `planner::Binder/Plan`: static names, aliases, parameter types/result metadata, schema epoch and exact basis; preparation performs no DML | Parameter/result OIDs, invalid/ambiguous names on empty inputs, boolean type errors, stale prepared plan and authorization tests |
| SQL-005 #46 | Atomic schemas/tables, add/drop/rename/type/default/nullability migrations, dependency-aware drops, views and durable schema publication | Migration/backfill uses private statement savepoint; restart; renamed CHECK expressions; historical stable column IDs and dropped object identity remain in envelopes |
| SQL-006 #47 | Typed multi-row INSERT, UPDATE, DELETE and RETURNING over native physical primary/version trees; row identity separate from primary key | Failed row two removes row one; explicit failed state/rollback-to; PK update keeps row identity; actual restart and forty failed publication boundaries |
| SQL-007 #48 | Primary/composite/nonunique index entries in native physical row storage, within original transaction; equality point access and baseline scan | UNIQUE NULL distinctness, indexed point versus baseline after failed sync/reopen; statement basis; staged index publication uses the same native WAL batch |
| SQL-008 #49 | Immediate PK/UNIQUE/NOT NULL/CHECK/FK with all SQL writers protected by bounded strict locking; FK RESTRICT/NO ACTION; deferred/cascade modes rejected | Duplicate contender and child/parent races under RC/RR/serializable; multi-row failures; SQLSTATE 23502/23503/23505/23514, persisted negative cases |
| SQL-009 #50 | Scans, three-valued WHERE, projection, DISTINCT, ORDER BY ASC/DESC/NULL placement, LIMIT/OFFSET, cancellation and operator budgets | Deterministic result/OID corpus; missing columns on empty tables; execution limit/cancel returns no partial statement; baseline scan remains available |
| SQL-010 #51 | Bounded inner/left/right/full/cross nested-loop joins; all sources share statement catalog/basis | Duplicate multiplicity, empty/NULL extension, joined predicate truth; differential outer joins; RC freeze across native reads |
| SQL-011 #52 | COUNT/SUM/AVG/MIN/MAX, DISTINCT aggregates, grouping, HAVING, FILTER, exact numeric accumulation | Empty aggregate outputs, NULL count, int4 sum OID, group/HAVING golden cases; explicit execution/memory limits |
| SQL-012 #53 | Stored defaults/generated immutable expressions, durable identity reservation with gaps, ON CONFLICT DO NOTHING/DO UPDATE | Actual default/upsert journeys and stored results; identity 1/rollback gap/3/restart/4; old committed user values never redrawn |
| SQL-013 #54 | Correlated EXISTS/IN/scalar queries, non-recursive CTEs, versioned views/dependencies, row_number/rank/dense_rank and aggregate windows | CTE/correlation/NULL IN fixtures; duplicate window peers; dependent drops rejected; unsupported recursive/explicit frame/named-window constructs rejected |
| SQL-014 #55 | Versioned scalar inventory below, JSON access/containment and checked casts | Missing path vs JSON null vs SQL NULL; exact JSON number parse; scalar signatures/type OIDs; unsupported signatures fail explicitly |

Sources are under `crates/`. `cargo test --locked -p vetra-types --test sql -p vetra-engine --test sql` runs value and native SQL qualification. The maintained `python3 tools/verify.py` includes these tests alongside M00–M02 regression gates.

`fixtures/sql/postgresql-17.json` contains 49 independently executed SQL cases; `postgresql-17-results.json` stores results, NULLs, result OIDs and SQLSTATEs returned by **PostgreSQL 17.10**. `tools/sql_differential.py` uses an independent bounded Python standard-library text protocol client against an isolated disposable loopback reference database and runs the actual file-backed `sql-fixture` binary. To regenerate, run PostgreSQL 17 with user postgres/database reference and execute `python3 tools/sql_differential.py --port 15439 --record --reset`. `--reset` explicitly discards that reference database's public schema. This is a test oracle, never a production client or authentication implementation.

Native tests add unique/FK concurrency races at all supported isolation levels, private statement rollback, physical restart, 40 injected SQL publication failures with external successful-reply expectations and index/row comparisons, authorization, identity gaps/recovery, cancellation and resource pressure. Existing M02 process-kill and physical restart qualifications remain applicable to the shared commit core. E1 modeled persistence and E2 process crash do not imply E3 power-loss qualification.

## Implemented subset and bounds

`Database::transactions()` remains the native participant API. `engine::sql::Session` (re-exported by embedded) is the owned SQL facade. `Access` is supplied by a trusted authenticated adapter: default denies all, explicit admin permits catalog DDL, and table IDs grant read/write. Native raw participant contexts are trusted internal adapters; they are not untrusted SQL admission. SQL/COPY/service adapters must enter the checked typed boundary.

DDL persists catalog snapshots under Schema object 100; user rows under Row object 101; internal durable non-rollback identities under Offset object 102; conservative constraint epoch under Offset object 103; serving indexes under Row object 104. Each new object/column/index/definition reserves a fresh identity. Internal reservations are visible as explicitly typed system allocator control operations, never as rolled-back user-row effects. Opening still starts no workers or network listeners.

Catalog shared/exclusive locks last to transaction end. SQL writers use conservative global serving-row protection and a constraint epoch. This deliberately serializes writes and can reject RR writes after other SQL commits even when finer locking could allow them; it never downgrades isolation. Native lock waits/deadlock/cancellation/resource limits remain the M02 limits. Read committed statements freeze a basis across all operators, then refresh on the next statement. Statement failure rolls back its own staged rows/index/catalog effects and marks the explicit transaction failed. SQL savepoint names starting `__vetra_` are reserved for adapter boundaries.

Catalog bounds: 4096 retained tables/views, 256 schemas/columns per table, 1 MiB encoded catalog. Values at most 1 MiB; numeric at most 1000 digits/absolute decimal scale 1000; index keys at most 2048 bytes; native transaction staging remains 16 MiB. SQL operator defaults: 10000 result/input rows, 16 MiB memory and 1000000 work steps. Sort/joins/groups are resident baseline algorithms. There is no spill/performance capacity claim; the optimizer milestone is separate.

Types: boolean, int2/int4/int8, float4/float8, text/varchar, hex bytea, UUID, finite exact numeric, date, time, timestamp and explicitly offset RFC3339 timestamptz, JSON/JSONB. Initial ordering is C byte order. Float NaNs sort above infinity and compare equal; signed zero compares equal while its persisted bits remain lossless. Numeric special NaN/infinity, arrays, locale providers, BC/infinite timestamps and implicit timezone conversions are unsupported. Explicit varchar overlength returns 22001 (rather than PostgreSQL's explicit-cast truncation); timestamp values use microseconds and UTC instants, with no session timezone conversion yet. These deviations are not part of the promoted differential corpus.

Lossless SQL value codec `VSQLV001` and catalog codec `VCAT0001` are described by [ADR 0020](../adr/0020-m03-relational-core.md). Old definitions/row images remain in the native ledger even when current serving catalog objects are dropped. M05 adds historical query admission and explicit snapshots; current indexes cannot be used to claim historical SQL support prematurely.

Supported FK actions are immediate RESTRICT/NO ACTION. Self-referencing table-level FKs are supported; deferred modes, cascade/set-null/default actions, expression/partial/concurrent indexes, index ordering modifiers, CASCADE DDL, TEMP tables, generated sequence options and stored user code are rejected. Supported ALTER performs synchronous bounded backfill; safe dependent table renames update stored views. Incompatible column changes with dependent views/FKs fail precisely. Prepared plans fail 0A000 when their catalog epoch changes, allowing explicit reprepare.

Supported windows: row_number/rank/dense_rank and the aggregate inventory with PARTITION BY/ORDER BY, default whole-partition or peer-inclusive ordered frame. Named or explicit frames, recursion, LATERAL, NATURAL/USING joins and aggregate set ALL variants outside UNION are rejected. Unordered queries promise no ordering; tied row_number may follow resident input order without an extra ordering guarantee.

## Function inventory v1

| Signature | Result | Volatility / NULL contract |
| --- | --- | --- |
| count(*), count(value), count(DISTINCT value), FILTER | int8 | Aggregate; NULL ignored for value |
| sum(int2/int4), sum(int8/numeric), avg(numeric/integer), min/max(value) | int8 / numeric / numeric / argument type | Aggregate; empty input NULL, exact accumulation |
| lower(text), upper(text) | text | Immutable, NULL propagates; Rust Unicode casing, C ordering |
| length(text), octet_length(text) | int4 | Immutable; character/UTF-8 byte lengths |
| abs(numeric/integer) | argument numeric family | Immutable, NULL propagates |
| coalesce(values...), nullif(value,value), concat(values...) | bound argument family / argument family / text | Immutable; coalesce first non-NULL, NULLIF SQL equality, concat ignores NULL |
| now(), current_timestamp, transaction_timestamp() | timestamptz | Transaction timestamp; stored default is evaluated once |
| json_typeof(json), jsonb_typeof(jsonb) | text | Immutable; SQL NULL propagates, JSON null is text `null` |
| JSON `->` integer/text, `->>` integer/text, `@>`/`<@` | JSONB / text / boolean | Missing path SQL NULL; JSON null differs from SQL NULL |
| row_number(), rank(), dense_rank() OVER ... | int8 | Defined partition/order windows |

CHECK/generated expressions cannot contain subqueries, bound parameters, aggregates, windows or non-immutable functions. Default parameters/subqueries are rejected before persistent catalog mutation. Unsupported functions/signatures return 0A000, not invented PostgreSQL OIDs.

## Review and delivery

Dependency rationale, pinned source/checksums/licenses and advisory/MSRV evidence are in [dependency review](dependency-review.md). M03 closure requires its complete platform/toolchain gate and linked PR/CI evidence. Merge/architecture/release ratification remains separate, as for M01/M02. M04 authenticates/exposes these sessions; M05/M06 extend the shared history/work boundary.

JSON/JSONB direct equality, ordering, DISTINCT and index keys are not advertised: they fail explicitly instead of substituting textual JSON ordering. Access and containment use the inventory above.
