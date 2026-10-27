# Arena cross-judge verdict (seat B): C is the base, at 26 points to B's 25 and A's 19

The candidates are `arena/candidate-1/DESIGN.md` (A), `candidate-2` (B) and `candidate-3` (C), cited below as A:line and so on. Each claim carries a label: [m] means I read it in the cited text, [i] means I inferred it, and [g] means it is a guess.

## Scores

| Criterion | A: Spokes quorum plus S3 fence | B: S3 write-ahead log | C: refs as DSQL rows |
| --- | --- | --- | --- |
| 1 Availability | 4 | 4 | 3 |
| 2 Durability and consistency | 3 | 4 | 5 |
| 3 Wire compatibility realism | 4 | 5 | 4 |
| 4 One artifact and operability | 3 | 4 | 5 |
| 5 Interface depth and simplicity | 2 | 3 | 5 |
| 6 Feasibility and sequencing | 3 | 5 | 4 |
| **Total** | **19** | **25** | **26** |

**Evidence for each score (one sentence each):**
- **A1 = 4.** Reads never touch S3, the quorum keeps working at 2 of 3 after an AZ loss, and Region drills run monthly and quarterly (A:246, A:610-613, A:660). But the self-hosted default is "Regional without DR learner" (A:683), whose Region RTO is "hours ... launches capacity" (A:252). [m]
- **A2 = 3.** The acknowledgment point is exact: two voters in two AZs, plus S3, plus the create-only fence (A:238). Pull request divergence is answered (A:367). But the multi-ref "burned seqs" protocol (A:242-244) has an atomicity hole, described under A's flaws. [m] text, [i] race
- **A3 = 4.** It has seeded client ids, GHES headers, the `ghs_` JWT and MessagePack node ids (A:304-313, A:575-591). But it serves rewritten bytes for SHA-pinned first-party actions by default (A:708), and it turns subdomain isolation on in every mode (A:579), where `research/clients.md` implication 1 says isolation should be optional. [m]
- **A4 = 3.** Every mode runs three stateful NVMe voters plus a new `roll:storage-fleet` stage (A:666, A:769). The design never says what a self-hoster repairs at 2am. [m]
- **A5 = 2.** It runs NVMe replicas, S3, DSQL, a placement schema with a snapshot, and OpenSearch. It has two `FenceStore` implementations (A:745), two outbox origins (A:431-434), and nine deployables including a Node SSR tier (A:472-483). [m]
- **A6 = 3.** Milestone 1 (A:728) correctly attacks the riskiest piece first. But A puts its own cost in the "high" row of the options table (A:761), while the research rates the shape A builds as "Very high, needs the Spokes base first". [m]
- **B1 = 4.** DSQL is off the git path, the Region-safe default is active-active with an RTO of about 2 minutes (B:644), and drills run monthly (B:702). But in Zonal cells, the BYOC and self-hosted default, every `info/refs` reads the S3 register (B:40), so "Zonal cell: writes and reads stop" in a Region event (B:661). [m]
- **B2 = 4.** The acknowledgment is the `compare_and_swap` return, and "merged" is a log entry that moves the ref atomically (B:52, B:390). But the default `AckInSurvivor` (B:788) quietly drops the "RPO 0" the cell class promises, and the projection has more than one writer (see B's flaws). [m]
- **B3 = 5.** It leaves SHA pins at their exact upstream bytes (B:745), marks the Desktop callback and the non-measured node-id prefixes as unverified, and enforces GraphQL connection limits. [m]
- **B4 = 4.** "At 2am the self-hoster repairs nothing on git nodes" (B:29), and `roll:git-fleet` is a drain-and-replace. But the single-node profile needs an EC2 launch to upgrade (B:712). [m]
- **B5 = 3.** One `RefRegister` trait with two implementations is clean (B:207). Ownership is split, though: open and closed state live in DSQL while merged and head live in the log, and org rulesets live in DSQL and are fanned out into per-repository `Policy` log copies (B:268). [m]
- **B6 = 5.** Milestone 1 builds the write-ahead log, both registers and a 24-hour fault harness, and it measures CAS rates and MRSC latency, which are the named unknowns (B:763). The core sits in the "Medium" research cost row and has walgit as prior art. [m]
- **C1 = 3.** The Paired default gives RPO 0 with no failover call (C:616). But every git request reads DSQL and C "serves no stale reads" (C:742-743). No recurring Region drill exists, only the Milestone 4 gate and weekly synthetic-push replays (C:606). Regional DR is a "restore ... from backups" (C:616), even though the next row says nothing in recovery calls a DSQL management API (C:617). [m]
- **C2 = 5.** The acknowledgment is the DSQL commit, enforced by the `Acked` and `DurablePack` types (C:260). Refs are linearizable per ref and atomic per push (C:261). Pull request state "cannot diverge ... by construction" because ref, PR and outbox share one transaction (C:375). [m]
- **C3 = 4.** The wire table is thorough, and the pin map requires admin approval before a SHA pin is substituted (C:697). But client-visible caps the wire table never mentions are imposed: 1,000 refs per atomic push and 100 open PRs per head branch (C:110). [m]
- **C4 = 5.** It ships one Rust binary with roles (C:470), the self-hoster has "nothing to repair by hand" (C:46), upgrades hot-swap through `SO_REUSEPORT` (C:659), and no stateful-fleet stage is needed (C:667). [m]
- **C5 = 5.** It has two stateful systems, DSQL and S3, plus a derived search index. `Change<T>` makes "forgot to emit" a compile error (C:442-447). Each fact has one owner, and there is no Node SSR tier (C:713). [m]
- **C6 = 4.** Milestone 1 has numeric gates (p99 under 1 s Regional and under 1.5 s Paired, at least 50 pushes/s to one repository) and attacks DSQL hot rows first (C:721). But the cost of its Rust receive-pack is never stated. [m]

## Strengths and flaws for each candidate

**A**
- Strength: reads come from disk digests and never touch S3, so an S3 impairment stops writes but not clones (A:246, A:619). [m]
- Strength: GitHub's 2026 per-reference ordering is taken literally, so different refs commit in parallel with no per-repository hot key (A:240). [m]
- Flaw: the multi-ref fence can break atomicity, and A labels it "least proven" (A:766). Suppose a coordinator stalls past the 30 s voter deadline (A:244). A later writer then sees a burned key, finds the other keys in that transaction still absent, and calls the transaction uncommitted (A:242). The stalled coordinator can still win its remaining claims and so meet the "won every ref" commit rule after the later writer has moved past it. No step lets the later writer pre-claim the missing keys to fence it off. [i]
- Flaw: it has the most moving parts of the three: quorum, placement, six sweepers, two fence stores and two event origins (A:257, A:431, A:745). [m]

**B**
- Strength: the merge commits as one log entry carrying `MergeAuthorization`, and policy sits in the same chain, so `PolicyMoved` orders protection changes against pushes (B:52, B:262). [m]
- Strength: nodes are disposable caches, which makes the 2am story the best of the three (B:29). The SHA-pin policy ("a SHA pin is a promise about bytes", B:745) is also the cleanest. [m]
- Flaw: B contradicts itself on who writes the projection. "The projector is the only writer of `RefProjection`" (B:390), yet `CaughtUp` reads "appl[y] the missing records inline" (B:49) and the merge module "applies that record to the projection" (B:53). [m]
- Flaw: the S3 register sits on every Zonal fetch (B:40). One CAS chain per repository carries pushes, merges, bot refs, policy and checkpoints, with a ceiling near 120 pushes/s (B:785, B:815). The MRSC strong-read latency on every Region-safe fetch is unverified (B:813). [m]

**C**
- Strength: refs, PR state, packs and the outbox commit in one DSQL transaction. That removes divergence, repair jobs and a third coordination point (C:78, C:375). [m]
- Strength: packs up to 1 MiB go inline into the commit (C:77, C:264). Small pushes are therefore cross-Region RPO 0 with one commit and keep working through an S3 impairment. [m]
- Flaw: git availability equals DSQL availability, so the main threat in GitHub's incident record (`research/uptime.md#43`, `#44`) sits on the git read path. All 40 `ref_shard` rows of a repository are hot for every push and every fetch (C:112, C:261). [m] text, [i] risk
- Flaw: the 100-PR and 1,000-ref caps leak into product behavior (C:110). The Regional DR path contradicts the control-plane row (C:616-617). The SLO text says "the four 99.99% targets", but the table has two (C:593). [m]

**Citations that misstate `research/`**
- A:761 calls its design the "high" cost row. A:238 says its design is "GitHub's announced design ... with blob storage as a voting member", and `git-storage.md`'s options table rates that row "Very high, needs the Spokes base first". [m]
- A:706 says Gitea "replaces the body of the bundled `isGhes()` with `return false`". `actions-protocol.md#57` says Gitea rewrites the literal `endsWith('.LOCALHOST')` to `endsWith('')` and also makes v2 cache prefer `ACTIONS_CACHE_URL`. B:745 likewise says the patch "changes only the hostname test" in Gitea's manner, which leaves out the cache-URL change. Both are minor. [m]
- C attributes the regex `ghs_[A-Za-z0-9\.\-_]{36,}` to Renovate (`clients.md#38`). The regex is GitHub's recommended pattern (`wire-census.md#30`); Renovate checks only the `ghs_` prefix. This is minor. [m]
- I checked every other citation a score depends on and found it accurate: the DSQL limits (#27), multi-Region DSQL (#26), MRSC (#29), conditional writes and bucket-policy keys (#31, #55), the Continuity figures (#29, #30), reftable locking (#18), and the OAuth ids (#46). [m]

## Base recommendation: C

The rubric's rule is the candidate a future maintainer can extend most easily without breaking invariants. In C the invariants are carried by one transaction boundary and by types (`Acked`, `DurablePack`, `Change<T>`, `Decision::Hidden`), so a new feature adds rows inside `run_tx` and cannot skip the outbox or acknowledge early. B spreads its invariants across a log, a DSQL projection with several writers, and a policy fan-out. A spreads them across a quorum, a fence, sweepers and two event origins. C also has the smallest public interface: one binary and one SQL crate (`trib-store`). [i]

## Grafts into C

1. **From A, a warm Regional DR cluster.** Replace C's "restore from backups" (C:616) with A's pre-created DSQL cluster in the DR Region, fed by CDC (A:252). Regional recovery then makes no control-plane call. [m]
2. **From A, a Region drill cadence.** Add A:660 to C's Availability section: a monthly dogfood-cell flip, a quarterly canary-cell flip, both through ARC routing controls only, and a failed practice run blocks deploys. [m]
3. **From A, a durability SLO row and daily replay.** Add A's "acknowledged push durability 100%" row, with a daily replay of every `push` row, not only synthetic pushes, checking that its pack resolves in S3 or `pack_inline` (A's SLO table; C:606). [m]
4. **From A, sweeper velocity caps.** Apply A's caps (5% of networks per hour, one hydrating node per AZ, A:257) to C's compaction, GC and cold-cache refill after an AZ loss. [m]
5. **From B, bucket-policy enforcement.** Require `s3:if-none-match` on pack keys in the bucket policy, so even a buggy release cannot overwrite (B:262, `uptime.md#31`). [m]
6. **From B, bot-ref coalescing.** Coalesce mergeability writes to `refs/pull/N/merge` last-writer-wins, behind user pushes (B:205-246 `Committer`, B:392). This keeps OCC conflicts off C's 8 pull shards. [i]
7. **From B, per-repository push admission.** Add B's admission of about 100 commits/s per repository, plus a per-tenant shuffle shard (B's Failure modes table, under ref-register noisy-tenant), to protect DSQL hot rows. [m]
8. **From B, the two-release format rule.** Apply "N reads format k+1, N+1 writes it" (B's Change safety) to `pack_inline` and stored webhook payloads. [m]
9. **From B, a plain SHA-pin annotation.** When an unapproved SHA pin fails, the annotation names the fork SHA to pin instead (B:745). Keep C's approved pin map. [m]

## Drop or fix in C

- **Drop the 100-open-PRs-per-branch cap (C:110, C:375).** Resolve an open PR's head from the head-branch ref row in the same snapshot, and synthesize `refs/pull/N/head` in the advertisement. The push transaction then needs no fan-out. [i]
- **Name the 1,000-ref atomic-push limit as a wire deviation.** Give it its own row and oracle in the Wire compatibility table instead of leaving it implicit. [m]
- **Fix the SLO text "four 99.99% targets" (C:593).** The table has two. [m]
- **Fix the control-plane contradiction between C:616 and C:617.** Graft 1 resolves it. [m]

## Flip condition

**Unknown:** can a DSQL cell sustain C's commit shape on one hot repository? The shape is a per-ref CAS, plus shard bumps, plus inserts into `push`, `ref_log` and `outbox`, while fetches read all 40 shard rows. If the 40001 retry rate climbs steeply or p99 misses C's own gate (at least 50 pushes/s, under 1 s Regional, under 1.5 s Paired), B becomes the base. B keeps git ordering off the metadata store, and its per-repository ceiling near 120/s is documented (`git-storage.md#30`). [i]

**Cheapest experiment:** a one-day SQL-only benchmark with no git and no Rust receive-pack.
1. Create one single-Region and one multi-Region DSQL cluster, the latter on the intended Region trio.
2. A small driver issues exactly the `commit_push` transaction from C (C:236-249) against one repository, at 10, 25, 50 and 100 commits/s.
3. Half the writers contend on `main` and half write to distinct branches spread across shards, while 200 concurrent readers run the 40-row `refresh` query.
4. Record sustained commits/s, the OC000 rate, p50 and p99 commit latency, and read p99.

This is cheaper than either B's or C's Milestone 1, and it settles the choice before any git integration exists. [g] on cost; [i] on decisiveness