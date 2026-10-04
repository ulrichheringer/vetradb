# ADR 0005: Progressive PostgreSQL compatibility

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Pin PostgreSQL 17 behavior and protocol 3.0 as the first reference. Expand grammar, semantics, codecs, catalogs and client workflows through explicit stages and versioned fixtures. Use a VetraDB namespace for native temporal/work/event extensions.

## Alternatives and consequences

Claiming blanket compatibility based on a connection or parser test hides type, isolation, catalog and error-state gaps. A separate proprietary SQL wire protocol would reduce compatibility work but make modern backend adoption harder.

Driver and ORM support are independent entries; unsupported semantic/security settings fail explicitly. PostgreSQL on-disk formats, PL/pgSQL/extensions and logical-replication protocol are not implied by pgwire support. New major/minor protocol references require separate negotiation and evidence tasks.
