# ADR 0011: Product scope and release vocabulary

Status: accepted design by repository owner on 2026-10-04; implementation guarantees remain unverified. Date: 2026-10-04. Issue: [GOV-001 / #14](https://github.com/ulrichheringer/vetradb/issues/14).

## Context

A feature checklist can confuse planned behavior with production support. Backend work, history and relational writes must have identical commit semantics in both adapters.

## Decision

Adopt the scope, capability vocabulary, journey evidence and exclusions in [foundation contract](../specs/foundation.md). A database is the transaction, CSN, queue, topic, authorization and retention boundary. Server multi-database hosting supplies independent handles, not a global commit coordinator. External worker/network side effects are at least once and require application idempotency.

## Alternatives and consequences

Blanket PostgreSQL compatibility and feature-based readiness would make unsupported behavior look safe. Calendar releases cannot replace correctness evidence. Every supported capability instead names its version, scope, limits, platform and passing evidence. No capability is currently implemented.

## Invariants, migration and verification

No durable invariant changes. This clarifies existing planning directions; there is no data to migrate. The journey-to-task table and review checklist in the foundation contract are the evidence artifact. Owner acceptance was recorded in the implementation conversation on 2026-10-04. Follow-ups: #17 compatibility grammar, #18 fault oracles, #19 workspace, #20 governance and the linked subsystem tasks.
