# Transactional queues, jobs and scheduling

## Contract and API boundary

Queue/job definitions and state live in protected engine tables/indexes. Applications use SQL functions or embedded methods that participate in their current transaction; these functions never create an autonomous commit. Internal records are not directly writable through ordinary user SQL.

Enqueue plus a business mutation commits atomically. A rollback, savepoint rollback or failed constraint removes both. Claims, acknowledgments and schedule materialization are also transactions. The engine schedules durable work records; application workers execute handlers outside the engine.

## State model

`scheduled -> ready -> leased -> succeeded` is the successful path. A leased failure or lease expiry becomes `retry_wait` or `dead_letter` when the attempt budget is exhausted. `retry_wait -> ready` happens when due. Cancellation becomes `cancelled` under a compare-and-set rule; running handlers may already have external effects and must cooperate with cancellation.

Fields include immutable job ID, queue/type, bounded versioned payload, transaction/correlation IDs, enqueue CSN, priority, available-at UTC, attempt budget, attempts, lease owner, lease expiry, fencing generation, last bounded error, completion time and optional scoped deduplication key. Configuration changes are versioned and their effects on existing jobs are defined.

## Claim and completion

Claim eligible jobs using transaction/key locks and a bounded skip-locked scan; reserve batch size and queue concurrency quota atomically. Commit the lease before invoking a worker. Every claim increments a fencing generation. Heartbeat, fail, complete and cancel compare the job's current generation/owner/state in a short transaction.

A stale worker cannot acknowledge or change database state through a job-completion API. Fencing cannot undo a network call already made by that worker. Exactly-once external effects are not promised. Use application idempotency keys or a downstream transactional protocol.

An application may atomically commit its business result and acknowledge a current fenced lease in the same database transaction. If lease validation fails, its business result also rolls back. A job retry may happen after a durable result if the worker lost the response; downstream and application idempotency remain necessary.

## Retry, priority and deduplication

Retries use configurable exponential backoff, a maximum interval, bounded jitter, a stored next-attempt time and an exact attempt budget. Persist the chosen delay so restart does not redraw it. Dead-letter inspection, bounded error storage and explicit replay/reset operations retain the original lineage in history.

Ready selection is priority first, then availability and stable enqueue order. Fairness/aging or per-queue quotas prevent indefinite low-priority starvation; publish the actual fairness policy and measured bounds. Priority is not a global total order across concurrently executing workers.

Deduplication keys are scoped to database/queue and protected by a unique index. Define retention/window behavior and distinguish insert-if-absent from rescheduling an existing job. Payload and key size limits are enforced before commit; arbitrary SQL results or secrets must not be copied into error logs.

## Delays and cron

One-shot delay uses a stored UTC deadline. Waiting uses a monotonic clock; due comparisons use UTC with observable skew handling. The scheduler supports a documented cron grammar and named IANA timezone with a pinned timezone-data version. Initial grammar is five-field minute cron; seconds and vendor dialects require a later explicit feature.

Store schedule identity/revision, enabled state, next occurrence and misfire policy (`skip`, `coalesce`, or bounded `catch_up`). DST gaps/folds have explicit policies. Atomically insert a unique occurrence `(schedule_id, revision, scheduled_instant)` and advance the schedule; retry/restart cannot create two jobs for the same occurrence. Pause, resume, modify, delete, overlap/concurrency policy and bounded catch-up are transactionally defined.

## Recovery and services

On recovery, ready and delayed work are rediscovered from durable indexes; expired leases become reclaimable under a transaction. A lost wakeup cannot lose a job. Embedded services start only when enabled by the owner. Server shutdown drains dispatch, stops claims and makes in-flight lease behavior explicit.

Time-travel reads show historical job state but never dispatch it. Backup restore and PITR pause workers and schedule dispatch by default to avoid replaying external side effects from a historical state. Metrics include queue depth/age, scheduling lag, leases, retries, dead letters, duplicate suppression and stale completion rejection.
