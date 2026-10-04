# Storage, WAL and recovery

## Initial physical design

Proposed initial page size: 8 KiB, fixed for a database and encoded in its superblock. The format-freeze issue must confirm it with overflow, cache and write-amplification measurements. A superblock carries magic, format version, database/timeline IDs, generation and checkpoint metadata with checksummed redundant copies.

Page headers contain kind, page ID, reuse generation, page LSN, checksum and bounds. Slotted pages store variable-length records; overflow chains store large values. No persisted data uses native Rust memory layout, pointer values, `usize` or unchecked enum discriminants. Key encoding must preserve SQL order, including NULL placement and collation identifiers.

Primary structures:

- Row-version B+Tree keyed by stable table ID, row ID and version identity. Payloads include schema ID and creator transaction; committed payloads remain logically immutable.
- Current row directory and secondary indexes as MVCC-aware serving projections. Old index entries remain usable for pinned snapshots until their reclamation horizon permits removal.
- Transaction status/CSN index and immutable committed transaction envelopes, indexed both by commit order and object identity. Historical secondary indexes may be introduced later; correct history scans are available before optimizing them.
- Versioned catalog, queue readiness, schedules, publications and consumer offsets using the same page, transaction and WAL machinery.

## B+Tree invariants

Keys are ordered and separators cover all descendants. Each non-root page has one owning tree; links, root changes and page generations are checked. Splits and merges must be recoverable across every intermediate write. Structural changes use documented system transactions/top actions; undoing a user insert must not reverse a split needed by another transaction. A tree traversal may retry when its observed generation changes.

Initial scans include point, range, ascending and descending traversal. Uniqueness is a transaction-level check with key locks and visibility, not merely a physical absence test. Physical deletion/reuse waits for every relevant reader, recovery and replication pin.

## Buffer and I/O

Pinned frames cannot be evicted. Dirty frames record the first recovery LSN and newest page LSN. Page flush requires WAL durability through the page LSN. Bounds on buffer memory, pinned pages and I/O queues are mandatory; pressure produces backpressure or an explicit error rather than unbounded growth.

The platform layer handles short writes, interrupted operations, sync errors, directory sync for file lifecycle and crash-safe manifest replacement. Local filesystems must honor the documented sync operations. Hardware or filesystems that falsely acknowledge durability are outside the fault model.

## WAL strategy

Adopt an ARIES-inspired steal/no-force design with physiological page redo, logical/physical undo where appropriate, transaction backward links, page LSN checks and compensation log records (CLRs). This is a design choice requiring its own recovery specification and tests; the repository does not claim an ARIES implementation.

WAL records carry version, length, kind, LSN, transaction ID, previous transaction LSN and checksum. The codec defines maximum record size, multi-record framing, segment identity and torn-tail handling. Records cover allocation, row/index/catalog/ledger/service mutations, begin/commit/abort, checkpoints and CLRs. Unknown format versions fail before mutation.

WAL-before-data and durable-commit-before-visibility are unconditional. The first change to a page after a checkpoint has a full-page WAL image, or an equivalently verified torn-page protection scheme approved by an ADR. A checksum detects corruption; it does not repair a page without a recoverable image.

## Recovery protocol

Open under exclusive ownership, validate superblocks and WAL segment lineage, then run analysis, redo and undo. Analysis reconstructs dirty pages and transaction state. Redo repeats logged history idempotently using page LSN/generation checks, including changes of transactions later undone. Undo follows loser transactions backward, emits CLRs and completes aborts; a crash during recovery is recoverable on the next open.

Commit records define committed transactions, but recovery publishes only complete valid commits in the durable prefix. An incomplete final WAL record may be discarded under the framing contract. Missing/corrupt records inside an acknowledged prefix MUST cause a corruption error; recovery cannot silently truncate acknowledged data. Structural top actions survive user rollback as specified while logical user effects disappear.

Fuzzy checkpoints record dirty-page and active-transaction tables. Checkpoint completion and file manifests are crash-safe. WAL recycling respects the minimum horizon needed by dirty pages, active undo, backups/PITR archive, CDC if using WAL, and future replicas. Logical history retention uses an independent horizon and cannot be inferred from checkpoint completion.

## Maintenance and format changes

Vacuum removes unreachable/aborted versions and obsolete serving-index entries only after proving no reader needs them. Complete committed history is preserved by default, even when hot serving structures compact. Rebuilds are resumable and publish replacement roots atomically. Backup records all physical roots and required WAL coverage.

Pre-1.0 formats may change only with an explicit version bump and migration/export path. 1.0 defines readable format ranges, supported upgrade paths, rollback constraints and checksummed fixtures. Replicas will not be considered compatible merely because they have the same SQL version.
