# ADR 0009: Qualify single node before replication and HA

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Version 1.0 qualifies single-node durability, isolation, history, backend primitives, security and operations. Replication/HA occupy later independent milestones. Stable lineage/IDs, deterministic envelopes and a durability provider seam prepare for that phase without claiming distributed correctness now.

## Alternatives and consequences

Starting distributed SQL/consensus before local recovery and transaction correctness makes failures harder to isolate and broadens the initial scope. Backups and asynchronous replicas cannot be labeled HA or no-data-loss failover.

Future quorum durability must precede visibility/acknowledgment and fence stale leaders, workers and schedules. Read guarantees, membership, snapshot/catch-up, rolling formats and partition tests are required. Single-node history must not be renumbered on conversion to a cluster.
