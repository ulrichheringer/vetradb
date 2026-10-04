# ADR 0017: Maintainer, disclosure and release ownership

Status: accepted decision under owner-authorized foundation epic completion on 2026-10-04. Issue: [GOV-007 / #20](https://github.com/ulrichheringer/vetradb/issues/20). Depends on accepted ADR 0011.

## Context and decision

Adopt [governance](../../GOVERNANCE.md), [security policy](../../SECURITY.md) and [conduct policy](../../CODE_OF_CONDUCT.md): named initial owner, review/ADR lifecycle, Apache-2.0 attribution and dependency review, no CLA/DCO requirement, explicit release blockers, no current supported versions/SLA, private vulnerability reporting and independent GitHub abuse reporting.

## Alternatives and consequences

An unverified personal contact or blanket support promise would mislead contributors. Use actual GitHub reporting surfaces and verify private vulnerability reporting via its authenticated read-only API; no test report or external message is sent. One initial maintainer means independent review capacity is limited; exceptions and evidence must be visible and future appointments update the policy.

## Verification, migration and follow-ups

On 2026-10-04, `GET /repos/ulrichheringer/vetradb/private-vulnerability-reporting` returned `enabled: true`, and repository metadata identified owner ulrichheringer and Apache-2.0. SECURITY and issue-template contact links use the same private advisory URL. Contributor commands match the actual gates; community documents and links pass the documentation checker. GOVERNANCE walks both private security remediation and a format-breaking release, including blocked publication, regression, version bump and migration/rollback evidence. No format or runtime change. Future release packaging/support inventory and security qualification remain #100/#111/#115/#118.
