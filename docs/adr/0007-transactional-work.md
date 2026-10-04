# ADR 0007: Fenced at-least-once work and schedule occurrences

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Persist enqueue, claims, retry schedules, completion and cron occurrences through ordinary engine transactions. Commit a fenced lease before invoking an application worker. Completion can share a transaction with business result writes and fails atomically when the lease token is stale.

## Alternatives and consequences

Executing handlers inside commit would make storage durability depend on arbitrary user/network code. Exactly-once external side effects are impossible to infer from a committed job acknowledgment alone.

Delivery/execution is at least once, with application idempotency for external actions. Persist chosen retry delays. Cron insertion and schedule advancement use one unique occurrence key and one transaction, with explicit timezone/DST/misfire rules. Historical reads do not replay jobs; restore pauses dispatch to prevent accidental external repeats.
