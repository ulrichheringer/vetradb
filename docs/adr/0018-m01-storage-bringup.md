# ADR 0018: M01 storage bring-up

Status: implemented experimental direction for the owner-requested #2 scope on 2026-10-04; merge/release ratification pending review. This does not declare M02 recovery complete.

## Context and decision

Implement the accepted 8192-byte v1 physical containers without dependencies or unsafe Rust. Expose synchronous owned structural actions through `recovery-api`, keeping storage independent of concrete WAL. The initial tree has one serialized writer and immutable pinned snapshots. Each mutation repacks ordered leaves and internal separators into newly allocated pages and publishes a root after the journal seals a complete action. This produces actual leaf/internal splits, merges and root height changes, with O(n) write cost. It is deliberately a correctness bring-up, not an incremental high-performance tree or out-of-core query engine.

Version and current-primary trees share an authoritative allocator and seal their two root changes as one batch. Old primary candidates remain until a future transaction-layer reclamation horizon permits removal. Row payloads are append-only by stable table/row/version identity; transaction visibility is injected rather than guessed.

Local directory ownership uses an exclusive `create_new` lock file, kept alive by all file handles. It survives an unclean exit and fails closed on the next open. Only an operator who has proved the previous owner is stopped may remove the stale lock. This conservative scheme avoids PID-based automatic unlock races and external dependencies. Kernel-managed crash release is an alternative for future platform work.

## Alternatives and consequences

Incremental in-place trees have lower write amplification but need multi-page latch/recovery orchestration before their bring-up can be trusted. The existing v1 layout permits them later. Whole-tree repacking makes snapshots and atomic failure straightforward; it requires room for both old and replacement pages and can return a capacity error despite reclaimable capacity being pinned. No persisted format version changes are introduced by this direction.

`StructuralAction` and allocator `VALLOC01` are owned staging representations, **not new format-v1 files**. The future WAL provider must encode the allocation/root and record-identity metadata using specified containers. It must log logical user insertion/deletion as undoable effects separately from structural top actions, preserve completed structure on user rollback, implement full-page protection, seal multi-root batches, and poison ambiguous sync failures. The volatile provider and logical-delete rollback regression do not substantiate crash recovery or ACID.

## Invariants and verification

Checksums and checked codecs match independently authored golden superblock/empty-leaf fixtures. Every live page has one owning tree and reuse generation. The walker checks reachability, unique parent ownership, sibling order, separator coverage, balanced depths and overflow identities. Failed staging cannot publish a root; snapshots retain their pages and pins block generation reuse. Cache frames are bounded and dirty writes require WAL durability through page LSN.

See [M01 evidence and API limits](../specs/m01-storage-evidence.md) and [the persistent format](../specs/persistent-format-v1.md). A future incremental implementation must keep the same reference-map, malformed-codec, allocation, fault-boundary and snapshot gates. No incompatible format migration is required for this experimental API; no SQL or durable release is published.
