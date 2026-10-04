# Primary design references

Consulted for the initial architecture on 2026-10-03. These references describe their own systems; the planned VetraDB contracts and tradeoffs are stated independently in this repository. Links use a pinned PostgreSQL major version to avoid silently changing the compatibility target.

| Source | Relevance |
| --- | --- |
| [PostgreSQL 17 protocol overview](https://www.postgresql.org/docs/17/protocol-overview.html) | Separate startup, simple/extended queries and value codecs |
| [PostgreSQL 17 message flow](https://www.postgresql.org/docs/17/protocol-flow.html) | Portals, Sync recovery, transaction state, COPY and cancellation |
| [PostgreSQL 17 transaction isolation](https://www.postgresql.org/docs/17/transaction-iso.html) | Differential reference for advertised isolation semantics |
| [PostgreSQL 17 write-ahead logging](https://www.postgresql.org/docs/17/wal-intro.html) | WAL-before-data and durability/recovery principles |
| [PostgreSQL full-page writes](https://wiki.postgresql.org/wiki/Full_page_writes) | Torn-page protection and why checksums alone cannot repair pages |
| [PostgreSQL 17 logical decoding concepts](https://www.postgresql.org/docs/17/logicaldecoding-explanation.html) | Committed decoding, replay and retention considerations |
| [Datomic database filters](https://docs.datomic.com/reference/filters.html) | Precise transaction basis, point-in-time and history views |
| [Datomic transaction log](https://docs.datomic.com/reference/log.html) | Transaction-ordered historical access and provenance inspiration |
| [ARIES paper, IBM Research](https://research.ibm.com/publications/aries-a-transaction-recovery-method-supporting-fine-granularity-locking-and-partial-rollbacks-using-write-ahead-logging) | Recovery with WAL, partial rollback and fine-grained locking |
| [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0) | Repository's open-source license text |

The storage design is ARIES-inspired, the history design is Datomic-inspired, and compatibility is tested against PostgreSQL. None of those descriptions implies an implemented subsystem or identical behavior.
