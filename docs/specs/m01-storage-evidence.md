# M01 storage implementation and evidence

Implemented for [epic #2](https://github.com/ulrichheringer/vetradb/issues/2), STO-001 through STO-009. Experimental physical storage foundation; no usable SQL database, durable transactions, ARIES recovery or PostgreSQL compatibility. [ADR 0018](../adr/0018-m01-storage-bringup.md) records the implementation direction and its review status.

## Implemented APIs and evidence

| Task | API / behavior | Runnable evidence |
| --- | --- | --- |
| STO-001 | `vetra-io`: `LocalDirectory`, file-handle ownership, positioned I/O, exact read/write loops, rename/directory sync, `FaultFile` | `io/tests/local.rs`: independent competing process, handle lifetime, symlink/path checks, real short/interrupted writes and sync, failed rename/cleanup, ENOSPC and poisoned sync |
| STO-002 | `codec::Superblock`, `Page`, `Address`; v1 field widths, CRC32C, reserved fields, generational links; `Pager` bootstrap/reopen/alternate publication | `storage/tests/codecs.rs`: independent hex fixtures, lineage/version/bounds conflicts; `pager.rs`: real filesystem reopen and torn alternate copy |
| STO-003 | v1 tree records/slots, stable-ID compaction, `Value` scalar framing, overflow descriptor and bounded chains | `codecs.rs`: maximum inline fit, NULL/empty/binary distinction, cyclic/truncated/owner mismatches; `trees.rs`: 100 KB and exact 16 MiB overflow values |
| STO-004 | `Allocator`: authoritative owner/generation/retired state, free-space reconstruction, pin horizon and generation exhaustion | `allocator.rs`: 1000 seeded allocate/free/reconstruct operations; owner, pin, stale generation and u64 exhaustion failures |
| STO-005 | `BufferPool`, owned `PagePin`, allocator-bound pins, fixed frame/pin budgets, first-dirty/newest LSN, compare-and-replace; `PagerIo` bridge | `buffer.rs`: eight simultaneous loaders create one writable frame, CAS retry, pin exhaustion/reuse, WAL failure before data and failed write; `pager.rs`: native one-frame reads |
| STO-006 | `BPlusTree`: point lookup, insertion, leaf/internal/root growth, generational sibling/child links, whole-action publication/replay | `trees.rs`: shuffled seeded operations and structural walker; every staged split/root/merge boundary fault; subsequent logical undo retains another insertion |
| STO-007 | Delete/repack/rebalance/root shrink, ascending/descending bounded scans with exclusive resume keys, immutable snapshots | `trees.rs`: mixed insert/delete/scan reference oracle, range endpoint checks, pinned scans across root shrink, allocation pressure |
| STO-008 | `RowStore`, `RowVersion`, `VersionId`, `Visibility`: immutable version tree and retained primary candidates; shared allocator, atomic two-root batches | `rows.rs`: schema/primary changes preserve row identity, reference visibility bases, tombstones, immutable payloads, disjoint physical ownership, batch replay and all batch failure boundaries |
| STO-009 | Independent BTreeMap query oracle, DFS integrity walker, checksum-repaired malformed inputs, seeded standalone campaign | `codecs.rs`: 2000 mutations, truncation/bounds checks; `trees.rs`: three seeds × 350 operations and replay; injected bad separator/generation/shared page defects fail |

Paths in the table are relative to `crates/`. Run `cargo test --locked -p vetra-io -p vetra-storage`. The maintained full gate is `python3 tools/verify.py`, which also runs `cargo run --locked -p vetra-storage --bin fuzz-storage -- 42 200`. A larger bounded campaign is `cargo run --release --locked -p vetra-storage --bin fuzz-storage -- 42 10000`. The campaign prints the seed and failing operation index; rerun exactly those parameters to reproduce. This is deterministic mutation/model fuzzing, not coverage-guided libFuzzer qualification (#115).

## API and resource limits

- Page size 8192; one database-scoped generation per physical page; bytewise codec 1 keys only. Scalar framing is versioned independently and does not assert SQL comparison/collation semantics (#44).
- Values up to 16 MiB; overflow chunks up to 8096 bytes and chains at most 2073 pages. Tree keys at most 2048 bytes to guarantee internal fanout. Leaf records obey the tighter page-fit bound without truncation.
- Default standalone tree: 65,536 allocated pages, 100,000 records and fanout 64; configurable bounded bring-up fanout 3..256, pages 1..1,000,000 and records 1..1,000,000. Repacking needs temporary old + new allocations. Rows use one global allocator for both trees; standalone trees are separate arenas and must not be written into the same data file without a shared allocator.
- Page images are resident in a tree arena; staging can temporarily hold an old arena, replacement arena, decoded records and owned journal action. Memory is proportional to configured page/record/value limits. This implementation does not claim an out-of-core tree or efficient production capacity. The separate cache has 1..65,536 frames and an explicit pin budget; no background or unbounded I/O queue exists.
- Snapshot handles own immutable arenas and bounded allocator pins. Mutations require `&mut BPlusTree`; callers serialize writers. Scans resume within the same snapshot, so concurrent root publication cannot duplicate or miss its rows. Resuming against a different snapshot has no snapshot-consistency guarantee.
- `SlottedRecords` references are logical IDs, not physical slot offsets; tombstoned IDs are not reusable. Physical page encoding repacks payload bytes and zeros unused space. Persistent stable record-ID indirection is part of the owned action metadata awaiting M02 encoding.
- `RowStore` chooses the greatest visible version identity within a stable row, retains old primary candidates and validates referenced versions. Version identities must be monotonically assigned per row by the future transaction layer; visibility hooks supply commitment/basis rules. Physical primary uniqueness and isolation are not implemented here. Committed payloads cannot be overwritten through append.

## Failure and recovery boundaries

`StructuralJournal::seal`/`seal_batch` must return success only for a complete action or batch. `MemoryJournal` exposes executed stage traces and pending unsealed page images for fault tests; replay consumes only completed actions. Failure before END leaves the published tree/allocator unchanged, including at split, merge and root changes. Completed action replay verifies previous-root lineage, all page images, ownership, identities and tree structure before installation; repeated latest replay is checked and idempotent. Row projection batches replay together.

This is an owned recovery interface and volatile failure model, **not a durable WAL implementation**. No process-kill, power-loss or acknowledged-commit recovery claim follows. M02 must implement v1 TOP/full-page/WAL framing, recovery pins, logical undo and ambiguous-outcome poisoning before these interfaces are used for durable transactions. `Pager` gates writes through `WalBarrier` and poisons after failed data/superblock I/O; a caller-supplied barrier is not evidence of real durable WAL. Physical round-trip tests explicitly supply an experimental barrier and retained recovery metadata.

The allocator generation/state serialization is an internal owned representation with checked counts. v1 allocation pages store generation maps; ownership must be persisted as authoritative allocation tree records by M02. No new proprietary manifest or competing durable store is installed.

## Platform/support matrix

| Platform | Current scope |
| --- | --- |
| macOS local filesystem | Native tests on the current ARM host; development validation, not power-loss qualification |
| Linux local filesystem | Code and tests enabled in existing x86_64/aarch64 CI; execution of this change awaits publication, no local Linux evidence claimed |
| Windows/other Unix/network filesystems | Unsupported local writable provider; no NFS/SMB or crash-durability claim |

Database directories must be in a trusted parent that other processes/users cannot rename during an operation. Existing directory/file permissions must deny group/other access; newly created directories/files use 0700/0600. Parent and child symlinks, traversal, lock-file names and non-file entries are rejected; Linux/macOS opens also use O_NOFOLLOW. Use a canonical trusted path (macOS `/var` is a system symlink). Hard-link/hostile-parent races are outside this trusted-parent contract. Identical database paths cannot be opened for writing by independent processes, and file handles keep the lease alive.

An unclean exit leaves `owner.lock`. Open fails with `OwnershipUnavailable`; there is no automatic PID-based stale-lock removal. An operator must prove every old owner/handle is stopped before removing that one lock file. Normal release removes it. File creation/bootstrap callers sync the containing directory; `rename` syncs it itself. Filesystems/storage must honor file and directory sync. The real fault tests verify typed failures, not filesystem detection or dishonest hardware.

## Local execution and measurements (2026-10-04)

The consolidated gate passed locally on macOS ARM with the pinned Rust 1.85.0 and 1.97.1 compilers. It includes 33 new storage/I/O integration tests, formatting, warning-free Clippy, the existing workspace/doc tests, embedded-only build, static format fixtures and both seed-42 campaigns. These are local results; this change has not been published to remote CI.

The release-mode seed-42 storage campaign passed 10,000 operations on 64 possible 8-byte keys, 8-byte values and fanout 3. It sealed 9,374 actions and staged 233,708 page images (1,914,535,936 cumulative image bytes). The harness drains completed test actions after each operation, preserving the journal LSN high-water, so this cumulative traffic is not retained memory. These are generated page-image counts, not measured disk/WAL traffic or a production benchmark. They substantiate the severe O(n) write-amplification cost of the correctness-first repacker.

The native pager/cache round-trip traverses persisted multi-level tree pages with exactly one resident cache frame. The maximum-value regression round-trips 16 MiB through the bounded overflow chain and rejects one extra byte without changing the root. Snapshot tests retain old rows across root shrink and reject reuse while pinned. These measurements do not qualify cache hit rate, production throughput, latency or durability; those require later workload/platform qualification.

## Review/closure

The local implementation gates provide reproducible M01 evidence, with the restrictions above. Architectural review and cross-platform CI for this change remain review/publication steps. No GitHub issue is auto-closed, and M02 prerequisites are not marked complete by a volatile replay test. Review this evidence against each child issue before closing the epic.
