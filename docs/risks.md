# Risks and research decisions

| Risk | Implication | Mitigation / required decision |
| --- | --- | --- |
| Combining relational SQL, history and backend services expands scope | A demo can look complete before correctness exists | Sequence milestone gates; experimental labels; keep HA after 1.0 |
| ARIES, B+Tree top actions and immutable history interact | Undo can accidentally damage another transaction or leave inconsistent history | Specify physiological/logical records and structural recovery before concurrent writes; fault oracle |
| Complete history grows forever | Storage and historical indexes can dominate write cost | Lossless compaction, measured amplification, explicit retention/pin budgets; no silent history vacuum |
| Row identity differs from a mutable primary key | Updates/DDL may misattribute old history | Stable internal row/object/schema IDs, key-change tests |
| SQL compatibility is deeper than parsing or pgwire | Existing clients/ORMs fail on catalogs, types or session behavior | Version-pinned differential and real-client matrix; accurate unsupported errors |
| Serializable locking may limit concurrency | Safe first implementation can create contention | Conservative reviewed algorithm, hot-key benchmarks; optimize after proof |
| Timestamp ambiguity and clock rollback | AS OF timestamps can be misunderstood | Monotonic CSN is precise; assigned timestamp tie/skew policy with metrics |
| History/CDC contains previously deleted sensitive values | Current SELECT grants can expose more than intended | Separate grants, current authorization, row-policy parity, redaction workflow |
| Queue/event retry duplicates external effects | Database atomicity cannot make arbitrary network actions exactly once | Fenced database state, stable event/job IDs, application idempotency guidance |
| Long snapshots and slow consumers pin disk | A forgotten client can cause ENOSPC | Quotas, expiry, pin inspection, explicit gap/resnapshot errors |
| Unsafe filesystem/IO assumptions | Process-kill tests may overstate power-loss durability | Injectable persistence model plus supported-platform tests |
| Future replication may invalidate local ordering assumptions | Log/ID/format choices become expensive to migrate | Stable lineage/IDs and a durability seam now; freeze replication details in M11 |
| Backup restore reactivates old scheduled work | Repeated external actions after PITR | Restore to a new timeline with dispatch paused and explicit operator resume |
| Encryption design can introduce unrecoverable key/nonce bugs | Strong feature names do not guarantee safe cryptography | Reviewed ADR, audited library, restore/rotation tests or explicit encrypted-volume requirement |

Open design items have owners through issues rather than implicit promises: exact page/record framing (M00/M01), serializable algorithm (M02), temporal grammar (M05), native streaming envelope/transport (M07), encryption-at-rest boundary (M09), benchmark/SLO budgets (M08/M10), and replicated-log/consensus selection (M11). Adopted principles remain normative; proposed sizes/API names become stable only after their issue's acceptance review.
