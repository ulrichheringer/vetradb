# Events, CDC, pub/sub and realtime

## Shared committed envelope

An event publication is a mutation in the application's transaction. Commit creates the durable publication, user-row changes and history together. Abort emits nothing. CDC derives committed logical changes from the same envelope, including transaction boundaries, stable schema IDs, before/after images, metadata and operation order.

Do not make network sends, callbacks or notifications part of the durable commit barrier. Dispatchers read durable records after commit; wakeups accelerate scans but do not determine correctness.

## Cursor and consumer contract

Cursor identity contains database, timeline, CSN, operation index and envelope version. A resume cursor selects the next event after its position. Ordering is transaction/operation order within a database timeline; no cross-database or wall-clock global ordering is implied.

Delivery is at least once. Durable consumer offsets and acknowledgments can be committed with application data in the same engine transaction. An event's stable ID supports deduplication. A consumer must not acknowledge a cursor beyond its delivered contiguous range or another timeline. Consumer groups use fenced ownership and explicit partition/order semantics.

The decoder emits only whole committed transactions or documented chunk frames with a final transaction boundary; a consumer cannot mistake an incomplete chunk for an atomic commit. Large transactions need bounded transport buffers and a recovery-safe continuation protocol.

## Topic modes

Durable topics retain publications for replay under an explicit retention/consumer policy. Topic definitions, subscriptions, consumer-group state and offsets are versioned control data. Publisher/subscriber grants are independent and payload limits apply before commit.

PostgreSQL-compatible LISTEN/NOTIFY is a distinct ephemeral channel. It follows commit/rollback, delivery and duplicate-folding rules from the compatibility specification and does not claim durable replay. Native durable pub/sub has its own namespace and cursor protocol.

## CDC and initial snapshots

Create a protected snapshot basis B and register/pin the replay range after B before scanning the snapshot. Read the authorized table/catalog snapshot at B. Then deliver every relevant committed envelope strictly after B. Registration and basis selection are atomic under the commit coordinator; buffered delivery is bounded or spooled to durable storage.

If the snapshot/consumer pin expires, fail with an explicit resnapshot requirement rather than leave a gap. Schema changes are ordered before or with the data encoded under them; consumers receive enough historical schema information to decode rows after restart. DDL and deletes must not silently disappear from a row-only decoder.

Native CDC is not PostgreSQL logical-replication wire compatibility. PostgreSQL documents restart/replay and retention considerations for logical decoding; these inform the independent VetraDB cursor contract. [Official logical decoding concepts](https://www.postgresql.org/docs/17/logicaldecoding-explanation.html).

## Realtime subscriptions

1.0 scope: authorized single-table filter/projection subscriptions with stable row identity, an initial snapshot and deterministic change deltas. Eligibility rejects joins, aggregates, volatile expressions and plans without a supported incremental strategy. Broader query maintenance requires new issues and evidence.

Events distinguish insert, update, delete, enter-filter and leave-filter. Change evaluation uses old/new values under the same transaction envelope and schema basis. Transaction boundaries allow clients to apply a group atomically. A permissions change invalidates/revalidates existing subscriptions before further delivery.

The native streaming API will have a versioned envelope and transport chosen by its RFC; proposed server transport is authenticated WebSocket, with an embedded bounded stream API. Backpressure limits connection memory, transaction bytes and replay lag. Slow consumers are paused/spooled or disconnected with a resumable cursor; policy is documented and observable.

## Retention and failures

Publication retention, history retention and WAL retention are distinct. The retention coordinator checks consumer, snapshot, archive and future-replica pins. A cursor older than the supported range produces `cursor_expired`; a different timeline produces `timeline_mismatch`.

Reconnect may replay acknowledged-but-not-persisted client messages. Crashes before acknowledgment repeat delivery; crashes after committed offset advancement resume after the offset. Security and row filtering apply to replay, historical images and metadata as well as live data. Metrics expose dispatch lag, consumer lag, pins, drops/expiry, memory pressure and decode errors.
