# Security policy

VetraDB has no executable implementation or supported release yet. Do not use the planning repository as a database for production data. Supported versions and response targets will be published before the first supported release.

Report security vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/ulrichheringer/vetradb/security/advisories/new). Include affected version/commit, impact, reproduction steps and a minimal sanitized example. Never include credentials or production data in a public issue.

If the private reporting form is unavailable, contact the repository owner through an available private GitHub contact route and request a secure reporting channel before sharing details. General design concerns without sensitive exploit material can use the design RFC issue template.

The security milestone covers threat modeling, dependency/unsafe-code review, secret redaction, auth/grants, quotas and disclosure/release remediation policy.
