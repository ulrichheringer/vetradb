# ADR 0015: External correctness oracles and labeled fault evidence

Status: accepted design under owner-authorized continuation on 2026-10-04. Issue: [#18](https://github.com/ulrichheringer/vetradb/issues/18). Depends on accepted ADRs 0013/0014.

## Context and decision

Adopt the [fault/oracle contract](../specs/fault-oracles.md). Successful client acknowledgments live in an independent oracle, not the engine's WAL. Unknown outcomes permit whole commit or absence; partial state is always a defect. Small serial histories and queue/stream reference models must reject injected contradictions. Use seeded persistence simulations before real platform qualification, labeling the evidence level explicitly.

## Alternatives and consequences

Mocked success and process-kill-only tests cannot establish power-loss durability. Internal MVCC/lock assertions cannot independently prove serializability. Exhaustive small-history witnesses cost factorial work, so budgets are explicit and larger workloads use separate qualification techniques rather than claiming the bounded oracle covered everything.

## Verification, migration and follow-ups

No format change. `vetra-test-support` supplies independent bounded example oracles and single-file persistence modeling, with tests injecting partial cross-feature state, write skew, phantom cycles, stale leases and stream gaps. Full namespace/sector/real-engine fault injection belongs to #21/#29/#41; acknowledgment campaigns and qualified filesystems belong to #113/#114, security/fuzzing to #115, soak/capacity to #116. Runnable examples do not qualify a nonexistent database engine.
