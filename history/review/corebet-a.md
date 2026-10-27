I recommend keeping the base and adding four named bulkheads. Flipping to candidate 2 would keep git reads and writes up through a database-wide failure, but merge and most APIs would still go down. It would also replace DSQL as the git commit point with stores that are just as new: DynamoDB multi-Region strong consistency (MRSC) or S3 conditional writes. The base's real exposure is failures the cell causes itself, and bulkheads can address that.

Citations: `D` = `DESIGN.md`, `C2` = `arena/candidate-2/DESIGN.md`, `U` = `research/uptime.md`, `GS` = `research/git-storage.md`. Labels: [m] measured from text, [i] inferred, [g] guess. In the table, R = git read, W = git write, M = PR merge, A = REST and GraphQL API, and each cell lists the surfaces that stay up.

| Scenario | Base, Paired (managed and BYOC default, D:777) | Base, Regional (self-hosted default) | C2, Region-safe (managed default) | C2, Zonal (BYOC and self-hosted default, C2:722) |
|---|---|---|---|---|
| (a) DSQL impaired in one Region for 2 h | R W M A after 1–3 min (D:711, 732) [m] | None. The only way back is promoting the warm DR cluster: about 60 min RTO, refs rolled back by up to 15 min (D:711) [m] | R W M A (C2:675) [m] | R W only. M down because reviews come from DSQL (C2:50, 268). A down except routes served by gitd. New tokens fail (C2:340) [m] |
| (b) Our bad migration or query plan saturates DSQL | None. The edge returns 503 (D:211, 859). Blast radius is one cell (D:713) [m]. Both Regions run the same schema, so Paired does not help [i] | None [m] | R W only, "git and auth unaffected" (C2:677) [m] | R W only [m] |
| (c) Actions exhausts 10,000 connections or 100 new per second | All four, but only while the client-side per-role budgets hold (D:128). The 100/s budget is split per node, not per role, and DSQL enforces only cluster-wide limits (U#27). A leak in the shared pool code leaves nothing up [i] | Same as Paired [i] | R W. M and A share Actions' cluster (C2:101, 398) [m/i] | R W [m/i] |
| (d) DSQL engine bug; AWS throttles the cluster | None, in every cell on that engine version, because engine upgrades are AWS-managed (D:764) [m/g] | None [m/g] | R W, plus MRSC as an added dependency on the git path (C2:80) [m] | R W [m] |
| (e) S3 impaired in one Region for 2 h | R W M A through the 2-of-3 bucket quorum (D:718) [m] | A, M (merge packs go inline, D:100), W for pushes up to 1 MiB, R from warm caches. Large pushes and cold cache fills fail (D:718) [m] | R W M A, but only after the ARC peer-health flip. `AckInSurvivor` lowers durability to one Region meanwhile (C2:272, 788) [m] | A only. R W M stop (C2:654, 661) [m] |

**Reading the table.** The base is all-or-nothing on DSQL. Candidate 2 keeps R and W through every DSQL-wide failure (b, c, d), with two holes: pushes to branches that require checks fail closed, and tokens newer than the last identity snapshot fail. It never keeps merge through a DSQL failure. Its BYOC and self-hosted default loses R, W and M in scenario (e), which the base survives [i].

**Maturity.** DSQL went GA on 2025-05-27 (U#26), about 17 months of operating history, and features were still landing monthly through 2026-09 (U#28) [m]. The research holds no DSQL incident record either way [m]. Most of S3's maturity advantage covers GET and PUT [g: S3 has run since 2006, which is outside the research]. That benefits pack storage, which both designs use the same way. Candidate 2's commit point is just as new:
- Zonal cells commit with S3 If-Match compare-and-swap, GA 2024-11-25 (U#31, GS#55) [m].
- Region-safe cells commit with DynamoDB MRSC, GA 2025-06-30 (U#29), which is younger than DSQL [m].

So AWS maturity risk at the commit point is roughly a wash [i]. The real difference is change and load the cell causes itself. DSQL carries Tributary's migrations, queries and Actions traffic. In GitHub's record the database is the largest source of core-surface downtime: 14 of 48 core incidents, about 2,500 incident-minutes, and a version bug that broke git reads and writes twice (U#43–44) [m]. Candidate 2's register is a fixed two-call API with no schema to migrate [m].

**Bulkheads to add to the base:**
1. **Two DSQL clusters per cell, `core` and `aux`.**
   - `core`: `tenant_state`, refs, packs, `push`, `ref_log`, credentials, permissions, PRs, reviews, branch rules, `check_run`, outbox.
   - `aux`: Actions runs, jobs, runners, leases, the cache index, artifacts, `hook_delivery`, notifications, audit.
   - `completejob` reaches `check_run` through an outbox consumer.
   - Cost: a second cluster, and in Paired cells a second multi-Region setup. Check results lag by seconds instead of committing with the job (today D:112 does it in one transaction). Two migration streams, and self-hosters run two clusters.
2. **Frozen `core` schema.** The CI linter rejects DDL on `core` tables except in a release that carries only that change and soaks 7 days in the canary cell [g]. No runtime index builds on ref tables. Cost: git storage evolves more slowly, and new features go into new tables.
3. **Per-role connection isolation.** Each role runs in its own OS process with its own DSQL database role, and git and API get a reserved 50 of the 100 new connections per second. Cost: more processes and less efficient pooling. Per-role connection limits in DSQL are unverified.
4. **Last-known-good git reads.**
   - When `core` has been unreachable for more than 10 s, the `git` role serves upload-pack from its NVMe cache at its last proven refresh. The edge authenticates from a credential and permission snapshot pushed to local disk, as candidate 2's identity snapshot does (C2:278). Writes return 503.
   - Staleness stops growing at the moment of the outage, because nothing can commit while `core` is down [i].
   - Cost: this reverses "no stale reads" (D:859), freezes revocations for the length of the outage, and adds a second auth path. That path is a fallback in AWS's sense (U#5), so it needs a weekly drill that blackholes `core` on the canary cell, with a failed drill blocking deploys (as D:752 does for Region flips).

With all four, the base keeps:
- (b): R always, and all four if the bad migration hit `aux`.
- (c): all four.
- (d): R only.
- (a) and (e): no change.

**Change to `DESIGN.md`.** Today Milestone 0 tests DSQL under load only (D:820), and the flip condition at D:11 follows that test. Add a failure day with three gates:
- Blackhole `core` for 30 min while clones run: fetch success must stay at or above 99%.
- Build an index on a 100M-row `aux` table at peak push load: push p99 must stay within 1.5× baseline.
- Open 150 new connections per second from the `aux` role: zero git errors.

If any of these gates fails, flip the base to candidate 2.

**Strongest counterargument.** Even with the bulkheads, a DSQL engine bug (d) or a bad migration on `core` (b) stops git writes, merges and APIs in every affected cell. Git writes rank first for the operator, and candidate 2 keeps them up in exactly those cases. Bulkhead 4 is also a mode that only runs during outages, which is the failure pattern GitHub hit with its March 2026 Redis failover (U#47). My answer: candidate 2 keeps writes only to branches without required checks, never keeps merges, and stakes its commit point on MRSC or S3 compare-and-swap, which are as new as DSQL. Its BYOC and self-hosted default also loses all git in (e), which the base survives.