# Foundation review evidence

Local deliverables for GitHub [#14](https://github.com/ulrichheringer/vetradb/issues/14), [#15](https://github.com/ulrichheringer/vetradb/issues/15), [#16](https://github.com/ulrichheringer/vetradb/issues/16). Status: design and example verification complete for review; **accepted by the repository owner on 2026-10-04**. Foundation completion and issue closure evidence are consolidated in [M00 completion](m00-completion.md); no database runtime correctness or production claim is made. Subsequent workspace/model results are recorded in [bootstrap evidence](bootstrap-evidence.md).

## Reproduce

From the repository root, run `python3 tools/verify_foundation.py` (Python 3.10+, standard library only). The current run passes 15 checks. This checks committed hexadecimal fixtures independently of their construction, selected invalid framing/bounds, CRC32C's published check vector, and the future crate DAG. It is intentionally not a full reference codec, Rust implementation or recovery oracle. Internal/overflow/allocation pages, all WAL payload kinds, fragmented envelopes and actual disk failures require #22/#23/#29/#30/#41 evidence.

| Issue | Artifact / acceptance evidence | Implementation follow-ups |
| --- | --- | --- |
| #14 | [Product/release contract](foundation.md): readiness vocabulary, database scope, exclusions, journey/task trace and evidence gates; README/product/architecture/roadmap agree that features are planned | ADR 0011 accepted; grammar #17, fault budgets #18, governance #20 |
| #15 | [Module policy](modules.md) and [DAG](modules.json): ownership, runtime-free embedded closure, MSRV/edition/targets, dependency/license review, safe API and injectable providers | ADR 0012 accepted; workspace/MSRV and dependency checks now recorded in [bootstrap evidence](bootstrap-evidence.md) (#19) |
| #16 | [Format specification](persistent-format-v1.md): field widths/endianness/versions/bounds, shared commit visibility, user undo vs structural actions, CSN gaps and outcome lookup; fixtures and traces below | ADR 0013 accepted; implementation/fault oracles #21–#41; SQL codecs #23/#44; cross-feature qualification #113 |

## Hand-worked binary examples

Fixtures are static hex bytes under [foundation fixtures](../fixtures/foundation/README.md). Every checksum covers a zeroed checksum field, then the entire container. They are data, not database files.

- Superblock: 8192 bytes, generation 1, DB `00112233445566778899aabbccddeeff`, timeline `102132435465768798a9bacbdcedfe0f`, null roots/checkpoint, 8 KiB pages. Recovery has no checkpoint and starts at the first WAL segment.
- Leaf: PageID 2, generation 1, tree 2, zero slots, `lower=128`, `upper=8192`, high-key sentinel 65535, no sibling/child. This proves an empty header fits and no slot claims reserved bytes.
- Row envelope: TxID 7, CSN 9, principal 1, timestamp 0; metadata is four zero bytes (count/reserved). Operation 0 inserts key `k` in object 2/schema 3. Before-image absent, after-image field ID 1 is UTF-8 `ok`. This is 163 bytes: 96 header + 4 metadata + 32 operation + 1 key + 30 payload.
- WAL chunk: LSN 64, kind 8, TxID 7, previous 0; one 163-byte envelope. Record length 243 = 64 header + 16 chunk framing + 163 envelope. Commit: LSN 307 (=64+243), previous 64, kind 9, record length 88; CSN 9/first-chunk 64/envelope length and CRC agree. The WAL segment header at offset 0 supplies matching lineage. These fragment examples omit BEGIN/page writes; they verify byte accounting and commit/envelope agreement, not a complete transaction log.

## Recovery walkthroughs (design review, not executable recovery tests)

**Structural split and abort.** TxID 20 inserts into a full leaf. Under structural latches, system action 4 allocates generation-1 page 8, moves existing records, updates sibling links/separators and publishes a root. It logs TOP_BEGIN, full-page images and TOP_PATCH records, then TOP_END. Before END no changed page can flush or be used by another transaction; restart discards unfinished action. After END, redo applies all structural patches with the action END LSN as their page LSN, even if TxID 20 aborts. The user insert is its own PAGE_PATCH linked to TxID 20; undo removes it via CLR while leaving page 8 and moved pre-existing rows valid. A restart after CLR repeats its redo and continues at undoNextLSN. #26/#31 must prove lookup/generation integrity at every boundary, including concurrent readers.

**Catalog and row together.** TxID 21 stages schema version S2 and a row referring to S2. Envelope positions are 0 schema-put and 1 row-put, followed by ledger and serving-index installation under TxID 21. A crash with missing envelope chunk or no COMMIT leaves a loser; both catalog/row undo and neither is queryable. A complete COMMIT whose chunks fail integrity is corruption, not a partial recovered schema. Successful WAL flush recovers both and the historical schema reference together. A rolled-back savepoint row never enters the sealed envelope.

**Order / job / event / offset.** TxID 22's envelope positions are 0 order-put, 1 inventory-put, 2 job-put, 3 event-put, 4 offset-put. Physical participant changes and envelope install all precede COMMIT. Before commit the attempt can be undone wholly; after a valid persisted commit the complete set replays. After sync but before response, lookup may return committed and the client treats the response loss as unknown. After successful response, absence of any participant fails the durability oracle. A crash before wakeup does not lose work: ready/event indexes are rescanned. External fulfillment/email execution can duplicate and does not join this atomic set.

**CSN reservation gap.** A reserves 10, B reserves 11; B's durable commit cannot publish while A is unresolved. If A definitively aborts, the frontier resolves the gap and publishes B at 11; no history row exists at 10. If A's flush errors, the path is poisoned; a later operation cannot label A aborted merely to unblock B. Recovery determines complete persisted outcomes before advancing the frontier. An AS OF basis uses the greatest committed CSN <= basis and never treats a gap as an invented transaction.

**Backup and restart.** Checkpoint tables include dirty-page recLSNs, active undo chains and unfinished top actions. A crash before checkpoint END uses the prior checkpoint plus retained WAL; a crash before superblock sync also retains the older valid copy. Archived WAL/backup and history retention horizons are independent. Restore verifies the full required WAL range, undoes losers, forks timeline, invalidates old cursors/fencing identities, and pauses outbound services. No delivered side effect can be undone by restore.

## Review limits

The repository owner accepted ADRs 0011–0013 on 2026-10-04 in the implementation conversation. Design dependencies #14–#16 are accepted; all M00 task evidence is consolidated in [M00 completion](m00-completion.md); actual runtime evidence is still required by the subsystem milestones. Production correctness remains unverified until the actual independent engine and platform oracles run. Each follow-up above names existing issues; no new external issue/comment/message is created by this local implementation.
