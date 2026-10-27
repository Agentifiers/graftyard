# Foundation attack on `cloudflare/DESIGN.md` (seat A)

Labels: [measured] means I read the cited file or doc, or ran the arithmetic or script. [inferred] is my reasoning from what I read. [guess] is an unbacked number. Cloudflare docs were fetched as `index.md` on 2026-10-26 into `/tmp/cfa`.

## 1. Verdict

**Holds with changes.** Keep the base: Raft groups of SQLite Durable Objects on three site scripts in one account, `Tri` across three continents. I tried to replace it and could not. The platform supplies what an acceptor needs. Uniqueness is enforced "upon accessing storage", and the output gate holds replies until writes are confirmed [measured: known-issues and sqlite-storage-api pages]. Candidate 1's fixed pairing still drops to a single-region copy after any failover, so it cannot match `Tri`'s RPO until Cloudflare says where SQLite followers live [measured: candidate-1 lines 185, 389]. What fails is the headline uptime claim. The design says a regional Durable Object incident costs about 15 s (line 9), but three needed mechanisms are missing. Nothing moves leadership off a sick site. Every reset of a leader instance (a site deploy, a runtime update, a failed storage write) forces a full election. A follower's `verify` refusal never removes the leader that wrote the bad entry. Separately, `verify` as written forces one entry in flight, which contradicts the 300 entries/s gate. Several Downtime rows say `removed` or `contained` where the honest label is weaker, and the 22% arithmetic ignores the design's own 10% retry budget. Add health-driven drain and leadership transfer, let a restarted leader resume its term, make `verify` refusal depose the leader, restate throughput, and relabel the rows. Add a second flip condition tied to Cloudflare's written answer on follower geography.

## 2. Findings

### F1. Blocker. No mechanism moves leadership off a sick site, so the regional-incident claim does not hold

- **Rests on:** lines 9, 166-168, 200, 391, 419, 455, 457, 481.
- **Failure path.** Take the 2026-10-22 "Elevated error rates for Durable Objects in Western Europe", 17:51 to 00:09 UTC (`research/cloudflare-uptime.md` finding 20) [measured]. Groups led from B (weur) meet one of two cases.
  - **Case a: the leader stays.** Edge-to-B calls fail some share of the time. B's calls to A or C succeed often enough to renew its lease, because one read-index round to any one follower renews it (line 200). Raft has no trigger to elect, so that share of reads and writes to B-led groups fails for 6 hours [inferred].
  - **Case b: the leader flaps.** B's storage writes fail. The output gate doc says "If the write fails, the system will reset the Object, discard all outgoing messages" (https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/) [measured]. B loses its lease, and the group waits up to 10 s for a promise to lapse (lines 166-167). B restarts eligible, can win the next election, and resets again. That is a 10 s outage per flap, repeated for 6 hours [inferred].
  - Neither case is "re-elect within about 15 s, and the others never notice" (line 419). The "stalled leader loses its lease" claim (line 391) holds only for a hard stall.
- **Fix:**
  1. Graft candidate 1's `suspect` path (candidate-1 lines 172-175). An edge whose member call misses its deadline reports to a follower on another site. A follower that gets reports from two distinct colos within 10 s stops granting the leader new promises, so the lease lapses within 9 s and the followers elect.
  2. The leader steps down and transfers leadership (Raft `TimeoutNow`) when its commit error rate or storage confirm latency over 10 s passes a compiled threshold.
  3. Add a signed `drain {site}` intent on the `Release` group, applied automatically on SLO burn for one site. Members on a drained site refuse to campaign, and leaders there transfer away.
  4. Pre-vote refuses candidates on a drained site or one that reset in the last 5 minutes.
  5. Add a Milestone 0 row. Inject 30% errors on edge-to-member calls for one site, then separately on that site's storage writes. Pass: every group led there moves within 20 s and stays moved. Fail: any group changes leader more than twice in an hour.
  6. Relabel rows 391 and 419 `mitigated` until that gate passes.

### F2. Major. Every leader reset costs a full election, including the design's own daily deploys

- **Rests on:** lines 41, 163-168, 398, 455, 484.
- **Failure path.**
  - A site deploy resets every member on that site. A Durable Object "will only be reset when it is assigned a different version" (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/with-durable-objects/), and a site script's deploy reassigns all of its objects [measured]. Objects are also "replaced ... [on] an update of the Durable Object's class code, or of the Workers system itself" (https://developers.cloudflare.com/durable-objects/platform/known-issues/) [measured]. "Cloudflare regularly updates the Workers runtime" (https://developers.cloudflare.com/workers/platform/limits/) [measured].
  - Line 455 says "a leader reset loses its lease and a follower wins within about 10 s". While a group is active, its followers' promises are fresh, so nobody can be elected until they lapse. Each reset of a leader therefore means up to 10 s with no reads and no writes for that group [inferred].
  - **Arithmetic.** A bad minute is one where more than 5% of requests fail (line 422). If a fraction f of traffic is led from the deploying site and is down for 10 s inside one minute, the failure share is f/6, which passes 5% once f is 0.3 or more [measured arithmetic]. An NA-heavy install leads most groups from A [inferred]. A weekly release is 3 site deploys, about 13 a month. If each costs one bad minute, that is about 13 against the 4.3-minute read budget (line 426) [inferred]. Runtime updates add more on top.
  - Row 398 presents these resets as a benefit.
- **Fix:**
  1. Persist `led_term` in `MemberState`. A restarted member that led the current term resumes as leader of that term after one AppendEntries round. This is safe because a term has one leader, and the replaced instance cannot append: in-flight requests that access storage are "stopped immediately" (https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/) [measured].
  2. Before a planned site deploy, append `drain {site}` (F1). Deploy only once that site leads no groups, and undrain after the watchdog passes.
  3. Roll each site script by gradual percentages (10%, 50%, 100%), which the with-durable-objects page supports per object, so resets spread out.
  4. Rewrite row 398 to state what a reset costs.

### F3. Major. A `verify` refusal never deposes the leader, so a bad release is a write outage, not `removed`

- **Rests on:** lines 11, 187-191, 348, 393, 460, 634.
- **Failure path, Monday.** A runs N+1 and leads group g. N+1 appends an entry that N cannot decode, and B and C refuse it. The design only says "the client gets a 503" (line 348). The leader keeps its lease and its log tail.
  - Every later entry chains `prev` to the refused one, so each is refused too, and g accepts no writes until the watchdog swaps A back [inferred from line 156].
  - That holds for every group A leads, which for an NA-heavy install is most of them. This is GitHub's August 2025 class (`research/uptime.md` finding 44) as an install-wide write outage [inferred].
  - After swap-back, A on N must boot with an N+1 entry it cannot decode in its own uncommitted tail. The design does not say what happens then.
- **Failure path, Tuesday.** Suppose the bad shape comes only from a rare command and never appeared Monday. On Tuesday A and B both run N+1, so they form a quorum and commit it. C refuses and stops applying, so the group has lost its fault tolerance. Rolling A and B back to N is now impossible, because a committed entry is undecodable by N [inferred].
- **Fix:**
  1. When a follower refuses an entry, the leader truncates its uncommitted tail, transfers leadership to the refusing follower, and persists a "no leadership until my release changes" flag.
  2. Members tolerate an undecodable uncommitted tail at boot by truncating it.
  3. During its first day, the rolling site deliberately leads a 1% sample of groups, so N+1 writes while N verifies.
  4. B rolls only after a full day with zero refusals on A.
  5. Relabel row 393 `contained`, and state the Tuesday residual.

### F4. Major. `verify` as specified allows one entry in flight, which contradicts the 300 entries/s scale row and gate

- **Rests on:** lines 171-174, 187-191, 364, 604, 609.
- **Failure path.** `commit` runs `decide` "on the fold at `applied`" and appends at last + 1 (lines 171-172). Either the leader pipelines or it does not.
  - **If it pipelines,** entry k+1 is decided on a fold that lacks the still-uncommitted k. Two pushes with the same `expect` both pass, and the second overwrites the first [inferred]. Followers also check `expect` "against the follower's own fold" (line 188), which stands at the follower's applied index. A follower can apply k only after it learns k committed, and it learns that from the append that carries k+1. So a dependent k+1 is refused falsely [inferred].
  - **If it does not pipeline,** the design is correct with one entry in flight per group. Throughput is then about 1 / (leader confirm + RTT + follower confirm): about 1 / (5 + 80 + 5 ms), or 11 entries/s, for an enam leader. It is about 6 entries/s for an apac leader, whose nearest follower is about 160 ms away [guess RTTs, measured arithmetic].
  - Line 364 claims 300 to 1,000 entries/s. Milestone 0 line 609 requires at least 300 and fails under 100, and "Any fail value sends the design back" (line 604). The gate fails by construction.
- **Fix:**
  1. State one entry in flight per group.
  2. Group commit packs every command that arrives during the round into the next entry (up to 1.5 MB), and each command's `decide` sees the fold plus the earlier commands in the same entry.
  3. Followers apply k before verifying k+1. This is guaranteed because the append carrying k+1 carries commit = k.
  4. Restate row 364 as about 10 entries/s with many commands per entry. Change gate 609 to at least 300 commands/s at a commit p99 of 150 ms or less.
  5. The workloads still fit: the monorepo needs about 2 entries/s (line 361), and the 50 comments/s cap (line 462) batches.

### F5. Major. The 22% arithmetic ignores the other calls in a push, the 10% retry budget, and resets on failed writes

- **Rests on:** lines 399, 410, 458, 624.
- **Evidence.** I reran the design's own figure: 0.78 x (1 - 0.22^2) = 0.742 per attempt and 0.983 over three tries [measured]. It is wrong for four reasons.
  1. **A push makes three member calls.** It reads the token leader and the `Authz` leader before the commit (line 279), so one attempt succeeds 0.78 x 0.78 x 0.742 = 0.452 of the time, and three whole attempts 0.835 [measured arithmetic].
  2. **Retries are capped.** Lines 399 and 458 cap retries at 10% per isolate. With 25.8% of commits failing, success is at most 0.742 + 0.1 x 0.742 = 0.82 [measured arithmetic].
  3. **A failed leader write resets the object** ("the system will reset the Object", sqlite-storage-api). With about 2 entries/s per busy group, the leader resets within seconds, and each reset costs an election (F2) [inferred].
  4. **Reads hit the leader alone,** so they succeed 78% of the time [measured arithmetic].
- Under every one of these models, each minute of the 2 h 28 min incident is bad for reads and writes, and it exhausts both monthly budgets [inferred].
- **Fix:**
  1. Replace the row's arithmetic with the per-push figure.
  2. Exempt `retryable` member calls on the commit path from the 10% budget, and state their own cap (two retries per call). That gives (1 - 0.22^3)^2 x 0.983 = 0.962 [measured arithmetic].
  3. Depend on F2's leader resume so a reset does not trigger an election.
  4. State that this class breaks both monthly SLOs.
  5. Change Milestone 0 row 624 to inject failures into storage writes, which reset objects, not only into calls, and to count whole pushes.

### F6. Minor. The 15 s RTO basis is incomplete, and the read RTO is wrong

- **Rests on:** lines 9, 166-168, 323, 328, 436, 610.
- **Gap 1: no call deadline.** The 15 s is 10 s of `ELECTION_AFTER` plus unstated terms. No member-call deadline is given, and the platform sets none: "There is no set time limit on individual subrequests" (https://developers.cloudflare.com/workers/platform/limits/) [measured].
- **Gap 2: no named campaign trigger.** The design does not say what makes a follower campaign. If it is an alarm, alarms "can be delayed by up to a minute due to maintenance or failures while failover takes place" (sqlite-storage-api) [measured].
- **Gap 3: the read figure.** "Reads at most 9 s" (line 436) is wrong. A new leader cannot serve `fresh` until it is elected, and that waits on the 10 s promise, so read RTO is at least write RTO [inferred].
- **Fix:**
  1. State a commit-path member-call deadline of 1 s, hedged to the other follower.
  2. Make campaigns request-driven: on a deadline, the edge calls a follower with `suspect`, which campaigns as soon as its promise lapses, with no alarm involved.
  3. State the RTO as promise remaining (10 s at most) + 1 s deadline + two B-to-C rounds (about 200 ms each [guess]), about 11.5 s worst case, and give reads the same figure.

### F7. Minor. Several Downtime rows count shared Cloudflare layers as separated

- **Rests on:** lines 106, 108, 389, 396, 412, 612.
- **Row 389.** Runtime changes ship by cohort, "starting with free customers first" (`research/cloudflare-uptime.md` finding 25) [measured]. Cohorts are by customer, not region, and the three site scripts share one account [inferred]. A bad runtime release that reaches the account's cohort hits all three sites at once, so `contained` should be `mitigated`, because late cohorts see fewer bad changes.
- **Row 396.** The binding path still depends on Cloudflare's shared object routing, which serves all three sites [inferred]. `removed` should be `mitigated`.
- **Row 412.** Durable Object storage has a "remaining third-party storage dependency" (finding 23) [measured], so `removed` overclaims.
- **Line 108.** "contains every account-scoped fault to one voter" is false for the fourth account, which holds the zone and edge and carries all client traffic [inferred].
- **Line 612.** A WAF rule applies to zone HTTP traffic, and a cron-driven member call never enters the zone, so this gate cannot fail and proves nothing about the FL1 path [inferred].
- **Fix:** relabel the three rows, correct line 108, replace gate 612 with a written answer from Cloudflare on the internal route of a Durable Object call, and keep row 415 `exposed`.

### F8. Minor. Placement verification measures the wrong thing, and hints belong to callers, not to one site

- **Rests on:** lines 100, 110, 274, 613, 702.
- **The trace.** The `/cdn-cgi/trace` page documents only "the Cloudflare data center serving your request" for a request to your domain (https://developers.cloudflare.com/fundamentals/reference/cdn-cgi-endpoint/) [measured]. A Durable Object subrequest to the zone also rides the public proxy path that line 106 tries to avoid [inferred].
- **The hints.** A hint is a parameter to `get()`, and "Only the first call to `get()` for a particular Object will respect the hint" (https://developers.cloudflare.com/durable-objects/reference/data-location/) [measured]. The hint table therefore lives in the edge Worker and other callers, not in "one script's config" (line 100). A bad edge release can misplace new members of all three sites [inferred].
- **Fix:**
  1. Measure the property directly. At join and daily, each member takes the median of 20 binding round trips to each other voter, and `Placement::new` refuses any pair under 30 ms [guess threshold; intra-region round trips are a few ms and enam to weur about 80 ms, guess].
  2. Keep `eu` against `us` jurisdictions where residency allows.
  3. Move line 100's hint fault to the edge's failure rows.

### F9. Minor. Membership changes and lease reads leave four gaps

- **Rests on:** lines 161-165, 199-203, 270, 291, 329, 373.
- **Gap a: leases under `Joint`.** Lease and read-index must need promises from a majority of both the old and the new config. Otherwise a leader holding a promise only from an outgoing member can serve reads after the new config has moved on [inferred].
- **Gap b: a removed member coming back.** The member on A, once its region returns, holds the old config. After its promise lapses it campaigns with a higher term and forces leader churn (Raft dissertation, section 4.2.3) [inferred].
- **Gap c: "durability is 2-of-3 meanwhile" (line 373) is wrong.** After A is lost, entries are on 2 of 2 remaining voters. Packs placed in {A, B} or {A, C} and not yet swept to the third bucket (line 291) have one copy [inferred].
- **Gap d: lease reads on a replaced instance.** In-flight requests "are allowed to finish if they do not access a Durable Object's storage" (lifecycle page) [measured]. A lease read served from an in-isolate cache on a replaced instance could therefore be stale.
- **Fix:**
  1. State the `Joint` lease rule.
  2. Add pre-vote, and refuse votes from non-members of the latest committed config.
  3. Correct line 373, make sweeper lag an alarmed metric, and have `ReplaceMember` copy single-copy packs first.
  4. Require every `Fresh` read to touch SQLite after its lease check, with no in-isolate fold cache.

### F10. Minor. Automatic stale mode serves 4-hour-old reads on the minority side of a partition

- **Rests on:** lines 204-206, 270, 296, 458.
- **Failure path.** In a partial partition like the 2026-10-23 North America event (open question 6), member A, cut off from B and C for 30 s, enters stale mode. It serves reads at `applied` for up to 4 hours, while B and C keep a quorum and accept writes from edges that reach them.
  - An edge colo that can reach only A serves `gh pr list` and fetches that miss acknowledged merges. That breaks line 296 ("The next read from any client sees the push") [inferred].
  - Automatic entry fires mainly in partitions, which is exactly when it is unsafe. In a provider-wide front-door failure it helps no one, because the front door is down [inferred].
- **Fix:** enter stale mode only by the signed flag, and state that stale reads break read-your-writes across clients.

### F11. Minor. The flip condition misses the one fact that would make candidate 1 the better base

- **Rests on:** lines 691, 699, 704, 738.
- **Candidate 1, fixed, is still weaker on RPO.** After any failover it acknowledges `Alone` on one region's copy until it reseeds (candidate-1 lines 185, 389) [measured]. `Tri` keeps two copies after one loss. So fixed candidate 1 meets "zero lost acknowledged pushes" across a second loss only if SQLite followers span regions (`research/cloudflare-uptime.md` open question 1) [inferred].
- **What would make it win.** If Cloudflare confirms in writing that followers span at least two regions, then `Alone` and Regional are already region-durable. Candidate 1's one-continent pair then gives faster writes with a smaller protocol, and its `suspect` promotion already handles gray failure [inferred].
- **Fix:**
  1. Restore candidate 1's pre-Milestone-1 request for that written answer (candidate-1 line 526).
  2. Add a second flip condition: if followers span regions, candidate 1 with the `Alone` fix becomes the base for `Na3`-class and cost-sensitive installs.
  3. Graft `suspect` now (F1).

### F12. Minor. The lifeboat stays within the self-hosted carve-out, except for two items

- **Rests on:** lines 7, 25-30, 542, 710, 714.
- **Within the carve-out.** workerd is Cloudflare's open-source runtime (`research/cloudflare.md` line 34) [measured]. The self-hosted mode that FRAME requires "as far as Cloudflare allows them" (`FRAME.md` line 39) [measured] already runs on non-Cloudflare hosts with Docker. A lifeboat on customer hardware fits the same carve-out [inferred].
- **Outside it:**
  - Line 710's second DNS provider is a non-Cloudflare service.
  - Line 714's vendor-hardware managed lifeboat would be a non-Cloudflare hosting dependency of the managed product [inferred].
- **Client reconfiguration.** Lifeboat reads need `url.insteadOf` or `GH_HOST`, so `actions/runner` and Octokit apps will not follow them without changes [inferred].
- **Fix:**
  1. Name the lifeboat as the only off-Cloudflare component, made only of workerd plus the shipped images.
  2. Drop or label the second-DNS suggestion.
  3. Record "no managed lifeboat" as the default unless the operator decides otherwise.
  4. State that lifeboat reads are opt-in per client.

## 3. Checked and sound

- **The acceptor.** Durable Objects make a sound Raft acceptor. Votes and appends write storage, replaced instances fail on storage access, and `MemberState` is written before any reply (line 163) [measured: known-issues and lifecycle pages].
- **Per-site deploys.** The premise is documented. A Durable Object stays on one version per deployment and resets only when it is reassigned [measured: with-durable-objects page].
- **Container placement.** The `regions` constraint exists with ENAM, WEUR, and APAC, and `eu` and `us` jurisdictions [measured: https://developers.cloudflare.com/containers/concepts/placement/].
- **Cost.** `cost.py` reproduces lines 574-576. The quorum's cost checks out: 5,845 - 4,386 = 1,459 (33%), and 168 - 118 = 50. Two 2025 front-door events of 190 + 25 minutes use 215 / 263 = 82% of the annual read budget. 1,000,000 / 300 = 56 minutes of Workflow starts [measured].
- **`Tri` topology.** Three continents is the right default given the 3-hour North America event of 2026-10-23 [measured: open question 6].
- **Rejected alternatives.** The rejections of Artifacts, D1, KV, and the R2 Disk Paxos rest on cited limits and incidents [measured: `research/cloudflare.md`, `research/cloudflare-uptime.md` findings 7 to 9, 37, 38].
