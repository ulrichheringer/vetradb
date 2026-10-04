# ADR 0012: Acyclic synchronous core and adapter dependencies

Status: accepted design by repository owner on 2026-10-04; implementation guarantees remain unverified. Date: 2026-10-04. Issue: [GOV-002 / #15](https://github.com/ulrichheringer/vetradb/issues/15). Depends on accepted ADR 0011.

## Context

The original architecture diagram made transaction management depend on catalog/history while its module table made catalog/history depend on transactions. Storage and WAL also require interfaces without a concrete dependency cycle.

## Decision

Use [module policy](../specs/modules.md) and its machine-readable graph. Pure recovery commands/interfaces live below storage; recovery orchestration sits above it. Catalog/history participate through typed mutations owned by the transaction layer, not callbacks into higher services. The synchronous core has no network executor dependency. Select edition 2024 and MSRV 1.85.0 for initial bootstrap; test this floor and the pinned stable toolchain before introducing dependencies.

## Alternatives and consequences

One monolithic crate hides ownership and dependency cycles. An async-only core forces embedded callers to adopt a runtime. Per-feature storage backends break the shared durability contract. Explicit lower-layer interfaces cost more design effort but make replay and fault injection reviewable. Optional networking belongs only to server crates.

## Invariants, migration and verification

The only authoritative persistence remains the native page/WAL engine. Public APIs own values/handles and never expose frames or runtime internals. No on-disk migration. Run the graph checker documented in the foundation evidence; bootstrap, Rust compilation and MSRV validation belong to #19. The I/O/clock contract feeds #21, #30, #80 and #81. Owner acceptance was recorded in the implementation conversation on 2026-10-04.
