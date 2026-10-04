# ADR 0010: Complete history by default and current authorization

Status: accepted planning direction, implementation unverified. Date: 2026-10-03.

## Context and decision

Keep every committed relational/schema/service change by default. Lossless compaction and hot MVCC vacuum do not expire logical history. Finite policies explicitly expose oldest available bases; extraordinary redaction uses a separate privileged audited workflow.

Historical queries, metadata, CDC and realtime are governed by current grants/policies, with rights separate from current SELECT. Credential verifiers remain behind the security-provider boundary rather than in general historical payloads.

## Alternatives and consequences

Implicit vacuum expiration undermines complete versioning. Reusing historical grants could resurrect revoked access. Checksums alone cannot make history tamper-proof against a host administrator.

Pins need quotas/expiry and operator inspection to prevent disk exhaustion. Redaction must account for backup/archive/replica copies and cannot claim erasure from unmanaged external systems. Retention/security edge tests are required alongside historical reconstruction.
