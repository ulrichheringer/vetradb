# ADR 0002: One transaction and commit boundary

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

An application must change relational data, enqueue work and publish events atomically. Use one transaction manager, lock/visibility system, sealed mutation envelope, WAL and durable commit sequencer for rows, catalog, history, job/queue/scheduler state, publications and offsets.

## Alternatives and consequences

Separate queue/broker engines or an asynchronously copied audit table create partial failure windows. External services may consume the engine's committed stream but do not decide its atomicity.

Participant operations cannot autonomously commit or make network calls. Publication and wakeups follow durability; restart discovers committed work by scanning durable structures. Failure tests must inspect every participant, not just table rows. Lost commit replies require outcome lookup/idempotency rather than a promise that retries are always harmless.
