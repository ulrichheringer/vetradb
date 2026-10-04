# PostgreSQL compatibility plan

Status: every row below is **planned and unimplemented**. Stage denotes intended evidence, not present support. The reference baseline is PostgreSQL 17; this deliberately pins semantics and frontend/backend protocol 3.0 while allowing newer client testing later. PostgreSQL 18+ minor-protocol negotiation is a future explicit addition.

PostgreSQL separates protocol startup, simple queries, extended queries and COPY. Passing a connection test is only one part of compatibility. [Official PostgreSQL 17 protocol overview](https://www.postgresql.org/docs/17/protocol-overview.html).

The accepted [conformance contract](specs/postgresql-conformance.md) and [machine-readable matrix](specs/postgresql-matrix.json) enumerate planned/unsupported/deferred status, native namespaces, session/catalog inventory, negative acceptance fixtures and promotion requirements. The foundation workspace has response state types only, not a PostgreSQL adapter.

## Progression

| Stage | Target | Evidence required |
| --- | --- | --- |
| C0 | Accepted grammar, supported types and SQLSTATE inventory | Parser/binder corpus; rejected constructs have precise errors |
| C1 | SQL semantics through the embedded engine | Differential results/errors against pinned PostgreSQL 17 |
| C2 | Protocol 3.0 startup, TLS/SCRAM, simple queries | Real psql/libpq connection, transactions and malformed packet tests |
| C3 | Extended queries, portals, prepared statements, cancellation | Typed text/binary round trips, Sync/error recovery, driver pools |
| C4 | Catalog subset, COPY, migration/ORM workflows | Version-pinned real client journeys and documented query gaps |
| C5 | Broader SQL/catalog behavior | Independently tracked fixtures, no implicit blanket compatibility claim |

## Planned 1.0 matrix

| Capability | Target | Relevant milestone |
| --- | --- | --- |
| Protocol 3.0; startup, ParameterStatus, BackendKeyData, ReadyForQuery | Supported subset with accurate I/T/E transaction state | M04 |
| Simple query, multi-statement implicit transactions | Compatible tested behavior | M04 |
| Parse/Bind/Describe/Execute/Close/Flush/Sync; portals | Compatible tested behavior, including suspended portals and error drain | M04 |
| SCRAM-SHA-256 and TLS negotiation | Required; no silent plaintext downgrade | M04 |
| CancelRequest, errors and connection termination | Required, with non-guessable cancellation secrets | M04 |
| Built-in scalar OIDs; text and binary codecs | Explicit supported type list | M03/M04 |
| CREATE/ALTER/DROP, DML, RETURNING, joins, aggregates | Supported feature list | M03 |
| Read committed, repeatable read, serializable, savepoints | Tested semantics, no isolation downgrade | M02 |
| pg_catalog/information_schema, SHOW/SET/session parameters | Minimal accurate subset for selected clients | M04 |
| COPY FROM/TO STDIN/STDOUT | Supported text/CSV; binary separately gated | M04 |
| JSON/JSONB, non-recursive CTE, views, upsert, common windows | Enumerated function/operator subset | M03 |
| LISTEN/NOTIFY | Ephemeral compatibility channel with PostgreSQL behavior | M07 |
| Durable topics, historical queries and transactional job APIs | VetraDB extensions; separately specified | M05/M06/M07 |
| PostgreSQL on-disk format, pg_dump/pg_restore blanket support | Not promised; VetraDB backup/export tools required | M09 |
| Logical replication protocol / pgoutput compatibility | Future independent adapter; native CDC is separate | M11 |
| PL/pgSQL, extensions, FDW, GSSAPI, replication slots as PostgreSQL objects | Outside initial scope | Future RFC |

## Client contract

Required candidates: psql/libpq, Rust `tokio-postgres` and `sqlx`, Node `pg`, Python `psycopg`, Java pgJDBC and Go pgx. Pin exact driver, runtime and PostgreSQL reference versions in the test matrix. Test authentication, prepared parameters, transactions, pools, cancellation, NULL and timestamp/numeric/JSON codecs. Prisma, Drizzle, SQLAlchemy and migration tools receive separately scoped smoke journeys; a passing driver does not prove an ORM is supported.

Report `server_version` and capabilities using a reviewed strategy that identifies VetraDB and avoids claiming features it lacks. Unsupported startup options and SET values are rejected or handled according to explicit negotiation rules. Never silently ignore a parameter that affects data semantics or security.

Protocol behavior follows the [official message-flow specification](https://www.postgresql.org/docs/17/protocol-flow.html). Isolation comparisons follow the [official transaction isolation contract](https://www.postgresql.org/docs/17/transaction-iso.html). Deviations must be documented next to the matching fixture and exposed in release notes.
