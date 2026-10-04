# ADR 0014: Enumerated PostgreSQL conformance and native namespace

Status: accepted design under owner-authorized continuation on 2026-10-04; implementation unverified. Issue: [#17](https://github.com/ulrichheringer/vetradb/issues/17). Depends on ADR 0011.

## Context and decision

Pin PostgreSQL 17 semantics and frontend/backend protocol 3.0 (196608). Adopt [conformance contract](../specs/postgresql-conformance.md), the machine-readable feature matrix and negative protocol fixture plans. Native history/work/events use the reserved `vetradb` namespace and independent capabilities; they never pretend to be PostgreSQL extensions or logical replication.

## Alternatives and consequences

Advertising a PostgreSQL version from one successful connection hides semantic/security gaps. Instead, tested status requires exact runtime/client/reference versions and evidence for errors, transactions and catalogs as well as successful requests. Unsupported semantic/security settings are rejected before work. Exact minor/reference and client versions are locked at the start of each real campaign, not fabricated before a runnable server exists.

## Invariants, migration and verification

No physical format or transaction invariant changes. Check the matrix and planned transcript fixtures with `python3 tools/check_contracts.py`; it validates the acceptance inventory, not a PostgreSQL implementation. Follow-ups #43–#65 implement grammar, codecs, protocol and clients; #67/#85 freeze native grammar/stream transport; #112 qualifies the supported release matrix. Any departure requires a deviation ID and matching regression fixture. No live driver or ORM support is claimed.
