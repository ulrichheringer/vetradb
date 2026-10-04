# ADR 0003: WAL recovery and logical history are separate

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Use an ARIES-inspired steal/no-force WAL design with page LSNs, physiological redo, transaction undo and CLRs. Torn-page protection and structural B+Tree top actions are explicit recovery requirements. Detailed record/undo specifications are ratified in GOV-003 and TXN-002.

The committed logical ledger is retained independently for provenance and reconstruction. Recycled physical WAL must not remove logical history.

## Alternatives and consequences

Copy-on-write/shadow paging could simplify some recovery paths but changes write amplification, tree concurrency and garbage collection. The initial direction favors a familiar page-cache/WAL model with stronger fault-test obligations.

Recovery repeats history then undoes losers; it may itself crash. An immutable committed logical record can reside on a physically mutable/compacted page without permitting SQL to alter its historical meaning. Durability acknowledgment follows the successful WAL barrier, never merely cache modification.
