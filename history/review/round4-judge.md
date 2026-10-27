# Round 4 cross-judge verdict (seat B)

**Recommendation: candidate 3 is the base.** It scores 41 of 50. Candidate 1 scores 36, candidate 2 scores 30 (33 before a fact cap), and the baseline scores 30.

I scored each design myself and traced journeys 1, 2, 4 and 5 through each one. I fetched the AWS pages listed under fact checks. I also ran the three cost scripts and `scripts/check_candidate.py`. The checker passes all three candidates. Each cost script reproduces its own table, and all three agree on the baseline-after-fixes figures ($1,433, $2,485, $3,103, $5,062, $16,384, $24,879) [measured].

## Scores (1 to 5, multiplied by the weight)

| Criterion (weight) | Baseline (DSQL) | C1: one head per network | C2: home-Region transactions + journal | C3: one chain per repo or owner + folds |
| --- | --- | --- | --- | --- |
| Downtime (3) | 3 | 4 | 2 (capped; 3 if the SLA fact is fixed) | 5 |
| Scale (2) | 3 | 3 | 4 | 3 |
| Correctness (2) | 4 | 4 | 4 | 4 |
| Build and operate risk (2) | 2 | 3 | 2 | 4 |
| Install fit (0.5) | 3 | 4 | 4 | 4 |
| Cost (0.5) | 3 | 4 | 4 | 4 |
| **Weighted total (max 50)** | **30** | **36** | **30 (33 uncapped)** | **41** |

### Why each score

**Downtime**
- **Baseline (3).**
  - It accepts that a DSQL engine regression rolled out under every cell at once stops git writes, merges and the API everywhere (DESIGN.md:1350).
  - It still has DDL migrations and connection quotas (DESIGN.md:139-152).
  - Identity writes stop with a continent's authority (DESIGN.md:1354). Paired cells do lose no acknowledged write.
- **C1 (4).**
  - Every database incident class is removed or contained, and Paired cells have RPO 0 with no failover step (C1:330-350).
  - One new coupling lowers it. Every cell and front must re-read the revocation log heads of each geography it follows within 5 s. If that fails, writes to all repositories owned in that geography return 503 (C1:216-220, 291).
  - So an impairment over 5 s of one continent's identity table, or a bad revocation-log publisher, stops pushes for a whole continent's repositories [inferred].
- **C2 (2, capped).**
  - It says the journal is "a dependency on a 99.999% service" (C2:467). That is refuted (fact check 22 below), and the claim is its reason for making the journal a hard dependency.
  - Without the cap it would score 3. A Paired cell stops serving reads and writes whenever the home lease cannot renew (C2:140-141). Region trouble then becomes a 45 to 70 s automated promotion (C2:258), which brings the failover incident class back, even though the lease fences it safely.
- **C3 (5).**
  - Every commit is one conditional put on a strongly consistent multi-Region table, so Region loss has RPO 0 with no failover (C3:166, 244, 256).
  - Authorization reads the owner's chain fresh, with reads shared per node, and has no freshness timer (C3:199-201). Identity load and failure stay inside the owner's cell.
  - It states what keeps serving while each store is impaired (C3:252).
  - The remaining worldwide blast radius is a bad deploy of the `global` table code. Tokens also keep resolving from the local replica with eventually consistent reads (C3:151-153, 286).

**Scale**
- **C1 (3).** One network head carries every ref change, every pull request state change, and an event for every issue, comment, review and check-run write in every fork (C1:155, 307). That head is capped at 250 writes per second by the per-partition limit and cannot be split (C1:537). C1 also never states the per-branch merge ceiling, which is about one merge per plan time [inferred].
- **C2 (4).** It spreads load best: 40 head items, per-dimension list collections, and an intent path for large atomic pushes. Its named ceilings are 330 ref writes per second per repository, 18 merges per second on `main` in Paired cells, and 100 to 150 issue writes per second (C2:268-276).
- **C3 (3).** Every repository-scoped write shares one chain partition. Merges run at about 4 per second per branch and comments are capped at 50 per second per repository. The fold engine's ceiling is unmeasured (C3:214-219, 356).

**Correctness**
- **Baseline (4).** One transaction per merge and snapshot reads.
- **C1 (4).** Reviews cannot race a merge because they bump a review counter the merge checks, and it reads its own writes everywhere. Required checks still race the merge, revocation takes up to 5 s, and its two-Region fallback relies on last-writer-wins conflict resolution, which it flags as an open question (C1:540, 570).
- **C2 (4).** It has the strongest merge: one transaction that also checks the check-run summary (C2:210). It states its weakenings: a fetch can see one per-shard-consistent ref set (C2:243), and unacknowledged writes can vanish after promotion (C2:468).
- **C3 (4).** It says its fork merge is weaker than the baseline's (C3:191, 407). In fact, merging the reviewed head is a valid ordering of the merge before a concurrent fork push, so it is not a real anomaly [inferred]. The real weakening is that token revocation takes up to 60 s (C3:209, 408). Membership removal takes effect at once.

**Build and operate risk**
- **C1 (3).** About 11 protocols: the sequencer, the view updater, metadata-write repair, a pump that wakes consumers plus a daily reconcile, revocation logs and leases, projections with verify-on-deny, the name-claim saga, continent rebuild and rejoin, table-set moves, and restore with a generation bump. Its Milestone 0 takes one day.
- **C2 (2).** The journal, home lease, resolver, promotion, standby applier, verifier, rejoin rollback, intent push, directory heartbeat fence, and repair of transactions cut by a restore. They interact and use clock bounds.
- **C3 (4).** Chain append, the freshness read, consumer markers, finish-on-first-touch, and the fold engine. The fold engine is large but local and checked by digest audit (C3:300, 419).

**Install fit and cost.** All three candidates run single-Region installs in all 34 DynamoDB Regions. Each is 7% to 25% cheaper than the baseline after its fixes. EC2 dominates every total [measured].

## Journeys traced

**Journey 1: atomic three-branch push to a busy monorepo.**
- **C1.** The node does a strong read of the network head and places the pack. One conditional write commits the push in a batch alongside queue merges and bot updates. It then updates the views and replies `ok`. Durable at `ok`: the record in two Regions, or one Region plus the witness. The next fetch sees all three branches.
- **C2.** A journal prepare (strongly consistent multi-Region write), then one home-Region transaction of about 20 items, then a commit mark. Durable at `ok`: the home Region plus the journal in two Regions. A fetch that races the commit converges on its next round.
- **C3.** Strong reads of the owner chain and the repository chain, the pack into 2 of 3 buckets, then one put. A clone sees all three branches or none.
- All three meet the journey's load by 10 times or more [inferred].

**Journey 2: fork pull request merged while a review and a check run land; then `gh pr view` and `gh pr list`.**
- **C1.** A review that lands first bumps the review counter, so the merge is rejected and re-plans. A check run that lands after the plan does not block the merge. Both `gh` reads are strong reads and see the merge.
- **C2.** The review bumps the pull request's version and the check bumps the check summary, so whichever commits second gets `Stale`. The list entry moves in the same transaction.
- **C3.** The review is an entry on the same chain as the merge, so the merge either sees it or comes before it. The check is read strongly but not checked at the put. The reads prove the chain fresh, then query the node's SQLite copy.
- All three are correct. C2 is strictest.

**Journey 4: an owner removes a member, whose next push must fail.**
- **C1.** The push is denied once the cell has applied the revocation: at most 5 s, about 1 to 2 s typically.
- **C2.** Takes effect at once: the membership item is read strongly at the org's cell.
- **C3.** Takes effect at once, plus one write round trip for a push already deciding.

**Journey 5: the Region with the newest writes is lost for good mid-push.**
- **C1 and C3.** The surviving replica plus the witness keep quorum, which the AWS docs confirm. RPO 0 in managed mode and in two-Region BYOC. Recovery is by health checks only.
- **C2.** The lease expires after 8 s and the peer waits a further 30 s, then resolves and promotes. RPO 0 for acknowledged writes, RTO about 1 minute.

## Fact checks

| # | Fact | Result | Source |
| --- | --- | --- | --- |
| 1 | Strongly consistent multi-Region writes return after reaching a second Region; strong reads return the latest version; conditions are checked against the latest version | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html |
| 2 | That mode has no transactions, TTL or local indexes; global secondary indexes are eventually consistent | confirmed | same |
| 3 | Exactly 3 Regions (two replicas plus witness allowed), created empty, no replica can be added | confirmed | same |
| 4 | Offered in 15 Regions, any trio, across continents | confirmed | same |
| 5 | The conflict exception is retryable | confirmed | same |
| 6 | Streams are identical on every replica (C3 runs dispatchers in both Regions) | confirmed | same |
| 7 | One replica plus the witness keep quorum; a returning Region errors until it catches up | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/bp-global-table-design.html |
| 8 | A strongly consistent Query on this table type sees every acknowledged item (C3's freshness read, C1's list and log reads, C2's promoter) | unverified, not refuted: the docs promise the latest version "of an item" | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/HowItWorks.ReadConsistency.html |
| 9 | 3,000 read and 1,000 write units per partition; no split by sort key when traffic follows a monotonic sort key; no split of item collections on tables with a local index | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/burst-adaptive-capacity.html |
| 10 | Writes round to 1 KB, strong reads to 4 KB, a Query rounds its total, transactions cost double | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/read-write-operations.html |
| 11 | Transactions: 100 items and 4 MB, same Region; idempotency token valid 10 minutes | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/transaction-apis.html |
| 12 | Query and batch reads are read-committed against transactions | confirmed | same |
| 13 | A restore can contain part of a transaction | confirmed in substance; the page names backup restore and point-in-time export | same |
| 14 | Local index: 10 GB per collection, held in one partition, readable strongly | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/LSI.html |
| 15 | Streams keep 24 hours, at most two readers per shard, ordered per item | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Streams.html |
| 16 | Streams can start reading at a timestamp | confirmed | https://aws.amazon.com/about-aws/whats-new/2026/10/amazon-dynamodb-streams-timestamp-processing/ |
| 17 | On-demand absorbs twice the previous peak at once; tables can set a maximum throughput | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/on-demand-capacity-mode.html |
| 18 | Pre-warmed throughput is available at once, global tables included | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/warm-throughput.html |
| 19 | Point-in-time restore: 35 days, always to a new table, does not copy streams, TTL or deletion protection, a restored replica becomes independent, cross-Region restore exists, deleting a table keeps a 35-day backup | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Point-in-time-recovery.html, https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/pointintimerecovery_restores.html, https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/PointInTimeRecovery_Howitworks.html |
| 20 | Point-in-time recovery on strongly consistent multi-Region replicas (C1 assumes it; C3 flags it) | unverified | Point-in-time-recovery.html; V2globaltables_HowItWorks.html |
| 21 | Exports, including incremental ones, can go to a bucket in another Region | confirmed | https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/S3DataExport.HowItWorks.html |
| 22 | The 99.999% SLA applies to C2's journal | **refuted.** The 99.99% Standard SLA applies "if any of your DynamoDB tables in the applicable AWS region are not part of Global Tables", and C2 keeps single-Region `repo`, `refs` and `eph` tables in the home Region. This caps C2's downtime score at 2. C3's 99.999% claim for Paired cells (C3:405) holds only if the cell account has no single-Region table in that Region [inferred]. | https://aws.amazon.com/dynamodb/sla/ |
| 23 | DSQL: 3,000 rows, 10 MiB, 5 minutes, 10,000 connections, 100 new connections per second | confirmed | https://docs.aws.amazon.com/aurora-dsql/latest/userguide/CHAP_quotas.html |
| 24 | DSQL has no multi-Region clusters across continents | confirmed | https://docs.aws.amazon.com/aurora-dsql/latest/userguide/what-is-aurora-dsql.html |
| 25 | Eventually consistent global tables: a replica in any Region, last writer wins per item, RPO of seconds | confirmed | V2globaltables_HowItWorks.html |
| 26 | DynamoDB in 34 Regions versus DSQL in 20; eu-central-2 has DynamoDB only | confirmed from `research/dynamodb.md` #24 and #26, not recounted | — |
| 27 | AWS Backup cross-Region copy of DynamoDB (C1 and C3, single-Region installs without a recovery replica) | unverified | — |
| 28 | Eventually consistent replicas deliver a writer's heartbeat no earlier than its earlier writes (C2's token fence) | undocumented; C2 itself flags it | — |

## Most serious flaw in each design

- **C3 (the winner).** The node-local SQLite copy is the only list and query engine. Every repository-scoped list, PR page and authorization needs a warm copy of that chain (C3:64, 178-182). After a node loss, an AZ loss or a new SQLite schema release, a giant repository's lists wait on a 2 GB snapshot load (the 30 s target is unmeasured, C3:356) or on a refold storm. C3's own flip rule says that if this fails, the baseline stays (C3:358). Second: token revocation takes up to 60 s.
- **C1.** The per-follower revocation lease turns a 5 s problem in one continent's identity table, or a bad revocation-log deploy, into a write outage for every repository owned in that continent (C1:291, 421). Second: the unsplittable network head also carries every metadata event (C1:155, 537).
- **C2.** RPO 0 in Paired cells rests on a home-grown protocol (prepare, then transaction, then commit mark, with a clock lease, resolve-by-reads, a standby applier and rejoin rollback; C2:122-154). It also halts all reads and writes in the cell whenever the lease cannot renew (C2:140-141, 467). Its SLA justification is false (fact check 22).
- **Baseline.** A DSQL engine regression rolled out under every cell at once stops writes everywhere, with no bulkhead (DESIGN.md:1350). It also runs only in DSQL's 20 Regions and keeps the identity tier's many protocols (DESIGN.md:1354-1361).

## Graft candidates: what each loser does better than C3

- **From C2:**
  - The merge checks the check-run summary version, which closes the required-check race (C2:30, 210). In C3, this means writing required-check conclusions to the repository chain.
  - Per-repository point-in-time restore through scratch tables and copy-back (C2:262). C3 can only restore whole tables.
  - Write requests re-read the token strongly where it was issued (C2:235). This would cut C3's 60 s token bound.
  - A Milestone 0 promotion gate with a linearizability checker (C2:421).
- **From C1:**
  - `RestoreRefs` and repository restore straight from the log, with no table restore (C1:453).
  - The 5 s token bound through revocation logs, if kept for tokens only (C1:213-237).
  - The token encoding that lets a front tell a too-new token from an unknown one (C1:230-232).
  - Small packs up to 256 KB stored inline at RPO 0 (C1:130).
  - Its one-day Milestone 0 on single-item write rate (C1:476-491).
- **From the baseline:** merge decisions read refs, the pull request and check runs in one snapshot, and `revoke` returns a coverage receipt (DESIGN.md:149, 945).
- **Already better in C3 than in C1:** entries are written once and never overwritten, so promoting the eventually consistent recovery replica can truncate each chain at its first gap safely (C3:259). That answers C1's open question about last-writer-wins on its heads (C1:570).

## Recommended base and flip condition

Base: **candidate 3**, with C2's check-summary condition and per-repository restore, and C2's strong token re-check for write requests, grafted in.

Flip condition: if C3's Milestone 0 fold-engine gate fails (on a repository with 200,000 issues and 2 million comments, `labels=bug&sort=updated` p99 over 200 ms, or a cold 2 GB snapshot load over 120 s; C3:356), **candidate 1** becomes the base. C1 keeps lists as DynamoDB index items read strongly and needs no fold engine. In that case, graft C3's owner-chain authorization in place of C1's revocation logs and leases.