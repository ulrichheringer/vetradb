# M00 foundation exit evidence

Epic [#1](https://github.com/ulrichheringer/vetradb/issues/1), v0.0 design. The repository owner accepted the foundation designs and authorized completion on 2026-10-04. This is permission to begin experimental subsystem implementation under the reviewed contracts, not evidence of implemented storage/SQL or production qualification.

| Task | Reviewed artifact and follow-ups |
| --- | --- |
| #14 product/release | [ADR 0011](../adr/0011-product-release-contract.md), [scope/journey trace](foundation.md), no cross-database/external exactly-once claims |
| #15 modules/dependencies | [ADR 0012](../adr/0012-rust-module-policy.md), [module policy](modules.md), actual checked crate DAG, embedded closure and MSRV |
| #16 persistent format | [ADR 0013](../adr/0013-persistent-format-v1.md), [exact layouts](persistent-format-v1.md), [fixtures and recovery walkthroughs](foundation-evidence.md) |
| #17 PostgreSQL/native contracts | [ADR 0014](../adr/0014-postgresql-conformance.md), [conformance inventory](postgresql-conformance.md), negative protocol/state acceptance plans, separate native namespace |
| #18 fault/oracles | [ADR 0015](../adr/0015-fault-oracle-contract.md), [fault evidence levels](fault-oracles.md), independent examples rejecting partial commit/write skew/phantom/stale lease/handoff gaps |
| #19 workspace/tooling | [ADR 0016](../adr/0016-workspace-bootstrap.md), [bootstrap evidence](bootstrap-evidence.md), locked std-only workspace, real contributor/CI gates and defect probe |
| #20 governance/disclosure | [ADR 0017](../adr/0017-governance-release-policy.md), [governance](../../GOVERNANCE.md), [security](../../SECURITY.md), [conduct](../../CODE_OF_CONDUCT.md), verified private reporting enabled and security/breaking-release walkthroughs |

## Gate evidence

The main-branch foundation commit `5deedef095e42b7843ca2043ed5d26e4314a7176` passed [CI run 37233402933](https://github.com/ulrichheringer/vetradb/actions/runs/37233402933): eight native combinations of Linux x86_64/ARM and macOS Intel/ARM with Rust 1.85.0/1.97.1. Format/lint, 15 Python fixture checks, 14 Rust unit/integration cases, one documentation example, local link/contracts/DAG checks, embedded-only build and deterministic campaign passed. The deliberately failing behavioral/format/link probes also passed by detecting their injected defects.

Private vulnerability reporting was checked with the authenticated repository API on 2026-10-04 (`enabled: true`); no report was submitted. Community files name the initial maintainer and Apache-2.0 licensing, provide concrete reporting routes, define reviewed publication/support boundaries and match actual contributor tooling. The governance completion itself is documentation/checker coverage, not a database code change.

## Reviewed invariants and remaining scope

One database/timeline owns the shared transaction/WAL/history/work/event/offset boundary. WAL precedes page flush; durable commit precedes shared visibility/acknowledgment. Losers undo without reversing completed structural actions. CSN gaps resolve without skipping unknown outcomes. Current authorization and bounded resources apply to retained history/services. Format fields and public ownership/runtime boundaries are explicit. The verification plan uses independent external outcomes, bounded serial/lease/handoff models, reproducible fault traces and separate process/power-loss evidence.

At M00, the relevant recovery/rollback/concurrency/resource cases are design walkthroughs and E0 fixture/reference-model regressions; actual engine versions and authorization/real-storage campaigns do not exist yet. They remain explicit M01–M10 requirements, including #21–#41, #65/#74/#84/#92, #100–#118. No unresolved foundation-design blocker is identified; incomplete runtime features are visible planned scope and cannot be promoted to supported/production status by this epic closure. Post-1.0 replication/HA remains gated by single-node qualification.
