# ADR 0013: Explicit v1 page, WAL and logical envelope layouts

Status: accepted design by repository owner on 2026-10-04; implementation guarantees remain unverified. Date: 2026-10-04. Issue: [GOV-003 / #16](https://github.com/ulrichheringer/vetradb/issues/16). Depends on accepted ADRs 0011 and 0012.

## Context and decision

Adopt [persistent format v1](../specs/persistent-format-v1.md): 8 KiB pages, redundant superblocks, little-endian fixed-width fields, CRC32C, bounded logical envelopes, physiological WAL, redo-only structural top actions, CLRs and full-page images. The layout is a candidate freeze for implementation review, not a supported database format.

## Alternatives and consequences

Native struct serialization depends on ABI and cannot validate hostile input safely. Shadow paging changes the accepted steal/no-force direction. SHA-based page checksums cost more without supplying tamper-proof history; CRC32C detects accidental corruption and is not authentication. Fixed pages simplify bounds and fixtures; overflow and write amplification must still be measured in #22/#23/#99. A change after ratification requires a version bump, fixtures and an explicit migration/export or incompatible-open policy.

## Invariants, migration and verification

WAL before pages and durable commit before publication remain mandatory. Transaction status publishes ledger, catalog, relational and service mutations together. CSN reservations may leave gaps but cannot skip unresolved predecessors. [Foundation evidence](../specs/foundation-evidence.md) contains examples and crash walkthroughs. Fixtures verify framing only; #29, #41 and #113 must supply engine recovery oracles. Owner acceptance was recorded in the implementation conversation on 2026-10-04; no existing data requires migration.
