# Immutable history and temporal semantics

## Three separate concepts

Physical WAL repairs storage pages. MVCC determines which versions concurrent transactions can see. The immutable committed ledger records how logical data changed and is retained independently of WAL recycling or hot-version vacuum.

Every committed user-row mutation records stable object/row IDs, schema version, operation, previous/resulting version references and lossless values. Transaction envelopes include catalog changes and supported queue/job/event/offset mutations. Current rows and historical queries refer to the same committed mutations; history is not an asynchronously populated audit table.

One transaction may change a row several times. The ledger retains each supported logical operation in statement/operation order. AS OF returns the final row state at a committed transaction boundary; ordinary history can expose intermediate operations as provenance, explicitly marked as intra-transaction operations rather than separately committed states. Aborted operations and rolled-back savepoint operations are excluded from committed logical history.

## Basis and ordering

A precise basis is `(database_id, timeline_id, commit_seq)`. It selects all committed transactions at or before the sequence. A row version is current at basis B when its creation is committed by B and no subsequent committed mutation through B replaces/deletes it. Delete records a tombstone/retraction; it does not destroy earlier values.

Store both observed UTC wall time and a nondecreasing commit timestamp assigned by the sequencer. Timestamp AS OF resolves to the greatest committed CSN whose assigned commit timestamp is <= the requested time. Clock rollback is clamped and observable; ties resolve to the greatest matching CSN. Timestamp resolution is less precise than a commit basis and must not be represented as a globally unique transaction ID. Future-basis policy: reject unknown CSNs rather than silently return current data.

Transaction-time history does not assert business valid time. Backdated application events retain their real commit basis and carry business time separately. [Datomic's time filters](https://docs.datomic.com/reference/filters.html) inform the distinction between a point-in-time view and a history view; the contracts above are VetraDB's own design.

## Schema and queries

An AS OF read pins both data and catalog to the same basis. Rename/drop/type change remain interpretable through stable schema IDs. Historical table selection by stable ID is available when a reused name would be ambiguous. A mixed-basis join is permitted only through explicit independent snapshot aliases; default queries use one consistent basis.

History queries expose transaction ID, CSN, operation position, table/row/column identities, historical schema, before/after values or lossless references, operation type and metadata. They support transaction-range scans, row change history, diffs and joins to transaction metadata. Pagination uses basis-aware keyset cursors, not offsets that shift as new commits arrive.

Snapshot handles are immutable read views with explicit close/TTL and resource quotas. They pin the relevant MVCC, schema and history horizons. A logical read snapshot is distinct from a physical checkpoint, backup or future replica snapshot. Restart persistence requires an explicitly named durable snapshot, not an in-memory handle.

## Metadata and audit

Trusted fields include authenticated principal, database/session identity and engine commit time. Application metadata includes request/correlation ID, actor claim, source and tags, capped at 16 KiB per transaction by the [accepted format-v1 contract](specs/persistent-format-v1.md) and a validated shape. Application-supplied actor labels never replace the authenticated principal.

Successful mutations are audited through the transaction ledger. Failed authentication, denied access and operational actions use a separate bounded security/operations audit sink because an aborted transaction must not enter committed data history. Secrets, password verifiers and transport credentials never appear as general history payloads or user-query logs; credential state is held behind the security-provider boundary. Full versioning covers relational application data and declared service state.

History grants are separate from current-table SELECT. Current authorization applies to every historical read and metadata projection; a past grant does not resurrect present access. Row policies, when available, must filter historical values and CDC/realtime consistently.

## Retention and integrity

Default: retain every committed logical data/schema version. Compaction and hot MVCC vacuum may change physical layout but preserve lossless committed history. Finite history policies are explicit per database/table and expose an oldest available basis. Queries outside it return `history_unavailable`; they cannot return an incomplete approximation.

Retention cannot remove data required by active snapshots, backups, consumer replay guarantees or future replicas without expiring the pin through a documented policy. Pin quotas and disk-pressure alarms prevent an unbounded forgotten reader from filling the disk.

An exceptional privileged redaction workflow is outside ordinary SQL: preview the affected history, require specific authority, append an audit marker, invalidate affected historical/export ranges and account for copies in backups, archives and replicas. It must not claim automatic erasure from external copies. Do not silently add a purge capability under the name vacuum.

Checksums and optional chained envelope digests detect accidental corruption. Without externally anchored signatures and an independently trusted key/retention system, this is not proof against a malicious administrator. [Datomic's transaction log](https://docs.datomic.com/reference/log.html) provides inspiration for transaction-ordered access, without dictating the relational encoding.
