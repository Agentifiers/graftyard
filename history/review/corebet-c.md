I'd keep the base and add four bulkheads plus a failure gate. When DSQL breaks as a whole (cases b, c and d), the base loses all four surfaces. Candidate 2 (C2) keeps git reads and writes, but neither design keeps PR merge. I read the files only and ran nothing. Tags: [m] measured from the cited text, [i] inferred, [g] guess. R = git read, W = git write, M = PR merge, A = REST/GraphQL, ½ = partial.

| Scenario | Base Paired / Regional | C2 Region-safe / Zonal |
|---|---|---|
| (a) DSQL impaired in one Region for 2 h | Paired: R W M A, after a 1–3 min Route 53 shift (DESIGN:711) [m]. Regional: nothing until `dr-apply` promotion, which loses up to 5 min of writes and launches capacity (711, 868) [m] | Region-safe: R W M A (C2:675) [m]. Zonal: R W, except pushes to branches that require checks (C2:268); M down (C2:50); A½, with auth from the snapshot (C2:278, 789) [m] |
| (b) Our own bad migration or query plan | Nothing in that cell; the canary cell and 2 s timeouts only limit the spread (713) [m] | R W up; M down; A½, git-backed endpoints only (C2:677, 757) [m] |
| (c) Actions uses up 10k connections or 100/s | R W M A if the per-role budgets hold (128) [m]. Nothing if they fail: one cluster, and BYOC/self-hosted run all roles in one process (785) [i] | R W up; M down; A½, even if the budgets fail (C2:398, 450, 678) [m] |
| (d) Engine bug, AWS throttles the cluster | Nothing. Multi-Region is one logical cluster, so it does not help [i]. AWS-managed engine upgrades (764, unverified) do not follow our waves, so 205's "stops at the cell" may not hold [i] | R W up; M down; A½ [m]. Matches GitHub's May 2023 database-version bug that broke git twice (uptime #44) [m] |
| (e) S3 impaired in one Region for 2 h | Paired: R W M A, through the 2-of-3 bucket quorum (718) [m]. Regional: R½ (warm caches), W½ (pushes ≤1 MiB go inline), M A up (718, 100) [m] | Region-safe: R W M A, but writes stall until the ARC flag flips (C2:272, 654) [m]. Zonal: no git, because the ref register is an S3 object (C2:654, 661); M down; A½ [m] |

- **One-Region failures (a, e):** in the managed defaults the two designs tie.
- **Whole-cluster failures (b, c, d):** C2 always wins, keeping R and W.
- **Single-Region modes:** each design wins one case. C2's S3-register cells (BYOC/self-hosted default) beat base Regional when DSQL fails (a); base Regional beats them when S3 fails (e).

**DSQL maturity vs S3.**
- **DSQL:** GA on 2025-05-27, so about 17 months old (uptime #26) [m]. AWS shipped engine features almost monthly through 2026: sequences (Feb), CDC GA (Jul), expression indexes and foreign keys (Aug), partial indexes (Sep) (#28) [m]. The research found no DSQL incident record either way (#36). That is missing evidence, not good evidence [i].
- **S3:** about 20 years in service (outside the research). Its failures in the research are physical and confined to one Region (#37) [m].
- **The SLA doesn't settle it:** DSQL's 99.999% figure (#26) is a credit promise. Cases b and d hit both Regions of one logical cluster at once, so multi-Region doesn't protect against them [i].
- **Caveat for C2:** its managed default puts the git register on DynamoDB MRSC, GA 2025-06-30, one month younger than DSQL (#29) [m]. But that register is one conditional write to one item, with no query planner or schema changes, so failure classes b and d mostly don't apply [i].
- **Judgment:** DSQL's maturity risk is clearly higher than S3's, and higher than MRSC's, for cases b and d [i].

**Recommendation: keep the base and add these named bulkheads.**
1. **Separate DSQL cluster per cell for Actions.**
   - Moves: `workflow_run`, `job`, `runner`, `runner_session`, `cache_entry`, log and artifact rows.
   - Stays in the core cluster: `check_suite`, `check_run`, the outbox.
   - `completejob` (DESIGN:112) becomes a commit in the Actions cluster plus an idempotent `check_run` write to the core cluster through an Actions-side outbox.
   - Cost: a second cluster, a second migration track and connection budget, seconds of lag on the check results that gate merges, and `completejob` loses its single transaction. It removes case c, and the Actions-caused share of b, from the core surfaces, and answers open question 888 [i].
2. **Fixed connection reservations per role.**
   - The flexible budgets at DESIGN:128 become fixed per-role shares of the 10,000 connections and 100 new connections/s, with one DSQL database role per Tributary role.
   - Edge and git get the largest share, `worker` sheds first, and CI tests the split.
   - Whether DSQL can enforce per-role limits server-side is unverified, so the pool code enforces them. Cost: a lower peak for any single role.
3. **Frozen schema for the git and auth tables.**
   - Tables: `ref`, `ref_shard`, `ref_tombstone`, `pack`, `pack_inline`, `push`, `ref_log`, `credential`, `tenant_state`, `permission`, `team_member`, `branch_rule`.
   - They change only in a dedicated release class with its own canary soak. The `git` role uses a closed set of prepared statements whose query plans are snapshotted and diffed in CI and on the canary cell, and backfills never touch these tables.
   - Cost: slower change to those tables. It answers C2:804, "schema migrations on the ref table would also become git outages".
4. **Bulkhead for git reads.**
   - Cache nodes keep their last-known refs plus a pushed credential and permission snapshot (C2's pattern, C2:278, 340).
   - When the DSQL freshness read misses its deadline, they serve fetches at the last shard versions they saw. Writes and merges stay fail-closed, so durability is unchanged.
   - Cost: this replaces "serves no stale reads" (DESIGN:859) for reads. Read-your-writes can break, revocations lag by up to the snapshot age, and permissions get a second copy to keep correct.
   - This path must run as a weekly drill in every cell that blocks deploys. Otherwise it becomes the untested fallback that uptime #5 and #8 warn against.
5. **A failure gate in Milestone 0.**
   - Today Milestone 0 tests load only (DESIGN:820).
   - Add three tests: cut off DSQL for 10 min, throttle it to 10%, and apply a deliberately bad index on the canary cell.
   - Pass condition: R keeps serving through bulkhead 4, the core SLOs stay green while Actions saturates its cluster, and pools reconnect within 40 s (4,000 connections at 100/s).
   - Also add an open question for AWS: can DSQL engine versions be staged per cluster?

**Strongest counterargument.**
- C2 keeps git writes through every DSQL failure by design, with no stale-read mode to drill. Bulkhead 4 is a fallback by the research's own definition (uptime #5), and the database is GitHub's largest cause of core outage minutes (#43, about 2,500 minutes) [m].
- Why it doesn't carry: in those same failures C2 also loses merges and most of the API (C2:50, 394). So it keeps only git writes beyond what the bulkheaded base keeps [i].
- C2 also has its own costs:
  - It loses all git when S3 fails in a Zonal cell (C2:654).
  - Its default `AckInSurvivor` acknowledges pushes held in only one Region while the peer Region is impaired (C2:788).
  - Its split ownership scored lower (arena-judge.md:28).
- **When to flip anyway:** switch to C2 if the failure gate shows bulkhead 4 can't serve reads with DSQL down, or if AWS can't stage DSQL engine upgrades per cluster.