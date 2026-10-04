# ADR 0006: Shared engine, explicit deployment ownership

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Server and embedded adapters use the same storage, query and transaction services. Writable database files have one exclusive process owner. Embedded callers may share a safe in-process engine handle; the server owns handles and exposes bounded network sessions.

## Alternatives and consequences

Separate engines per deployment mode multiply correctness and format drift. Concurrent multi-process page ownership would require its own cache coherence/locking protocol and is outside initial scope.

Core storage does not depend on a network runtime. Workers and streams have explicit start/drain/stop controls; opening a database/read snapshot does not dispatch work. Page latches cannot cross async waits. Adapter parity and competing-process ownership tests are required before either mode is supported.
