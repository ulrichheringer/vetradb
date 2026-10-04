# Persistent format v1

Accepted normative design for [#16](https://github.com/ulrichheringer/vetradb/issues/16) / [ADR 0013](../adr/0013-persistent-format-v1.md); accepted by the repository owner on 2026-10-04. This defines physical containers and transaction envelopes. SQL scalar/key ordering codecs are separately frozen by #23/#44; no unspecified codec may be persisted or advertised as format-v1 compatible.

## Common rules and identities

All integers are unsigned little-endian unless marked i64 (two's complement); byte offsets start at zero. UUIDs are 16 opaque RFC UUID bytes, not endian-swapped integer fields. Magic is literal ASCII. Version is u16 = 1 for each container. Reserved/flags fields MUST be zero in v1; unknown versions, kinds or nonzero reserved fields fail before mutation. Readers validate lengths/checksums before allocation, bounds before arithmetic, and exact consumption (no ignored trailing data). CRC32C uses reflected Castagnoli polynomial 0x82f63b78, initial/final XOR 0xffffffff; checksum covers the full container with its checksum field zeroed. `123456789` => 0xe3069283. CRC detects accidental corruption, not hostile modification.

DB/timeline UUID scope every file and durable cursor. TxID, table/column/schema/row/tree/object IDs are nonzero u64 scoped to a database and never reused after rollback; reserve durable ID high-water ranges before exposing IDs. PageID is u64 physical index in `data.v1`, PageGeneration u64 starts at 1 and increases on reuse; exhaustion fails closed. Data pages start at PageID 2; superblock copies occupy 0 and 1. Null page references are `(0,0)`; mixed zero/nonzero pairs are invalid. File length is an exact multiple of 8192. IDs are not Rust pointer/usize encodings.

LSN is u64 virtual byte position: `segment_no * 67108864 + segment_offset`, checked for overflow; segment header occupies offsets 0–63. Zero means absent. TxIDs identify attempts, not ordering. CSN is a database/timeline u64 commit ordering key; 0 is the empty basis. Every increment and size calculation is checked; exhaustion is an explicit error, never wraparound. UTC timestamps are i64 microseconds since Unix epoch; they are informational and not unique ordering keys. Timestamp lookup chooses the greatest eligible CSN under the temporal contract, not WAL order.

## Superblocks (8192 bytes each)

| Offset | Width | Field / validation |
| --- | --- | --- |
| 0 | 8 | `VETRASB1` |
| 8 | 2 | version = 1 |
| 10 | 2 | header bytes = 128 |
| 12 | 4 | page bytes = 8192 |
| 16 | 16 | database UUID, nonzero |
| 32 | 16 | timeline UUID, nonzero |
| 48 | 8 | generation, nonzero |
| 56 | 8 | completed checkpoint end LSN, 0 initially |
| 64 | 8 | retained WAL start LSN, 0 initially |
| 72 | 8 | catalog root PageID |
| 80 | 8 | catalog root PageGeneration |
| 88 | 8 | allocator root PageID |
| 96 | 8 | allocator root PageGeneration |
| 104 | 8 | durable identity high-water, initially 0 |
| 112 | 4 | flags = 0 |
| 116 | 4 | CRC32C over 8192 bytes |
| 120 | 8 | reserved = 0 |
| 128 | 8064 | zero padding |

Write the older copy with generation+1 only after its referenced roots and checkpoint WAL are durable; sync the data file before considering it selected. At bootstrap write/sync both copies plus containing directory before successful open. Choose the highest valid generation with matching lineage; equal-generation different contents, conflicting valid lineages or two invalid copies are corruption. Never guess a new lineage from damaged bytes. A bad newer copy permits the older copy only with its required WAL still retained. Completed checkpoint references are hints; WAL/commit validation determines visibility. No file is recycled until a durable new superblock and all retention pins allow it.

## Data page header (64 bytes)

| Offset | Width | Field / validation |
| --- | --- | --- |
| 0 | 4 | `VPG1` |
| 4 | 2 | version = 1 |
| 6 | 2 | kind: 1 leaf, 2 internal, 3 overflow, 4 allocation |
| 8 | 8 | PageID >= 2, matches file offset |
| 16 | 8 | PageGeneration >= 1 |
| 24 | 8 | page LSN, 0 only for never-mutated page |
| 32 | 8 | owning tree ID, nonzero (allocator has reserved tree ID 1) |
| 40 | 2 | lower bound |
| 42 | 2 | upper bound |
| 44 | 2 | slot count |
| 46 | 2 | flags = 0 |
| 48 | 4 | CRC32C over 8192 bytes |
| 52 | 12 | reserved = 0 |

Tree pages have a 64-byte special region at 64: left sibling ID/gen at 64/72 (u64 each), right ID/gen at 80/88, first-child ID/gen at 96/104, level at 112 (u16), high-key slot at 114 (u16, 65535 = +infinity), reserved zero at 116 (12 bytes). Leaves have level 0 and null first-child; internal pages level >=1 and nonnull first-child. Slots start at 128, each `(offset:u16, length:u16)`. `lower = 128 + 4*slot_count <= upper <= 8192`; occupied records lie in `[upper,8192)`, do not overlap, length >=32. Unused free bytes are zeroed when encoded. High-key slot, if present, names a valid slot and excludes the record from ordinary key iteration. Key order and separator coverage must be checked using the declared comparator; CRC alone is insufficient.

Each tree record: at 0 key_len u32, 4 value_len u32, 8 creator TxID u64, 16 schema ID u64 (0 only for physical/internal records), 24 codec ID u16 (1 bytewise structural, SQL IDs reserved until #44), 26 flags u16 (0 inline, 1 overflow), 28 reserved u32=0; then key bytes and value bytes with exact length `32+key_len+value_len`. Maximum inline record/key length 8060 bytes, additionally limited by page space; no truncate-on-fit. Internal records use codec 1, flags 0 and a 16-byte child ID/gen value; creator/schema are zero. Leaf user records require nonzero creator/schema. Oversized values use a 24-byte descriptor: first overflow ID/gen u64 each and total byte length u64 <=16777216. Key comparison for SQL data MUST await #44, not assume little-endian integer bytes sort numerically.

Overflow special fields: next ID/gen at 64/72, owner record ID u64 at 80 nonzero, chunk_len u32 at 88 <=8096, reserved u32 at 92=0. Payload starts at 96; lower=96, upper=96+chunk_len, slots=0. Remaining bytes zero. Chains validate generation, owner, exact total, no cycles and at most 2073 pages for a 16 MiB value (including short non-final chunks requires rejecting beyond that bound). Allocation pages: base PageID u64 at 64, count u32 at 72 <=1014, reserved u32 at 76=0; entries start at 80, each generation u64 (0 never allocated). Allocation state is encoded as a tree record separately; this generation map is not a second source of ownership. lower=80, upper=80+8*count, slots=0, remainder zero. Reuse requires persisted generation increment and all reader/recovery pins released.

## WAL files and framing

Segment size is 64 MiB including header. Filename is 16 lowercase hex digits of segment number followed by `.wal`; DB/timeline live in the header. Segment header is 64 bytes: magic `VETRAWL1` at 0 (8), version at 8 (u16=1), header size at 10 (u16=64), segment size at 12 (u32=67108864), DB UUID at 16 (16), timeline UUID at 32 (16), segment number at 48 (u64), CRC at 56 (u32), reserved at 60 (u32=0). Sync file and directory on creation. A record never crosses a segment; remaining space is zero padding and the next record starts at the next segment's offset 64. Missing interior segments fail open.

Record header (64 bytes): magic `VWR1` at 0 (4), total_len u32 at 4, version u16 at 8, kind u16 at 10, flags u32 at 12=0, LSN u64 at 16, TxID u64 at 24, prev_tx_lsn u64 at 32, page ID u64 at 40, page generation u64 at 48, payload_len u32 at 56, CRC u32 at 60. Exact total is `64+payload_len`, max 1048576, no alignment padding within records. Previous TxLSN is 0 or an earlier record for the same attempt. Non-page records use null page address. Payload integers obey common rules.

| Kind | Payload layout and meaning |
| --- | --- |
| 1 BEGIN | Empty; nonzero TxID |
| 2 PAGE_PATCH | offset u16, length u16, reserved u32=0, before bytes[length], after bytes[length]; range within [64,8192), nonzero length; header LSN/CRC rebuilt by apply |
| 3 FULL_PAGE | Exactly 8192 bytes pre-change page image with valid CRC/address; precedes first post-checkpoint modification and can repair torn pages before replay |
| 4 CLR | undo_next_lsn u64, offset u16, length u16, reserved u32=0, redo bytes[length]; follows loser chain, never undone; same patch bounds |
| 5 TOP_BEGIN | action ID u64 nonzero; TxID=0, prev=0 |
| 6 TOP_PATCH | action ID u64 then PAGE_PATCH payload; TxID=0, prev=0 |
| 7 TOP_END | action ID u64; TxID=0, prev=0 |
| 8 ENVELOPE_CHUNK | chunk index u32, chunk count u32 (1..17), total envelope bytes u32 (<=16777216), chunk length u32 (<=1048496), then exact bytes; nonzero TxID; all chunks precede COMMIT |
| 9 COMMIT | CSN u64 nonzero, envelope total u32, envelope CRC u32, first chunk LSN u64; nonzero TxID |
| 10 ABORT | Empty; decision starts undo, not proof undo completed |
| 11 END | Empty; commit or completed undo only |
| 12 CHECKPOINT_BEGIN | checkpoint ID u64 nonzero |
| 13 CHECKPOINT_CHUNK | checkpoint ID u64, index u32, count u32 (1..17), byte length u32, reserved u32=0, then bytes; same bounded total as envelope |
| 14 CHECKPOINT_END | checkpoint ID u64, begin LSN u64, total table bytes u32, table CRC u32 |

Checkpoint table concatenates: dirty count u32, active count u32, top-action count u32, reserved u32=0; dirty entries `(page ID:u64, gen:u64, recLSN:u64)`; active entries `(TxID:u64, lastLSN:u64, undoNextLSN:u64, reservedCSN:u64, state:u8, reserved:7 zero bytes)`; top entries `(action ID:u64, beginLSN:u64)` for unfinished actions. State 1 active, 2 committing, 3 aborting; counts <=262144 each and exact total <=16 MiB. Checkpoint chunks are hints until matching END/checksum; never truncate WAL based on incomplete tables. Min retained LSN includes dirty recLSNs, active transaction backward chains, unfinished top actions, backup/archive and replica pins. CSN reservation high-water comes from the largest valid observed reservation/commit; unused values can be skipped only after recovery proves their attempts cannot commit.

A top action logs all page changes including sibling/root/allocator changes, then TOP_END before a user commit or affected page flush; page LSN for durability protection is the action END LSN. Other transactions cannot use half-installed structure. A crash before END discards the action during analysis; no involved page may have reached disk. Analysis buffers top-action patches by action ID until END is validated; redo applies each completed action atomically under structural ownership and stamps its pages with the END LSN. Completed actions redo even when their initiating user aborts. Their patches include relocation of pre-existing rows; the new user's logical insert is a separate undoable operation. Full-page protection also applies to top actions (TxID=0). #26/#27 must verify latch ordering and multi-page publication.

## Logical envelope

Header (96 bytes): magic `VENV0001` at 0 (8), version u16 at 8=1, header size u16 at 10=96, total_len u32 at 12 (96..16777216), DB UUID at 16 (16), timeline UUID at 32 (16), TxID u64 at 48, CSN u64 at 56, committed_at i64 at 64, operation count u32 at 72 (<=65535), metadata length u32 at 76 (<=16384), principal ID u64 at 80 (nonzero trusted engine identity), CRC u32 at 88, flags u32 at 92=0. TxID/CSN nonzero, match COMMIT and lineage. Metadata then operations follow without padding.

Metadata: count u16 (<=64), reserved u16=0, then repeated key_len u16 (1..128), value_len u16 (0..4096), key UTF-8 bytes, value UTF-8 bytes. Entries sorted uniquely by UTF-8 key bytes, exact metadata consumption; caller keys cannot use `vetra.` prefix. Trusted principal is outside caller metadata. Total metadata <=16 KiB; passwords/tokens must not be injected as automatic metadata. Metadata is immutable with the envelope but not asserted truthful merely because persisted.

Operation header (32 bytes): position u32 at 0 (0..count-1, contiguous), participant u8 at 4 (1 row, 2 schema, 3 job, 4 schedule, 5 event, 6 offset, 7 permission), action u8 at 5 (1 put, 2 delete), codec u16 at 6=1, object ID u64 at 8 nonzero, schema ID u64 at 16 (nonzero for row/schema, otherwise 0), key length u32 at 24 (1..8192), payload length u32 at 28 (<=16777216 subject to whole envelope bound). Key bytes then payload bytes. IDs identify stable catalog objects, not names. Delete payload retains its before-image.

Payload codec 1: before_len u32, after_len u32, followed by canonical before/after byte sequences with exact consumption. Each image: field count u16 (<=4096), reserved u16=0, then fields sorted by nonzero stable field ID u64, value tag u8 (0 null, 1 bytes, 2 UTF-8, 3 u64, 4 i64, 5 bool), reserved 3 zero bytes, byte length u32, value bytes. Null length=0; u64/i64 length=8; bool length=1 and value 0/1; UTF-8 valid; bytes/text length <=16 MiB within total bounds. Put requires after_len>0, delete after_len=0 and before_len>0; new insert before_len=0. Both empty is invalid. SQL types requiring richer encoding must register a versioned codec in #44 before use; generic bytes cannot silently advertise SQL support.

Operations preserve the order of successful staged mutations across all participants. Savepoint rollback removes rolled-back operations and renumbers survivors before sealing; no statement re-evaluation on replay. Ledger envelope installation and participant/index writes occur under the same TxID before COMMIT. Envelope chunk completeness, CRC and lineage must agree with COMMIT before publishing. Its CRC covers reserved CSN and trusted principal too.

## Commit, recovery and failure boundaries

The sequencer serializes reservation and publication. Reservation gaps are legal; later durable commits wait for earlier reservations to become durable commits or definitively aborted. The visible frontier is a resolved prefix of reservations, not the largest number seen. A flush failure stops all successful write acknowledgments until reopen/recovery; it cannot simply mark earlier unknown reservations aborted. Outcome lookup takes DB/timeline/TxID and returns committed(CSN), aborted, or unknown; missing records never automatically prove abort. A retention policy must state when outcome lookup expires.

Analysis validates segment continuity and checkpoint, identifies valid complete envelope/commit pairs and finished top actions, rebuilds transaction state. Redo restores full-page images when required, then replays physiological patches by address/generation and page LSN, including losers; generation mismatch is skipped only with proof of logged later reuse, otherwise corruption. Undo follows prevTxLSN/CLR undoNextLSN, writes CLRs under WAL-before-data, appends END when complete. A restart redoes CLRs and continues undo without applying it twice. Recovery completes loser undo and confirms ledger/serving prefix equivalence before listeners/workers start.

Tail truncation is permitted only for an incomplete final record in the last segment with no valid later record. A complete record with bad CRC, missing chunk, corrupt interior record or missing required segment is corruption, even if truncation would open the database. No checksum heuristic can establish that acknowledged data was absent. Fuzzy checkpoint uses first-dirty recLSN; flush must obey full-page/top-action barriers. Restore forks a new timeline and invalidates old cursors/tokens; outbound services remain paused.

| Crash point | Required recovered result |
| --- | --- |
| User patch before COMMIT | Redo then undo; no ledger/job/event visibility |
| Split before TOP_END | No structural page flush; ignore incomplete action, undo user work |
| Split after TOP_END, user abort | Retain valid split, undo user insertion only |
| Catalog + row staged, envelope incomplete | Entire attempt loses; schema/row/history invisible |
| Complete cross-feature COMMIT, before successful sync | Whole attempt may survive or disappear; never partial; no success was acknowledged |
| Successful sync, before publication/response | Entire commit recovered; client outcome unknown if response lost |
| Publication/response then process/power failure | Every acknowledged participant recovered; wakeups reconstructed by durable scans |
| CLR persisted, recovery crashes | Redo CLR, resume at undoNextLSN; no double undo |
| Checkpoint END before superblock sync | Older superblock plus retained WAL still sufficient |

[Foundation evidence](foundation-evidence.md) supplies hand-worked fixtures and review traces. They do not prove an implemented recovery engine. Algorithmic and real-platform qualification remain #29, #31–#41, #113/#114.

## Implemented internal COW action metadata

[ADR 0019](../adr/0019-m02-durable-core.md) specifies the internal `VACT0001`/`VALLOC01` metadata carried in v1 WAL overflow images for completed M02 top actions. It defines root/allocator/record-identity replay without changing the framing above or claiming SQL codec compatibility. See [M02 evidence](m02-transaction-evidence.md) for real native recovery, retained-WAL limits and qualified crash levels.
