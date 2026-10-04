# Security contract

## Trust boundaries

Treat SQL text, parameters, protocol frames, imported data, job/event payloads, transaction metadata, cursors, filesystem contents and remote peers as untrusted. Embedded mode trusts the host process's ability to access memory; it still validates inputs and file ownership. The host administrator can alter files or keys, so historical immutability is a database-level contract rather than protection against full host compromise.

Maintain a threat model covering malformed inputs, auth bypass, cross-database access, current/history data exfiltration, cursor forgery, stale-worker actions, resource exhaustion, unsafe filesystem paths, dependency vulnerabilities and failed/partial durability.

## Authentication and authorization

Server authentication requires TLS and SCRAM-SHA-256 for network password use; explicit local development trust mode is opt-in and loopback/local-only. No default remote trust or hard-coded credentials. TLS certificate reload and authentication failure rate limits are required. A validated crypto implementation is a dependency decision, not a bespoke protocol project.

Role/ownership/grant checks cover schemas, tables, DDL, history, transaction metadata, queues, schedules, topics, publications, subscriptions, offsets, backups and administrative operations. Internal service tables reject arbitrary user writes. Authorization is enforced in the engine service boundary so embedded and network paths cannot bypass it accidentally.

Current grants govern historical reads and replay. Separate HISTORY and METADATA grants prevent SELECT on a current table from exposing deleted secrets or actor metadata. Realtime/CDC must revalidate authorization when roles or policies change. Row-level policies are a scoped 1.0 requirement for multi-tenant applications, with matching filtering of before/after images and historical schema access. If a current policy cannot be safely evaluated against a historical schema, deny that historical access with a precise error until a reviewed mapping is available.

## Secrets and auditing

Security-provider state holds credentials outside general-purpose historical payloads. Query logs, error details, crash diagnostics, traces, job errors and metadata exports use explicit redaction; parameters and raw payloads are excluded by default. Cancellation keys and resume/lease tokens have appropriate entropy and scope. Cursor authenticity and authorization do not rely on hiding numeric CSNs.

Committed mutation provenance lives in the ledger. Rejected authentication/authorization and privileged operational actions use a separate tamper-evident/bounded audit path whose failure policy is explicit. Log rotation and disk exhaustion must not consume the database's durability reserve silently.

## Availability and file safety

Limits cover SQL/protocol frame size, nesting, parameter count, result bytes, memory, spill disk, transaction size/duration, locks, snapshots, job batches, subscribers, replay pins, connections and worker concurrency. Cancellation and deadlines propagate across waits and I/O. Backpressure cannot rely on unbounded channels.

Opening/backup/restore obey exclusive ownership and path validation, reject unsafe symlink/traversal behavior under the chosen contract, and preserve restrictive file permissions. Encryption-at-rest is a separately evaluated pre-1.0 ADR: either implement a reviewed authenticated encryption/key rotation design with restore evidence or publish a mandatory encrypted-volume deployment requirement and its limits. No hand-written cryptography or unverified encryption claim.

## Disclosure

Follow [SECURITY.md](../SECURITY.md). Production release requires parser/protocol/format fuzzing, dependency/license review, unsafe Rust review, a vulnerability reporting channel and a remediation policy.
