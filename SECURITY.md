# Security policy

The foundation Rust workspace and verification tooling exist; there is no usable database or supported release. Do not use VetraDB for production data. There is no guaranteed security response or patch SLA at this stage.

## Supported versions

| Version | Security support |
| --- | --- |
| Foundation / unreleased main | Best-effort design/tooling triage; no supported database release |
| Released database versions | None yet |

Before the first supported release, publish the exact maintained versions, support/retirement window and available response policy here. Experimental or unsupported versions do not acquire production/backport guarantees from a version number. [Governance](GOVERNANCE.md) defines release blockers and disclosure ownership.

## Private reporting

Report sensitive vulnerabilities through [GitHub private vulnerability reporting](https://github.com/ulrichheringer/vetradb/security/advisories/new). The authenticated GitHub repository API confirmed `enabled: true` on 2026-10-04; this checks availability without sending a report. The same route is visible in the repository issue-template contact links. Reporters need to sign in to GitHub.

Include affected commit/version/platform, impact, sanitized reproduction and minimal synthetic input. Never post credentials, production data or sensitive exploit details in a public issue. If the private reporting form is unavailable, withhold exploit details and use [GitHub Support](https://support.github.com/contact) to resolve access; do not invent a private maintainer email or publish secrets to obtain attention. General nonsensitive design concerns may use the design RFC issue template.

## Triage and disclosure

The initial security/release owner is [ulrichheringer](https://github.com/ulrichheringer). The maintainer determines affected code/versions, requests only necessary sanitized evidence, coordinates a regression and fix, reviews relevant gates and agrees on disclosure timing with the reporter where possible. Keep access to private advisories limited to necessary participants. Publish remediation and affected/fixed versions together with a public advisory when ready; avoid exposing exploit details through a premature public PR/commit. If a fix cannot be delivered safely, withdraw affected claims/artifacts and document the limitation.

Data loss/partial commit, authorization bypass, credential disclosure, unsafe recovery and unbounded hostile-resource behavior block supported/production releases until remediated and revalidated. No disclosure action implies legal non-repudiation, protection against a privileged host attacker or automatic erasure from backups/external copies. Threat modeling, dependency/unsafe review, auth/grants, resource limits and independent security campaigns remain explicit subsystem/qualification work.
