I recommend keeping the base and adding five bulkheads. Candidate 2 keeps git reads and writes through a DSQL outage, but in every DSQL-wide failure it still loses PR merge and most of the API. In its BYOC and self-hosted default it also loses git, merge and the API's git-backed reads together in a regional S3 outage.

The base wins in three of five scenarios for the managed default and loses the other two. Candidate 2 wins in three for the self-hosted default (b, c, d) and loses (a) and (e) to it. No design keeps all four surfaces in every scenario.

**Key.** R = git read, W = git write, M = PR merge, A = REST/GraphQL API. D = DESIGN.md, C2 = candidate 2, U = research/uptime.md. Labels: [m] = measured from the cited text, [i] = inferred, [g] = guess. Defaults: base is Paired for managed and BYOC and Regional for self-hosted (D:777). Candidate 2 is Region-safe for managed and Zonal for BYOC and self-hosted (C2:722).

| Scenario | Base Paired | Base Regional | C2 Region-safe | C2 Zonal |
|---|---|---|---|---|
| (a) DSQL impaired in one Region, 2 h | All four up after a 1–3 min Route 53 shift (D:711, D:732) [m]. A gray failure that passes the canary write may not trip it [i]. | None up. Or all four after a ~60 min promotion that gives up up to 5 min of metadata and up to 15 min of pushes (D:711, D:868) [m]. | All four up. Git never reads DSQL (C2:82, C2:675) [m]. Routing for merge and API is [i]. | R and W up. M down, because merge reads reviews and checks from DSQL (C2:50). A down except git-backed views (C2:757) [m]. |
| (b) Our own migration or query plan saturates DSQL | None up in that cell. Every surface reads DSQL (D:85, 86, 88, 90, 98). Paired gives no protection because it is one cluster (D:116, D:147). Limited to the canary cell by waves and 2 s timeouts (D:713) [m]. | Same as Paired. | R and W up. M and A down (C2:50, C2:677) [m]. | Same as Region-safe. |
| (c) Actions exhausts 10,000 connections or 100 new/s | The connection count is designed out: per-role budgets total 4,000 (D:128) [m]. The 100/s rate is enforced only by our own clients, so a budget bug leaves nothing up [i]. | Same as Paired. | R and W up. M and A down, because Actions shares the cluster (C2:398, C2:678) [m]. | Same as Region-safe. |
| (d) DSQL engine bug, AWS throttles the cluster | None up. Paired gives no protection. Several cells probably fail at once, because AWS manages engine upgrades (D:764, unverified) [i]. | Same as Paired. | R and W up, because the ref register is on DynamoDB MRSC (C2:80). M and A down [m]. | R and W up, because the register is on S3. M and A down [m]. |
| (e) S3 impaired in one Region, 2 h | All four up through the 2-of-3 bucket quorum (D:718, D:732) [m]. | W up for packs up to 1 MiB. R up from warm caches. M up, because merge packs go inline (D:100). A up except S3-backed endpoints (D:718) [m]. | All four up after an ARC flip. The default `AckInSurvivor` drops RPO 0 (C2:272) [m]. | R, W and M down, because the register is an S3 object (C2:40, C2:654, C2:661). A partly up [m]. |

**Maturity.** DSQL went GA on 2025-05-27, about 17 months ago (U#26) [m]. The engine changed at least five times in 2026: sequences, CDC, expression indexes, foreign keys and partial indexes, the last on 2026-09-15 (U#28) [m]. The research has no DSQL incident history at all, and the 99.99% and 99.999% figures are SLA credit terms, not measured uptime [m].
- S3 has about 20 years in production [g: not in the research].
- The S3 conditional writes that candidate 2's commit point relies on are newer: 2024-08 and 2024-11 (U#31) [m].
- Candidate 2's managed default puts git on DynamoDB MRSC, GA 2025-06-30 (U#29) [m]. That is as young as DSQL.

So the maturity risk is real, but only Zonal candidate 2 actually moves git onto a mature store. Its price is losing git, merge and the API's git-backed reads in a regional S3 event, as row (e) shows.

The research also argues for bulkheads. GitHub's database incidents caused the most core outage minutes (U#43) and were mostly self-inflicted: migrations, expensive queries, key overflow (U#44) [m]. U#15 says auth, refs and PR metadata should each get their own store, and the base currently puts all three in one cluster [m].

**Recommendation: keep the base and add these bulkheads.**
1. **A separate DSQL cluster per cell for Actions and background writes.** It holds `workflow_run`, `job`, `runner`, `runner_session`, `cache_entry`, the log index and `hook_delivery`. `completejob` sends the `check_run` conclusion to the core cluster through an idempotent relay keyed by job attempt.
   - **Covers:** (c), plus any (b) or (d) caused by Actions traffic.
   - **Cost:** a second cluster bill and second quotas per cell (doubled in Paired cells), and a second migration stream. Merge sees check conclusions a few seconds later. The "one database holds every canonical fact" claim (D:857, D:864) narrows to the core cluster.
2. **A frozen schema and query set for the git path.**
   - **Tables:** `tenant_state`, `credential`, `permission`, `team_member`, `branch_rule`, `ref`, `ref_shard`, `ref_tombstone`, `pack`, `pack_inline`, `push`, `ref_log`, `outbox`, and the merge columns of `pull_request`.
   - **Rule:** these tables take DDL only in a quarterly release after two weeks on the canary cell, and the migration runner refuses any other DDL on them. Git, edge and merge run only a fixed set of prepared statements, and CI checks their query plans against a production-sized fixture.
   - **Covers:** most of (b).
   - **Cost:** slower feature work on those tables. New fields go into side tables.
3. **Reserved connection budgets per role.** Of the cell's 4,000 connections, git, edge and api reserve 3,000, plus 60 of the 100 new connections per second. Actions, workers, migrations and `dr-apply` share the rest and are shed first. CI checks that the sums stay under DSQL's quotas, as candidate 2 does (C2:678).
   - **Cost:** Actions and workers recover more slowly after a reconnect storm.
4. **An identity snapshot pushed to S3, plus bounded stale git reads.** The edge authenticates git requests from the snapshot (C2:278, C2:340). If the `ref_shard` read fails or takes more than 200 ms, the git role serves fetches from its cache for up to a signed per-tenant limit (default 2 h) and refuses pushes. Each week, production blocks DSQL for one AZ's git role so this path is always exercised and never becomes an untested fallback (U#6, U implication 20).
   - **Covers:** git reads in (b), (c) and (d), and in Regional cells during (a).
   - **Cost:** it reverses the "no stale reads" tradeoff (D:859). During an outage, revoked access lingers up to the snapshot's age, and a push made seconds before the outage may not show up in a fetch.
5. **A failure gate added to Milestone 0 and 1.** Next to the load test (D:820), run three fault tests: 30% packet loss on one Regional endpoint, a full-scan query at 50-way concurrency from the api role, and an Actions-role driver opening 150 connections per second. Record per-surface uptime.
   - **Gate:** a Paired cell keeps all four surfaces through the one-Region fault, and git and merge stay up through the Actions storm.
   - **Cost:** about one extra day.

**DESIGN.md text changes.**
- Add to "Tradeoffs accepted": a fleet-wide DSQL engine regression takes down all four surfaces in every cell at once, and no bulkhead covers it. Bulkhead 4 keeps git reads only, for its time limit.
- Add to "Open questions": ask AWS whether DSQL engine upgrades roll out cluster by cluster in waves customers can see.
- Extend the D:11 switch rule: if a Milestone 0 failure gate fails, the base switches to candidate 2.

**Strongest counterargument.** Scenario (d) is the one AWS would cause and Tributary cannot fix. A young engine (U#28) that changes monthly, under every cell at once, is exactly the correlated failure cells cannot contain. Candidate 2 is the only design where git writes survive it.

My answer: candidate 2 also loses merge and the API in (d), so its gain is two of the four top-priority surfaces. Bulkhead 4 recovers git reads in the base. A flip would cost one-transaction merge atomicity (D:873), and in candidate 2's BYOC and self-hosted default it would lose git, merge and the API's git-backed reads in row (e).