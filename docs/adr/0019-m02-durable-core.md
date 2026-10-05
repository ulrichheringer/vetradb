# ADR 0019: M02 durable native core

Status: implemented experimental direction for the owner-requested [M02 #3](https://github.com/ulrichheringer/vetradb/issues/3), 2026-10-04. Merge/release ratification is a separate review step. Refines ADRs 0013/0015/0018 without changing existing page, envelope or WAL framing.

## Context and decision

Implement all TXN-001–TXN-012 through the synchronous native API. The M01 COW tree remains resident and serialized at physical publication; transactions can stage concurrently. The commit sequencer prepares the complete version/primary projection and bounded envelopes, logs complete structural batches, appends COMMIT, flushes once for the group, then publishes all participants under one mutex. No page or participant becomes visible before the barrier. Attempt IDs are reserved with a synced BEGIN before exposing a handle. CSNs reserved by physical row versions are preserved across restart, including failed attempts.

Physical COW actions log initialized replacement pages as v1 FULL_PAGE records within TOP_BEGIN/END; they do not overwrite existing live pages. Their root, allocation, retired-page and record-ID metadata is encoded by the internal codec below, inside v1 overflow pages in the WAL. The metadata arena is never a second file/store and is not materialized into data.v1. Root histories are replayed/validated from the retained WAL before opening the native API. Completed structure survives an aborted creator; creator visibility and vacuum exclude aborted versions. Ordinary in-place PAGE_PATCH/CLR recovery is a separately tested lower-layer capability; native rows use COW batches.

## Internal action codec 1

`VACT0001`, action count u32 (1..16), then each action: tree u64, previous root ID/generation u64 each, new root ID/generation u64 each, record identity high-water u64; allocator blob (length u32 + bytes); page count u32 and `(ID,generation)` u64 pairs; retired count u32 and pairs; record-ID count u32 and entries `(key length u32, key bytes, record ID u64)`. Exact consumption, at most 16 MiB metadata, counts at most 1,000,000, unique image references. Unknown magic/codec, absent/extra images, invalid allocator ownership/generation, root predecessor, record identity or tree structure fail before publication.

The allocator blob freezes the existing `VALLOC01` representation: magic 8 bytes; entry count u64; entries `(page ID:u64, generation:u64, owner:u64 [0 means free], retired:u8 [0/1], reserved:7 zero bytes)`, sorted uniquely by ID. It is decoded by `Allocator::restore` with exact consumption and configured limits. Retired entries retain an owner; free-space information is derived. See the executable codec in `crates/storage/src/allocator.rs`.

Metadata is chained through v1 overflow images with tree owner 1, owner-record equal to the action ID, page IDs starting at 1,000,002, and page generation equal to the action ID. This WAL-only arena cannot overlap the bounded tree arena (at most 1,000,000 pages starting at ID 2). Chunks are at most 8096 bytes, strictly contiguous IDs with matching generations, valid checksums, exact terminal next pointer. All referenced user images precede TOP_END. The END LSN stamps the completed pages and roots. No data flush is permitted before the associated END barrier. Standalone `Journal` can sync on seal; the engine defers that barrier to the group's final COMMIT flush. An unfinished serial top action is abandoned when a new TOP_BEGIN follows after recovery; no unfinished image is published.

This is an explicitly specified internal storage metadata codec, not a SQL value codec or PostgreSQL compatibility claim. Golden page/WAL/envelope framing stays byte-for-byte v1 compatible. Old foundation readers reject unspecified content rather than silently reinterpret it. There is no supported previous database release requiring migration.

## Isolation and progress

Read uncommitted maps explicitly to native read committed. Each read/scan call is a statement; RC refreshes its basis, RR holds its transaction basis and rejects stale writes. RR write skew is allowed and demonstrated. Staged own writes override the committed projection. Row, missing-key and inclusive range/full-scan shared locks protect serializable reads; every writer, including RC/RR, takes exclusive key locks. Serializable reads revalidate after lock acquisition and reject a stale fixed snapshot. Locks are strict through transaction end, including after rollback-to-savepoint; this is intentionally more conservative than future PostgreSQL savepoint lock behavior.

For mixed-mode cycles, nonserializable attempts record bounded read predicates. If a serializable transaction commits during their lifetime, they must revalidate those predicates before committing. Changed dependencies return a retryable serialization error. This preserves the demonstrated mixed-mode histories without imposing serializability on RR-only workloads. The independent exhaustive serial-order oracle qualifies generated small histories, not an unbounded formal proof.

Compatible lock requests can progress concurrently. Conflicting requests are FIFO; cycles abort the youngest attempt completely, release every lock and wake waiters. Timeouts/cancellation fail the statement and cancel its queue entry. The explicit transaction stays failed until rollback-to-savepoint or full rollback. Handle drop performs full abort/release. Bounded diagnostics report only counts and attempt IDs.

## Recovery, checkpoints and retention

Analysis validates complete envelope/COMMIT pairs, action batches and checkpoints. COW action replay verifies roots, allocation, page generations, identities and both projections. Physiological redo repairs damaged pages from eligible full images, replays patches/CLRs and undoes loser chains with synced redo-only CLRs. Restart during undo skips already compensated records. A valid mismatching generation is rejected unless the validated COW allocation history proves reuse. Reopen flushes validated WAL before exposing any recovered state.

Fuzzy checkpoints persist conservative dirty/active tables and complete BEGIN/chunks/END with checksum. They are hints; startup currently scans all retained WAL. No WAL segment recycling or fast-checkpoint-only restart is advertised. This deliberately retains every active undo, dirty page, backup/service pin and logical history dependency. Normal mutations stop at 256 MiB resident WAL; an additional 256 MiB is reserved for CLR/ABORT/END recovery. Real device exhaustion still requires free space before recovery can write; failures never acknowledge a commit.

Vacuum rewrites serving roots through durable top batches, retaining the version at the minimum reader/undo/backup/service horizon plus later versions. It preserves complete ledger/schema images. Expired explicit pins are removed only by `vacuum(now)`; subsequent reads fail with Expired. Active transaction pins do not expire. All snapshot expiry ticks are supplied by the native caller's clock policy, not wall-clock sampling inside reads.

## Alternatives and limits

An envelope-only restart adapter was insufficient for M01 physical metadata; physical top batches are required and now integrated. Incremental trees, SSI, streaming/out-of-core recovery, WAL recycling, production capacity tuning, SQL/pgwire and actual service dispatch remain separate milestones. The initial implementation favors bounded, inspectable correctness over write amplification. Authentication adapters and historical query authorization are not implemented by a caller-constructed trusted native Context.

See [M02 evidence](../specs/m02-transaction-evidence.md) for per-task APIs, independent oracles, crash levels and supported limits. Production E3/E4 storage/power-loss and operational release qualification are not implied by native CI or process-kill tests.
