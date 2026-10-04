# PostgreSQL conformance and native extension contract

Accepted foundation design for [#17](https://github.com/ulrichheringer/vetradb/issues/17) / [ADR 0014](../adr/0014-postgresql-conformance.md). PostgreSQL **17**, wire **3.0 / 196608**. [Official message flow](https://www.postgresql.org/docs/17/protocol-flow.html), [message layouts](https://www.postgresql.org/docs/17/protocol-message-formats.html), and [SQLSTATE inventory](https://www.postgresql.org/docs/17/errcodes-appendix.html) are the reference sources. Semantic compatibility must be measured separately from network connectivity.

## Inventory and status

[postgresql-matrix.json](postgresql-matrix.json) is the current machine-readable acceptance inventory. `planned` means required implementation; `tested` needs exact versions, fixture/evidence links and declared deviations; `unsupported` means intentional rejection in initial scope; `deferred` needs a future scope decision. No row is tested yet. These conformance statuses are distinct from supported/production-qualified release status.

SQL inventory includes transactional CREATE/ALTER/DROP; INSERT/UPDATE/DELETE/RETURNING; primary/unique/NOT NULL/CHECK/FK; SELECT/filter/order/limit; INNER/LEFT/RIGHT/FULL/CROSS joins; DISTINCT; GROUP BY/HAVING and count/sum/min/max/avg; EXISTS/IN and scalar subqueries; non-recursive WITH, views, ON CONFLICT, default/identity/sequence, selected JSON and row_number/rank/lag/lead windows. Exact expression/function/operator inventories and semantic fixtures land in #43–#55; this list does not authorize accepting other syntax. Recursive CTEs, deferred constraints and unsupported CASCADE variants fail before mutation. Unsupported functions do not return plausible placeholder results.

| Scalar | Reference OID | Required edge fixtures |
| --- | --- | --- |
| bool | 16 | text spellings, NULL, binary 0/1 |
| int2 / int4 / int8 | 21 / 23 / 20 | signed bounds, overflow, network-byte-order binary |
| float4 / float8 | 700 / 701 | infinity, NaN ordering/equality, negative zero |
| text / varchar / bytea | 25 / 1043 / 17 | UTF-8, embedded binary zeros, typmod/length |
| uuid / numeric | 2950 / 1700 | UUID length, exact decimal/scale, overflow |
| date / time / timestamp / timestamptz | 1082 / 1083 / 1114 / 1184 | PostgreSQL epoch conversion, precision, infinity and timezone/DST |
| json / jsonb | 114 / 3802 | null vs SQL NULL, duplicate keys, binary version and lossless decoded values |

These are PostgreSQL protocol OIDs, not native storage tags. #44/#61 must demonstrate all advertised text/binary formats; a type with only one implemented codec cannot silently accept another format. Arrays, domains, enums, UDTs and locale collation are deferred. Initial collation is explicit C/bytewise and UTF-8; no locale behavior inferred from host settings.

## Protocol flow requirements

Startup validates packet size, protocol version, user/database and settings before authentication completes. TLS and SCRAM are required server paths; failed authentication closes the session without ReadyForQuery. SSLRequest and CancelRequest are separately framed; GSSAPI and unsupported authentication mechanisms are explicit rejections without insecure downgrade. Protocol >3.0 negotiation remains deferred; it cannot be interpreted as 3.0 silently.

Simple Query emits complete command responses and a final ReadyForQuery state: I idle, T in transaction, E failed transaction. A multi-statement simple query runs in one implicit transaction unless explicit transaction controls intervene. A statement error in an explicit transaction changes T to E; ordinary work then fails with 25P02 until ROLLBACK or valid ROLLBACK TO SAVEPOINT. COMMIT in a failed transaction follows PostgreSQL's rollback behavior, not a successful commit of prior partial statements.

Extended Parse/Bind/Describe/Execute/Close/Flush/Sync validates statement/portal ownership, parameter count/type/format, result format and suspension. After an extended error, discard later messages until Sync; Sync ends the implicit cycle but does not clear a failed explicit transaction. Do not send ReadyForQuery for every Execute. COPY has bounded buffers, CopyData/Done/Fail and cancellation; malformed input aborts its transaction. Text and CSV are planned; binary COPY is deferred until separate qualification. Invalid packets produce protocol violation and terminate when safe resynchronization is impossible.

Error inventory: 0A000 unsupported feature, 42601 syntax, 42P01 undefined table, 42703 undefined column, 42883 unknown function, 22003 numeric range, 22012 division by zero, 23502 not-null, 23503 FK, 23505 unique, 23514 CHECK, 25006 read-only, 25P02 failed transaction, 40001 serialization, 40P01 deadlock, 57014 cancellation, 42501 authorization, 28P01 invalid password, 08P01 protocol violation. Preserve primary SQLSTATE, transaction state and statement effect; do not leak secrets/denied values through DETAIL/HINT. Expected negative transcripts are in [protocol-cases.json](../fixtures/conformance/protocol-cases.json); they are future adapter acceptance inputs, not test executions.

## Session and catalog contract

Allowlisted settings: `client_encoding=UTF8`, `DateStyle=ISO, MDY`, `TimeZone=UTC` initially, `standard_conforming_strings=on`, `application_name` (bounded untrusted label), `search_path` (authorized schema list), `default_transaction_isolation` (supported modes), `default_transaction_read_only`, `statement_timeout` and `lock_timeout` (nonnegative bounded integers). #62 may expand values with fixtures. Startup `options` must parse the same allowlist; it cannot inject arbitrary settings or bypass authentication. Unknown names => 42704, unsupported values/features => 0A000 or 22023 according to the registered fixture. Transaction-affecting settings change only at legal boundaries; never ignored. Secrets are neither ParameterStatus values nor logs.

`server_version` is a separately qualified compatibility baseline value, never evidence of blanket parity. `version()` and capability discovery MUST identify VetraDB; #62 verifies whether drivers require numeric `server_version` and records the chosen value/deviations. Until that work passes, no PostgreSQL server version is advertised by this workspace.

Required catalog query shapes: pg_namespace (oid,nspname); pg_class (oid,relnamespace,relname,relkind); pg_attribute (attrelid,attnum,attname,atttypid,atttypmod,attnotnull,attisdropped); pg_type (oid,typname,typlen,typcategory); pg_index (indexrelid,indrelid,indisprimary,indisunique,indkey); pg_constraint (contype,conrelid,confrelid,conkey,confkey); pg_attrdef (adrelid,adnum); information_schema tables/columns/table_constraints/key_column_usage/referential_constraints; reviewed pg_get_expr/pg_get_indexdef/format_type/regclass casts. Missing introspection must fail precisely rather than invent constraints/indexes. Returned metadata is filtered by current authority. #62/#65 capture each candidate ORM's real catalog queries and qualify the exact subset.

## Native namespace and versioning

Reserve schema `vetradb` for engine-owned functions/table-valued functions and capability discovery. User DDL cannot shadow it or mutate internal objects. History/transaction queries (#67–#70), enqueue/claim/complete (#75–#83) and durable publish/offset APIs (#85–#90) are VetraDB capabilities with separate versioned contracts and grants. Function names in earlier planning docs remain proposed until their subsystem grammar/API freeze. LISTEN/NOTIFY belongs to PostgreSQL ephemeral semantics; durable pub/sub and native CDC never claim LISTEN replay or pgoutput support. Unknown native capability returns explicit unsupported error; do not interpret an unknown cursor codec.

## Driver/ORM campaigns and deviations

Candidate drivers: psql/libpq, Rust tokio-postgres/sqlx, Node pg, Python psycopg, Java pgJDBC, Go pgx. ORM/migration candidates: Prisma, Drizzle, SQLAlchemy and sqlx migrations. No candidate is supported yet. At campaign start, commit a manifest recording exact driver/ORM and runtime versions, PostgreSQL 17 minor and immutable image digest (if containerized), VetraDB commit/artifact, OS/arch, TLS config without secrets, fixture IDs and result evidence. Null/moving version fields block `tested` promotion. Upgrade a pin deliberately and rerun affected cases.

Every driver campaign covers startup/settings/auth, simple and extended typed parameters, NULLs and binary formats, transaction/savepoint errors, pools and disconnects, cancel, COPY and restart/outcome lookup. ORM campaigns add schema create/alter/drop, introspection and rollback migration; a passing driver does not qualify an ORM. Deviations have stable ID, exact trigger, expected PostgreSQL behavior, actual behavior/SQLSTATE/state, impacted versions, regression fixture, rationale and task/milestone. Unsupported security/semantic behavior cannot be waived by a deviation.
