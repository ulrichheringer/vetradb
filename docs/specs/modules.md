# Rust module and dependency policy

Design deliverable for [#15](https://github.com/ulrichheringer/vetradb/issues/15), [ADR 0012](../adr/0012-rust-module-policy.md). The [foundation workspace](../development.md) now declares these crates. Most are boundary-only bootstrap crates, not feature implementations.

## Dependency ownership

[modules.json](modules.json) is the authoritative direct dependency DAG. Each name becomes a `vetra-<name>` crate unless #19 records a reviewed equivalent module grouping. Only dependencies listed in that file are allowed; transitive access does not imply ownership.

| Layer | Ownership |
| --- | --- |
| types | IDs, owned values, errors and versioned primitive encodings; no I/O |
| io | Exclusive directory ownership, local file operations and sync; no database semantics |
| recovery-api | Owned redo/undo commands, page addresses and WAL append/durability traits; no storage implementation |
| wal | WAL codec/segments/flush using I/O and recovery-api; cannot open a second authoritative store |
| storage | Page codec, allocator, frames, B+Trees; uses recovery-api WAL trait; cannot depend on concrete wal |
| recovery | Coordinates analysis/redo/undo using wal and storage; never calls SQL or feature services |
| txn | Visibility, locks, typed participant mutations, sequencing and sealed envelopes using storage/wal |
| catalog / history | Versioned schemas/grants and temporal query services over txn; no txn callback into these crates |
| sql / planner / executor | Bind owned schema views, plan and execute through transaction services |
| work / events | Typed transactional records and bounded dispatch scans; external effects happen after commit |
| engine | Wires storage, WAL, recovery and features, lifecycle and bounded service execution |
| embedded | Owned safe handles and synchronous calls; no server/runtime dependency |
| pgwire / server | Protocol/session types and network/runtime adapter; bounded blocking executor wraps core calls |
| admin / cli | Management facade and command presentation through engine |

`txn` owns the internal sealed mutation enum for row, schema, ledger, job, event and offset writes. Higher layers validate and submit typed operations before sealing; they never execute callbacks during commit. History indexing uses these lower-layer primitives. `engine` injects implementations of lower-layer traits. This resolves both catalog↔txn and storage↔wal cycles in the initial diagram.

## Rust and target matrix

Select Rust 2024, `rust-version = "1.85"`, workspace resolver 3; stable Rust only. This is a chosen baseline, not a compilation claim. The [edition guide](https://doc.rust-lang.org/edition-guide/rust-2024/index.html) identifies 1.85.0 as the edition release; [Cargo MSRV guidance](https://doc.rust-lang.org/cargo/reference/rust-version.html) requires explicit compatibility verification. #19 pins an exact stable toolchain, tests MSRV and stable, and commits the application lockfile. A dependency requiring a newer compiler needs an ADR and compatibility note; nightly is never mandatory for library users.

| Target | Bootstrap checks | Release meaning |
| --- | --- | --- |
| x86_64-unknown-linux-gnu | Compile/test, real filesystem and fault tests | Candidate production, qualification still required |
| aarch64-unknown-linux-gnu | Native runner tests plus filesystem/fault tests | Candidate production, qualification still required |
| aarch64-apple-darwin / x86_64-apple-darwin | Compile/test embedded path and local I/O | Development/validation |
| Windows, musl, other architectures / network filesystems | No promised gate | Unsupported until separately reviewed |

## Dependencies and unsafe boundaries

The bootstrap uses no third-party Cargo crates. Initial core primitives use `std`; candidate checksum/parser/crypto/runtime dependencies require an inventory before bootstrap inclusion. For each dependency record exact version/features, source, SPDX license and transitive licenses, maintenance/release history, security advisories, MSRV/target tests, why std cannot suffice, alternatives and removal plan. Apache-2.0/MIT/BSD/ISC are candidates for approval; unknown/custom/copyleft terms need explicit legal review. Review build scripts, proc macros, native libraries and network downloads. Pin the lockfile, check advisories/licenses and publish SBOM at #111; do not assert a dependency is safe from popularity alone.

A network runtime (such as a reviewed Tokio version) may exist only in server/CLI orchestration, never in the embedded dependency closure. Parsing, checksums and cryptography are bounded utilities, not authoritative storage providers. SQLite/PostgreSQL/RocksDB cannot own persistence. Crypto implementations require reviewed libraries, not hand-written authentication primitives.

Default crates forbid unsafe. Platform I/O exceptions must isolate unsafe in small modules with documented preconditions, ownership, alignment/lifetime reasoning and a safe wrapper; review and Miri/sanitizer evidence belong to #100. No public raw page pointer, frame reference, file descriptor, internal lock guard or executor-specific type. Handles own IDs and bounded copied data; internal page guards must not escape calls or cross an await. Lock ordering and cancellation checkpoints are documented before concurrent paths ship.

## Injectable interfaces

The I/O interface exposes positioned read/write (byte count includes short operations), file length/truncate, sync-data/sync-all, atomic rename, directory sync and exclusive lock acquire/release. Offsets are checked u64; lengths are bounded, conversion to host usize checked; retry EINTR, loop short operations, reject unexpected EOF. Errors distinguish interrupted, full, corruption and poisoned durability without returning secrets. Failure injection can lose/reorder unsynced writes, tear sectors, interrupt any operation or fail sync. Reads of a previously synced write must be stable within the qualified platform contract.

Clock exposes UTC wall microseconds (i64) and monotonic deadlines as process-local opaque ticks. Monotonic ticks are never persisted; persisted schedules and lease decisions use wall timestamps with backward/forward jump policy defined in #80/#81. Randomness is injected for IDs/secrets, fallible and cryptographically suitable where required; deterministic test providers cannot be enabled silently in production. Durability accepts a sealed transaction and commit LSN, returning a successful barrier or poisoned write failure. These interfaces cannot invoke application code or network effects during a transaction.

The development-only `test-support` crate depends on `types` and `io` and is outside every production adapter closure. It owns independent E0 oracles and a single-file persistence model, not authoritative database storage.

## #19 bootstrap checklist

Create the dependency graph crates, workspace/license/MSRV metadata and lockfile; run DAG checks and prove `embedded` builds without server features or runtime. Add fmt/clippy/unit/doc tests, native target matrix, advisory/license audit and reproducible commands. Add fault I/O/clock providers and boundary tests before storage work. Record exact dependency rationale and unsafe inventory; reject accidental async/network imports in the core. These bootstrap commands now exist in [development.md](../development.md); database/runtime campaigns remain future work.
