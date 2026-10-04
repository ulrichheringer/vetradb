# Architecture decision records

These records capture accepted **planning directions**, not implemented or verified behavior. The M00 and subsystem RFC tasks ratify detailed encodings, APIs and algorithms before implementation. A decision that changes must be superseded visibly, with affected issues/contracts updated.

| ADR | Decision |
| --- | --- |
| [0001](0001-native-rust-storage.md) | Native Rust page/B+Tree engine |
| [0002](0002-unified-transaction-boundary.md) | One commit boundary for data, history and backend primitives |
| [0003](0003-wal-and-recovery.md) | WAL-based recovery separate from immutable logical history |
| [0004](0004-relational-temporal-ledger.md) | Relational row/schema versions and stable committed transaction basis |
| [0005](0005-progressive-postgresql.md) | Progressive, evidence-based PostgreSQL compatibility |
| [0006](0006-server-and-embedded.md) | Shared engine with explicit embedded/server ownership |
| [0007](0007-transactional-work.md) | Fenced at-least-once jobs and transactional schedule occurrences |
| [0008](0008-events-and-realtime.md) | Committed-envelope replay and bounded realtime scope |
| [0009](0009-single-node-before-ha.md) | Single-node qualification before replication/HA |
| [0010](0010-retention-and-history-security.md) | Complete default history, explicit retention and present-day authorization |

The detailed foundation ADRs below were accepted by the repository owner on 2026-10-04 and refine the planning directions.

| ADR | Decision |
| --- | --- |
| [0011](0011-product-release-contract.md) | Product scope and release vocabulary (#14) |
| [0012](0012-rust-module-policy.md) | Acyclic synchronous core and adapter dependencies (#15) |
| [0013](0013-persistent-format-v1.md) | Explicit v1 pages, WAL and transaction envelopes (#16) |

The owner authorized implementation continuation on 2026-10-04; these follow-up decisions document that scope.

| ADR | Decision |
| --- | --- |
| [0014](0014-postgresql-conformance.md) | Enumerated PostgreSQL conformance and native namespace (#17) |
| [0015](0015-fault-oracle-contract.md) | External correctness oracles and labeled fault evidence (#18) |
| [0016](0016-workspace-bootstrap.md) | Rust workspace and runnable contributor gates (#19) |
| [0017](0017-governance-release-policy.md) | Maintainer, disclosure and release ownership (#20) |

New ADRs should include status/date, context, decision, alternatives, consequences, invariant changes, migration/verification and links to affected tasks. Do not delete old decisions to hide incompatible changes.
