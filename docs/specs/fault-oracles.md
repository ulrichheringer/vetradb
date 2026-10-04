# Fault model and independent correctness oracles

Accepted foundation contract for [#18](https://github.com/ulrichheringer/vetradb/issues/18), [ADR 0015](../adr/0015-fault-oracle-contract.md).

## Evidence levels

| Level | Proves / limits |
| --- | --- |
| E0 reference example | An independent oracle rejects a known inconsistent state; no engine guarantee |
| E1 deterministic persistence simulation | Engine behavior under explicitly modeled lost/reordered/torn unsynced writes and injected errors; only as complete as the provider/model |
| E2 real process kill | Actual reopen on a real filesystem after process termination; OS cache may survive |
| E3 storage/power loss | Qualified local filesystem, sync implementation and device behavior under actual power/storage fault campaigns |
| E4 operational release drill | Restore/PITR/upgrade of release artifacts on supported platforms with measured RPO/RTO and outbound-service policy |

All evidence records level, exact artifact/commit, toolchain, OS/arch, filesystem/mount/device, seed, workload, fault index/trace, limits and outcome. E2 must never be labeled E3. Network filesystems, malicious privileged host modification and devices lying about successful sync are outside the supported durability model. Linux targets still require actual E3/E4 evidence before production support; macOS build/tests are development evidence only.

## Persistence provider requirements

The full #21 provider separates volatile writes, file-synced bytes and directory-synced namespace. Successful positioned writes may be short; interrupted operations and ENOSPC can occur at any boundary without creating an acknowledged commit. Inject sync failure, sector-sized partial writes, arbitrary loss and reordering of unsynced operations, rename/truncate/create/directory-sync interruption and crashes during recovery. Successful sync protects all earlier file data it covers; directory sync protects names, not merely file bytes. A write later overwriting synced data can tear on crash; this does not contradict earlier sync and must be repaired by full-page/WAL protection.

Fault selection is explicit and reproducible. No random event is sampled without a recorded seed and operation index. A failed flush poisons successful write acknowledgment until reopen/recovery. A crash may preserve unknown-outcome commit bytes; it cannot convert unknown into an assertion of abort. Invalid crash plans fail atomically, not halfway through applying survivors.

The current Rust `SimFile` is a bounded **E0 single-file model**: at most 16 MiB file/4096 pending writes; visible bytes separate from durable bytes; sync protects visible content; `power_loss([(write_index,prefix_bytes),...])` selects survived prefixes in physical order; missing writes vanish. It tests torn/reordered bytes and poisoned sync, but does not implement directory namespace, short I/O, truncate or actual engine recovery. Unsupported truncate returns an error. Prefix tears are examples, not an exhaustive sector fault model. These missing mechanisms are mandatory #21 follow-ups, not waived by current tests.

## External acknowledgement and reference-state oracle

The workload recorder owns transaction identities, submitted effects, acknowledged/aborted/unknown outcomes and separate per-participant observations. Persist acknowledgments outside the database under test before inducing the crash. After recovery, independently query relational, catalog, ledger, job, event and offset projections by identity; feed these to the oracle, never infer truth from the engine's reported transaction status alone.

Acknowledged => exact full expected effects; aborted => no committed effects/history/delivery; unknown => full expected effects or absence. Any subset, extra effect or changed value fails. `check_atomicity` implements this E0 rule with hand-authored effects including row/ledger/job/event/offset. Global reconstruction campaigns additionally fold committed effects in CSN/operation order and compare current/historical projections at every retained basis. Reservation gaps are not committed transactions. #113 supplies real database extraction and checks CSN-prefix/global effects, catalog and indexes.

## Concurrency, leases and snapshot handoff

`serial_witness` explores every candidate serial order for up to 8 committed transactions, evaluating recorded point and half-open range reads against an independent map before applying writes/deletes. Explicit predecessor edges constrain the order when a test requires them; commit order alone does not define serialization order. No witness => anomaly; budget exceeded => inconclusive/error, never success. Write skew and phantom cycles are intentionally injected in tests. Transactions in this small model report reads before their final writes; workloads with interleaved own writes must normalize read-your-writes separately. Larger isolation campaigns and model checks belong to #38/#39/#41.

`check_leases` checks the committed claim/expire/complete trace: one active token per job, strictly newer fencing token on reclaim, completion only under current ownership, completed jobs cannot be reclaimed. Expiry observations come from the external test clock/model; this oracle does not certify lease timestamp/DST arithmetic or worker exactly-once execution. #76/#80/#81/#84 add clock/lease scheduling and real restart campaigns.

`check_handoff` folds external committed row changes through basis B into the expected snapshot, then compares all later `(CSN,position,key,value)` entries to delivered stream entries. Matching duplicates are allowed; gaps, invented payloads, order reversal and incorrect initial snapshot fail. Tests must supply a complete observation window and matching database/timeline/authorization/filter from the external workload; this E0 helper does not test expiration, grants or transport disconnects. #89–#92 add those campaigns and transaction boundary delivery checks.

## Reproducibility and minimization

`cargo run --locked -p vetra-test-support --bin fuzz-foundation -- 42 10000` runs a deterministic bounded persistence-model campaign, capped at 100000 cases. This is a maintained smoke entry point, not coverage-guided codec fuzzing. The [M02 qualification suite](m02-transaction-evidence.md) now applies these independent oracles to native transactions, physical WAL/recovery and real process kills, with labeled E1/E2 limits. Future coverage-guided fuzzers remain #115; no nightly/compiler fuzz dependency is mandatory for current contributors.

Store failures as synthetic traces with seed/case, submitted transaction IDs/effects, acknowledgments, surviving sector ranges, operations and expected/observed results; no credentials or production payloads. Minimize by deleting transactions/operations/faults, then shrinking values while rerunning the independent oracle and preserving the failure. Record original and minimized trace plus replay command. The current campaign reports replay seed/case/prefix and cases are already a two-write minimal shape; a general shrinker and durable trace archive are future provider/campaign work.

`cargo run --release --locked -p vetra-test-support --bin bench-foundation` measures harness overhead only. Database throughput/capacity promises require #99/#116's real workloads, pinned hardware, durability settings and full resource accounting.
