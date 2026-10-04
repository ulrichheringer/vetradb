# ADR 0001: Native Rust page/B+Tree engine

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

The project requires control over physical storage, WAL, MVCC and lossless history. Use a native Rust storage engine with checksummed fixed-size pages, slotted/overflow records, a bounded buffer pool and B+Trees. The proposed initial page size is 8 KiB, ratified by GOV-003/STO-002 before format freeze.

## Alternatives and consequences

Embedding SQLite/RocksDB/PostgreSQL would shorten bring-up but move the authoritative engine and its invariants outside the requested architecture. Those are not the primary store. Parsing, cryptography and runtime libraries may be reviewed dependencies.

This decision increases engineering and verification cost. Explicit portable encodings, structural reference-model tests, safe ownership and platform I/O fault injection are mandatory. Initial public format versions remain experimental until the migration policy is established.
