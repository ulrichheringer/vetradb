# Governance

Foundation policy accepted under the repository owner's implementation and epic-completion authorization on 2026-10-04. Decision record: [ADR 0017](docs/adr/0017-governance-release-policy.md).

## Ownership and review

The initial maintainer and release owner is [ulrichheringer](https://github.com/ulrichheringer). The maintainer owns scope, dependency/license review, security triage, evidence review and publication authority. New maintainers require an owner-approved public nomination recording role, repository permissions and conflict-of-interest handling; update this file when appointments change. Contributors do not receive release or disclosure authority automatically.

Routine changes use issues and pull requests with scope and validation evidence. Maintainer review is required before merge/release; the initial sole maintainer may author and approve work, recording acceptance explicitly. As maintainers grow, security/storage/format changes SHOULD receive a second qualified review; a sole-maintainer exception must be visible with its evidence. CI passing is necessary but not sufficient to approve guarantees.

## RFC and ADR lifecycle

Storage format, isolation, history, compatibility, security and replication changes need an ADR or RFC. Lifecycle: proposed -> accepted -> superseded or withdrawn. A proposal records context, alternatives, invariants, failure cases, migration/rollback implications, verification and affected tasks. Acceptance records maintainer identity/date and evidence; superseding decisions link both directions and preserve previous text/history. A design acceptance authorizes implementation under its contract, not a supported-feature or production claim.

## Contribution and conduct

Contributions are Apache-2.0 under [LICENSE](LICENSE); retain attribution and [NOTICE](NOTICE). There is no CLA or mandatory DCO/sign-off requirement. New third-party code/dependencies require the version/features/SPDX/transitive license, maintenance, advisory, target/MSRV and removal review in [module policy](docs/specs/modules.md); retain required notices. Do not copy code with unknown provenance. Unsafe exceptions require a superseding policy decision and documented review; the current workspace forbids unsafe.

Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Public technical disagreement is welcome. Private safety reports use the documented GitHub reporting route; never post sensitive evidence or personal details publicly. The maintainer may remove content, limit interaction or revoke contributor permissions, recording an appropriate sanitized rationale. Reports concerning the maintainer use GitHub Support independently.

## Release authority and support

Only the designated release owner may approve/publish a release after reviewing milestone evidence, support matrix, unresolved blockers and artifact provenance. Record the release tag/commit, checksums, target/toolchain/dependency inventory, migration/rollback instructions and reviewed evidence. Experimental alpha/beta artifacts must prominently state their limits. No calendar deadline waives a gate.

There are currently **no supported database releases** and no promised response SLA. Future supported versions must be listed explicitly in [SECURITY.md](SECURITY.md) and release notes with maintenance/retirement policy before publication; a SemVer number alone does not imply maintenance. Unsupported lines receive no promised backports. Version 1.0 requires all M00–M10 release gates and single-node production qualification; it never implies full PostgreSQL parity or HA. Update the support matrix and roadmap before changing scope/platform claims.

Blocking conditions include acknowledged data loss/partial commit, unsafe recovery/upgrade, isolation or authorization violations, credential exposure, unbounded resource abuse, unresolved high-impact security findings, missing required restore/platform evidence, incompatible format changes without an explicit migration/export path, or failed required CI. Release labels cannot turn an unresolved blocker into an accepted risk. Pause affected artifacts/support claims while investigating a discovered blocker.

## Security and breaking-release walkthroughs

A security fix starts through the verified [private reporting channel](SECURITY.md), with sanitized reproduction and affected commits. The maintainer privately triages, limits access, adds a regression, checks all affected maintained versions and coordinates disclosure with the reporter. Do not publish exploit details in an ordinary issue/commit before the disclosure decision. Re-run relevant auth/corruption/resource and release gates, publish fixed artifacts/advisory together when possible, and document any unsupported line. No emergency bypass may weaken the shared commit boundary.

A format-breaking proposal needs a superseding ADR, explicit format version bump, old/new fixtures, readable-version range and migration/export path. Test interruption and rollback constraints against release artifacts before publishing. Refuse incompatible open before mutating files. Update release notes, support matrix and affected tasks; never hide a break behind the same persisted format version or a blanket PostgreSQL version string.
