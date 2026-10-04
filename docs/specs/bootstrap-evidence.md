# Conformance, fault model and workspace evidence

Implementation continuation authorized by the repository owner on 2026-10-04. ADRs 0011–0013 were explicitly accepted; subsequent ADRs 0014–0016 record decisions made within that authorized work.

| Issue | Delivered evidence |
| --- | --- |
| [#17](https://github.com/ulrichheringer/vetradb/issues/17) | [Conformance contract](postgresql-conformance.md), 20-feature status matrix, 11 planned protocol/error/state cases, driver/ORM pin and deviation policy, native namespace distinction |
| [#18](https://github.com/ulrichheringer/vetradb/issues/18) | [Fault/oracle contract](fault-oracles.md), independent E0 atomicity/serial/predicate/lease/handoff oracles, bounded single-file simulator and seeded campaign; tests inject partial commit, write skew, phantoms, stale completion and stream gaps |
| [#19](https://github.com/ulrichheringer/vetradb/issues/19) | 21 Rust workspace crates, shared MSRV/edition/license/unsafe policy, locked std-only dependencies, contributor/docs/DAG/format/lint/test gates, native target CI, bounded artifacts and fuzz/benchmark smoke entry points |

## Local verification

Host: aarch64 macOS. Default/MSRV compiler: Rust 1.85.0. Additional installed stable compiler: Rust 1.97.1. Reproduce with `python3 tools/verify.py`; for the installed second compiler, use `RUSTUP_TOOLCHAIN=stable python3 tools/verify.py` and verify `rustc +stable --version` matches 1.97.1. CI uses the exact compiler version rather than a moving stable label.

Local evidence comprises 15 Python binary-format checks, 14 Rust unit/integration tests, warning-free Clippy/format, one executable documentation example, embedded-only build, documentation/contract/crate-policy checks and seed-42 smoke campaign. `python3 tools/probe_gates.py` demonstrates behavioral/format/link rejection in a temporary local branch. The model benchmark command runs successfully and reports only harness timing.

Configured CI native runners: ubuntu-24.04, ubuntu-24.04-arm, macos-14 and macos-15-intel, each with 1.85.0/1.97.1. These runner labels follow [GitHub's hosted runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). Remote CI and Linux/Intel execution are not verified locally; the clean local bootstrap verifies one development target. No engine, driver/ORM, power-loss, production or remote CI result is implied by the examples or configuration.

## Contract corrections found while continuing

The accepted format bounds application metadata to 16 KiB; the temporal overview previously said proposed 64 KiB. It now uses the format-v1 bound. Rust boundaries and scripts now exist, so README/contributor/verification/index wording distinguishes bootstrap tooling from the still-unimplemented database. The foundation gate is not closed: governance #20 and actual subsystem implementation/qualification remain necessary.
