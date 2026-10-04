# Development and maintained workflows

The foundation Rust workspace exists. It is **not a usable database**: most crates currently declare approved boundaries; owned identities, injectable I/O traits, protocol state bytes and independent test models/oracles are implemented. Native file ownership, pages, WAL recovery, SQL, listeners and application APIs remain subsystem issues.

## Fresh checkout

Install Git, rustup and Python 3.11+ (CI uses 3.12). The root `rust-toolchain.toml` selects Rust 1.85.0 with rustfmt/clippy; rustup installs it automatically. No third-party Cargo packages are used; Cargo.lock is committed for reproducible bootstrap. No services, credentials or database fixtures are needed.

Run `python3 tools/verify.py` from the root. It validates local documentation links, compatibility inventory, crate dependency/license policy, binary design fixtures, Rust formatting, warning-free Clippy, unit/integration/doc tests, an embedded-only build and a seed-42 model campaign. Cargo operations use `--locked`; format checking does not rewrite files. Use `cargo fmt --all` deliberately to format edits.

`python3 tools/probe_gates.py` copies only source/docs/tooling into a temporary directory and initializes a disposable Git branch. It demonstrates that an intentionally incorrect behavioral test, unformatted Rust and a broken local Markdown link all fail their gates. Your checkout and changes are not modified. Temporary artifacts are removed on completion/failure.

## Maintained entry points

| Command / workflow | What it actually verifies |
| --- | --- |
| `python3 tools/verify.py` | Consolidated contributor/CI gate |
| `python3 tools/probe_gates.py` | Representative gate failures in a disposable branch |
| `python3 tools/verify_foundation.py` | Static page/WAL/envelope example framing, bounds and graph regressions |
| `python3 tools/check_contracts.py` | Feature status/evidence requirements and planned negative protocol fixture inventory |
| `cargo test --locked -p vetra-test-support` | Independent atomicity/serial/lease/handoff examples and single-file persistence model |
| `cargo run --locked -p vetra-test-support --bin fuzz-foundation -- 42 10000` | Seeded bounded persistence-model campaign; not coverage-guided codec fuzzing |
| `cargo run --release --locked -p vetra-test-support --bin bench-foundation` | Harness-only timing; no database performance claim |
| `.github/workflows/ci.yml` | Native Linux x86_64/aarch64 and macOS Intel/ARM, Rust 1.85.0/1.97.1; 20-minute jobs, pinned action SHAs, contents-read-only, seven-day failure artifacts |
| `.github/dependabot.yml` | Weekly action pin update proposals, max three open PRs; reviewed before merge |

Default/MSRV is 1.85.0; 1.97.1 is the second pinned validation compiler. Refresh compiler/action pins deliberately and rerun gates. Python scripts use only the standard library. Every Cargo crate inherits Apache-2.0, repository metadata, MSRV and `unsafe_code=forbid`. The checked DAG and Cargo.lock forbid unreviewed external/runtime/build dependencies. When a dependency is adopted, include the version/features/license/maintenance/advisory/MSRV rationale from [module policy](specs/modules.md), update the machine policy and add advisory/license scanning for that inventory. Current inventory is empty; no audit tool can substantiate nonexistent third-party packages.

The development-only `test-support` crate is not in the embedded closure. Public core providers are synchronous and do not expose pointers, frame guards or runtime types. Production eligibility still requires the real filesystem/fault/restore evidence defined in [fault-oracles.md](specs/fault-oracles.md).

## Deferred workflows

Real page/codec coverage fuzzing and corpus shrinking (#29/#115), actual engine crash/isolation campaigns (#41/#113), driver/ORM reference containers (#65/#112), database benchmarks/soak (#99/#116), security review (#100/#115), packaging/SBOM/signing/restore (#111/#114) are not implemented by this bootstrap. The foundation commit `5deedef` passed all eight native [CI jobs](https://github.com/ulrichheringer/vetradb/actions/runs/37233402933). This proves bootstrap/tooling execution, not database/platform durability.
