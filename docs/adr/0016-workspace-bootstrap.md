# ADR 0016: Rust workspace and runnable contributor gates

Status: accepted implementation decision under owner-authorized continuation on 2026-10-04. Issue: [#19](https://github.com/ulrichheringer/vetradb/issues/19). Depends on accepted ADRs 0012/0015.

## Context and decision

Bootstrap all approved boundaries as Rust 2024 workspace crates with shared MSRV 1.85, forbid unsafe, no external packages and checked Cargo.lock. Add a development-only test-support crate (types/io dependencies) for independent E0 oracles and model campaigns. Most engine crates are declared empty boundaries, not implementations. Pin default toolchain 1.85.0 and CI validation toolchains 1.85.0/1.97.1.

## Alternatives and consequences

Implementing storage or SQL while bootstrapping would bypass their dedicated issues. A networking runtime would violate embedded isolation. The current standard-library-only inventory avoids external maintenance/license risks while meaningful independent oracles establish behavioral gates. New dependencies require updating the reviewed inventory/policy rather than bypassing checks.

## Verification and migration

`python3 tools/verify.py` checks docs/contracts/DAG/fixtures, formatting, warnings, unit/integration/doc tests, an embedded-only build and deterministic model campaign. `python3 tools/probe_gates.py` uses a disposable local branch to prove behavioral/format/link failures. CI runs native Linux x86_64/aarch64 and macOS Intel/ARM jobs with least privilege, pinned actions and seven-day failure artifacts. Local results are in [bootstrap evidence](../specs/bootstrap-evidence.md); remote CI/other platforms are unverified until published and executed. No data migration. Maintained workflows and deferred campaigns are listed in [development guide](../development.md).
