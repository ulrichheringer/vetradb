# Foundation product and release contract

Design deliverable for [#14](https://github.com/ulrichheringer/vetradb/issues/14); accepted by the repository owner on 2026-10-04 under [ADR 0011](../adr/0011-product-release-contract.md). No database implementation or supported release exists.

## Scope and terminology

A database owns one UUID, active timeline, catalog, transaction manager, CSN sequencer, native storage and WAL. Every transaction, queue, topic, offset and snapshot belongs to exactly one database. A server may host independent databases; names are not global ordering keys. Cross-database transactions, two-phase distributed commit and atomic external side effects are excluded from 1.0. Opening a historical snapshot cannot execute work.

| Capability status | Required evidence and meaning |
| --- | --- |
| Planned | Design/backlog only; unavailable to applications |
| Experimental | Runnable versioned implementation, published limitations and behavioral tests; no support or production claim |
| Supported | Exact API/SQL/client/platform subset has reviewed conformance, resource and operational evidence; versioned support policy |
| Production-qualified | Supported release artifact additionally passes all M00–M10 gates, independent durability/security/restore/soak evidence and blocker review |
| Unsupported | Explicit rejection with a documented error; no silent downgrade |

Compatibility stages C0–C5 describe conformance coverage, not readiness. A capability record MUST include release/artifact ID, status, scope, supported versions/platforms, resource limits, fixture/evidence links and deviations. Every current capability is planned. An accepted design does not change that status.

## 1.0 and exclusions

The roadmap M01–M10 covers native disk-backed pages/WAL, concurrent ACID transactions (including serializable), the enumerated SQL/PostgreSQL subset, complete retained row/schema history, transactional jobs/events, authorization, bounded resources and backup/PITR/upgrade operations. Both adapters use the same services and permissions. Full PostgreSQL replacement, its physical format, extension ABI, stored native code, sharding, replication, HA, native bitemporal correction and legal tamper-proof audit are excluded. M11/M12 follow single-node qualification.

Production targets are Linux x86_64/aarch64 on separately qualified local filesystems. macOS is development/embedded validation only. Windows, network filesystems and hardware lying about sync have no production guarantee. Acknowledged commits survive documented process/power failures; response loss yields unknown outcome. Full default logical history does not mean indefinite physical WAL or publication retention.

## Journey traceability and negative cases

| Journey | Required result and failure behavior | Tasks / gate |
| --- | --- | --- |
| Order + stock + work + event | One envelope/CSN; rollback removes all; crash never leaves a partial committed set; response loss is unknown | #34–#36, #47, #49, #75, #86, #113 / M02,M03,M06,M07,M10 |
| Explain update / time travel | Resolve historical schema and trusted actor; caller metadata is untrusted; denied/deleted values stay inaccessible | #42, #67–#74, #101–#102 / M05,M09 |
| Worker crash | Expired lease can be reclaimed; stale completion rejected; external duplicate effects require idempotency | #76–#84 / M06 |
| Consumer reconnect | Replay committed operations in order; duplicates allowed; expired/wrong-lineage cursor requires explicit resnapshot | #85–#92 / M07 |
| Realtime bootstrap | Authorized snapshot and later stream have no gap; slow clients have bounded queues and permission revalidation | #89–#92, #102–#103 / M07,M09 |
| Host restore | Verify backup/WAL coverage, fork timeline, stop at complete target commit; outbound services paused; corrupt input fails closed | #106–#107, #110–#111, #114 / M09,M10 |
| Existing client | Pinned drivers, typed parameters, accurate transaction states, cancellation; unsupported SQL/settings rejected | #17, #43–#55, #58–#65, #112 / M03,M04,M10 |

Other capabilities map to storage #21–#29, recovery/isolation #30–#41, optimizer #93–#99, operations/security #100–#111 and release qualification #112–#118. No unmapped requested capability was found in the existing product journeys. Exact function/grammar inventories (#17), fault budgets (#18), and capacity/SLO numbers (#99/#116) remain explicit follow-ups; they are not implied by this review.

## Release review

Reviewers MUST check the product, architecture, roadmap and compatibility matrix against the table; verify each advertised feature has a task and evidence; reject cross-database/external exactly-once claims; and confirm restore/time-travel examples preserve authorization and lineage. #118 may approve 1.0 only with all M00–M09 gates and M10 evidence, no unresolved correctness/security blockers, and a support matrix matching the actual artifacts. Dates do not waive gates. Scope changes require a superseding ADR and updated tasks/matrix.
