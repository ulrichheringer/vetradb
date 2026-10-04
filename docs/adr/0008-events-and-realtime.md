# ADR 0008: Committed-envelope delivery and bounded realtime scope

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Durable publish and CDC use committed envelopes, lineage-aware cursors and transactional consumer offsets. Atomically register an initial snapshot/replay basis to prevent a snapshot-to-stream gap. Native realtime 1.0 supports eligible single-table filters/projections; broader query maintenance is deferred.

## Alternatives and consequences

Best-effort in-memory callbacks lose changes after commit/restart. Arbitrary query subscriptions require join/aggregate incremental-maintenance semantics that cannot be promised by a generic pub/sub API.

Backpressure, retention, pin quotas and explicit expiry/resnapshot errors are part of the protocol. Current authorization applies to replay and before/after values. PostgreSQL LISTEN/NOTIFY remains an ephemeral compatibility path; native durable replay and pgoutput compatibility are separate contracts. Network dispatch never blocks the durability barrier.
