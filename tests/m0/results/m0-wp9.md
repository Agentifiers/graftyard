# M0-WP9: agent load on the base (2026-10-30)

Rows run: B0-29, B0-32, and B0-33 on Cloudflare, and B0-35 in this orb with real git (first pass). A second pass ran B0-31 and B0-34 on Cloudflare and B0-30 accelerated on one Durable Object (sections near the end). Raw files are `results/raw/wp9-<row>-<run>.*` (`.summary.json` holds the gate quantities, `.json` every command row and member counter, `.members.json` the member counters read 3 s after the load, and `.log` the runner's output).

## What was built

**Members** (`src/wp9/member.ts`, `types.ts`, `sha256.ts`, `health.ts`). This is a copy of the M0-WP2 Raft member in `src/raft/`, which another agent is editing, deployed as `graft-wp9-a` (enam), `graft-wp9-b` (weur), and `graft-wp9-c` (apac). The Raft core is unchanged: terms, leases, `led_term`, `suspect` with the two-colo rule, pre-vote, `STEP_DOWN`, transfer, one entry in flight, and `verify` on followers. Faults are not bound, so nothing reads `graft-m0-tools`. The copy adds:

- **Inline packs.** A push or merge is a compare-and-set on its ref (`refs/heads/agent/<n>/task`, or `refs/heads/main` for merges) and carries random pack bytes. The bytes are stored in the entry's `packs` blob and covered by the chained digest. An entry holds at most 1.5 MB (`EntryBody`). Catch-up AppendEntries are capped at 4 MB.
- **Admission** (`AdmitClass`). Each class has its own queue, and the Agent queue has a sub-queue per actor, served round-robin. Each entry takes Human, then MergeQueue, then Agent up to `AGENT_FLOOR` of capacity over one round, then Bot, then Agent while the `AGENT_SHARE` token bucket has tokens. Capacity is (1 / the round at a full entry of the current mix) × (commands per entry at the current mix). The round comes from a least-squares fit of round time against entry bytes over the last 300 entries. A group named `capN-...` pins capacity at N. An actor that passes `DEMOTE_PER_MIN` writes in a minute on the group is classed Agent. Agent commands still queued after `AGENT_WAIT` (30 s) get the refusal text. `AGENT_QUEUE_MAX` (1,000 commands, 64 MiB) refuses at once, before staging. A push gets `ng <ref> agent admission full, retry after <n>s`; any other write gets `429 retry-after: <n> You have exceeded a secondary rate limit`. `<n>` is the queue divided by the agent rate, plus 1.
- **Staging** (`PackRef::Stored`). On a hot group (above `HOT_PUSHES`, 2 pushes/s over 5 s), a pack over `INLINE_MAX_HOT` is answered `stage`. Under agent pressure, any agent pack is answered `stage`. The caller then writes the pack to the `graft-wp9-stage` R2 bucket (enam) and sends the command again without it. The design does not define pressure; here it means more agent bytes queued than one entry holds, or more agent commands queued than the bucket admits in 1 s. Pack validation, index-pack, and the git cache shard are not modeled.
- **Activity feed** (`feed`). Answered only by a member that is following: a follower whose last AppendEntries left no gap, or a learner its leader has fed with no gap. The answer comes from the applied log, with no read-index call. The leader never answers; it counts `feedOnLeader` and returns `notServing`. A member that stops following answers its held polls at once. Each member holds at most `FEED_HELD_MAX`. Polls without `paths` batch for `FEED_BATCH_ALL` (10 s), and polls with `paths` batch for `FEED_BATCH` (1 s) after a matching event or wait `FEED_WAIT_MAX` (25 s). `paths` are prefixes, not globs.
- **Learners fed by the leader.** On each commit the leader sends committed entries to each learner slot, one call in flight per learner. In the M0-WP2 prototype, learners pulled entries inside read-index (`m0-wp2.md`, B0-18). Learners never count toward a quorum.
- **Counters** per member (`w9stats`): commit latency per admitted class, entries waited, refusals by cause and shape, a per-second leader row (capacity, tokens, queued agent commands and bytes, request totals), a per-entry row (commands, bytes, inline bytes, class mix), confirm times, and leadership events.

**Load** (`src/wp9/load.ts`, `graft-wp9-load`). `Load9` is a Durable Object load client, created with a location hint and run from its alarm, as in `src/rehydrate.ts`. Clients use the hints enam, enam, wnam, weur, apac in rotation: 30 clients for B0-29 and B0-32, and 60 for B0-33. Each client runs the caller side of the write path itself. That covers one request id per operation, `commit` with `CALL_DEADLINE`, `outcome`, following `NotLeader`, and one `suspect` to the other two members after a missed receipt. It never forwards a command. It also applies B0-25's per-call budget: it gives up when one call has failed 1 + `COMMIT_RETRIES` times. Pushes and merges come only from the enam clients, because a push reaches the leader from the network's shard on site A (`design/DESIGN.md`, journey 1, step 2). Other commands, reads, and feed polls come from all clients.

A Worker invocation runs at most 6 member calls at once (`m0-wp2.md`). A Durable Object does not have that limit for RPC: a throwaway probe made 200 concurrent RPCs from one object in about 1 s, and from an alarm too. Each client keeps at most 400 operations in flight. The highest seen was 352, in the first B0-33 run.

**Runner** (`run/wp9.ts`): `deploy`, `run <row>`, `collect`, `delete`. `run/wp9-report.ts` prints one run's gate quantities. `run/wp9-testmerge.ts` is B0-35. Configs are in `wp9/`. Long runs ran as `amp orb service` jobs.

**Journey 9 as replayed** (`scripts/cost_agents.py --a1`, per 500 agents):

- 500 agents each push their own ref every 120 s: 4.17 pushes/s. Packs are uniform 10 to 70 KB, and 5% are 1 MiB, so the mean is about 40 KB plus the 1 MiB tail.
- 21.0 agent side commands/s of 1 KB, standing for PrSync, test-merge records, check runs, tasks, leases, and REST writes.
- 1.25 Human and 1.0 Bot commands/s.
- 10 merges a minute, chained by parent tip. Each merge names the tip the previous one will leave.
- 33 ref reads/s, 200 API reads/s, and 43 agent reads/s, all `fresh` on the leader.
- 500 feed pollers: 250 with `paths` (one package of 50) and 250 without.

That totals 27.6 commands/s, 25.2 of them Agent. B0-33 uses the same mix at 5,000 agents: 41.7 pushes/s plus 210 side commands/s, 252 agent-class commands/s offered. It adds one user token writing 100 commands a minute, which the edge classes Human, and drops feed polls and agent reads.

## Runs

| Run | Member colos (a, b, c) | What differs |
| --- | --- | --- |
| `b0-29-r1` | IAD, CDG, SIN | 600 s |
| `b0-29-r2` | ATL, LIS, HKG | 600 s |
| `b0-29-r3` | ORD, AMS, KIX | 600 s |
| `b0-29-sd3r1` | IAD, MAD, KIX | 600 s, `STEP_DOWN` confirm rule needing 3 slow writes (outside the design) |
| `b0-32-r1` | ATL, LHR, SIN | 180 s, burst at 60 s |
| `b0-32-r2` | ORD, AMS, HKG | same |
| `b0-32-ih64r1` | ATL, CDG, KIX | same, `INLINE_MAX_HOT` 64 KiB (outside the design) |
| `b0-33-r2` | EWR, LHR, SIN | 300 s, 60 clients, measured capacity |
| `b0-33-ih0r1` | MIA, LHR, ICN | same, `INLINE_MAX_HOT` 0 (every hot push staged; outside the design) |
| `b0-33-cap300r1` | EWR, AMS, NRT | same, capacity pinned at the gate's 300 commands/s, so the bucket admits 150/s |
| `b0-35-r1`, `-r2` | this orb | 300 s; pushes and base moves touch 3 or 20 files |
| `b0-35-over2` | this orb | 120 s, budget forced to 0.03 vCPU (outside the design) |

Discarded runs:

- `b0-29-smoke*`: harness bring-up (files deleted). The first smoke run found a bug in this copy: follower `verify` recomputed the digest without the inline packs, so every follower refused the first entry that had a pack.
- `b0-33-r1` (raw files kept as evidence): the old client retried without a budget and was not bounded. The retries overloaded all three members ("Durable Object is overloaded"), reset two, and stalled the group. That was a harness storm, not the design's caller, which gives up after its budget. The client now follows B0-25's per-call budget with backoff.
- The B0-32 run first queued in the chain: it never started (a runner bug).

## Verdicts

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-29 | Leader commit p99 396, 602, 566, and 625 ms over four 600 s runs (p50 177 to 311 ms, 16,530 to 16,688 commands each), carrying 27.6 commands/s. Merge p99 367 to 795 ms at the leader, at most 874 ms at the client, every merge in the next entry. 0 `Stale` on 10,000 distinct-ref pushes. 0 feed polls on the leader in all runs, 45.4 feed polls/s served (r2, r3) by two learners at 25 to 28 req/s each. Leader at 330 to 332 req/s over the load window (r2, r3, `sd3r1`), 303 to 305 without `outcome`. Leader changes: r2 and `sd3r1` none; r1 and r3 one each, from `STEP_DOWN` on one slow storage confirmation (543 and 638 ms) | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Pass on every gate quantity except commit p99, which was over 600 ms in two of four runs: 602 ms in r2 (ATL, LIS, HKG) and 625 ms in `sd3r1` (IAD, MAD, KIX, below)**. No fail value was hit: p99 is under 1 s, no merge waited over 10 s, and no feed poll reached the leader. Like B0-01, the result depends on placement. The row does not gate leader changes, but see "STEP_DOWN" below |
| B0-32 | 505 of 505 burst pushes (256 KiB each, over 1 s) acknowledged within 5 s in both runs: client p99 3.32 s (r1) and 2.46 s (r2), 0 refused, 0 lost, 0 errors. 488 to 493 of them staged under pressure. Queued agent bytes peaked at 1.67 to 1.68 MB (2.6% of 64 MiB), and queued commands at 556 and 359. 0 leader changes, 0 `suspect` reports, no member reset. All 5 queued merges committed in the next entry after their admission, but at 1.96 to 2.30 s (r1) and 0.84 to 1.46 s (r2) from admission | Every push within 5 s or refused in the `ng` shape, queued bytes under `AGENT_QUEUE_MAX`, every merge in one of the next 2 entries, 0 leader changes, 0 `suspect`; fail on a merge delayed over 2 s | **Fail in r1** (4 of 5 merges over 2 s, worst 2.30 s). **Pass in r2** (worst 1.46 s). Why: the burst's inline packs, not admission (below). With `INLINE_MAX_HOT` at 64 KiB, which is outside the design, the same burst passed with margin: merges 149 to 266 ms, pushes client p99 1.38 s |
| B0-33 | At the measured capacity (`r2`, `ih0r1`) every one of 252 agent commands/s offered was admitted, because measured capacity was 350 to 2,350 commands/s, not 300. Human and MergeQueue commit p99 1,126 and 871 ms in r2, and 849 and 594 ms in ih0r1. With capacity pinned at 300 (`cap300r1`): 149.5 agent commands/s admitted (the bucket's 150), 101.7/s refused, all 29,418 refusals in the `ng` or 429 shape and none of another shape, Human and MergeQueue p99 787 and 1,003 ms, merges at most 1.04 s at the client. The fast user token was classed Agent after its 60th write a minute on the leader that saw it (399 of 399 after the first minute in ih0r1). Leader changes: r2 two, ih0r1 none, cap300r1 three, every one from `STEP_DOWN` (one of them, in r2, on commit errors, the rest on the confirm rule). `suspect` reports: 400, 0, and 320 | Human and MergeQueue p99 at most 600 ms, at least 140 agent commands/s admitted, the user token classed `Agent`, refusals in the `ng` or 429 shape, 0 leader changes, 0 `suspect` | **Fail**: leader changes and `suspect` reports in two of three runs, and Human/MergeQueue p99 over 600 ms in all three. The admission half passes: at least 140/s admitted in every run, so the ref-lane flip condition (under 100/s) is not met, every refusal had the right shape, and no merge-queue command waited over 2 s |
| B0-35 | Real `git merge-tree` on a 100,000-file synthetic monorepo, 500 PRs. Head pushes every 2 min, base moving 10 times a minute, 500 clients each GETting their PR every 5 s. One test merge cost 3 to 4 ms of CPU (p99 8 to 12 ms). Test-merge CPU averaged 0.07 to 0.09 vCPU against a 7 vCPU budget (k = 7). Dispatch p99 16 to 19 ms. 0 computes from list fields. With the budget forced to 0.03 vCPU, use stayed at 0.027, dispatch p99 was 253 ms, and 1,602 GETs were answered `null` | Test-merge vCPU within `TEST_MERGE_BUDGET`, dispatch p99 at most 10 s, 0 list computes | **Pass**, with the caveat that the synthetic merges are about 400 times cheaper than `MERGE_VCPU_S` (1.5 s) assumes (below) |

## What limits each number

**B0-29 commit p99 is the network, as in B0-01.** At 27.6 commands/s, entries held 3 to 6 commands (p99 9 to 12) and 4.8 to 8.4 entries/s committed. A commit is about 1.5 rounds to the nearest follower. Across the three runs the leader's round to that follower varied with the colos it landed in:

- r1 (IAD to CDG): commit p50 177 ms, p99 396 ms.
- r3 (ORD to AMS): p50 228 ms, p99 566 ms.
- r2 (ATL to LIS): round p50 203 ms and p99 385 ms, commit p99 602 ms.

The inline packs did not set the tail. In r2, entries were p50 39 KB and p99 80 KB, and the leader's own confirm p99 was 5 ms. One 1 MiB push in 20 adds about 300 ms to its entry's round (the echo cost in `m0-wp2.md`, B0-01). Those entries are about 2% of entries, so they sit at the p98 to p99 boundary. The agent load did not move commit latency off the B0-01 distribution (p99 399 to 690 ms at 100 commands/s).

**B0-29 leader requests: 330 req/s, of which 303 to 305 are the design's 305 (`cost_agents.py --a1`).** The other 26 to 28 req/s are `outcome` calls. In the design, `outcome` is a second call per command; in the prototype it is also a separate RPC. Reads were 277 req/s. The peak one-second rate was 383 to 498 req/s, against 1,000 req/s.

**Feed polls.** 45.4 polls/s, matching the design's 45/s. They were served by two learners, each holding at most 272 to 291 polls at once, a quarter of `FEED_HELD_MAX`, at 25 to 28 req/s per learner. Hold time p50 was 10.1 s (the `FEED_BATCH_ALL` pollers) and p99 12.4 to 12.6 s. Pollers with `paths` returned within 1 s of a matching push in their package. In r1 and r2, 730 and 766 polls got the client's 429. Every one of those came before the run started, when the learners had not yet been fed. Followers refused those polls as not following, and the 429 came after three members were tried. Pollers now start at the run's start, and r3 had 0 429s. Followers served 4 to 46 polls in each run, as the third try when both learners refused.

**B0-32: inline bytes, not admission, delay the merges.** The burst arrived while the group was not yet under pressure, so the first 7 to 14 pushes (1.3 MB) went inline. Two 1.33 to 1.37 MB entries were then in flight, and each took 0.48 to 1.92 s to commit, because AppendEntries of 1.3 MB to LHR or SIN is slow (B0-01's echo: 533 ms at 1.5 MB to weur). The merges arrived during those entries and waited for them. Admission still put every merge in the entry right after its admission (`waited` 1 for 5 of 5), and Human and MergeQueue went first in every entry (the class mix in `.json`). After the first entries, pressure staged the rest of the burst. One entry then carried 556 staged pushes in 464 KB (r1), and the queue drained in about 2.3 s. `AGENT_FLOOR`/`AGENT_SHARE` never refused, because the measured capacity rose to 2,000 to 5,750 commands/s for small staged commands. Staging is an R2 PUT from the client's colo, p50 451 to 506 ms and p99 1.7 to 2.1 s. That is most of the push's client time; validation and index-pack would come on top.

**B0-33: admission works; leadership does not survive the load on this prototype.**

- **Leader CPU and request rate.** At about 255 commands/s, the leader served 650 to 754 req/s over the window (494 without `outcome`), with one-second peaks of 1,054 to 1,764 req/s. The peaks come from every edge re-sending after a leader change. Entries held p50 47 to 51 commands of 300 KB (r2) or 50 KB (ih0r1, all staged).
- **`STEP_DOWN` fired on single slow storage confirmations.** In r2, B's leader saw one 1,550 ms confirm. In cap300r1 the leaders saw 535 to 680 ms. The prototype's confirm rule is the window's p99, and with under 100 confirmations in 10 s that is the window's maximum, so one slow write moves the leader. This also explains the B0-29 changes in r1 and r3. B0-33 r2's commit-error step (166 s: errors 0.06/0.07 to b and c) came from AppendEntries to SIN missing `CALL_DEADLINE` (1 s) while the 300 KB entries were in flight. Over 10 s the error rate passed 5% for both followers.
- **`suspect` reports follow a change.** Each leader change makes in-flight commits miss their receipt, and every edge with a missed receipt reports. The reports were 400, 0, and 320, with no reports in the run without a change (ih0r1).
- **Human and MergeQueue p99 is the entry's round, not the queue.** They always went first, waiting 1 entry. In r2 their p99 was 1,126 ms with a commit p50 of 383 ms. The tail is entries of 300 KB to 1.1 MB (p99 1.09 MB) of inline agent packs, which take one slow round to SIN or LHR. In ih0r1, where agent packs were staged, entries were p99 117 KB and Human p99 was 849 ms. Neither reaches 600 ms on `Tri` at 4.6 to 4.7 entries/s and a round p50 of 208 to 226 ms. The merge queue's p99 was 594 ms in ih0r1.

**B0-33 admission numbers.** The token bucket binds only when capacity is near 300 commands/s, as the design assumes (`cap300r1`). There, agent commits were 149.3 to 150.8/s in every 30 s window after the first, and agent refusals were 99 to 105/s. 13,963 were refused at `AGENT_QUEUE_MAX` (1,000 commands) and 46 after `AGENT_WAIT`. The client saw refusals p50 114 ms, because the queue cap refuses before staging. Admitted agent commands waited p50 6.2 s and p99 14 s in the queue, all inside `AGENT_WAIT`. The measured capacity was never near 300: at the B0-33 mix it was p50 434 (r2) and 2,353 (ih0r1) commands/s. B0-02's 280 to 573/s were 1 KB commands at entries up to 441 commands, and the prototype here uses the same round-time fit.

**The fast user token.** It was classed Agent from its 61st write in a minute on the leader that counted it. Writes after a leader change were classed Human again until the new leader had counted 60: 108 of 399 in r2, at 166 to 230 s after the change at 166 s. The design does not say where `DEMOTE_PER_MIN` counts live. Here they are leader memory; a per-group fold or a per-actor counter would survive a change.

**B0-35: the synthetic merge is cheap, so the budget is never tested at design cost.** `merge-tree --write-tree` of a 3-file or 20-file PR on a 100,000-file tree cost 3 to 4 ms of CPU at p50. Recomputing every open PR on each base move would be 0.25 to 0.33 vCPU at that cost, against 125 vCPU at the design's `MERGE_VCPU_S` of 1.5 s. GETs found a current test merge 5.4 to 5.9% of the time, and 78% got `null` because of the once-per-30-s coalescing. The forced-budget run shows the order holds: push and dispatch merges were served first (dispatch p99 253 ms), GETs were dropped to `null` (1,602), and use stayed under budget. What a real monorepo merge costs, with large trees, renames, and conflicts, is what decides `TEST_MERGE_BUDGET`. B0-34's real network would measure it.

## What is modeled and what is real

- **Real:** the Raft group on Cloudflare: three Durable Objects with enam, weur, and apac hints, and the members' own colos. Cross-script bindings, SQLite logs with inline pack bytes, quorum commits, lease reads, learners, `suspect`, `STEP_DOWN`, transfer, and leader changes. Load clients pinned by hint in 20 or more colos on 4 continents. R2 PUTs for staged packs. `git merge-tree` CPU for B0-35.
- **Modeled:**
  - Commands are register writes: a push is a compare-and-set on its ref with random pack bytes, and side commands are 1 KB puts.
  - No `validate`, no index-pack, no git cache shard, no `Authz` or credential `deps` read per entry. No `ExtendLeases`, no PrSync fan-out, no events or cursors.
  - Feed `paths` are prefixes.
  - "Pressure" is this prototype's definition.
  - Capacity is a fitted estimate.
  - The edge is the load client itself: no Worker front, no TLS, no `ReplyMeta`.
  - The REST 429 and git `ng` texts are strings in replies, not HTTP responses or report-status pkt-lines.
  - B0-35 runs on one 8-vCPU orb, not on k shards, and its merge cost comes from a synthetic tree.
- **Not run in the first pass:** B0-30, B0-31, and B0-34. The second pass below ran them.

## Guessed constants

| Constant | Held? | Evidence |
| --- | --- | --- |
| `AGENT_SHARE` 0.5, `AGENT_FLOOR` 0.2 | Holds for the order, untested for the share's size. With capacity at 300, exactly 150/s was admitted, and Human, MergeQueue, and Bot always went first. At measured capacity the share never bound | `b0-33-cap300r1`, `b0-33-r2` |
| `DEMOTE_PER_MIN` 60 | Holds, but resets on a leader change | `b0-33-*`, `fastUser` |
| `AGENT_WAIT` 30 s | Holds. At cap 300, admitted agent commands waited p99 14 s, and 46 hit the 30 s line | `b0-33-cap300r1` |
| `AGENT_QUEUE_MAX` (1,000, 64 MiB) | The command cap binds first. 1,000 queued at 150/s is 6.7 s of queue, and 1,000 commands of 1 KB is 1 MB. The byte half never came near: peak 1.7 MB in B0-32 | `b0-32-*`, `b0-33-cap300r1` |
| `INLINE_MAX_HOT` 256 KiB | Too high for bursts. At 256 KiB a burst's first entries fill with 1.3 MB of inline packs and the merges behind them wait 1 to 2.3 s. At 64 KiB the same burst passed with merges under 270 ms | `b0-32-r1`, `-r2`, `-ih64r1` |
| `HOT_PUSHES` 2/s | Every journey 9 group is hot (4.17 pushes/s), so the hot cap always applies | all |
| `TEST_MERGE_BUDGET` 0.25 | Not tested at real cost. The synthetic merges used about 1% of it | `b0-35-*` |
| `STEP_DOWN.confirm_p99_ms` 500 | Fails as written. On a window with under 100 confirmations the p99 is the max, and one slow write in 10 s moved the leader 6 times in 4 of the 9 kept runs | all |

## STEP_DOWN under load

Every leader change in the kept runs, 7 in all, was a `STEP_DOWN` the prototype's rule fired by itself. No fault was injected. 6 were on the confirm rule, each from one storage confirmation of 535 to 1,550 ms in a window of 40 to 80 confirmations (the max is the p99 there). The other one was on commit errors: AppendEntries to apac missing the 1 s deadline while 300 KB to 1.1 MB entries were in flight. Each change cost about 0.4 s of no leader, plus re-sends, `suspect` reports, and up to 199 client give-ups (`gaveUp`). These are what fail B0-33. B0-32's three 180 s runs had none, but at the same rate a longer burst run would see some.

**STEP_DOWN variant.** Run `b0-29-sd3r1` (IAD, MAD, KIX) used the rule "at least 3 confirmations over 500 ms in the window", which is outside the design. The leader saw one 575 ms confirmation; the design's rule would have moved the leader, and the variant held it. The run had 0 leader changes, 0 `suspect` reports, and 0 client give-ups. Commit p99 was 625 ms (p50 251 ms) on that placement. Merge p99 was 782 ms, 0 `Stale` on 2,500 pushes, 45.4 feed polls/s, 0 on the leader, and the leader at 332 req/s. So B0-29 had 4 runs: commit p99 396, 566, 602, and 625 ms. Two are under 600 ms, two are 2 to 25 ms over, and all are under the 1 s fail line.

## Proposed design edits

1. **`design/DESIGN.md`, Shape, `Health` and `STEP_DOWN` doc comment.** Add: "The confirm rule counts a window only when it holds at least 100 confirmations, or it needs at least 3 confirmations over `confirm_p99_ms` in the window, because on a group committing about 5 entries a second the p99 of 50 confirmations is their maximum, and one slow write moved the leader." Evidence: in M0-WP9, 6 of 7 leader changes were a confirm `STEP_DOWN` on one slow write (`results/raw/wp9-b0-29-r1`, `-r3`, `wp9-b0-33-r2`, `-cap300r1`). Tag it [measured].
2. **`design/DESIGN.md` and `design/AGENT-LAYER.md` (agent block, byte-identical), `INLINE_MAX_HOT`.** Change "`pub const INLINE_MAX_HOT: usize = 256 << 10; // [guess]`" to "`pub const INLINE_MAX_HOT: usize = 64 << 10; // [measured, B0-32]`". Add to its comment: "An entry that takes a burst's first pushes inline holds 1.3 MB and commits in 0.5 to 1.9 s to a far follower, which delays merges queued behind it past 2 s; at 64 KiB a burst of 500 pushes of 256 KiB kept every merge under 270 ms." Evidence: `wp9-b0-32-r1` (merge 2.30 s, fail), `-r2` (1.46 s), `-ih64r1` (0.27 s). Note that this stages about 60% of agent pushes at journey 9's size mix: R2 PUT p50 450 ms, and Class A writes go up. `scripts/cost_agents.py` should price that before the change is accepted.
3. **`design/DESIGN.md`, agent block, admission comment.** After "AGENT_QUEUE_MAX caps queued agent commands", add: "A command refused at AGENT_QUEUE_MAX is refused at once, before staging; at 150 agent commands/s the 1,000-command cap is about 7 s of queue and binds long before the 64 MiB byte cap." Evidence: `wp9-b0-33-cap300r1` (13,963 refusals at the cap, 46 at `AGENT_WAIT`, peak bytes 1.0 MB).
4. **`design/DESIGN.md`, agent block, `DEMOTE_PER_MIN`.** Add: "The count is kept per actor in the group's fold (or carried in `ExtendLeases`-like term-start state), so a leader change does not reset it." Evidence: 108 of 399 fast-user writes were classed Human for a minute after a leader change (`wp9-b0-33-r2`).
5. **`design/DESIGN.md`, journey 9, step 1, and Scale "Agent fleet, leader requests".** Keep 305 req/s, and add "plus one `outcome` call per command, about 28/s, 330 req/s measured [measured, `tests/m0/results/m0-wp9.md`]". Change "a burst of 500 agent pushes in 1 s acknowledges within about 3.3 s while merges keep their place [inference]" to "acknowledged within 2.5 to 3.3 s at p99 with merges in the next entry, but merges waited up to 2.3 s behind entries of inline packs at `INLINE_MAX_HOT` 256 KiB [measured]".
6. **`design/DESIGN.md`, Scale "Agent fleet, group commands" (the 5,000-agent sentence).** Change "At 5,000 the token bucket admits 150/s" to "At 5,000 the token bucket admits `AGENT_SHARE` of the measured capacity, 150/s if capacity is 300; Milestone 0 measured 350 to 2,350 commands/s of capacity at this mix, so all 252/s were admitted, and Human and MergeQueue p99 was 0.85 to 1.1 s from the round to a far follower, over the 600 ms gate [measured]". B0-33's Human and MergeQueue line needs the operator: either restate it to about 1 s, as B0-01's was, or accept that the agent mix on `Tri` misses it.
7. **`design/DESIGN.md`, agent block.** Define pressure ("Under pressure only `PackRef::Stored` pushes queue"): "Pressure is more agent bytes queued than one entry holds, or more agent commands queued than the agent bucket admits in 1 s [inference, the M0-WP9 prototype's rule]."
8. **`decisions/OPEN-QUESTIONS.md`, A14.** Answer it: at least 140 agent commands/s were admitted in every B0-33 run (149.5/s at capacity 300, 252/s at measured capacity), so ref lanes stay closed [measured]. **A1**: the 300 commands/s gate holds at journey 9's 27.6/s with commit p99 396 to 602 ms; the git CPU half waits for B0-34.
9. **`decisions/OPEN-QUESTIONS.md`, new question.** "What does one test merge on the real monorepo cost? Synthetic 100,000-file merges cost 3 to 4 ms, 400 times under `MERGE_VCPU_S`, so `TEST_MERGE_BUDGET` and k = 7 rest on the guessed constant (`tests/m0/results/m0-wp9.md`, B0-35)."

The design files were not edited: the task forbids it. Every edit above is a proposal.

## B0-31: apac leader, every agent push 1 MiB, placement failing (second pass, 2026-10-30)

**What changed in the prototype.** The B0-31 replay is journey 9 at 500 agents (as B0-29) with every agent push 1 MiB (`pushFixed`). Setup moves the group's leader to site C (apac), and the pushers are the apac-hinted clients, because a push reaches the leader from the network's shards on the leader's site. Staging goes to an apac bucket, `graft-wp9-stage-c`. Group-name variants, all outside the design unless named, select the rule under test:

- `ih64-`: `INLINE_MAX_HOT` 64 KiB. Without it, the design value of 256 KiB applies.
- `nop-`: `PackStore::place` fails for every bucket. The member never answers `stage`, the edge does not stage, and the hot cap yields: every pack up to `INLINE_MAX` (1 MiB) goes inline, as the design's `INLINE_MAX_HOT` comment says.
- `sd3-`: the 3-slow-writes `STEP_DOWN` confirm rule from the first pass. Without it, the design's rule applies: window p99, which under 100 confirmations is the window's max.
- `hold-`: `STEP_DOWN` never fires. This keeps the leader on apac for the whole run, which the row asks for. Under both `STEP_DOWN` rules the apac leader stepped down within 9.5 to 11.1 s (below), so the other runs measure an enam or weur leader.
- `cap300-`: capacity pinned at 300 commands/s (diagnostic only).

**Runs** (600 s each, 30 clients, learners in slots 0 and 1). The columns are: commit at the leader for every admitted command (p50/p99/max), the same p50/p99 from 30 s on (after the start-up transient), merge p99 at the leader, agent pushes ok/refused/`Stale`/error, entries a second, `suspect` reports, the leader's req/s over the load window (feed polls on the leader were 0 in every run), the peak queued agent bytes, and leader changes.

| Run | Members (a, b, c) | Rule | Commit p50/p99/max ms | From 30 s | Merge p99 | Pushes ok/ref/`Stale`/err | Entries/s | `suspect` | Leader req/s | Queued bytes peak | Leader changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `b0-31-base` | ORD, LHR, HKG | `STEP_DOWN`, 256 KiB | 192/862/11,997 | 189/503 | 862 | 2,493/0/0/7 | 7.45 | 620 | b: 331 | 0.25 MB | 1: commit errors 0.80/0.80 at 10.9 s; the transfer to a was abandoned and b won the election at 22.4 s |
| `b0-31-ih64` | MIA, AMS, NRT | `STEP_DOWN`, 64 KiB | 209/1,791/5,879 | 208/376 | 2,775 | 2,494/0/4/2 | 7.05 | 18 | a: 333 | 2.1 MB | 1: commit errors 0.18/0.70 at 9.9 s, c to a |
| `b0-31-sd3` | EWR, AMS, HKG | 3 slow writes, 256 KiB | 161/586/11,850 | 159/545 | 612 | 2,496/0/0/4 | 8.64 | 104 | a: 332 | 0.15 MB | 1: commit errors 0.70/0.70 at 9.5 s, c to a |
| `b0-31-sd3ih64` | EWR, LHR, NRT | 3 slow writes, 64 KiB | 141/956/5,758 | 140/390 | 2,485 | 2,500/0/0/0 | 10.11 | 14 | a: 331 | 2.1 MB | 1: commit errors 0.18/0.60 at 9.8 s, c to a |
| `b0-31-hold` | EWR, LHR, KIX | none, 256 KiB | 318/758/4,681 | 318/641 | 1,476 | 2,500/0/0/0 | 4.66 | 2 | c: 331 | 2.2 MB | 0 |
| `b0-31-holdih64` | EWR, LIS, KIX | none, 64 KiB | 333/6,687/11,857 | 331/885 | 8,576 | 2,496/0/1/3 | 4.36 | 168 | c: 330 | 2.2 MB | 0 |
| `b0-31-nop` | ORD, CDG, SIN | `STEP_DOWN`, placement failing | 20,985/30,044/30,692 | 23,453/30,474 | 1,296 | 969/1,448/3/80 | 4.04 | 854 | a, b: 331, 329 | 66.8 MB | 3: commit errors at 11.1 s (c to b), then confirm 1,385 ms at 551 s (b to a) and 561 ms at 606 s |
| `b0-31-sd3nop` | ORD, AMS, KIX | 3 slow writes, placement failing | 21,289/29,960/30,677 | 24,298/30,505 | 2,378 | 979/1,496/4/21 | 3.98 | 114 | b: 329 | 66.7 MB | 2: commit errors at 10.4 s (c to a), then at 21.6 s (a to b) |
| `b0-31-holdnop` | MIA, LHR, SIN | none, placement failing | 25,308/30,181/30,914 | 28,451/30,745 | 3,103 | 744/1,753/0/3 | 3.19 | 210 | c: 330 | 66.8 MB | 0 |
| `b0-31-holdnopcap` | ORD, LHR, SIN | none, placement failing, capacity 300 | 20,065/30,267/31,138 | 22,691/30,628 | 3,335 | 1,152/1,348/0/0 | 1.89 | 0 | c: 330 | 66.8 MB | 0 |

Every run carried 27.4 to 27.6 commands/s offered, and Human, MergeQueue, and Bot were never refused. Feed polls were 36.8 to 46.3/s, all on learners and followers, 0 on the leader. In every run, merges were 100 of 100 ok with 0 `Stale`, and each landed in the entry after its admission. The `sd3`, `sd3ih64`, `ih64`, and `base` rows are measurements of an enam or weur leader after 10 s, not of an apac leader.

**With placement working, every 1 MiB push is staged, so 256 KiB against 64 KiB makes no difference here.** 2,491 to 2,498 of 2,500 pushes were staged in every run with buckets up. Once the group is hot, both caps are below 1 MiB. Only the first 2 to 9 pushes, sent before the first commit reply carried `hot`, went inline (the "Queued bytes peak" of 2.1 to 2.2 MB, and the 1.05 MB entries at 0 to 3 s). The differences between the 256 and 64 KiB rows come from placement and the start-up transient, not from the cap. `holdih64`'s 8.6 s merge and 6.7 s commit p99 sit in the first 3 s: 270 commands over 3 s, all at t = 0 to 2 s, behind the first inline 1 MiB entries to LIS and EWR from KIX, while those calls missed `CALL_DEADLINE`. After 30 s its p99 was 885 ms. B0-32 remains the row that separates the two caps.

**The apac leader steps down under both `STEP_DOWN` rules, on commit errors.** In all six runs that allowed it, the apac leader moved to enam or weur within 9.5 to 11.1 s. The trigger was the commit-error rule, not the confirm rule: AppendEntries from HKG, NRT, KIX, or SIN to the enam and weur members missed the 1 s `CALL_DEADLINE` 9 to 80% of the time in the first 10 s, per follower. In that window the first 1 MiB packs were inline (the "append to a failed: call deadline" events, then "ok after 1 to 13 failures, 830 to 999 ms"). The 3-slow-writes variant changes only the confirm rule, so it did not hold the leader. With buckets up, a held apac leader (`hold`) committed at p50 318 ms and p99 641 ms after 30 s, and 758 ms over the whole run. That is about 130 ms more at p50 than the enam leaders that took over in the other runs (p50 140 to 209 ms). With the leader held, the far calls stayed slow but under the deadline: apac to enam AppendEntries p50 201 ms, p99 620 ms; apac to weur p50 253 ms, p99 388 ms.

**Placement failing: the group cannot carry journey 9's pushes at 1 MiB inline, and pushes were refused.** In all four placement-failing runs, 1,348 to 1,753 of 2,500 agent pushes were refused, with the shape `ng <ref> agent admission full, retry after <n>s`. Another 3,887 to 6,083 agent side commands got the REST 429. Agent commit p50 was 21 to 28 s, and p99 was 30 s, which is `AGENT_WAIT`. Committed pushes were 1.24 to 1.92/s, or 1.3 to 2.0 MB/s, against 4.17/s and 4.37 MB/s offered. Why:

- One 1 MiB pack fills an entry. `EntryBody` holds at most 1.5 MB, so two 1 MiB packs never share an entry, and pushes a second can be at most entries a second.
- An entry of 1.05 MB from apac takes p50 549 to 606 ms and p90 650 to 750 ms to reach an enam member, and p50 192 to 481 ms to reach a weur member (`holdnop`, `holdnopcap` `.members.json` `appendMs`). The tail sits at the 1 s `CALL_DEADLINE`: 43 to 54 calls failed and were sent again.
- So the group made 1.9 to 3.2 entries a second, each with at most one pack. The queue filled to `AGENT_QUEUE_MAX`'s 64 MiB (peak 66.8 MB), and the edge's caller was refused at once (894 to 1,103 refusals at the queue cap) or after `AGENT_WAIT` (the rest).
- It is not admission's capacity estimate. With capacity pinned at 300 (`holdnopcap`), the token bucket allowed 150 agent commands a second, and pushes still committed at only 1.92/s, because the entry round is the limit. Pinning raised throughput a little, from 1.24 to 1.92 pushes/s, because more side commands rode each push's entry.
- Human, MergeQueue, and Bot still went first in every entry, with Human p99 1.3 to 2.0 s and merge p99 1.3 to 3.3 s. Each waited for one 1 MiB round to a far follower.

The design's own capacity line says otherwise: `scripts/cost_agents.py --a1` prints "Inline bytes, every push 1 MiB: 4.17 MB/s against 15 MB/s enam, 9 MB/s apac". Measured from apac with one entry in flight, the group committed at most 2.0 MB/s of inline packs. The 9 MB/s is 4.5 times what one in-flight entry can carry at a 0.5 to 0.6 s round.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-31 | Buckets up, apac leader held (`hold`, `holdih64`): commit p99 641 and 885 ms after the first 30 s, 758 ms and 6.7 s over the whole run, the latter from the first 3 s; 0 pushes refused; 2,497 of 2,500 staged. Under the design's `STEP_DOWN` and the 3-slow-writes variant, the apac leader stepped down to enam or weur within 9.5 to 11.1 s in 6 of 6 runs. In the four of those with buckets up, whole-run commit p99 was 586 to 1,791 ms. Placement failing: 1,348 to 1,753 of 2,500 pushes of 1 MiB refused with `ng ... agent admission full`, agent commit p99 30 s, 1.3 to 2.0 MB/s committed against 4.37 MB/s offered, in all 4 runs | Commit p99 at most 1 s, and pushes up to 1 MiB still commit while placement fails | **Fail.** The placement half fails in every run: pushes up to 1 MiB were refused while placement failed, which is the row's fail value. The latency half passes after the first 30 s with the leader held (641 and 885 ms). Over whole runs, commit p99 is over 1 s in 2 of 6 buckets-up runs: `ih64` at 1.79 s, and `holdih64` at 6.69 s. `holdih64` is over the 3 s fail line, from its first 3 s of 1 MiB inline entries before `hot` reached the edge |

**What limits each number.**

- Placement-failing throughput: one entry in flight, at most one 1 MiB pack per 1.5 MB entry, and a 0.5 to 0.6 s apac to enam round for 1 MB (B0-01's echo cost, `m0-wp2.md`: 533 ms at 1.5 MB enam to weur; apac is farther). Admission, capacity estimates, and the token bucket are not the limit (`holdnopcap`).
- The apac leader's steady-state latency: the round to the nearer of enam and weur, p50 about 200 ms from KIX, plus the leader's own write.
- Start-up spikes: the first pushes go inline before any reply has said `hot`. That is a property of this prototype's edge, which learns `hot` only from commit replies. The design's edge would read it from the same reply, so a group that just turned hot sees the same thing [inference].
- `STEP_DOWN` from apac: 1 MiB inline entries to a far follower miss the 1 s `CALL_DEADLINE`, so the commit-error rule passes 5% for both followers within one 10 s window.

**Modeled versus real.** The same as the first pass: a push is a compare-and-set with random pack bytes. There is no validate, no index-pack, and no git cache shard. Staging is an R2 PUT from the client's colo to `graft-wp9-stage-c` (apac). "Placement failing" is the member answering `placeOk: false` by group name, not a real R2 outage. A real outage would also fail the R2 PUTs the edge tries first, so pushes would reach the group later, not sooner.

**Proposed design edits (B0-31).**

10. **`design/DESIGN.md` and `design/AGENT-LAYER.md` (agent block, byte-identical), the `INLINE_MAX_HOT` comment.** Replace "When placement fails or a bucket's readiness probe is red, it inlines up to INLINE_MAX, and LOG_MAX_BYTES archives the extra later." with "When placement fails or a bucket's readiness probe is red, it inlines up to INLINE_MAX, and LOG_MAX_BYTES archives the extra later. One entry then carries one 1 MiB pack, so a group commits at most one such push per round: from apac, 1.3 to 2.0 MB/s, and journey 9 with every push at 1 MiB (4.37 MB/s) is refused at AGENT_QUEUE_MAX, 1,348 to 1,753 of 2,500 pushes in 600 s [measured, `tests/m0/results/m0-wp9.md`, B0-31]. During the outage, agent pushes over INLINE_MAX_HOT are refused with the agent refusal rather than queued, and Human and MergeQueue pushes keep INLINE_MAX." The last sentence is the proposed rule. Without it, 64 MiB of queued agent packs wait 21 to 30 s and then get refused anyway, and human and merge entries ride behind 1 MiB rounds (Human p99 1.3 to 2.0 s).
11. **`design/DESIGN.md`, Failure modes, `pack buckets` provider-wide row.** Change "so a hot group inlines up to 1 MiB like any group. Its entries hold fewer pushes, admission paces agents, and `LOG_MAX_BYTES` archives the extra after recovery [inference]" to "so a hot group inlines up to 1 MiB for Human and MergeQueue pushes. Agent pushes over INLINE_MAX_HOT get the agent refusal, because at 1 MiB a push per entry the group commits 1.3 to 2.0 MB/s from apac, under half of journey 9's 4.37 MB/s [measured, `tests/m0/results/m0-wp9.md`, B0-31]." Also change B0-31's fail value to read "any Human or MergeQueue push up to 1 MiB refused while placement fails". As written, the row cannot pass at journey 9's rate on `Tri`.
12. **`scripts/cost_agents.py`, the `--a1` line "Inline bytes, every push 1 MiB".** Change "against 15 MB/s enam, 9 MB/s apac" to the measured one-in-flight rate: "against 2.0 MB/s measured from an apac leader (one 1 MiB pack per entry, round p50 0.55 s to enam)". Then rerun the script and copy the line into `design/AGENT-LAYER.md`.
13. **`design/DESIGN.md`, Shape, `Health` and `STEP_DOWN`.** Add: "An apac leader carrying 1 MiB inline entries misses `CALL_DEADLINE` to both far followers and steps down within 10 s under either confirm rule; the commit-error rule, not the confirm rule, fires [measured, B0-31]." That is consistent with the design's choice of home sites. It means B0-31's "leader on apac" holds only while the group's entries stay small. The operator decides whether that is acceptable or whether the row moves to a `hold` measurement.

## B0-34: git CPU on a real large network (second pass, 2026-10-30)

### Per-operation cost on torvalds/linux (this orb)

`run/wp9-gitcpu.ts` times real git work on a bare clone of torvalds/linux, made 2026-10-30. The pack is 6.4 GiB with 12.0 million objects, plus a commit-graph. The git version is 2.56.0, and the machine is this orb's 8 vCPU. CPU is the git children's user + sys time. Raw: `results/raw/wp9-b0-34-gitcpu-r1.json`.

- **pull**: `merge-tree --write-tree` of the two parents of each of the newest 60 first-parent merges on master, which are real subsystem pulls.
- **pr-1d, -1w, -1m, -6m**: the agent PR shape. For a real non-merge commit C from that long ago, the head is A, which has C's tree on C's parent, so A is not in master and its change is C's real change. Each sample is the test merge of A onto today's tip.
- **push**: the receive side for A: `index-pack --stdin --fix-thin` of a thin pack, then the connectivity check `rev-list --objects A --not --all`.
- **clone-miss**: a depth-1 pack of the tip, built as upload-pack builds it.

| Operation | n | CPU p50 | p90 | p99 | Mean | Non-zero exit (conflict) |
| --- | --- | --- | --- | --- | --- | --- |
| Test merge, real pull (`merge-tree`) | 60 | 28 ms | 57 ms | 203 ms | 32 ms | 10 |
| Test merge, agent PR on a 1-day-old base | 58 | 5 ms | 19 ms | 122 ms | 12 ms | 6 |
| Same, 1-week-old base | 60 | 6 ms | 11 ms | 34 ms | 7 ms | 14 |
| Same, 1-month-old base | 60 | 12 ms | 17 ms | 150 ms | 15 ms | 5 |
| Same, 6-month-old base | 60 | 45 ms | 55 ms | 338 ms | 50 ms | 30 |
| Push receive (index-pack plus connectivity), one commit | 58 | 142 ms | 154 ms | 177 ms | 143 ms | 0 |
| Clone-cache miss, depth 1 of the tip (300 MB pack) | 5 | 25.4 s | 25.8 s | 25.8 s | 25.5 s | 0 |

The same operations in the container (`standard-4`, Alpine git, 31 packs instead of one) cost more: a push receive 0.20 s, a queue merge 0.10 s, and a stock depth-1 miss 42 to 46 s (`b0-34-r1`, below).

**What this does to the constants.** These are the guesses in `scripts/cost.py`:

- **`MERGE_VCPU_S` (1.5 s) is 30 to 300 times the real cost.** A test merge on Linux costs 5 to 50 ms at p50 and at most 338 ms at p99. The head-push test merges of journey 9 (4.17/s) need 0.02 to 0.2 vCPU, not 6.25. Recomputing all 500 open PRs on each of 10 base moves a minute would be 83 merges/s, 0.4 to 4 vCPU, not 125. So `TEST_MERGE_BUDGET` 0.25 of one `standard-4` (1 vCPU) covers the head-push merges with room to spare. Even eager recompute fits within k = 2 to 4.
- **`PUSH_VCPU_S` (0.5 s) is 2.5 to 3.5 times the real cost** for an agent push of one commit (0.14 to 0.20 s). Pushes of many commits cost more. These pushes were 0.5 to 290 KB.
- **`CI_MISS_VCPU_S` (1.0 s) is 25 to 45 times too low** for a depth-1 checkout of a large repository built the stock way. Most of that is delta search and compression over 103,000 tip objects: `pack-objects` with `--window=0` cost 9.1 s instead of 25 s, for a pack 0.5% larger. A miss is the dominant git CPU term. At journey 9's 1.5 misses/s (1.25 agent CI plus 0.25 agent clone), stock misses need 38 to 69 vCPU, which no k ≤ 8 of `standard-4` (32 vCPU) can carry.
- **A composed miss costs 0.7 s.** The design does not have this, and it is a candidate change. An agent CI checkout of `refs/pull/N/merge` differs from the tip by a few files. A shard that keeps the tip's depth-1 pack, which the clone cache already stores, can answer the miss with that pack plus a small pack of the merge commit's objects missing from it, under one header and trailer (`wp9git compose`). Every object is a full object or a delta inside its own part, so `index-pack` accepts the result. The local check gave fsck clean and a tree equal to the tip's. CPU was 0.72 to 0.74 s, almost all of it reading the tip's 103,000-object list and hashing 300 MB for the trailer. Time to first byte was 0.35 s against 20 to 27 s stock. The stored base must be renewed when the tip moves far from it: 20 commits behind, the incremental pack is 9.8 MB and costs 2.7 s [measured, this orb].

**A limit of the agent-PR shape.** Each agent head replays a change that master already has: C is an ancestor of the tip. So a clean test merge's per-file work is trivial, and the merge ref's tree equals the tip's. The composed miss's incremental part was 1 object, the merge commit. A real agent change adds a few content merges and the changed blobs and trees on their paths, tens of objects. That moves neither cost much [inference]. What the measurement does carry is the part that scales with a big repository: the tree diff from an old merge base to the tip, which grows from 5 ms (1 day, about 8,000 changed files) to 45 ms (6 months). The 30 conflicts on 6-month bases are real overlaps between C and later master edits.

### The replay on real containers

**Bed** (`src/wp9/git.ts`, `graft-wp9-git`, its own script; image `wp9/git/Dockerfile` with the shard server `wp9/git/main.go`; bucket `graft-wp9-git`, enam):

- **`Shard9`**: one container-backed Durable Object per (site, shard name), under the `durable_object` policy. Each starts `standard-4` (4 vCPU) with `enableInternet: false` and rehydrates a network by streaming its packs from the bucket into the container, as in B0-22. Git work goes through the container's queue, one slot per vCPU, served Human, then MergeQueue, then Bot, then Agent. An Agent job still waiting after 20 s is shed with 503.
  - `push`: a real agent push of a Linux commit as above, receive side plus the head-push test merge.
  - `human`: the same push with no test merge.
  - `merge`: `merge-tree` of a real pull.
  - `miss`: a depth-1 checkout of the PR's merge ref, built stock or composed.
  - `hit`: an edge read of a cached pack from the bucket.
- **`Net9`**: stands for the network group's leader. Every 10 s it reads `NetworkLoad`: the most loaded of its ready shards' CPU over 10 s (cgroup `cpu.stat`), and the summed queue depth. It sets k per warm site with hysteresis, at most `FANOUT_MAX` 8, drawing the extra shards from the site's hot pool.
- **`Pool9`**: one per site. It holds `HOT_POOL` 7 slots, granted by rendezvous, with a per-owner budget: the monorepo's owner 6, the second network's owner 2, the hostile owner 2.
- **`Drive9`**: 12 load clients created with enam, weur, and apac hints, run from alarms in 9-minute segments.
- **API guard**: a fetch wrapper in the script fails and counts any request to `api.cloudflare.com`.

Networks:

- `linux`: a bare clone of torvalds/linux repacked into 31 packs of at most 250 MB, 8.0 GB of packs plus indexes and commit-graph, 8.76 GB copied per shard.
- `gitnet` and `hostile1` to `hostile3`: git/git, 0.49 GB each. These are real repositories, so the second network and the hostile networks are small.

**Phases** (seconds from the start of load):

- 0: `linux` (owner `mono`) warm on sites a and b, under journey 9's git work. That is 4.17 agent pushes/s with a test merge each, 1.25 agent CI misses/s, 0.25 agent clone misses/s routed to the client's nearest warm site, 10 queue merges a minute, 0.3 human pushes/s, and 2 clone-cache hits/s.
- 420: site a killed. Every container of site a is destroyed, and site c, the spare, becomes warm.
- 600: `gitnet` (owner `other`) turns hot: 2 pushes/s and 3 misses/s.
- 840: the hostile owner makes `hostile1` to `hostile3` hot, with 1 push/s and 3 misses/s each.

Hysteresis is accelerated, outside the design: up after 30 s above 0.75 (design 3 min), down after 120 s below 0.4 (design 1 h).

**Runs.**

- `b0-34-r1`: misses built the stock way (`pack-objects --revs --shallow`, as upload-pack does). Its load clients ran as one 20-minute alarm, and the clients' rows exist only from 902 s on (17,348 rows). The likeliest cause is the platform ending each alarm handler after 15 minutes of wall time [inference; the rows before 902 s were never stored]. The k timeline, pool, and shard loads are complete. The client fix is the segmented alarm in `Drive9`.
- `b0-34-r2`: misses composed from the tip's depth-1 pack (outside the design, above), with segmented clients. All rows were kept, 16,784 operations.

| Quantity | `b0-34-r1` (stock misses) | `b0-34-r2` (composed misses) |
| --- | --- | --- |
| Rehydrate of 8.76 GB `linux` per shard | 66 to 206 s (43 to 134 MB/s), 20 starts | 77 to 207 s (42 to 114 MB/s), 8 starts |
| Monorepo k at its home warm site (a) | 1, then 7 by 230 s. 6 owner-budget denials, CPU 1.0, queue 100 to 290 | 1 throughout, CPU 0.5 to 0.7 |
| After site a killed | b went from 5 to 7 at 1.0 CPU; c ready at +162 s and grew to 6 | b took the load: 1 to 7 by 690 s (budget denials at 670 and 710 s), CPU fell to 0.4 to 0.5; c ready at +158 s |
| Second network, API guard | `gitnet` k 3 on c (2 from the pool, then owner budget), 0 API calls | `gitnet` k 1 (CPU 0.1 to 0.36), 0 API calls |
| Hostile owner | `hostile1` to `3` asked for pool shards on c and got "pool full" each time. The `mono` owner held 5 of c's 7 slots and `other` 2 | k 1 each (CPU 0.2 to 0.67); no pool request |
| Monorepo k reduced by another owner | No: `linux` kept k = 7 on b and 6 on c to the end | No |
| k over 8, Cloudflare API calls | No; 0 | No; 0 |
| Human push p99 | 40.5 s (902 s to end; queue p99 40 s) | 0.68 s before the kill; 1.48 s in the 180 s after it; 1.56 s while the second network came up; 1.21 s under the hostile owner |
| Queue merge p99 | 38.8 s | 0.63, 1.04, 0.81, 1.69 s |
| Agent push p99 (with test merge) | 37.9 s, and 2,137 of 4,972 shed | 1.65 s; 20.0 s after the kill (5 of the 641 shed or errored); 1.35 s under the hostile owner |
| Clone time to first byte p99, hits and misses together | 36.3 s (902 s to end) | 1.52 s (journey 9), 20.1 s (site killed), 18.9 s (second network), 1.50 s (hostile owner) |
| Clone-cache hit TTFB p99 (edge from bucket) | 0.36 s | 0.34 to 0.45 s in every phase |
| Agent miss TTFB p99 | 46 to 64 s | 1.55 to 1.87 s outside the kill window, 19 to 20 s inside it |

**What limits each number.**

- **r1 is CPU-bound on stock misses.** One depth-1 miss of the Linux tip took 42 to 46 CPU-s in the container. 1.5 misses/s need about 65 vCPU, and 7 `standard-4` shards give 28. So every shard sat at 1.0 CPU with 100 to 290 queued jobs. Agent work was shed at the 20 s line. Human pushes and queue merges went first, but each still waited for a running 45 s miss to finish, because nothing preempts a job: queue p99 40 s for Human. This is the failure the row's fail value names, and at the design's guessed `CI_MISS_VCPU_S` of 1.0 s nobody would have predicted it.
- **r2's latency after the kill is the rehydrate.** With site a gone, all load went to b's single base shard until `fanout` raised k. Each new pool shard had to copy 8.76 GB before it could serve: 166 to 207 s at 42 to 53 MB/s, slower than B0-22's 52 to 118 s for 5.4 GB, because six shards copied from one bucket at once. So for about 270 s, from 420 to 690 s, one shard on b ran at 0.86 to 0.90 CPU with up to 87 queued, and agent pushes and misses waited up to 20 s. Human pushes stayed at p99 1.5 s because they went first. The ttfb gate counts misses as clones, so it read 19 to 20 s in that window. Clone-cache hits stayed at 0.34 to 0.37 s throughout.
- **The design's "warm shards carry it" assumes the second warm site already has k shards.** It had k = 1, because `NetworkLoad` is per site and only site a was loaded. Requests went to the home site first, as in the design, where a push goes to the leader's site.
- **Owner budget and pool.** In r1 the budget held both ways. The monorepo was denied at its own budget of 6, so k stopped at 7. The hostile owner got nothing because the pool on c was already full: `mono` held 5 slots and `other` 2. So the hostile owner's budget was never the binding check. The pool's exhaustion was, and that also kept the second network at k = 3. In r2 no small network asked for the pool.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-34 | Scaled down: one real 8.0 GB network (torvalds/linux) and four 0.49 GB networks (git/git) on up to 15 `standard-4` shards a run, 20 min of load, accelerated hysteresis, and 12 clients rather than journey 9's 500 agents. k was set in band from `NetworkLoad` and the per-site pool, at most 7 (budget 6 plus base), 0 API calls, and the monorepo's k was never reduced by another owner. With stock misses (r1): human push p99 40.5 s, TTFB p99 36.3 s, 43% of agent pushes shed. With composed misses (r2): human push p99 0.68 to 1.56 s in every phase, and TTFB p99 1.50 to 1.52 s outside the kill window but 18.9 to 20.1 s for 270 s after the kill, while new shards rehydrated 8.76 GB each | k in band, at most 8 per site, hostile owner held to its budget, human push p99 at most 3 s beyond transfer, clone TTFB p99 at most 1 s throughout; fail on k over 8, any API call, the monorepo's k reduced by another owner, or TTFB p99 over 5 s | **Fail (partial).** The k, API, and owner checks pass in both runs. TTFB passes 5 s only outside the kill window of r2 and fails inside it (20.1 s). Over 1 s it fails in every phase, because agent misses, which the gate counts as clones, take 1.2 to 2.5 s at p50 even composed. The design's stock miss fails everything (r1). Human push p99 passes in r2 |

**Modeled versus real.**

- **Real:** containers (`standard-4`, `enableInternet: false`) started by `ctx.container.start()` from Durable Objects with location hints. The 8.0 GB Linux network copied through the object from R2. Real `index-pack`, `rev-list`, `merge-tree`, and `pack-objects` on real Linux history. Cgroup CPU as `NetworkLoad`. Rendezvous pool slots with per-owner budgets in Durable Object storage. Killing a site's containers with `destroy()`. The API guard in the script.
- **Modeled:**
  - The network group's leader is `Net9`, not a Raft group.
  - k changes are not committed entries.
  - Hysteresis is 30 s up and 120 s down, not 3 min and 1 h.
  - A push is the receive side only: no `PackStore::place`, no `.idx` to `R2Key::InlineIdx`, no member commit.
  - A "miss" includes building the merge ref.
  - The clone cache is not written back, so every miss stays a miss.
  - Agent heads replay commits master already has (above).
  - The killed site is the network's warm site a, all of whose containers were destroyed. Its members, edge, and other networks are not part of this bed.
  - 12 load clients stand for the fleet's rates, not 500 agents.
- **Scaled down:** the second and hostile networks are 0.49 GB, so they never needed more than 1 to 3 shards, and the "beside it" pressure on the pool came from the monorepo itself. 8 shards a site were reached by the pool rule but not by the second network.

**Proposed design edits (B0-34).**

14. **`scripts/cost.py`, `MERGE_VCPU_S`, `PUSH_VCPU_S`, `CI_MISS_VCPU_S`.** Set `MERGE_VCPU_S = 0.05` (Linux p50 5 to 45 ms by base age, pulls 28 ms, p99 at most 338 ms), `PUSH_VCPU_S = 0.2` (one-commit agent push 0.14 s in the orb and 0.20 s in a `standard-4`), and `CI_MISS_VCPU_S = 45` for a stock depth-1 miss of a large network. That last one makes clone-cache misses the whole git CPU budget, so it must come with item 15. Rerun `scripts/cost_agents.py` and copy its figures into `design/DESIGN.md` (Scale, journey 9 step 2) and `design/AGENT-LAYER.md` (journey A1 step 5). Evidence: `results/raw/wp9-b0-34-gitcpu-r1.json`, `wp9-b0-34-r1.json`, `wp9-b0-34-r2.json`. Tag [measured, torvalds/linux].
15. **`design/DESIGN.md`, Shape, "Git cache shards (derived)", and `CloneKey`.** Add: "A clone-cache miss for a depth-1 fetch of a commit whose tree is close to a cached tip's is composed: the shard sends the tip's cached depth-1 pack and a pack of the wanted commit's objects missing from it, under one header and trailer, and renews the base when the tip moves. A stock depth-1 miss of a 6 GB network costs 25 to 46 CPU-s; a composed one 0.7 s [measured, `tests/m0/results/m0-wp9.md`, B0-34]." The client's `index-pack` accepts it as is. Whether agent CI checkouts of `refs/pull/N/merge` should share keys with the tip is the related open question.
16. **`design/DESIGN.md`, Shape, `TEST_MERGE_BUDGET` doc comment.** Add: "A test merge on torvalds/linux costs 5 to 50 ms at p50 and at most 338 ms at p99 [measured, B0-34], so head-push merges at journey 9 need 0.02 to 0.2 vCPU and the budget binds only on eager recompute." Keep 0.25. With this cost, k = 7 is no longer set by test merges.
17. **`design/DESIGN.md`, Failure modes, git caches, data-center and region rows, and the B0-34 gate.** Add: "A newly warm site serves a hot network only after `rehydrate` copies it into each of its k shards; six shards copying 8.8 GB from one bucket took 166 to 207 s, and clone time to first byte for misses was 19 to 20 s p99 meanwhile, while cache hits stayed at 0.37 s [measured, B0-34]. The second warm site therefore holds the hot network's k shards warm, sized by the home site's `NetworkLoad`, not its own." Without that, the gate's "TTFB p99 at most 1 s throughout" cannot hold across a site kill. Alternatively, measure TTFB on hits only, which passed in every phase.
18. **`decisions/OPEN-QUESTIONS.md`, the test-merge question from item 9.** Answer it: a real test merge on a 6 GB monorepo is 5 to 50 ms, not 3 to 4 ms and not 1.5 s. The open cost is clone-cache misses.

## B0-30: 30 days of hot-group storage, accelerated (second pass, 2026-10-30)

**Bed** (`src/wp9/store.ts`, `graft-wp9-store`, its own script; runner `run/wp9-store.ts`). There is one Durable Object, `Store9`, with an enam hint. It stands for one member of the monorepo's group: its SQLite holds what a member holds. The archive buckets are `graft-wp9-arch-a` (enam) and `graft-wp9-arch-b` (weur). The run:

1. **Seed** a metadata tier of 4 GB of row bytes: 222,223 issues and 2,000,000 comments of about 1.8 KB each, which is the design's largest repository (Scale, "Member storage").
2. **Replay** journey 9's entries for 30 simulated days, as fast as the object takes them. That is 4 active hours a day at journey 9's rates: 4.17 agent pushes/s and 23.4 other commands/s. `scripts/cost_agents.py --a1` counts 1.8 million pushes in 30 days, which is 4.17/s for 14,400 s a day. Each push carries an inline pack, uniform 10 to 70 KB (mean 40 KB, `cost.py` `inline_push_kb`), and 1 in 1,000 is 1 MiB. The hot cap is not applied, so this is the uncapped inline volume behind the script's "71 GB". Commands are grouped into entries of at most 1.5 MB with a chained digest. The agent rows `cost_agents.py` counts are written beside them:
   - a 2 KB PR row and a 1 KB task row per task;
   - a 300 B check row per CI job (3 per task) and per second push, kept 90 days, so none expire within 30;
   - a 150 B provenance row per push;
   - a 16 KB live `PathSet` per open PR, deleted when the PR closes 20 minutes later.
3. **`LOG_MAX_BYTES`** after each simulated active hour: while the log's bytes exceed 2 GiB, the oldest entries are archived in ranges of about 16 MB under `archive/wp9-b0-30/<from>-<to>-<digest>`. Each range gets create-only puts in both buckets, then an `Archived` row (from, to, `from_at`, `to_at`, key, digest), and only then are the entries deleted from the log. The 30-day age rule is not applied, because inside a 30-day replay nothing reaches 30 days. So the byte cap alone bounds the log.
4. **Readback**: 30 archived ranges spread over the run are read from both buckets. Each check covers byte count, SHA-256 against the `Archived` row, index continuity, and every entry's `prev` link and digest.

`databaseSize` is read after every step, and any `SQLITE_FULL` is caught and counted. Raw: `results/raw/wp9-b0-30-r1.jsonl`, one line per step, written as it happened.

**What acceleration changes.**

- **Write rate.** A simulated active hour (645 MB of log) is written in about 60 s of wall time instead of an hour, and a simulated day in about 5.5 min. The object's own write rate is about 11 MB/s, against 0.18 MB/s for journey 9 in real time. That stresses SQLite and the object more than real time would. A `SQLITE_FULL` or an object reset would show up sooner, not later.
- **Archiving cadence.** Archiving runs once per simulated hour, not continuously. So the log overshoots the cap by up to one hour of entries (645 MB) before each archive pass, and the peak size seen after a replay chunk is about 0.65 GB above steady state. The steady-state number is the size after each archive pass.
- **What does not happen.** There is no idle time, so the idle hours are skipped rather than lived through. That means no snapshot schedule, no follower lag, no compaction, and no Durable Object eviction or restart between writes. A member that restarts would replay nothing here, because the replay holds no in-memory state that matters.
- **Unchanged.** Bytes per entry, rows per task, and the order of writes, deletes, and archives are the same as in real time. So SQLite's page use and free-page reuse, which set `databaseSize`, should be the same [inference].

**Run `b0-30-r1`** (one Durable Object; wall time 98 s seed and 2 h 47 min replay):

- **Seeded:** 222,223 issues and 2,000,000 comments, 3.93 GB of row bytes, member SQLite 4.57 GB.
- **Replayed:** 30 simulated days. That is 435,600 entries, 1,815,000 agent pushes with 74.4 GB of inline packs (cost_agents.py: 71 GB), 10.2 million other commands, and 181,500 tasks, with their agent rows: 181,500 PRs, 1,452,000 check rows, 1,815,000 provenance rows, 181,500 task rows, and 500 live `PathSet` rows, 1.27 GB in all (cost_agents.py: 1.18 GB).
- **`LOG_MAX_BYTES`:** 4,544 archive ranges, 75.9 GB, each in both buckets. A range whose create-only put did not land in both was checked by HEAD in both before its entries were deleted, and none failed that check. After each archive pass the log held 2.06 to 2.15 GB.
- **Member SQLite after each archive pass** (the steady state), against `CHILD_GROUP_AT` 8 GiB (8.59 GB):

| Simulated day | 0 (seeded) | 1 | 5 | 10 | 15 | 20 | 25 | 29 | 30 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SQLite, GB | 4.57 | 6.80 | 7.04 | 7.35 | 7.63 | 7.85 | 8.14 | 8.38 | 8.45 |

The peak just before an archive pass was 9.09 GB: up to one simulated hour's 0.65 GB over the cap, an artifact of the hourly archive cadence above. `SQLITE_FULL` 0, in every one of 121 replay steps and 4,544 archive passes.

- **Readback:** 30 ranges spread from day 0 to day 29.1, each read from both buckets, 60 reads in all. 60 of 60 matched on byte count and SHA-256, held contiguous indexes, and verified every entry's `prev` link and digest (90 entries a range). A read of a 16.7 MB range took p50 788 ms and at most 1.76 s. Each `Archived` row carries `from_at` and `to_at`, so a `RestoredTo` for time t finds its range from the fold alone, as M0-WP8 asked.

**Where the bytes go.** 4.57 GB is the metadata tier on disk: 3.93 GB of row bytes, plus about 16% for SQLite pages and the primary keys. The log at the cap adds 2.06 to 2.15 GB. Agent rows add 1.27 GB of row bytes, which is about 1.8 GB on disk (8.45 GB minus the tier and the log), 40% over their row bytes. The member grew 0.055 GB per simulated day after the first day, from 6.80 to 8.45 GB over days 1 to 30. That is close to `cost_agents.py`'s 0.40 + 0.77 GB a month for check rows and PR, provenance, and task rows (1.17 GB a month, 0.039 GB a day). The difference is SQLite's per-row overhead on 3.6 million small rows.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-30 | Accelerated, on one Durable Object: 30 simulated days of journey 9 on a member seeded with a 3.93 GB metadata tier (4.57 GB on disk). Member SQLite 8.45 GB at day 30 (steady state after archiving), passing 8 GiB (8.59 GB) on no day but within 0.14 GB of it, and 9.09 GB at the hourly peak. 0 `SQLITE_FULL`. `LOG_MAX_BYTES` held the log at 2.06 to 2.15 GB by archiving 75.9 GB in 4,544 ranges to both buckets; 60 of 60 archived range reads verified | Member SQLite under `CHILD_GROUP_AT` with zero `SQLITE_FULL`, and archived entries readable for `RestoredTo` | **Pass, at the margin.** Steady state stayed under `CHILD_GROUP_AT` through day 30 (8.45 of 8.59 GB). The member crosses it about day 32 at the measured 0.055 GB a day, not at the 1.7 months the design states. The peaks just before archiving went over it (9.09 GB), so the leader's split check must read the size after archiving, or archive more often than hourly. 0 `SQLITE_FULL`, and archived entries read back intact |

**Modeled versus real.**

- **Real:** one Durable Object's SQLite on Cloudflare, `databaseSize`, real R2 create-only puts and reads in two buckets in two regions, SHA-256 digests, and the bytes and row counts of journey 9 for 30 days.
- **Modeled:**
  - One member, not a group: no replication, no followers' own archives, no snapshots, no folds beyond the rows named.
  - The digest chain covers the entry's JSON body but not its pack bytes. Hashing 74 GB of packs in JS would have made the replay CPU-bound.
  - The 30-day age rule and `ref_log` are not modeled.
  - The `Archived` row is local, not a committed entry.
  - Metadata rows are fixed-size filler.
  - Accelerated as above.

**Proposed design edits (B0-30).**

19. **`design/DESIGN.md`, journey 9 step 3, and Scale "Agent fleet, member bytes on the monorepo group".** Change "about 7.2 GB after 30 days" and "reaches `CHILD_GROUP_AT` in about 1.7 months" to "8.45 GB after 30 days on a member seeded with a 3.93 GB metadata tier (4.57 GB on disk), growing 0.055 GB a day, so it reaches `CHILD_GROUP_AT` in about 32 days [measured, `tests/m0/results/m0-wp9.md`, B0-30]". The design counts row bytes. On disk, SQLite adds about 16% to the metadata tier and about 40% to the small agent rows. `scripts/cost_agents.py` should apply those on-disk factors (1.16 and 1.4) to its row bytes, then rerun.
20. **`design/DESIGN.md`, Shape, `CHILD_GROUP_AT` doc comment (Split).** After "The leader reads `ctx.storage.sql.databaseSize` after each snapshot", add: "after `LOG_MAX_BYTES` archiving has run, because the log can stand above its cap by the entries since the last archive pass (0.65 GB an hour at journey 9's uncapped inline rate) [measured, B0-30]." That way a split is not started by a transient peak, and the margin below 10 GB covers the peak. At 9.09 GB the peak sat 0.9 GB under the 10 GB limit.
21. **`decisions/OPEN-QUESTIONS.md`.** Add: "An A1-class monorepo reaches `CHILD_GROUP_AT` in about a month, not 1.7 months, so metadata child groups (Milestone 7) gate A1 sooner than planned (`tests/m0/results/m0-wp9.md`, B0-30)." Every other part of B0-30 held. `LOG_MAX_BYTES` 2 GB holds as a cap, and archive and readback worked at 75.9 GB.

## Second-pass summary

| Row | Verdict | Key numbers |
| --- | --- | --- |
| B0-31 | **Fail** | Placement failing: 1,348 to 1,753 of 2,500 pushes of 1 MiB refused in 4 of 4 runs; a group commits 1.3 to 2.0 MB/s of 1 MiB inline packs from apac. Buckets up, leader held on apac: commit p99 641 and 885 ms after 30 s. The apac leader steps down within 11 s under both `STEP_DOWN` rules |
| B0-34 | **Fail (partial, scaled down)** | k in band at most 7, 0 API calls, the monorepo's k never reduced. Stock clone misses cost 25 to 46 CPU-s and saturate 7 shards (human push p99 40 s). With composed misses, human push p99 at most 1.56 s, but clone TTFB p99 19 to 20 s for 270 s after a site kill while 8.8 GB rehydrates. Real test merge on Linux: 5 to 50 ms p50 |
| B0-30 | **Pass, at the margin (accelerated)** | 8.45 GB at day 30 against 8.59 GB `CHILD_GROUP_AT`, 0 `SQLITE_FULL`, 75.9 GB archived, 60 of 60 range reads verified |

**Guessed constants, second pass.**

| Constant | Held? | Evidence |
| --- | --- | --- |
| `INLINE_MAX_HOT` 256 KiB against the proposed 64 KiB | B0-31 cannot tell them apart: at 1 MiB every hot push stages under either. B0-32 stays the evidence for 64 KiB | `b0-31-*` |
| `INLINE_MAX_HOT` yielding to `INLINE_MAX` while placement fails | **Fails** for agent pushes at journey 9's rate: one 1 MiB pack per entry | `b0-31-*nop*` |
| `STEP_DOWN` (window p99 500 ms, commit errors 5%) | The confirm variant does not help an apac leader; commit errors move it within 11 s | `b0-31-base`, `-sd3*` |
| `MERGE_VCPU_S` 1.5 s | **30 to 300 times too high** (Linux 5 to 50 ms p50) | `wp9-b0-34-gitcpu-r1` |
| `PUSH_VCPU_S` 0.5 s | 2.5 to 3.5 times too high for one-commit pushes | same, `b0-34-r1/r2` |
| `CI_MISS_VCPU_S` 1.0 s | **25 to 46 times too low** for stock depth-1 misses of a 6 GB network; 0.7 s composed | same |
| `TEST_MERGE_BUDGET` 0.25 | Holds with large margin at real cost | same |
| `FANOUT_MAX` 8, `HOT_POOL` 7, per-owner budget | Held in band; the pool ran out before the hostile owner's budget bound | `b0-34-r1` |
| `fanout` hysteresis (3 min up, 1 h down) | Not tested at design values (30 s and 120 s used) | `b0-34-*` |
| `LOG_MAX_BYTES` 2 GB | Holds | `b0-30-r1` |
| `CHILD_GROUP_AT` 8 GiB and the "1.7 months" | The cap holds; the time to reach it is about 32 days, not 1.7 months | `b0-30-r1` |

## Teardown

`node run/wp9.ts delete` removes `graft-wp9-a/b/c` and `graft-wp9-load` and empties and deletes the `graft-wp9-stage` and `graft-wp9-stage-c` buckets. `node run/wp9-git.ts delete` removes `graft-wp9-git`. Its container application and its bucket `graft-wp9-git` were deleted by hand, because the runner does not do it. `node run/wp9-store.ts purge` then `delete` removes `graft-wp9-store` and the buckets `graft-wp9-arch-a` and `graft-wp9-arch-b`.

All of these ran at the end of the second pass on 2026-10-30. Nothing with the `graft-wp9` prefix is deployed, and no `graft-wp9` bucket or container application remains: every script answers 404, and every bucket reports "does not exist". The orb-local data was deleted too: the Linux and git/git clones, the split packs, and B0-35's `/tmp/wp9-mono`. To rerun B0-34 or B0-30, clone and repack the repositories again as `run/wp9-git.ts` describes, then deploy.

## Third pass (2026-10-30): the adopted rules

The operator adopted composed clone-cache misses with base renewal every 20 tip moves, the same k at both warm sites sized from the larger `NetworkLoad`, and the placement-outage rule. This pass reruns B0-34 and B0-31 on those rules, with the same isolation as before: `graft-wp9-*` scripts and buckets only. The prototype now uses `INLINE_MAX_HOT` 64 KiB, as the design does.

### B0-31 on the outage rule

**What changed.**

- `src/wp9/member.ts`: while placement fails, an Agent push whose pack is over `INLINE_MAX_HOT` gets `ng <ref> agent admission full, retry after <n>s` from `commit` at once, before admission. It never enters a queue. Human and MergeQueue pushes keep `INLINE_MAX` (1 MiB).
- `src/wp9/load.ts` and `run/wp9.ts`, row `b0-31o`: journey 9 at 500 agents, every agent push 1 MiB, the leader on apac, and pushes from the apac clients, as in the second pass. Added to that are Human pushes at journey 9's human push rate (0.3/s), each to the user's own ref, and the 10 merges a minute. Each Human push and merge carries a pack uniform in 64 KiB to 1 MiB, so every one is over `INLINE_MAX_HOT` and goes inline. Placement fails in every run (`nop-`).
- Runs are 600 s, 30 clients, with learners in slots 0 and 1. They are `b0-31o-nop4` (the design's `STEP_DOWN`) and `b0-31o-holdnop4` (`hold-`: no `STEP_DOWN`, outside the design). A first attempt, `nop3`, was discarded: a deploy service restarted and redeployed the members during the run.

**Run `b0-31o-nop4`** (IAD, CDG, SIN; leader c on SIN; the design's `STEP_DOWN`; placement failing throughout):

- **Agent pushes over the cap:** 2,500 of 2,500 agent pushes of 1 MiB were refused at once with `ng <ref> agent admission full, retry after <n>s`. Client time was p50 62 ms, p99 427 ms, max 988 ms: one `commit` call to the apac leader and no queue wait. The leader counted all 2,500 under the new `outage` cause, and 0 under `AGENT_QUEUE_MAX` or `AGENT_WAIT`. Queued agent bytes peaked at 46 KB (41 commands), against 66.8 MB in the second pass. Every refusal had a valid shape (`badShapes` empty).
- **Human and MergeQueue:** 168 of 168 Human pushes (64 KiB to 1 MiB, inline) committed, as did 100 of 100 merges (same sizes, chained by parent tip, each in the entry after its admission). That is 0 refused and 0 `Stale`. Human and MergeQueue commit at the leader:
  - p50 401 ms and p99 1,825 ms over the whole run;
  - p99 1,529 ms from 30 s on;
  - Human pushes alone p99 1,926 ms, merges p99 1,952 ms.
- **Everything else:** Agent side commands, 20.8/s, were all committed and not refused, with p99 1,668 ms. Commit p99 over all classes was 1,679 ms, and 1,451 ms from 30 s on. Entries ran at 4.08/s, with entry bytes p99 991 KB. 152 of 2,446 entries carried more than 500 KB inline.
- **Leadership:** 0 leader changes. The apac leader held under the design's `STEP_DOWN` for the whole 600 s. There were 12 `suspect` reports, from AppendEntries to IAD missing `CALL_DEADLINE` in the first 22 s, while the first 1 MiB human packs were in flight. 0 client give-ups.
- **Load:** the leader served 328 req/s (305 without `outcome`), with 0 feed polls on it. Learners and followers served 34.3 polls/s.

**Why the leader held this time.** In the second pass every entry carried a 1 MiB agent pack, so far AppendEntries missed the 1 s deadline 9 to 80% of the time, and the commit-error rule fired within 11 s. Now only the 0.47/s Human pushes and merges carry big packs: 152 entries, 6% of them. The error rate stayed under 5% per follower in every window after the first 22 s.

**What sets the Human and MergeQueue p99.** It is the round of entries that carry a 0.5 to 1 MiB pack from SIN to IAD and CDG. Commands with packs over 512 KiB had p99 1,952 ms, and those at or below had 1,598 ms. A Human side command with no pack, riding behind one of those entries, had p99 1,825 ms against p50 339 ms. So the tail is the inline megabyte's round, and it lands on whoever shares or follows that entry.

**Run `b0-31o-holdnop4`** (ATL, MRS, NRT; leader c on NRT; `hold-`, so `STEP_DOWN` is off; placement failing throughout):

- **Agent pushes over the cap:** 2,498 of 2,500 agent pushes of 1 MiB were refused at once in the `ng` shape, with client p50 57 ms and p99 326 ms. The other 2 got client errors: their `commit` call to the leader missed the 1 s deadline three times (around 200 s). None was queued: the leader counted 2,500 `outage` refusals, and queued agent bytes peaked at 74 KB.
- **Human and MergeQueue:** 178 of 178 Human pushes and 100 of 100 merges committed, with 0 refused and 0 `Stale`. 98 merges landed in the entry after their admission and 2 in the second.
  - Human and MergeQueue commit at the leader: p50 353 ms, p99 1,973 ms; from 30 s on, p99 1,760 ms.
  - Human pushes alone p99 2,184 ms; merges p99 2,994 ms.
- **Everything else:** commit p99 over all classes was 1,973 ms (1,354 ms from 30 s on). There were 24 `suspect` reports, 3 client give-ups, and 0 leader changes. The leader served 327 req/s with 0 feed polls; learners and followers served 33.6 polls/s.

The two runs agree on the gate's admission half. The latency half depends on placement: NRT to ATL and MRS is farther than SIN to IAD and CDG.

| Quantity | `nop4` (design `STEP_DOWN`) | `holdnop4` (`hold-`) |
| --- | --- | --- |
| Members (a, b, c) | IAD, CDG, SIN | ATL, MRS, NRT |
| Leader changes | 0 | 0 (held by rule) |
| Agent 1 MiB pushes refused at once, `ng` shape | 2,500 of 2,500, client p99 427 ms | 2,498 of 2,500 (2 client errors), client p99 326 ms |
| Agent commands queued at refusal | 0 (peak queue 46 KB of side commands) | 0 (peak 74 KB) |
| Human pushes and merges refused | 0 of 268 | 0 of 278 |
| Human and MergeQueue commit p50 / p99 (from 30 s) | 401 / 1,825 (1,529) ms | 353 / 1,973 (1,760) ms |
| Merge p99 at the leader | 1,952 ms | 2,994 ms |
| Commit p99, all classes (from 30 s) | 1,679 (1,451) ms | 1,973 (1,354) ms |
| `suspect` reports, client give-ups | 12, 0 | 24, 3 |

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-31 (third pass, outage rule) | Placement failing, leader on apac, every agent push 1 MiB, plus Human pushes (0.3/s) and merges (10/min) of 64 KiB to 1 MiB inline. 0 of 546 Human and MergeQueue pushes refused in two runs. 4,998 of 5,000 agent pushes over the cap refused at once in the `ng` shape (client p99 326 to 427 ms), never queued; the other 2 were client deadline errors. Human and MergeQueue commit p99 1.83 s and 1.97 s (1.53 and 1.76 s after the first 30 s). The apac leader held under the design's `STEP_DOWN` (0 changes in `nop4`) | 0 Human or MergeQueue pushes up to 1 MiB refused while placement fails; agent over-cap pushes refused in the `ng` shape at once, never queued; commit p99 at most 1 s; fail over 3 s | **Pass on the outage rule. Commit p99 at most 1 s fails in both halves, by 0.8 to 1.0 s** (1.68 to 1.97 s over all commands). No run reached the 3 s fail line over all commands, but merges alone reached 2.99 s in `holdnop4`. So under the adopted rule the row's commit p99 passes on neither half. The design half (leader steps down when it must) is the better one here, because the leader no longer needs to step down: 0 changes and the lower p99 |

**What limits the commit p99.** One entry in flight, and the round to the nearer far follower for an entry that holds a 0.5 to 1 MiB Human or merge pack (B0-01's echo cost). On `Tri` with an apac leader, that round is 0.8 to 1.9 s at p99. Commands without a pack that land in or behind such an entry inherit it. 6% of entries carried such a pack, enough to set the p99 of every class. Every other part of the outage rule held.

### B0-34 on the adopted rules

**What changed in the bed** (`src/wp9/git.ts`, `wp9/git/main.go`, `run/wp9-git.ts`, new `run/wp9-gitprep.ts`):

- **Both warm sites sized alike** (`alike`). `Net9` reads `NetworkLoad` from every warm site each 10 s and judges each site by the larger CPU. A site whose k is below the other's catches up at once. After a site kill, the spare starts at k = 1 and catches up on the next tick.
- **The tip moves.** The `linux` network starts at the tip 220 first-parent commits back. Every request names the tip's position from a clock of 10 merges a minute (`tipPerS` 1/6), so all shards move alike. Each shard's test merges and merge refs use that tip.
- **Composed misses with renewal every 20 tip moves.** Two implementations ran:
  - **r3 (per-miss increment):** each miss diffs the PR's merge ref against the base tip and packs the difference. The base is renewed every 20 moves by composing it with the increment.
  - **r4 (per-move parts):** a background worker (nice 19, outside the slot queue) builds one part per tip move and precomputes the trailer's hash state. Every 20 moves it folds base and parts into a new base, with no delta search. A miss sends base, parts, and a pack of the merge ref's own new objects. It hashes only that last pack.

  A local check on the real Linux network (base, two parts, merge commit) passed: `index-pack` accepted it, `fsck` was clean, and the checked-out tree equals the tip's. The composed stream took 0.14 s of system CPU.
- **Network source:** a fresh bare clone of torvalds/linux at the same tip as the second pass (`d89e1d18`). It is one 6.47 GB pack plus a commit-graph, 7.32 GB copied per shard, uploaded in 250 MB byte parts that the shard joins. The second pass used 31 packs. A repack to 250 MB packs was abandoned after it used 26 GB of temporary disk. git/git is unchanged.
- Hysteresis is still accelerated (30 s up, 120 s down), and labelled as such. Phases, rates, 12 load clients, and the API guard are as in the second pass.

**Run `b0-34-r3`** (per-miss increment; 20 min):

- **The adopted sizing rule did what it was for.** Both warm sites went 1 → 2 → 3 together in the first 2 minutes (site a CPU 0.94 to 0.98, b at 0.0, so b was sized from a's load). At the kill (420 s), b already had k = 3 warm. The spare c started at k = 1 and asked the pool for 3 on its first tick (461 s).
  - Monorepo TTFB p99, hits excluded: 9.35 s in phase 1 (a start-up transient), 5.90 s in the kill window, against 20.1 s in the second pass.
  - Human push p99: 1.38 s in phase 1 and 1.69 s in the kill window. Queue merges p99: 1.49 and 0.99 s.
- **It failed later, on the miss implementation.** As the tip moved, the diff from the base tip to the merge ref grew. From 600 s, linux misses cost 6.4 to 11.5 CPU-s each, against 1.4 to 1.8 before. Each shard renewed its own base (12 to 28 CPU-s per renewal from move 100 on, at 1,600 to 6,200 changed paths per 20 moves). Shards that joined late renewed from move 0. Site b saturated: k went to 7, with owner-budget denials from 600 s, and agent work was shed (389 pushes, 119 misses).
  - Human push p99: 17.2 and 21.4 s in the last two phases. Merge p99: 16.8 and 25.1 s. Miss TTFB p99: 40 to 44 s.
  - The small networks were not affected: `gitnet` and `hostile1` to `3` had pushes and misses under 1 s, apart from `hostile2` sharing b's base shard.
- **Clone-cache hits** were not measured: every hit errored, because `base.pack` went up as byte parts and the hit read a key that did not exist (2,374 errors). This is fixed for r4, where a hit reads the first part.
- **Checks:** k at most 7, 0 API calls, and the monorepo's k was never reduced by another owner. The hostile owner's `hostile2` got k = 2 and then "pool full". The monorepo held 4 of each site's 7 slots plus its base.

r3 is kept as evidence that a composed miss must not diff against a base that is 20 moves stale, and that each of k shards renewing its own base multiplies the renewal cost by k. r4 fixes both.

**Run `b0-34-r4`** (per-move parts; 20 min):

| Phase (s) | Clone TTFB p99, hits and misses | Hits only p99 | Agent miss TTFB p99 | Human push p99 | Queue merge p99 | Agent push p99 | Linux k (b, c) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Journey 9 (0 to 420) | **0.75 s** | 0.63 s | 0.99 s | 0.58 s | 0.49 s | 0.64 s | 1, 1 (a 1) |
| Site a killed (420 to 600) | **1.04 s** | 0.63 s | 1.51 s | 1.50 s | 0.67 s | 1.44 s | 1, 1 |
| Second network (600 to 840) | 26.5 s | 0.68 s | 27.3 s | 7.8 s | 5.5 s | 20.5 s | 1 → 3 |
| Hostile owner (840 to 1,200) | 59.3 s | 0.68 s | 59.7 s | 36.3 s | 50.9 s | 20.4 s | 3 → 7 |

- **Per-move parts cost little while they keep up.** One part cost 0.3 to 0.8 CPU-s, or 2 to 2.3 CPU-s for a 400 to 650 object move. A fold of base and 20 parts cost 1.2 to 1.7 CPU-s. A composed miss cost 0.25 CPU-s at the mean (0.35 at p50), against 0.7 in the second pass and 1.4 to 1.8 in r3. With the load on one shard, NetworkLoad CPU was 0.3 to 0.45, so k stayed 1 at both sites, which is correct under `fanout`.
- **The kill window passed for the first time.** With b's single base shard warm, it took all of journey 9 at 0.57 to 0.64 CPU with an empty queue. Clone TTFB p99 was 1.04 s and human push p99 1.50 s, against 20.1 s and 1.48 s in the second pass. Spare c's base rehydrated in 144 s and was ready at 566 s.
- **Phases 3 and 4 failed on late-joining shards, a harness bug.**
  - From 720 s, b's base shard ran at 0.9 to 0.98 CPU. The kill had doubled its load (0.6), and the second network's `gitnet` agents add their own. `fanout` then raised k on b and c together: 2 at 761 s, 3 at 821 s, 5 at 881 s, 7 at 941 s, with 16 owner-budget denials at k = 7.
  - Each new pool shard rehydrated 7.3 GB in 96 to 166 s, then started serving from the base at move 0, while its niced worker built one part covering 150 to 180 moves.
  - Misses on those shards diffed the merge ref against a 150-move-stale tree: 17 to 75 CPU-s each (median per minute 0.6 to 2.9 CPU-s, max 75). That saturated b's pool shards at 1.0 CPU, and agent work was shed (531 pushes, 171 misses).
  - The design's shard would not serve a network before it is caught up, so the bed now makes `setup` build the catch-up part before the shard is ready (unniced, with no delta search). That is r5.
- **Checks:** k at most 7 (6 pool slots, then owner budget), 0 API calls, and the monorepo's k never reduced by another owner. The hostile owner got k = 2 for `hostile2`, then "pool full". `gitnet` and the hostile networks had p99 0.47 to 1.06 s throughout.

**Run `b0-34-r5`** (per-move parts, and a shard catches up to the tip in `setup` before it is ready; 20 min):

| Phase (s) | Clone TTFB p99, hits and misses | Hits only p99 | Agent miss TTFB p99 | Human push p99 | Queue merge p99 | Agent push p99 (shed) | Linux k (b, c) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Journey 9 (0 to 420) | **0.74 s** | 0.73 s | 0.75 s | 0.66 s | 0.45 s | 0.71 s (0) | 1, 1 (a 1) |
| Site a killed (420 to 600) | 1.10 s | 0.69 s | 1.58 s | 1.08 s | 0.55 s | 1.33 s (0) | 1, 1 |
| Second network (600 to 840) | 25.0 s | 0.65 s | 26.5 s | 10.0 s | 10.6 s | 20.4 s (101) | 1 → 2 |
| Hostile owner (840 to 1,200) | **0.85 s** | 0.72 s | 0.98 s | 0.84 s | 2.67 s | 6.53 s (0) | 2, 2 |

- **Catch-up at setup fixed r4's late joiners.** The two pool shards that joined at 857 and 915 s served from their first request. k stayed at 2 at both sites through the hostile owner's phase, at 0.2 to 0.5 CPU with empty queues. TTFB p99 there was 0.85 s and human push p99 0.84 s.
- **The kill window:** b's single base shard carried all of journey 9 at 0.45 to 0.51 CPU with no queue. Clone TTFB p99 was 1.10 s (misses 1.58 s, hits 0.69 s) and human push p99 1.08 s.
- **Phase 3 spiked for about 100 s, from 700 to 800 s.** TTFB p99 per 20 s was 0.6 to 1.1 s before 700 s, then 22.1, 32.1, 20.7, 13.4, 2.3, and 3.3 s, then 0.58 s from 820 s. Excluding 700 to 820 s, phase 3's TTFB p99 is 0.67 s. The cause:
  - Tip move 119 is a real merge (`mm-stable`) that adds 2,346 objects. Its part cost 10.2 to 10.5 CPU-s.
  - On b's base shard, at 0.86 to 1.0 CPU after the kill plus `gitnet`'s start, the niced worker took 42.7 s to build it (10.4 s on c).
  - Meanwhile every miss diffed against move 118 and packed those 2,346 objects itself, which kept the shard saturated (queue 170). `fanout` added a pool shard at 841 s, after the 30 s accelerated window.
  - So the worker's priority caused a positive feedback loop. The part must be built at normal priority, or once per network instead of once per shard (next run).

### B0-30: per-table sizes

They were not kept. `b0-30-r1` logged `databaseSize` and row-byte totals after each step (`results/raw/wp9-b0-30-r1.jsonl`), not `dbstat` per table. Its Durable Object was deleted at the end of the second pass, so the split cannot be read now. What the totals give, per row:

| Rows | Count | Mean row bytes | On disk per row | Factor | How |
| --- | --- | --- | --- | --- | --- |
| Metadata tier (issues and comments, `INTEGER PRIMARY KEY` and three or four columns) | 2,222,223 | 1,766 B | 2,056 B (+289 B) | 1.16 | `databaseSize` 4.57 GB after seeding, over 3.93 GB of row bytes, nothing else in the database |
| Agent rows (PR 2 KB, task 1 KB, check 300 B, provenance 150 B, `PathSet` 16 KB) | 3,630,500 | 350 B | about 502 B (+152 B) | about 1.43 | Day 30's 8.45 GB minus the seeded 4.57 GB minus the log's 2.06 GB of row bytes, over 1.27 GB of agent row bytes |

The 1.43 is an upper bound for the agent rows alone. The subtraction charges them with the log table's own page overhead, which the run did not measure separately: the log was counted in row bytes, 2.06 GB, not in pages. The per-row overhead is close for both kinds, 150 to 290 B. That is page headers, cell pointers, record headers, and free space left by deletes. So the factor is large for small rows and small for large ones. A sizing model should add about 200 B a row rather than multiply by one factor [inference, from two points]. A rerun that reads `dbstat` per table (`SELECT name, SUM(pgsize) FROM dbstat GROUP BY name`, if the Durable Object SQL API allows it) would settle the split. It was not rerun in this pass.

**Run `b0-34-r6`** (r5 with the part worker at normal priority; 20 min):

| Phase (s) | Clone TTFB p99, hits and misses | Hits only p99 | Agent miss TTFB p99 | Human push p99 | Queue merge p99 | Agent push p99 (shed) | Linux k (b, c) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Journey 9 (0 to 420) | 1.42 s | 0.69 s | 4.22 s | 0.65 s | 0.63 s | 1.85 s (0) | 1, 1 (a 1) |
| Site a killed (420 to 600) | 1.54 s | 0.69 s | 1.82 s | 1.76 s | 0.73 s | 1.66 s (0) | 1, 1 |
| Second network (600 to 840) | 14.1 s | 0.70 s | 15.6 s | 10.9 s | 7.6 s | 15.7 s (0) | 1, 1 |
| Hostile owner (840 to 1,200) | 4.21 s | 0.66 s | 5.19 s | 1.08 s | 1.14 s | 5.34 s (0) | 1, 1 |

TTFB p99 per minute (s): 1.4, 6.2 (start-up), 0.7, 0.8, 0.7, 1.0, 0.7, 1.0 (kill), 1.4, 1.8, 1.0, **16.1, 17.6**, 1.9, **8.1, 3.8**, 0.7, 0.6, 0.8, 1.0.

- **The worker kept up, but the misses still paid for the big moves.**
  - At normal priority the part for move 119 (2,346 objects, 10.3 CPU-s) took 13.8 s on b, against 42.7 s in r5.
  - In those 13.8 s, and again for move 142 (2,268 objects), each miss composed against the last built tip and packed the same objects itself, at 8.7 to 11.4 CPU-s a miss. b's base shard went to 0.92 CPU with 69 queued for about 40 s. It recovered without `fanout`, because the load stayed above 0.75 for under the 30 s window (0.75 at 749 s does not count).
  - Spare site c gets few requests of its own (only the clients near it), so its tip jumps several moves at a time and its parts cover 2 to 37 moves (up to 17.5 CPU-s). Misses on c during those builds cost 17 to 19 CPU-s. c's base took 208 s to rehydrate 7.3 GB and was ready at 630 s.
- **Shards fixed the problem without the pool:** k stayed at 1 everywhere, and no agent work was shed. The minute p99 was under 1.1 s in 13 of 20 minutes and over 5 s in 3.

**What limits B0-34's clone TTFB on the adopted rules.** The design's sentence "the shard renews the base when the tip has moved 20 commits" leaves two gaps open:

1. **Between moves:** a miss that arrives before the shard has packed the newest tip move must pack it itself. On Linux a single merge can add 2,300 objects and cost about 10 CPU-s. Every concurrent miss pays that, and the shard saturates for as long as the part takes to build. r3 (a per-miss diff against a base up to 20 moves old) and r5 (a starved worker) are the same failure, larger.
2. **A shard behind the tip:** a shard that joins late, or that sees the tip only through its own requests, serves from a stale base until it catches up (r4: 17 to 75 CPU-s misses).

What closes them [inference from r4 to r6]:

- (a) Each tip move's part is built once per network, by the shard that took the merge, and stored beside the clone cache's tip pack, so other shards and the second site fetch it instead of rebuilding it.
- (b) A miss waits for the part of the move in progress (single flight), or is served against the last built tip, as GHES serves `refs/pull/N/merge` from its last computed test merge.
- (c) A shard is ready for a network only once it has caught up to the tip. r5 and r6 showed that (c) alone removes r4's failure.

**Proposed design edits (third pass, B0-31).**

22. **`design/DESIGN.md` and `design/AGENT-LAYER.md` (agent block, byte-identical), the `INLINE_MAX_HOT` comment.** After "...and queued agent packs waited 21 to 30 s and were refused anyway [measured, `tests/m0/results/m0-wp9.md`, B0-31]." add: "Under this rule 4,998 of 5,000 such agent pushes were refused at once (client p99 0.33 to 0.43 s, never queued), 0 of 546 Human and MergeQueue pushes up to 1 MiB were refused, and an apac leader no longer stepped down [measured, B0-31, third pass]." Evidence: `results/raw/wp9-b0-31o-nop4.summary.json`, `-holdnop4.summary.json`. The rule holds as adopted.
23. **`design/DESIGN.md`, Milestone 0 table, B0-31 row, and `build/BUILD-PLAN.md` B0-31.** The gate "Commit p99 at most 1 s" does not hold during the outage on `Tri` with an apac leader. Human and MergeQueue p99 was 1.83 and 1.97 s, and all classes 1.68 and 1.97 s. The tail is the round of the 6% of entries that hold a 0.5 to 1 MiB Human or merge pack. Either split the row: "commit p99 at most 1 s with buckets up; while placement fails, Human and MergeQueue commit p99 at most 3 s and 0 refused". Or keep 1 s and accept a fail value at about 2 s. The fail line of 3 s held over all commands; merges alone reached 2.99 s in one run. Operator decision.
24. **`design/DESIGN.md`, Shape, `STEP_DOWN` doc comment.** Change "An apac leader carrying 1 MiB inline entries misses CALL_DEADLINE to both far followers and steps down within 10 s by the commit-error rule" to "...steps down within 10 s by the commit-error rule when every entry carries one; under the outage rule, with only Human and MergeQueue packs inline (6% of entries), it held for 600 s [measured, B0-31, third pass]". The row's "leader on apac" half can now pass as the design has it. `hold-` is not needed.

**Run `b0-34-r7`** (r6, plus a composed miss builds the merge ref on the last tip the shard has packed, outside the design; 20 min):

| Phase (s) | Clone TTFB p99, hits and misses | Hits only p99 | Agent miss TTFB p99 | Human push p99 | Queue merge p99 | Agent push p99 (shed) | Linux k (b, c) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Journey 9 (0 to 420) | **0.70 s** | 0.70 s | 0.62 s | 0.63 s | 0.57 s | 0.73 s (0) | 1, 1 (a 1) |
| Site a killed (420 to 600) | **0.71 s** | 0.71 s | 0.74 s | 0.84 s | 0.46 s | 1.17 s (0) | 1, 1 |
| Second network (600 to 840) | **0.95 s** | 0.75 s | 1.05 s | 1.14 s | 0.68 s | 1.75 s (0) | 1, 1 |
| Hostile owner (840 to 1,200) | 1.03 s | 0.81 s | 1.06 s | 0.95 s | 0.65 s | 1.75 s (0) | 1, 1 |

- **Every phase is at or near 1 s.** Over the whole run, clone TTFB p99 was 0.93 s across 9,110 clones. Per minute, the p99 was 0.60 to 1.26 s, over 1 s in 4 of 20 minutes (1.02 to 1.26 s). Linux alone (hits and misses, 4,228) was 1.09 s.
- **The kill window and the big moves are flat.**
  - The kill window (TTFB 0.71 s, human push 0.84 s): b's base shard carried all of journey 9.
  - Move 119 (2,346 objects) and move 142: b's shard stayed at 0.6 to 0.82 CPU with at most 2 queued, because no miss packed a tip move. Linux misses cost 0.20 to 0.52 CPU-s on the mean.
- **What is left over 1 s:**
  - Linux misses under the hostile owner, p99 1.46 s (p50 0.37 s). Linux shares b's base shard with `hostile2`, whose 3 misses/s and pushes arrive there too, and b's base ran at 0.56 to 0.78 CPU in that phase.
  - Network transfer from b (weur) to clients in enam and apac.
  - `fanout` never raised k for Linux, because CPU stayed under 0.75. So 0.75 CPU on one shard gives about 1 s at p99. An up threshold of 0.6 would hold the p99 under 1 s [inference].
- **Errors:** 92 small-network clones got "no shard": requests in the 10 to 30 s after `gitnet` and the hostile networks were created and before their first shards were ready. 7 linux misses errored with "container not running" at the kill.
- **Checks:** k at most 2 (`hostile2` on b and c, judged from the shared shard), 0 API calls, 0 owner-budget or pool denials, and the monorepo's k never reduced by another owner. The pool was never contended, because Linux never needed more than its base.

**Run `b0-34-r8`: the design's hysteresis** (r7's bed, 20 min, phase 1 only: journey 9's Linux git work at 3 times its rate, 12.5 agent pushes/s and 4.5 misses/s, with hits unchanged; `fanout` up above 0.75 for 180 s and down below 0.4 for 3,600 s, as the design has it). The scaling is needed because at 1× a single base shard never passes 0.75 CPU (r4 to r7), so k never moves.

- **How long k takes to reach steady state.**
  - Site a's base shard passed 0.75 CPU at about 60 s and held 0.92 to 0.95. `fanout` raised k to 2 at both sites at 241 s: 180 s above the line, plus the 10 s tick and the first sample.
  - The new pool shards copied 7.3 GB and caught up to the tip in 101 to 161 s, and were ready at 293 s (b) and 353 s (a).
  - So from overload to k shards serving took about **290 s**, and TTFB p99 per minute was 5.3 to 9.9 s for the first 6 minutes.
  - With k = 2 serving, a's shards ran at 0.67 to 0.91 CPU, still above 0.75 most of the time. TTFB p99 per minute was 0.95 to 3.2 s. k went to 3 at 1,022 s, the next 180 s window that held, and those shards were ready at 1,185 s and 1,226 s, near the end of the load.
  - No owner-budget or pool denials, and 0 API calls. b followed a's k throughout (`alike`), at 0.0 to 0.09 CPU.
- **Errors:** 267 client errors, all "internal error" from the Workers runtime on calls to the shard object, between 384 and 719 s, at the 3× rate (about 25 requests a second to one object). The 660 to 720 s minute had TTFB p99 12.2 s and human push p99 26.3 s: 65 pushes and 12 misses errored there, and others waited. The bed does not show whether this was the object or the platform. The rows are kept.
- **What it shows about the design's values.** With 3 minutes up and a 7.3 GB copy, a monorepo shard that saturates gives clones about 5 minutes at 5 to 10 s TTFB before the next shard serves. Each later step takes another 3 minutes plus a copy. The design's comment says k reaches steady state "with hysteresis", but at these values steady state takes 15 to 20 minutes for k = 3. That is too slow for a fleet that ramps in one step. A pool shard that keeps the network warm (an LRU hit, so no copy) would cut each step to the 180 s window [inference].

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-34 (third pass, adopted rules) | Scaled down as in the second pass (Linux 7.3 GB and four git/git networks, up to 9 `standard-4` shards a run, 12 clients, 20 min), with accelerated hysteresis (30 s up, 120 s down) unless named. Six runs (r3 to r8). With per-move parts, catch-up before ready, and misses built on the last packed tip (r7): clone TTFB p99 0.70, 0.71, 0.95, and 1.03 s by phase (0.93 s over the run), human push p99 0.63, 0.84, 1.14, and 0.95 s, queue merges at most 0.68 s, 0 agent work shed. k at most 2, 0 API calls, monorepo k never reduced. With the design's rule as written (renew every 20 moves, misses on the current tip; r3 to r6), TTFB p99 reached 14 to 59 s in the second-network and hostile phases, when big tip moves and late-joining shards made misses cost 10 to 75 CPU-s. With design hysteresis at 3× load (r8): about 290 s from saturation to k = 2 serving, TTFB p99 5 to 10 s meanwhile | k in band, at most 8 per site; hostile owner held to its budget; human push p99 at most 3 s beyond transfer; clone TTFB p99 at most 1 s throughout; fail on k over 8, any API call, the monorepo's k reduced by another owner, or TTFB p99 over 5 s | **Partial pass, scaled down.** The kill window now passes (0.71 s in r7, 1.04 to 1.10 s in r4 and r5, against 20.1 s in the second pass): sizing both warm sites alike works. The k, API, owner, and human-push checks pass in every run. TTFB p99 at most 1 s throughout passes in 3 of 4 phases of r7 and misses by 0.03 s in the fourth, and only with two changes outside the design (catch-up before ready, misses at the last packed tip). On the design's rule as written it fails (r3 to r6: over 5 s in phases 3 and 4). With the design's 3-minute hysteresis, a saturating step in load gives about 5 minutes over 5 s |

**Modeled versus real (third pass).** As in the second pass, plus:

- The tip moves on a clock (10 merges a minute) along real Linux first-parent history, not by merges the queue commits. All shards share the clock.
- Each shard builds its own parts. The design would build them once per network.
- Pool shards are new containers that copy the network, never LRU hits.
- `WP9_MISS_AT_BUILT` and catch-up-before-ready are the bed's, not the design's.
- r8 scales the Linux streams by 3 to make the base shard pass 0.75.

**Proposed design edits (third pass, B0-34).**

25. **`design/DESIGN.md`, Shape, "Git cache shards (derived)", the composed-miss sentences.** Replace "The shard renews the base when the tip has moved 20 commits." with: "For each tip move, the shard that took the merge packs the new tip's objects the base and earlier parts lack into one part, stored beside the clone cache's tip pack, and every 20 moves folds base and parts into a new base. A miss composes base, parts, and a pack of the wanted commit's own new objects, and builds `refs/pull/N/merge` on the last tip whose part exists, so a miss never packs a tip move. A shard serves a hot network only once it holds the parts up to the tip. On torvalds/linux a part cost 0.3 to 2 CPU-s, up to 10 CPU-s for a 2,300-object merge, a fold 1.2 to 1.7 CPU-s, and a miss 0.2 to 0.5 CPU-s; with misses on the current tip, each miss during a big move's part cost 9 to 19 CPU-s and saturated the shard [measured, `tests/m0/results/m0-wp9.md`, B0-34, third pass]." Evidence: `results/raw/wp9-b0-34-r3` to `-r7`.
26. **`design/DESIGN.md`, Shape, `NetworkLoad` and `fanout` comment.** Replace "In B0-34 it held k = 1, and while new shards rehydrated 8.76 GB each, for 166 to 207 s, miss time to first byte was 19 to 20 s at p99" with "With both warm sites at one k, a site kill kept clone time to first byte p99 at 0.71 to 1.10 s and human push p99 at 0.84 to 1.50 s [measured, B0-34, third pass]".
27. **`design/DESIGN.md`, `fanout` constants.**
    - Change "up above 0.75 for 3 min ... [guess]" to "up above 0.6 for 1 min, down below 0.3 for 1 h [measured, B0-34]".
    - Add: "At 3 min up and a 7.3 GB rehydrate, a saturating step in load waited 290 s for k = 2 to serve, with TTFB p99 5 to 10 s meanwhile; one shard at 0.75 CPU already gives TTFB p99 about 1 s."
    - Alternatively keep the hysteresis and have the hot pool keep the network warm on idle pool shards (LRU), so a step needs no copy.
    - Operator decision. The run did not test 0.6 or 1 min. They are the inference from r7 (0.6 to 0.82 CPU and p99 0.93 to 1.26 s a minute) and r8.
28. **`design/DESIGN.md`, Milestone 0 table, B0-34 row.** Add to the method: "with the tip moving at 10 merges a minute along real history". At a fixed tip the composed miss looks free, but the cost is in the moves. Keep the gate.
29. **`design/DESIGN.md`, Scale "Agent fleet, git cache CPU for one network".** Add the part builds: about 0.1 vCPU per shard at 10 moves a minute on Linux (0.5 CPU-s mean per move), or once per network under item 25.

**Third-pass summary.**

| Row | Verdict | Key numbers |
| --- | --- | --- |
| B0-31 (outage rule) | **Pass on the rule; commit p99 at most 1 s fails in both halves** | 0 of 546 Human and MergeQueue pushes up to 1 MiB refused. 4,998 of 5,000 agent 1 MiB pushes refused at once in the `ng` shape (client p99 0.33 to 0.43 s, never queued). Human and MergeQueue commit p99 1.83 s (design `STEP_DOWN`, leader held, 0 changes) and 1.97 s (`hold-`); merges up to 2.99 s |
| B0-34 (adopted rules) | **Partial pass, scaled down** | Kill window TTFB p99 0.71 s (was 20.1 s). With per-move parts, catch-up before ready, and misses on the last packed tip (r7), TTFB p99 0.70 to 1.03 s by phase and human push p99 at most 1.14 s. On the rule as written (r3 to r6), TTFB p99 14 to 59 s in later phases. Design hysteresis: 290 s to k = 2 |
| B0-30 per-table | Not kept | Tier +289 B a row (×1.16), agent rows +152 B a row (×1.43 upper bound), from totals |

## Teardown (third pass)

`node run/wp9.ts delete` removed `graft-wp9-a/b/c` and `graft-wp9-load` and emptied and deleted the buckets `graft-wp9-stage` and `graft-wp9-stage-c`. `node run/wp9-git.ts delete` removed `graft-wp9-git`. Its container application `graft-wp9-git-shard9` was deleted with `wrangler containers delete`. The bucket `graft-wp9-git` (48 objects, 7.68 GB) was emptied object by object and deleted. Each run stopped its containers at the end (`/stop`).

Checked on 2026-10-30 after teardown:
- every `graft-wp9-*` script answers 404;
- `wrangler r2 bucket list` shows no `graft-wp9` bucket;
- `wrangler containers list` shows no `graft-wp9` application;
- no `wp9-*` orb service is running.

The orb-local clones (torvalds/linux at `d89e1d18`, git/git) and their split parts were deleted. Nothing with the `graft-wp9` prefix is deployed. To rerun B0-34, clone the two repositories into `/home/user/wp9-data/linux.git` and `git.git`, then run `node run/wp9-gitprep.ts`, `node run/wp9-git.ts deploy`, and `node run/wp9-git.ts run --run X --mode compose --alike --tip-per-s 0.1667`.


## Fourth pass (2026-10-30): the new `STEP_DOWN` rules

**What changed in the bed.** The design changed three `STEP_DOWN` rules. The reference implementation in `src/raft/member.ts` and `src/raft/types.ts` was ported into `src/wp9/member.ts` and `src/wp9/types.ts`. Those files were not copied whole, because the wp9 files carry the admission, inline-pack, feed, and learner additions. `src/raft/` was not edited.

- **Confirm rule:** `STEP_DOWN` trips on confirm latency only with at least `confirmSlowMin` = 6 confirmations over 500 ms in the 10 s window. Before, it took the window's p99, which is its maximum under 100 confirmations. A group named `sdN-` still overrides N, for comparison only.
- **Transfer target:** a `STEP_DOWN` transfer never targets a follower whose own `stepped_down_at` is within `RESET_QUARANTINE`. Every AppendEntries reply, ok or refused, now carries it. Among the rest, the leader prefers the follower whose calls fail less, then the more caught up.
- **Heard followers:** a new leader counts a follower's failed calls only after that follower has answered an AppendEntries in its term.

`INLINE_MAX_HOT` was already 64 KiB in the prototype (third pass). The bed was redeployed as `graft-wp9-a/b/c` and `graft-wp9-load`. Runs are in `results/raw/wp9-b0-33-p4r*`, `wp9-b0-31-p4up2`, `wp9-b0-31o-p4nop*`, and `wp9-b0-29-p4r*`.

### B0-33 (5,000 agents, measured capacity, design rules)

Each run is 300 s with 60 clients, 252 agent commands/s offered plus the user token at 100 a minute, and no feed polls or agent reads, as in the first pass. The gate now reads: Human and MergeQueue p99 at most 1 s, at least 140 agent commands/s admitted, the user token classed `Agent`, every refusal in the `ng` or 429 shape, 0 leader changes, and 0 `suspect`.

Six runs were made. Runs `p4r1` to `p4r3` ran on the first deploy of the fourth pass. That deploy had a harness bug: the member's `outcome` raced a 35 s `sleep` and never cleared the timer. At about 255 commands/s that left about 9,000 live timers on the leader, and in `p4r3` the leader hit `QuotaExceededError: active timeouts` at 290 s. The timer is now cleared. Runs `p4r4` to `p4r6` ran on the fixed deploy and are the gate runs. The first three are kept as evidence: until 290 s the bug had not yet bitten.

| Run | Members (leader first) | Leader changes | `suspect` received | Human+MQ p99 (from 30 s) | Agent committed/s | User token as Agent after 1st min | Refusals (`ng`, 429, other) |
|---|---|---|---|---|---|---|---|
| p4r1 | ATL, MRS, SIN | 0 | 6 | 1,492 ms (1,148) | 252.6 | 399 of 399 | 0, 0, 0 |
| p4r2 | EWR, AMS, HKG | 2 (elections) | 3,708 | 3,707 ms (1,449) | 240 | yes | 0, 0, 0 |
| p4r3 | EWR, CDG, KIX | 1 (timer bug, 293.7 s) | 1,334 | 1,310 ms (1,414) | 252 | 390 of 399 | 0, 0, 0 |
| **p4r4** | MIA, MAD, SIN | 2 (`STEP_DOWN`, 266 s and 278 s) | 6,188 | 3,731 ms (4,301) | 242.8 | 343 of 399 | 22, 121, 0 |
| **p4r5** | ATL, LHR, ICN | 1 (`STEP_DOWN`, 142 s) | 1,938 | 6,877 ms (8,702) | 239.5 | 332 of 399 | 166, 391, 0 |
| **p4r6** | ATL, MRS, SIN | 6 (from 11 s) | 27,959 | 7,556 ms (6,772) | 28.8 | 0 of 399 | 544, 5,432, 0 |

In the table, Human+MQ is the Human and MergeQueue commit p99 at the leader. The 429s are the API's secondary rate limit on agent side commands. The user token column counts the token's writes after its first minute that were classed Agent.

**What each fixed-deploy run did.**

- **p4r4.** It was steady for 240 s. Commit p99 was 1.4 to 1.8 s per 30 s window (p50 about 0.9 s), with 2.15 entries/s of p50 445 KB.
  - At 265.9 s, a stepped down on commit errors (0.06 and 0.23) and moved the lead to b.
  - At 277.5 s, b stepped down on commit errors (0.36 to a, 0.60 to c). a was inside `RESET_QUARANTINE`, so the new target rule left c (SIN) as the only target, although c's link was the worse one.
  - c took the lead and was at once `Durable Object is overloaded` for the rest of the run: 6,404 commit deadlines and 4,008 overload errors.
- **p4r5.** a stepped down at 141.9 s on commit errors (0.08 and 0.15) and moved the lead to b. The window around it had 835 client errors and Human+MQ p99 4.5 s. b stepped down at 294.4 s on 0.09 and 0.29; that transfer was abandoned after the load ended.
  - Leader a saw a 2,349 ms storage confirmation. One slow write no longer trips the confirm rule, which needs 6, so a held.
  - In quiet windows, Human+MQ p99 was 0.7 to 1.9 s.
- **p4r6.** Calls to leader a (ATL) missed `CALL_DEADLINE` from 3.3 s into the load. Clients sent `suspect` reports, and b took the lead in term 3. b was `Durable Object is overloaded` from 17 s on, and the group went through six leaders by term 8.
  - From 90 s to 210 s almost nothing committed.
  - From about 225 s on, the load clients also hit the Workers limit `Too many API requests by single Worker invocation`. That limit is the harness's, not the member's.

**What limits B0-33 on this bed.**

- **The leader object's request rate.** At 252 agent commands/s plus 3.8 MB/s of inline packs, the leader object serves 750 to 760 requests/s: commits, outcomes, reads, and AppendEntries. This happened in every run.
- **What happens past that rate.** A cold leader, or one that has just taken over, sometimes cannot hold the rate. The runtime then sheds with `Durable Object is overloaded` (p4r2, p4r6, and c in p4r4). Each shed sends a burst of `suspect` reports and starts an election.
- **The commit-error rule.** It still moves the lead when AppendEntries to a far follower misses the 1 s deadline under 0.4 to 1.5 MB entries (p4r4, p4r5).
- **The new rules removed one cause.** No run in the fourth pass stepped down on the confirm rule. They did not remove the other two. In p4r4, the quarantine rule sent the lead to the only remaining follower, which was also the worse link.

**Verdict.**

- **Admission passes:** at least 140 agent commands/s admitted in five of six runs (239.5 to 252.6/s), every refusal in the `ng` or 429 shape, and the user token classed Agent after its first minute.
- **The leadership half fails** in all three gate runs: 1 to 6 leader changes, 1,938 to 27,959 `suspect` reports, and Human and MergeQueue p99 3.7 to 7.6 s. p4r1 is the only run with 0 changes, and it still had 6 `suspect` reports and Human+MQ p99 1.49 s.
- Even without leader changes, Human and MergeQueue p99 is 1.1 to 1.9 s at this mix. A Human or MergeQueue command commits in the same entries as about 84 to 96 agent commands and 450 KB of inline packs. The 1 s line therefore needs either fewer inline bytes per entry or a lane that commits Human and MergeQueue ahead of the agent batch.

### B0-31 (leader on apac, every agent push 1 MiB, `INLINE_MAX_HOT` 64 KiB)

The gate is now split. With buckets up, commit p99 must be at most 1 s. While placement fails, Human and MergeQueue p99 must be at most 3 s with none of them refused, and agent pushes over the cap must be refused at once.

**Buckets up, run `b0-31-p4up2`** (members EWR, AMS, SIN, with leader c on SIN, apac; the design's `STEP_DOWN`; 600 s; learners on a and b).

- **The apac leader stepped down at 9.9 s,** on commit errors of 0.60 and 0.70. The first 1 MiB packs were then staging and AppendEntries to EWR and AMS missed the deadline. The lead moved to a (EWR), which won term 5 at 11.3 s and held it for the rest of the run, with 0 changes. 286 `suspect` reports came around the transfer.
- **The confirm rule held:** a saw one storage confirmation of 2,166 ms among 5,078. In the third pass that would have moved the lead.
- **Every agent push was over the 64 KiB cap,** so all 2,499 were staged in R2 (stage p50 295 ms, p99 870 ms) and committed as small entries (p50 9.9 KB, 8.47 entries/s).
- **Commit p99 at the leader was 547 ms** over the whole run and 479 ms from 30 s on (p50 168 ms). By kind:
  - Human 547 ms, merges 416 ms (100 of 100, all in the next entry), agent pushes 492 ms.
  - 2,488 of 2,500 pushes committed; 3 came back `Stale` and 9 failed client-side, all around the transfer.
- **The leader carried 331 req/s.** Learners answered 45.4 feed polls/s, and the leader answered none.
- **Client-side push p99 was 7.8 s.** The client times the stage plus the 1 MiB upload from the load object plus the commit, and its tail includes the transfer.

This half passes on the design's rule: commit p99 547 ms, at most 1 s. It passes because the leader leaves apac in the first 10 s. With the leader held on apac at 64 KiB (`b0-31-holdih64`, second pass), commit p99 was 6.7 s over the run, from the first inline 1 MiB entries, and 885 ms after 30 s. `hold-` was not rerun on the new rules.

**Placement failing, run `b0-31o-p4nop2`.** Members were EWR, MRS, SIN, with leader c on SIN. The run used the design's `STEP_DOWN` and the outage rule, with placement failing for 600 s. Learners were a and b. Human pushes ran at 0.3/s and merges at 10 a minute, each pack 64 KiB to 1 MiB and inline.

- **Agent pushes:** 2,498 of 2,500 agent 1 MiB pushes were refused in the `ng <ref> agent admission full, retry after <n>s` shape. At the client, refusals took p50 272 ms, p99 856 ms, and at most 4.0 s, including the leader changes. The other 2 were client errors.
  - The final leader counted 2,413 refusals with cause `outage`. The rest were refused by c and b before their terms ended.
  - Queued agent work peaked at 25 commands and 28 KB, all side commands. No push was queued.
- **Human and MergeQueue:** 0 refused. 182 Human pushes committed, and 1 came back `Stale`. Merges were 100 of 100 ok with 0 `Stale`.
  - Commit p99 at the leader was 1,252 ms for the two together over the run, and 1,196 ms from 30 s on.
  - By kind: Human pushes p99 1,346 ms, merges p99 2,585 ms, Human side commands p99 1,022 ms.
  - The 2,585 ms merge was the first one, admitted at 0 to 5 s while c's appends to both followers were missing `CALL_DEADLINE`.
- **Leadership:** c stepped down at 7.4 s on commit errors of 0.40 and 0.07, and b (MRS) took term 4 at 7.8 s. a was still promised to c, so it refused b's vote. At 21.5 s a won term 5 in an election, not a `STEP_DOWN`, and it held to the end. These were 2 leader changes and 58 `suspect` reports, and the gate's placement half does not count them. a then carried 326 req/s, and the learners answered every feed poll. a's appends to c (SIN) missed the deadline about once every 10 s, each time with entries up to 1 MiB, and recovered on the next send.

This half passes: 0 Human or MergeQueue pushes refused, their p99 1.25 s against 3 s, and every over-cap agent push refused at once, never queued. The earlier `b0-31o-p4nop` run, disturbed by a concurrent run, gave the same result: 2,492 refused, 0 Human or MergeQueue refused, and p99 929 ms. That run's first merge took 10.6 s across a transfer.

| Half | Run | Result | Gate | Verdict |
| --- | --- | --- | --- | --- |
| Buckets up | `b0-31-p4up2` | Commit p99 547 ms (479 from 30 s); apac leader stepped down at 9.9 s, enam after | Commit p99 at most 1 s | **Pass** (on an enam leader after 10 s) |
| Placement failing | `b0-31o-p4nop2` | 0 of 283 Human/MQ pushes refused, Human+MQ p99 1.25 s, 2,498 of 2,500 agent pushes `ng` at once (p99 0.86 s), 0 queued | Human/MQ p99 at most 3 s, 0 refused, agent over cap refused at once | **Pass** |
### B0-29 (journey 9, design rules)

**Run `b0-29-p4r1`** (ORD, LHR, KIX, leader a; 600 s, 30 clients, learners in slots 1 and 2):

- **Commit:** p99 531 ms, p50 211 ms, max 1,584 ms over 16,500 commands; 519 ms from 30 s on.
- **Merges:** p99 809 ms at the leader and 840 ms at the client, 100 of 100 ok, each in the next entry.
- **Pushes:** 2,500 of 2,500 agent pushes ok, 0 `Stale`.
- **Leadership:** 0 leader changes. 20 `suspect` reports, from two moments when one AppendEntries missed its deadline (52.8 s and 276.3 s), and 1 client give-up.
- **Load:** the leader served 331 req/s (304 without `outcome`), with 0 feed polls on it. Learners and followers served 43.9 polls/s. Entries ran at 7.1/s, p99 71 KB.
- **The confirm rule:** the leader saw 2 storage confirmations over 500 ms (max 1,052 ms) among 4,272. Under the first pass's rule, one of them would have moved the leader, as in `b0-29-r1` and `-r3`. Under the 6-slow rule it did not.

**Run `b0-29-p4r2`** (ATL, LHR, SIN, leader a; the fixed deploy; same load):

- **Commit:** p99 501 ms, p50 187 ms, max 833 ms; 500 ms from 30 s on.
- **Merges:** p99 369 ms at the leader and 419 ms at the client, 100 of 100 ok.
- **Pushes:** 2,500 of 2,500 ok, 0 `Stale`.
- **Leadership:** 0 leader changes (term 2 throughout) and 2 `suspect` reports.
- **Load:** the leader served 332 req/s, with 0 feed polls on it. Learners and followers served 45.4 polls/s.
- **The confirm rule:** the slowest storage confirmation was 302 ms.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-29 (fourth pass) | Commit p99 531 and 501 ms, merge p99 809 and 369 ms, 0 `Stale`, leader 331 to 332 req/s, 0 feed polls on the leader, 0 leader changes in both runs | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Pass.** Commit p99 is in the first pass's range (396 to 625 ms) on this placement. The new rules did not change it, and they removed the one-slow-write leader change |


### Fourth-pass verdicts

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 | Gate runs p4r4 to p4r6: 1, 2, and 6 leader changes; 1,938 to 27,959 `suspect`; Human+MQ p99 3.7 to 7.6 s; agent admitted 242.8 and 239.5/s in p4r4 and p4r5, 28.8/s in p4r6; user token Agent; every refusal `ng` or 429. Best run (p4r1, timer-bug deploy): 0 changes, 6 `suspect`, Human+MQ p99 1.49 s, 252.6/s | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s admitted, user token Agent, `ng` or 429 shapes | **Fail.** Admission and shapes pass. Leadership and the 1 s line fail in every run |
| B0-31, buckets up | `p4up2`: commit p99 547 ms; the apac leader stepped down at 9.9 s | Commit p99 at most 1 s | **Pass** on the design's rule, which moves the leader off apac |
| B0-31, placement failing | `p4nop2`: 0 of 283 Human+MQ pushes refused, Human+MQ p99 1.25 s, 2,498 of 2,500 agent pushes refused in the `ng` shape at once, none queued | Human+MQ p99 at most 3 s, 0 refused, agent over cap refused at once | **Pass** |
| B0-29 | Commit p99 531 and 501 ms, merge p99 809 and 369 ms, 0 `Stale`, leader 331 to 332 req/s, 0 feed polls on the leader, 0 leader changes | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Pass** |

**What the new `STEP_DOWN` rules changed.**

- **The confirm rule now holds.** In ten runs, no leader stepped down on the confirm rule. Leaders saw single storage confirmations of 1,052 to 2,349 ms. Under the first pass's rule each of those moved the lead.
- **The commit-error rule still fires,** in two cases:
  - an apac leader with 1 MiB entries in its first 10 s (B0-31, both halves);
  - an enam leader under 0.4 to 1.5 MB B0-33 entries to a far follower (p4r4, p4r5).
- **The quarantine target rule made one collapse worse.** In p4r4 it ruled out a, the only healthy target, and moved the lead to SIN. The leader's own calls to SIN had failed 60% of the time in that window, and SIN was overloaded as leader at once.
- **The heard-follower rule never decided a run.** No new leader in these runs stepped down on errors to a follower it had not yet heard from.

**What is modeled and what is real.** As in earlier passes:
- Real: the members, the load clients, R2 staging, storage, and the network between colos.
- Modeled: the packs are random bytes, and `decide` checks only the ref's tip.
- Modeled: the placement outage is a flag in the member (`nop-`), not a real bucket failure.
- The B0-33 load clients hit the Workers per-invocation subrequest limit late in p4r6, after the group had already collapsed. That limit belongs to the harness.

### Proposed design edits (fourth pass)

30. **`design/DESIGN.md`, Shape, `STEP_DOWN` doc comment, the target sentence.** After "then the one with the fewest failed calls in the window, then the most caught up, and holds if none qualifies", add: "A follower whose calls failed in the window at more than `commit_errors` does not qualify. In B0-33 p4r4, a leader whose calls to SIN had failed 60% of the time quarantined its only healthy peer, transferred to SIN, and SIN, overloaded as leader, cost 6,188 `suspect` reports and every remaining commit [measured, `tests/m0/results/m0-wp9.md`, fourth pass]."
31. **`design/DESIGN.md`, Scale "Agent fleet, group commands", and the Milestone 0 table, B0-33 row.**
    - Add: "At 252 agent commands/s with 40 KB inline packs, the leader object serves 750 to 760 requests/s and about 3.8 MB/s. A leader that has just started sometimes cannot, and the runtime sheds with `Durable Object is overloaded`; 3 of 6 runs lost their leader this way or by the commit-error rule [measured, fourth pass]."
    - Make B0-33's 0-leader-change line `mitigated` until a lane or a lower per-group request rate passes it.
32. **`design/DESIGN.md`, Milestone 0 table, B0-33 row, the Human and MergeQueue line.** Even in the run with 0 leader changes (p4r1), Human and MergeQueue commit p99 was 1.15 to 1.49 s. Those commands ride in entries of 84 to 96 agent commands and 450 KB of inline packs.
    - The 1 s line needs one of these: Human and MergeQueue commands commit in their own entry ahead of the agent batch; `INLINE_MAX_HOT` drops for agent pushes once the agent lane is past `AGENT_SHARE`; or the line moves to 1.5 s. This is an operator call.
    - Evidence: `results/raw/wp9-b0-33-p4r1` to `-p4r6`.
33. **`design/DESIGN.md`, Shape, `STEP_DOWN` doc comment, the B0-31 sentence.** Keep it, and add: "With the 6-slow confirm rule, the apac leader still steps down within 10 s on commit errors in both halves of B0-31, and the buckets-up half passes on the enam leader that follows (commit p99 547 ms); the placement-failing half passes its split gate (Human and MergeQueue p99 1.25 s, 0 refused) [measured, B0-31, fourth pass]." This replaces the third pass's "it held for 600 s", which no longer holds: in `p4nop2` the leader moved at 7.4 s.

## Teardown (fourth pass)

On 2026-10-30, after the last run, `node run/wp9.ts delete` removed `graft-wp9-a/b/c` and `graft-wp9-load`, and emptied and deleted the buckets `graft-wp9-stage` and `graft-wp9-stage-c`. Checks after teardown:

- every `graft-wp9-*` script answers 404;
- `wrangler r2 bucket list` and `wrangler containers list` show nothing named `graft-wp9`;
- no `wp9-*` orb service is running.

Nothing with the `graft-wp9` prefix is deployed. The discarded run `b0-31-p4up` was not kept. Its `/start` returned 500, yet its clients started and ran alongside `b0-31o-p4nop`.


## Fifth pass (2026-10-30): B0-33 with the per-entry agent inline budget and the failing-target rule

**What changed in the bed.** Two decisions from the fourth pass's edits were implemented in `src/wp9/` only. `src/raft/` was not edited.

- **`AGENT_INLINE_ENTRY` = 128 KiB** (`src/wp9/types.ts`), from item 32. On a hot group, an entry carries at most 128 KiB of Agent-class inline pack bytes. Human and MergeQueue packs do not count.
  - **At admission:** the leader keeps a count of the Agent inline pack bytes that are queued and not yet in an entry. An agent pack that would take that count past the budget is answered `stage`. The edge stores it in R2 and sends it again as `Stored`. While placement fails, it gets the agent refusal at once instead, with cause `outage-budget`.
  - **At the cut:** the entry builder also stops taking Agent commands once the next one would pass the budget. That covers packs admitted before the group was hot.
  - **Counter:** the entry row records each entry's Agent inline bytes.
- **The failing-target rule** (`src/wp9/member.ts`, from item 30). A `STEP_DOWN` transfer target whose calls failed in the window at more than `commit_errors` does not qualify. If none qualifies, the leader holds and logs `STEP_DOWN held ... no follower qualifies (<site> fails <rate>, ...)`.

The bed was deployed fresh as `graft-wp9-a/b/c` and `graft-wp9-load`, with the fixed `outcome` timer from the fourth pass. Each run used a fresh group. The load was the same as `p4r4` to `p4r6`: 60 clients for 300 s, 252 agent commands/s offered, and the user token at 100 a minute. Raw files are `results/raw/wp9-b0-33-p5r1` to `-p5r3` and `wp9-b0-29-p5r1`.

### B0-33 runs

| Quantity | `p5r1` (ATL, AMS, ICN) | `p5r2` (IAD, LHR, NRT) | `p5r3` (ATL, LHR, NRT) |
| --- | --- | --- | --- |
| Leader changes and cause | 7 (8 leader starts), all elections after `suspect` storms; 0 `STEP_DOWN` transfers (14 holds) | 7 (8 starts), all elections; 0 transfers (12 holds); stable from 155.5 s | **0** |
| `suspect` received (client give-ups) | 15,976 (13,286) | 11,549 (7,963) | **0 (0)** |
| Human+MQ commit p99, whole / from 30 s | 5,109 / 5,109 ms | 7,197 / 7,197 ms | **616 / 570 ms** |
| Merges ok of 50, leader p99 | 38 (3 `Stale`), 5,109 ms | 12 (33 `Stale`), 7,197 ms | **50, 379 ms** |
| Agent committed / refused per s | 201.4 / 8.6 | 221.0 / 7.8 | **248.4 / 0** |
| Entries/s; commands p50 | 2.08; 60 | 2.44; 61 | 5.68; 43 |
| Entry bytes p50 / p99 | 90 / 729 KB | 76 / 583 KB | 161 / 225 KB |
| Agent inline bytes per entry p50 / p99 / max | 0 / 131 KB / 1,049 KB | 0 / 130 / 274 KB | 120 / 131 / 219 KB |
| Agent pushes staged by the budget | 834 (final leader only) | 1,564 (final leader only) | 5,090 |
| Leader req/s (peak) | a 778 (1,945), b 1,030, c 913 | a 740, b 785 (3,439), c 776 | a 759 (898) |
| `Durable Object is overloaded` client errors | 28,002 (a 14,684, b 13,318) | 6,819 (a 6,207, b 612) | **0** |
| Deadline client errors | 25,522 | 25,219 | 0 |
| User token classed Agent after its first minute | 239 of 399 (77 errors, 39 refused) | 206 of 399 (42 errors, 37 refused) | **399 of 399** |
| Refusals: `ng`, 429, other shapes | 432, 2,126, 0 | 327, 1,707, 0 | 0, 0, 0 |
| Slowest storage confirmation | 2,365 ms (c) | 1,091 ms (a) | 204 ms |

"Agent pushes staged by the budget" counts the pushes staged by `AGENT_INLINE_ENTRY` rather than by `INLINE_MAX_HOT` (`budgetStaged`). After a leader change, it is the final leader's count only.

**The budget does what it says on a hot group.** Agent inline bytes per entry stayed at or under 131 KB in every 30 s window in which the leader had been leading for 5 s. In `p5r3` the leader's entries were 161 KB at p50 and 225 KB at p99, against 450 KB and 1.03 MB in `p4r1` and `p4r4`. Agent pushes were not slowed: 12,497 of 12,500 pushes committed, 5,090 of them staged. The larger entries in `p5r1` and `p5r2` are not inline packs. They are 300 to 600 queued 1 KB side commands per entry, once the round time had grown (see below).

**`p5r3` passes every B0-33 line**: 0 leader changes, 0 `suspect`, Human+MQ p99 616 ms, 248.4 agent commands/s admitted, the user token Agent from its first minute on, and no refusals at all. It is the first B0-33 run in five passes with 0 leader changes and 0 `suspect` reports.

**`p5r1` and `p5r2` fail it, and the cause is not `STEP_DOWN`.** Neither run made a single `STEP_DOWN` transfer (14 and 12 holds). Every leader change was an election after clients' `suspect` reports. The member logs show the same loop each time:

1. **The leader object falls behind.** Its AppendEntries to both followers miss `CALL_DEADLINE` at the same instant, and each succeeds on the next send in 100 to 450 ms. Examples:
   - `p5r2`, leader a (IAD): "4.6 append to b failed", "4.6 append to c failed", then "4.9 append to b ok after 1 failures, 300 ms" and "5.0 append to c ok after 1 failures, 350 ms". The same pattern recurs at 5.9, 11.2, 18.1, and 19.6 s.
   - `p5r1`, leader a (ATL): the same at 4.2, 5.4, 8.2, and 10.8 s.

   Two links on different continents failing in the same millisecond and recovering together point to the leader's own event loop, not to the network.
2. **Its round time grows.** Leader a's round EWMA went from 522 to 1,279 ms in the first 9 s of `p5r1` and from 347 to 956 ms in 13 s of `p5r2`. In `p5r3` it fell from 297 to 170 ms. The request rate was the same in all three: 750 to 950 req/s, of which 270 to 330 were `outcome`.
3. **Entries grow with the round.** The queue backs up, and the next entry carries 300 to 600 commands (728 KB at 2.3 s in `p5r1`). That makes the next round slower still.
4. **Clients' calls miss their deadline,** and clients report `suspect` to both followers. The followers stop granting promises. In `p5r2`, a logged "21.1 pre-vote for 4 refused as suspected", and b won term 3 at 21.3 s.
5. **The new leader starts cold and is shed at once.** Logged: `p5r2` c "37.2 append to b failed: Durable Object is overloaded"; `p5r1` a "28.1 append to b failed: Durable Object is overloaded", 0.3 s after it won term 4. The loop repeats until one leader settles: b at 155.5 s in `p5r2`, and none in `p5r1`.

One bed detail made step 5 worse. `hot` is the leader's own count of pushes in the last 5 s, so a new leader is not hot for its first seconds. In that time it inlines agent packs up to 1 MiB with no budget. `p5r1`'s 1,049 KB maximum agent inline entry came at 0.7 s and again right after the elections at 240 s.

**The failing-target rule turned every commit-error trip into a hold.** In `p5r1` and `p5r2`, leaders logged 26 holds such as "STEP_DOWN held in term 4: commit errors 0.07/0.20, but no follower qualifies (b fails 0.07, c fails 0.20)". This is structural, not chance. The commit-error rule trips only when both followers' error rates pass `commit_errors` (`Math.min(...rates) > commitErrors`, as in `src/raft/member.ts`). Item 30 then excludes every follower above that same line. So under the two rules together, a commit-error trip can never transfer, and only the confirm rule and elections can move a leader. That removed the fourth pass's bad transfers, such as `p4r4`'s transfer to a failing SIN. It also means the commit-error rule now only logs. The leader still lost the lead in `p5r1` and `p5r2`, through elections.

**What limits B0-33 now:**
- **The leader object near 750 to 950 requests/s.** It either settles (`p5r3`, and `p4r1` in the fourth pass) or enters the loop above in its first 5 to 20 s (`p5r1`, `p5r2`, and `p4r2`, `p4r6` before them).
- **What decides which way it goes** is how fast the first rounds are. In the two runs that collapsed, the first rounds were slow: the first entries were large, from the unbudgeted 1 MiB pack and the backlog, and IAD and ATL were far from AMS, ICN, and NRT. On this evidence the 128 KiB budget is necessary but not sufficient.
- **Across five passes,** 2 of 9 B0-33 runs on the current agent mix had 0 leader changes (`p4r1`, `p5r3`). Only `p5r3` also had 0 `suspect` reports.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 (fifth pass) | `p5r3`: 0 changes, 0 `suspect`, Human+MQ p99 616 ms, 248.4/s admitted, token Agent, no refusals. `p5r1`, `p5r2`: 7 changes each by election after leader overload (28,002 and 6,819 overload errors), 15,976 and 11,549 `suspect`, Human+MQ p99 5.1 and 7.2 s, 201 and 221/s admitted, token Agent 239 and 206 of 399; every refusal `ng` or 429 | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s admitted, user token Agent, `ng` or 429 shapes | **Fail, 1 of 3 runs passes.** The per-entry budget fixes the Human+MQ line when the leader holds (616 ms, against 1.15 to 1.49 s in `p4r1`). Admission and shapes pass in all 3. Leadership still fails in 2 of 3, from leader overload, not `STEP_DOWN` |

### B0-29 on the new code

Two runs were made, each 600 s with 30 clients and learners in slots 1 and 2, on the same deploy as the B0-33 runs.

| Quantity | `b0-29-p5r1` (EWR, CDG, NRT) | `b0-29-p5r2` (ATL, LHR, HKG) |
| --- | --- | --- |
| Commit p50 / p99 / max | 188 / **809** / 1,837 ms | 192 / **399** / 609 ms |
| Merges ok of 100, leader p99 | 100, 1,679 ms | 100, 323 ms |
| Pushes ok of 2,500, `Stale` | 2,499, 0 | 2,500, 0 |
| Leader changes, `suspect` | 0, 124 | 0, 0 |
| Leader req/s, feed polls on the leader | 331, 0 (45.4/s on learners) | 332, 0 (45.4/s on learners) |
| Leader storage confirmations over 500 ms | 22 of 4,511 (p99 402 ms, max 1,573 ms) | 0 of 4,584 (max 405 ms) |
| Agent pushes staged by the budget | 13 | 0 |
| Entry inline bytes p99 | 75 KB | 65 KB |

**The budget barely engages at journey 9's rate,** so it cannot be what moved `p5r1`. At 4.2 pushes/s, 57 of 4,511 entries held more than 64 KiB of inline packs, and only 13 pushes were staged by the budget. Entries looked like the fourth pass's (p50 7 KB).

**`p5r1`'s 809 ms p99 came from the leader's storage and event loop.** Its commit p99 by 30 s window was 490 to 960 ms, with spikes of 1,674 to 1,679 ms at 270 s and 540 s. Two things line up with those windows:
- its leader saw 22 storage confirmations over 500 ms, against 2 and 0 in `b0-29-p4r1` and `-p4r2`;
- the leader's appends to both followers missed `CALL_DEADLINE` together four times (96.6, 296.6, 501.7, and 546.7 s), each recovering on the next send.

`p5r2`, on the same code, had commit p99 399 ms, with 0 slow confirmations and 0 `suspect`.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-29 (fifth pass) | `p5r2`: commit p99 399 ms, merge p99 323 ms, 0 `Stale`, 0 changes, leader 332 req/s, 0 feed polls on it. `p5r1`: commit p99 809 ms, merge p99 1,679 ms, during storage stalls on its leader (22 confirmations over 500 ms) | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Pass in 1 of 2.** The budget does not hurt B0-29: it staged 13 and 0 pushes. `p5r1`'s miss is a storage stall on its leader, which journey 9's 600 ms line does not absorb. The first pass also had runs at 602 and 625 ms |

### Fifth-pass verdicts

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 | 1 of 3 runs passes every line (`p5r3`: 0 changes, 0 `suspect`, Human+MQ p99 616 ms, 248.4/s). `p5r1` and `p5r2`: 7 elections each after leader overload, Human+MQ p99 5.1 and 7.2 s. Admission at least 201/s and `ng` or 429 shapes in all 3 | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s, token Agent, `ng` or 429 | **Fail** (1 of 3) |
| B0-29 | Commit p99 399 and 809 ms, merge p99 323 and 1,679 ms, 0 `Stale`, 0 changes | Commit p99 at most 600 ms, merge p99 at most 2 s | **Pass in 1 of 2**; the miss is a storage stall on the leader, not the budget |

**What is modeled and what is real.** This is the same as in earlier passes:
- Real: the members, the load clients, R2 staging, storage, and the links between colos.
- Modeled: packs are random bytes, `decide` checks only the ref's tip, and `hot` is the leader's own count of pushes in 5 s.
- The budget is counted on queued Agent inline bytes at admission, and again when the entry is cut.

### Proposed design edits (fifth pass)

34. **`design/DESIGN.md`, Shape, the `STEP_DOWN` doc comment.** Items 30 and the commit-error trip together leave no transfer on commit errors, for this reason:
    - The trip needs the error rate to both followers over `commit_errors`, as in `src/raft/member.ts`, and item 30 excludes any target over that same line.
    - In `p5r1` and `p5r2` this gave 26 holds and 0 transfers.

    Either say so: "A commit-error trip transfers only when a follower's own window is under `commit_errors`, which the trip's both-followers condition excludes, so commit errors alone hold the leader and log; a sick leader loses its lease through `suspect` instead [measured, `tests/m0/results/m0-wp9.md`, fifth pass]". Or keep a commit-error transfer by letting a target qualify on its link probe (the target's own calls to the other follower under `commit_errors`) rather than on the leader's calls to it. This is an operator call. The probe reading matches what the fourth pass's bad transfer lacked: `p4r4` moved to SIN while b>c's own probe said 0.05.
35. **`design/DESIGN.md`, Shape, `AGENT_INLINE_ENTRY` and `INLINE_MAX_HOT`, the hot rule.** Add: "A new leader takes `hot` from the log, as the push count of its last committed entries, not from its own pushes since it took the lead. Otherwise it inlines agent packs up to `INLINE_MAX` with no per-entry budget in its first seconds, the moment its round is slowest." Evidence: `p5r1`'s 1,049 KB agent inline entries at 0.7 s and after the elections at 240 s.
36. **`design/DESIGN.md`, Scale "Agent fleet, group commands", and the Milestone 0 table, B0-33 row.**
    - Replace the fourth pass's leadership evidence with: "With `AGENT_INLINE_ENTRY`, a leader that settles commits Human and MergeQueue at p99 616 ms with 0 `suspect` (`p5r3`). In 2 of 3 runs the leader object at 750 to 950 requests/s fell behind in its first 20 s: its calls to both followers missed `CALL_DEADLINE` together, the round grew from 0.35 to 1.3 s, entries grew to 600 commands, and clients' `suspect` reports moved the lead 7 times by election [measured, fifth pass]."
    - The remaining lever is the leader object's request rate, not entry bytes. About 270 to 330 of its 750 to 950 req/s are `outcome` calls. An outcome long-poll answered from the `commit` call when the entry lands fast, or an outcome served by a follower after apply, would remove a third of them.
    - Keep B0-33's 0-leader-change line `mitigated` until that is measured.

## Teardown (fifth pass)

On 2026-10-30 after the last run, `node run/wp9.ts delete` removed `graft-wp9-a/b/c` and `graft-wp9-load`, and emptied and deleted the buckets `graft-wp9-stage` and `graft-wp9-stage-c`. Checks after teardown:

- every `graft-wp9-*` script answers 404;
- `wrangler r2 bucket list` and `wrangler containers list` show nothing named `graft-wp9`;
- no `wp9-*` orb service is running.

Nothing with the `graft-wp9` prefix is deployed.


## Sixth pass (2026-10-30): probe-qualified targets, `hot` from the log, and `outcome` folded into `commit`

**What changed in the bed.** The three decisions from the fifth pass's edits were implemented in `src/wp9/` and `run/wp9.ts` only. `src/raft/` was not edited.

- **Item 34, the transfer target.** A `STEP_DOWN` target qualifies on its own link probe: its calls to the other follower must fail under `commit_errors`. It must also be outside `RESET_QUARANTINE` and allowed to lead. Among qualifying targets, the leader picks the one whose calls failed least, then the most caught up. If none qualifies, the leader holds. This is the same logic as the reference in `src/raft/member.ts`, ported without the wp4-only `hold10-` variant.
- **Item 35, `hot` from the log.** On becoming leader, a member counts the compare-and-sets in the committed entries of the last 5 s. That count seeds its `hot` window, so `AGENT_INLINE_ENTRY` applies from its first entry. The member logs `hot from log: <n> pushes in the last 5 s, hot <bool>`.
- **Item 36, `outcome` folded into `commit`.**
  - **Hold:** the leader holds a `commit` call until the command's ticket resolves, then answers with the outcome (`{ result }`). A ticket resolves only on the commit-advance path under the quorum rule, on an `AGENT_WAIT` or queue refusal, or at the end of leadership. So nothing reports success before the quorum rule holds.
  - **`COMMIT_HOLD` = 1 s (a guess):** after it, the leader answers with the ticket, as before, and the edge calls `outcome` once. The edge waits `COMMIT_HOLD` + `CALL_DEADLINE` = 2 s for the `commit` reply.
  - **Fallback:** an edge whose `commit` call timed out calls `outcomeRid(rid)`. That answers from the request rows, waits on the ticket, or says `unknown`, and then the edge sends `commit` again with the same rid.
  - **New counters:** held calls at peak (`heldPeak`), holds that expired (`holdExpired`), and client fallbacks.
- **Placement filter (harness only).** `run/wp9.ts run --want 'IAD|ATL,<follower colos>'` keeps making fresh groups (suffix `g1`, `g2`, ...) until the leader's colo and both followers' colos match. A location hint does not pin a colo.

The bed was deployed fresh, with the fifth pass's `AGENT_INLINE_ENTRY` and the fixed `outcome` timer. Each run used a fresh group, with the same load as `p4r4` to `p5r3`: 60 clients for 300 s, 252 agent commands/s offered, and the user token at 100 a minute. Runs `p6r1` and `p6r2` used the placement filter, and `p6r3` took the placement it got. All three came out far-flung, with an IAD or ATL leader and followers in Europe and Asia. Raw files are `results/raw/wp9-b0-33-p6r1` to `-p6r3` and `wp9-b0-29-p6r1`. The first group tried for `p6r1` landed on EWR, CDG, ICN and was replaced by `b0-33-p6r1g1`.

### B0-33 runs

| Quantity | `p6r1` (ATL, LHR, NRT) | `p6r2` (ATL, MAD, SIN) | `p6r3` (IAD, AMS, KIX) |
| --- | --- | --- | --- |
| Leadership held | **Yes** | One `STEP_DOWN` transfer at 35.9 s, then held | **Yes** |
| Leader changes and cause | **0** | 1: `STEP_DOWN` on commit errors 0.06/0.09; probes b>c 0.00 and c>b 0.00; transferred to b (MAD), which won term 3 at 37.4 s | **0** |
| `suspect` received (client give-ups, fallbacks) | **0 (0, 0)** | 118 (56, 8) | **0 (0, 0)** |
| Human+MQ commit p99, whole / from 30 s | **688 / 497 ms** | 1,865 / **902 ms** | **895 / 819 ms** |
| Merges ok of 50, leader p99 | 50, 507 ms | 50, 1,268 ms | 50, 692 ms |
| Agent committed / refused per s | 253.7 / 0 | 253.4 / 0 | 254.0 / 0 |
| Entries/s; commands p50 | 5.64; 43 | 4.10; 56 | 4.91; 48 |
| Entry bytes p50 / p99 | 161 / 222 KB | 173 / 326 KB | 167 / 260 KB |
| Agent inline bytes per entry p50 / p99 / max | 120 / 131 / 1,049 KB | 120 / 131 / 1,326 KB | 122 / 131 / 159 KB |
| Leader req/s, total | **494** | **459** (b) | **498** |
| ... by method, per s | commit 266, read 227, outcome 1 | commit 245, read 203, outcome 10 | commit 270, read 227, outcome 1 |
| Held commit calls, peak; holds expired | 290; 194 | 576; 6,392 | 269; 286 |
| `Durable Object is overloaded` / deadline client errors | **0 / 0** | 0 / 8 | **0 / 0** |
| User token classed Agent after its first minute | 399 of 399 | 380 of 399 | 399 of 399 |
| Refusals (`ng`, 429, other) | none | none | none |
| Slowest storage confirmation | 139 ms | 497 ms | 240 ms |

Request rates are over the whole snapshot. For comparison, the fifth pass's leaders served 750 to 950 req/s, of which 270 to 330/s were `outcome`.

**The `outcome` fold took a third of the leader's requests away.** `outcome` fell from 270 to 330/s to 1 to 10/s. The leader's total fell from 750 to 950 req/s to 459 to 498 req/s at the same offered load. In `p6r1` and `p6r3`, all but 194 and 286 commit calls were answered with their outcome inside the 1 s hold. Those that expired took the old ticket-then-`outcome` path: 194 and 286 `outcome` calls, 0.1 to 0.2% of commands. In `p6r2` the new leader b held 6,392 calls past 1 s in the 30 s after the transfer, while its round was 1 to 2 s; it answered 2,928 `outcome` calls. At peak, 269 to 576 commit calls were held open on the leader at once, and no run hit a limit on them.

**On far-flung placements, leadership held.** In the fifth pass, 2 of 3 runs collapsed into 7 elections each. Here:
- **`p6r1` (ATL, LHR, NRT) and `p6r3` (IAD, AMS, KIX):** 0 leader changes, 0 `suspect` reports, no overload or deadline errors, and none of the both-followers append failures that started the fifth-pass collapses. IAD with AMS and KIX is the shape of `p5r2`, which collapsed; ATL with an Asian follower is `p5r1`'s shape.
- **`p6r2` (ATL, MAD, SIN):** 1 change, and it was the designed one, a `STEP_DOWN` transfer:
  - a's first entries carried 1.33 MB and 1.05 MB of agent packs. The group was fresh, with no pushes in its log yet, so it was not hot (see below).
  - a's appends to SIN then missed `CALL_DEADLINE` 14 times from 4 to 18 s, and again at 19 to 34 s. At 35.9 s its error rates to both followers passed 5%.
  - Both probes were clean (b>c 0.00, c>b 0.00), so the item-34 rule transferred to b (MAD, fewer failed calls). That is exactly what the fifth pass's rule could not do.
  - b took over hot: `hot from log: 159 pushes in the last 5 s, hot true`. It held for the rest of the run. Its first agent entries were within the 128 KiB budget, and it had no overload errors.
  - The transfer cost 118 `suspect` reports and 56 client give-ups.

**What still costs the 1 s line: a fresh group's first seconds.** Item 35 makes a new leader of a busy group hot at once. But a group with no pushes in its log is not hot until 2 pushes/s have arrived over 5 s. In that time each agent pack up to 1 MiB goes inline.
- `p6r1` had one 1,049 KB agent entry at 0.9 s, and `p6r2` had 1,326 KB and 1,049 KB at 0.2 and 2.2 s. The backlog behind them built 333- and 796-command entries.
- `p6r1`'s Human+MQ p99 was 820 ms in its first 30 s and 374 to 560 ms after. `p6r2`'s worst Human+MQ commit in each 10 s window was 2,488, 1,448, and 1,510 ms over 0 to 30 s, while a's appends to SIN were missing their deadline; 1,016 ms at the transfer; and under 1 s after 50 s (30 s windows 480 to 929 ms).
- `p6r3` did not get a 1 MiB pack in its first seconds (agent inline max 159 KB). Its first 30 s had Human+MQ p99 1,044 ms, and its later windows 599 to 910 ms.
- In production this is a group that turns from quiet to 250 agent commands/s within one second. The harness does exactly that, and journey 9 grows gradually. It is still the only path left to an unbudgeted 1 MiB entry.

**Verdict for B0-33.** Taking each gate line over the whole run:

| Line | `p6r1` | `p6r2` | `p6r3` |
| --- | --- | --- | --- |
| 0 leader changes | Pass | Fail (1 `STEP_DOWN`) | Pass |
| 0 `suspect` | Pass | Fail (118) | Pass |
| Human+MQ p99 at most 1 s | Pass (688 ms) | Fail (1,865 ms; 902 ms from 30 s) | Pass (895 ms) |
| At least 140 agent/s admitted | Pass (253.7) | Pass (253.4) | Pass (254.0) |
| User token Agent | Pass | Pass | Pass |
| Refusal shapes | Pass (none) | Pass (none) | Pass (none) |

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 (sixth pass) | `p6r1`, `p6r3` (far-flung, ATL and IAD leaders): 0 leader changes, 0 `suspect`, Human+MQ p99 688 and 895 ms, 253.7 and 254.0 agent/s, token Agent, no refusals, leader 494 and 498 req/s. `p6r2` (ATL, MAD, SIN): 1 `STEP_DOWN` transfer at 35.9 s after cold-start 1.3 MB entries to SIN, 118 `suspect`, Human+MQ p99 1,865 ms (902 ms from 30 s), then held | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s, user token Agent, `ng` or 429 shapes | **Pass in 2 of 3 runs, Fail as a row.** The first B0-33 runs to pass on far-flung placements, and the first with no overload errors. The remaining failure is a fresh group's unbudgeted first seconds, not leader overload |

### B0-29 on the sixth-pass code

**Run `b0-29-p6r1`** (IAD, LHR, KIX, leader a; 600 s, 30 clients, learners in slots 1 and 2):

- **Commit:** p99 **798 ms**, p50 185 ms, max 1,490 ms.
- **Merges:** p99 1,254 ms at the leader, 100 of 100 ok.
- **Pushes:** 2,500 of 2,500 ok, 0 `Stale`.
- **Leadership:** 0 leader changes, 8 `suspect` reports, 4 client fallbacks.
- **Load:** the leader served 301 req/s, by method commit 27, read 274, and `outcome` 0.1. The fourth and fifth passes' leaders served 331 to 332 req/s, with `outcome` 27/s. Learners served 45.9 feed polls/s, and the leader served none.
- **The budget and the hold:** 5 pushes were staged by the budget, 43 commit calls were held at peak, and 67 holds expired.

**Why p99 is over 600 ms.** The commits over 600 ms are not spread out. They fall in bursts, and each burst lines up with something on the leader:
- **0 to 2 s:** the start transient.
- **24 to 28 s, 124 to 129 s, 233 to 243 s, 284 s, and 548 to 573 s:** a's AppendEntries to LHR or KIX missed `CALL_DEADLINE`. Examples: "124.7 append to b failed", "124.8 append to c failed", "125.6 append to c ok after 1 failures, 794 ms". The leader also saw 51 storage confirmations over 200 ms and 7 over 500 ms.

Entries were as small as in earlier passes (p50 7 KB, 3 commands). The leader's commit time runs from admission to the quorum ack, and holding the call does not change it: p50 was 185 ms, against 187 to 192 ms in the fourth and fifth passes. So the miss is the IAD leader's long round to KIX and its stalls, not the new code. The first pass's IAD, MAD, KIX run (`b0-29-sd3r1`) had 625 ms for the same reason.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-29 (sixth pass) | Commit p99 798 ms (p50 185), merge p99 1,254 ms, 0 `Stale`, 0 changes, leader 301 req/s, 0 feed polls on the leader | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Fail on commit p99 only** (every other line passes). On IAD, LHR, KIX, the misses sit in the leader's append-deadline misses and storage stalls. The new code took 30 req/s off the leader and did not move p50. Not rerun on a nearer placement |

### Sixth-pass verdicts

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 | 2 of 3 runs (`p6r1` ATL, LHR, NRT; `p6r3` IAD, AMS, KIX) pass every line: 0 changes, 0 `suspect`, Human+MQ p99 688 and 895 ms, 254/s, token Agent. `p6r2` (ATL, MAD, SIN): one `STEP_DOWN` transfer after cold-start 1.3 MB entries, 118 `suspect`, Human+MQ p99 1,865 ms | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s, token Agent, `ng` or 429 | **Fail (2 of 3 pass)** |
| B0-29 | Commit p99 798 ms, merge p99 1,254 ms, 0 `Stale`, 0 changes, leader 301 req/s | Commit p99 at most 600 ms, merge p99 at most 2 s | **Fail on commit p99** on IAD, LHR, KIX, from append-deadline misses and storage stalls on the leader |

**What is modeled and what is real.** This is the same as in earlier passes:
- Real: the members, the load clients, R2 staging, storage, and the links between colos.
- Modeled: packs are random bytes, and `decide` checks only the ref's tip.
- `COMMIT_HOLD` (1 s) is this bed's guess.
- The placement filter makes fresh groups until the colos match, so every run's group was new: no group had pushes in its log at the start.

### Proposed design edits (sixth pass)

37. **`design/DESIGN.md`, Shape, the write path (`commit` and `outcome`).** Write the fold in as adopted, with its numbers:
    - "The leader holds `commit` until the entry commits, up to `COMMIT_HOLD`, and answers with the outcome. A call held past `COMMIT_HOLD` answers the ticket, and the edge calls `outcome` once. An edge whose `commit` timed out asks for the outcome by request id."
    - Evidence: on B0-33 the leader's requests fell from 750 to 950/s to 459 to 498/s, `outcome` from 270 to 330/s to 1 to 10/s, with up to 576 calls held at once [measured, `tests/m0/results/m0-wp9.md`, sixth pass].
    - `COMMIT_HOLD` = 1 s is a [guess]. Under it, 0.1 to 0.2% of commands fell back in a steady run, and 9% in the run with a transfer.
    - The edge's `commit` deadline becomes `COMMIT_HOLD` + `CALL_DEADLINE`. A `suspect` then needs 2 s of silence, not 1 s. Say so beside `CALL_DEADLINE`.
38. **`design/DESIGN.md`, Shape, `AGENT_INLINE_ENTRY` and the hot rule.** Item 35 covers a new leader, not a new or quiet group. Add: "Until a group has had `HOT_PUSHES` for 5 s, an Agent-class pack over `INLINE_MAX_HOT` is still staged when placement works. The per-entry budget then needs no hot test, because Human and MergeQueue packs do not count against it."
    - In other words, apply `AGENT_INLINE_ENTRY` on every group, not only hot ones. A quiet group's agent entries are then at most 128 KiB, and a quiet group rarely has more than one agent pack an entry anyway.
    - Evidence: `p6r1` and `p6r2` each had 1.05 to 1.33 MB agent entries in their first 3 s. In `p6r2` they led to the only leader change and the only Human+MQ p99 over 1 s of the pass. `p6r3` had none and passed.
39. **`design/DESIGN.md`, Milestone 0 table, B0-33 row, and Scale "Agent fleet, group commands".** Replace the fifth pass's "leader overload" evidence:
    - "With `outcome` folded into `commit`, `AGENT_INLINE_ENTRY`, and probe-qualified `STEP_DOWN` targets, two of three far-flung runs (ATL and IAD leaders, Europe and Asia followers) held their leader with 0 `suspect`, Human and MergeQueue p99 688 and 895 ms, and 254 agent commands/s at about 500 leader req/s. The third changed leader once, by a `STEP_DOWN` transfer the probe rule placed, after a fresh group's first entries carried 1.3 MB of unbudgeted agent packs [measured, sixth pass]."
    - Keep the 0-leader-change line `mitigated` until item 38 is measured.
40. **`design/DESIGN.md`, Shape, the `STEP_DOWN` doc comment, the item-34 sentence.** Add the first measured transfer under it: "On B0-33 `p6r2`, a leader whose appends to apac missed `CALL_DEADLINE` for 30 s tripped at 0.06/0.09, both followers' probes read 0.00, and it moved to the weur follower, which held for the rest of the run [measured, sixth pass]."

## Teardown (sixth pass)

On 2026-10-30, after the last run, `node run/wp9.ts delete` removed `graft-wp9-a/b/c` and `graft-wp9-load`, and emptied and deleted the buckets `graft-wp9-stage` and `graft-wp9-stage-c`. Checks after teardown:

- every `graft-wp9-*` script, including `graft-wp9-git`, answers 404;
- `wrangler r2 bucket list` and `wrangler containers list` show nothing named `graft-wp9`;
- no `wp9-*` orb service is running.

Nothing with the `graft-wp9` prefix is deployed.


## Seventh pass (2026-10-30): the agent budget on every group, and the revised target rule

**What changed in the bed** (`src/wp9/` and `run/wp9.ts` only; `src/raft/` was not edited):

- **Edit 38, adopted: `AGENT_INLINE_ENTRY` on every group.** The 128 KiB budget on Agent-class inline pack bytes per entry now applies whether or not the group is hot, both at admission and when an entry is cut. Before this pass it applied only on hot groups. At admission, an agent pack is staged when it would take the queued Agent inline bytes past 128 KiB. A pack larger than 128 KiB is therefore always staged when placement works, and refused at once when it fails.
- **The transfer target, as revised by M0-WP4.** A target qualifies only if both of these fail under `commit_errors`: the leader's calls to it in the window, and the other follower's probe of it (`linkErrors` from the other follower to the target). It must still be outside `RESET_QUARANTINE`; then the leader picks fewest failed calls, then most caught up, and holds if none qualifies.
  - When this pass started, `src/raft/member.ts` (last changed 19:09Z) still had the sixth-pass rule, so the runs used the revision as described: both bars on every trip.
  - M0-WP4's version landed in `src/raft/member.ts` at 23:11Z, after the runs had started. It differs from the description. On a commit-error trip it skips the leader-calls bar, which every follower is over by the trip's own condition, and keeps the other follower's probe; a probe that cannot run disqualifies the target. On a confirm trip it keeps the leader-calls bar.
  - After the runs, `src/wp9/member.ts` was brought in line with the landed version.
  - No seventh-pass run tripped `STEP_DOWN` (0 `STEP_DOWN` events in all five runs), so the difference did not touch any measurement.
- **Harness:** `--want` takes a follower list starting with `+` to require only one follower from it. The near B0-29 placement used `ATL,+LHR`.

The bed was deployed fresh, with the sixth pass's `outcome` fold (`COMMIT_HOLD` 1 s), `hot` from the log, the fixed timer, and the 64 KiB `INLINE_MAX_HOT`. Every run used a fresh group and the same load as before. Raw files are `results/raw/wp9-b0-33-p7r1` to `-p7r3`, `wp9-b0-29-p7near`, and `wp9-b0-29-p7far`.

### B0-33 runs (all far-flung, fresh groups)

| Quantity | `p7r1` (ATL, CDG, HKG) | `p7r2` (ATL, AMS, ICN) | `p7r3` (ATL, LIS, SIN) |
| --- | --- | --- | --- |
| Leadership held | **Yes** | **Yes** | **Yes** |
| Leader changes | **0** | **0** | **0** |
| `suspect` received (client give-ups, fallbacks) | **0** (0, 0) | **0** (0, 0) | **0** (0, 5) |
| Human+MQ commit p99, whole / from 30 s | **746 / 652 ms** | **942 / 744 ms** | **943 / 799 ms** |
| Merges ok of 50, leader p99 | 50, 881 ms | 50, 795 ms | 50, 809 ms |
| Agent committed / refused per s | 252.6 / 0 | 253.7 / 0 | 252.6 / 0 |
| Entries/s; commands p50 | 5.50; 44 | 4.46; 55 | 3.95; 61 |
| Entry bytes p50 / p99 | 163 / 220 KB | 175 / 245 KB | 181 / 276 KB |
| Agent inline bytes per entry p50 / p99 / **max** | 121 / 131 / **131.07 KB** | 123 / 131 / **131.07 KB** | 123 / 131 / **131.07 KB** |
| Agent pushes staged by the budget | 5,205 | 6,100 | 6,668 |
| Leader req/s | 494: commit 267, read 227, outcome 0 | 501: commit 271, read 230, outcome 1 | 504: commit 272, read 230, outcome 2 |
| Held commit calls, peak; holds expired | 246; 63 | 300; 247 | 354; 527 |
| `Durable Object is overloaded` / deadline client errors | 0 / 0 | 0 / 0 | 0 / 5 |
| User token classed Agent after its first minute | 399 of 399 | 399 of 399 | 399 of 399 |
| Refusals (`ng`, 429, other) | none | none | none |
| Leader storage confirmations over 200 / 500 ms; max | 8 / 2; 643 ms | 0 / 0; 50 ms | 11 / 1; 580 ms |
| AppendEntries deadline misses to the apac follower | 3 to 27 s, then 3 more | 13 to 21 s, then 4 more | 6 to 25 s (14 failures), then 2 more |

**The budget now holds from the first entry.** No entry in any run carried more than 131,072 bytes of Agent inline packs: 128 KiB exactly. That includes the first entries of these fresh groups: 15 to 121 KB at 0.2 to 0.3 s, and 119 to 128 KB by 0.7 s. In the sixth pass, `p6r1` and `p6r2` had 1.05 and 1.33 MB first entries. The backlog entries of 333 and 796 commands are gone too: the first five entries here had 7 to 171 commands.

**Every line of B0-33 passed in all three runs:**
- 0 leader changes and 0 `suspect` reports;
- Human+MQ p99 746 to 943 ms;
- 252.6 to 253.7 agent commands/s admitted;
- the user token classed Agent;
- no refusals at all, so no wrong shapes.

The leader served 494 to 504 req/s with 0 to 2 `outcome` calls/s. No member saw an overload error.

**Margins.**
- **Human+MQ p99 is 6 to 25% under the 1 s line,** and it is closest in the first 30 s. There, `p7r3` was 997 ms and `p7r2` 1,050 ms, so `p7r2`'s 30 s window passed 1 s while its whole-run p99 did not.
- **Where the first-minute tail comes from:** the leader's AppendEntries to its apac follower missed `CALL_DEADLINE` 3 to 14 times in each run's first 30 s, while the groups were warming up. Later they missed a few times a minute.
- **Error rates stayed under 5%,** so no `STEP_DOWN` tripped.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 (seventh pass) | 3 of 3 far-flung runs (ATL leader; CDG, AMS, or LIS and HKG, ICN, or SIN followers): 0 leader changes, 0 `suspect`, Human+MQ p99 746, 942, 943 ms, 252.6 to 253.7 agent/s admitted, token Agent, no refusals, 0 overload errors, leader 494 to 504 req/s | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s, user token Agent, `ng` or 429 shapes | **Pass** (3 of 3). The first pass of B0-33 in seven. Margin on the 1 s line is 57 to 254 ms over the whole run |

### B0-29 near and far

Each run was 600 s with 30 clients and learners in slots 1 and 2, on fresh groups.

| Quantity | `b0-29-p7near` (ATL, LHR, HKG) | `b0-29-p7far` (ATL, CDG, NRT) |
| --- | --- | --- |
| Commit p50 / p99 / max | 184 / **378** / 733 ms | 199 / **485** / 1,226 ms |
| Merges ok of 100, leader p99 | 100, 466 ms | 100, 379 ms |
| Pushes ok of 2,500, `Stale` | 2,500, 0 | 2,500, 0 |
| Leader changes, `suspect` | 0, 2 | 0, 0 |
| Leader req/s (by method per s) | 303 (read 275, commit 28, outcome 0) | 303 (read 275, commit 28, outcome 0) |
| Feed polls on the leader (on learners) | 0 (45.5/s) | 0 (45.5/s) |
| Leader storage confirmations over 200 / 500 ms; max | 2 / 0; 492 ms | 29 / 2; 909 ms |
| AppendEntries deadline misses | none | one, to both followers at 243.4 s, each recovering in 118 to 181 ms |
| Agent pushes staged by the budget; held calls peak | 2; 26 | 3; 30 |

**Both runs pass, and B0-29 needed no attribution this time.** The near placement ("ATL, LHR, and one more") came out ATL, LHR, HKG. To get it, the filter discarded ten groups that did not land an ATL leader with an LHR follower (group `b0-29-p7nearg10`).

`p7far`'s one slow window, 240 to 270 s with p99 987 ms, sits on its single both-follower append miss at 243.4 s. Its worst storage confirmations (909 ms) were in that window too. Without that window its p99 would be about 450 ms. That is the same mechanism that failed `b0-29-p5r1` and `-p6r1`, which had 10 and 12 failed AppendEntries in 600 s (4 and 2 of them to both followers at once). Here there were 2 (1 to both), which is why the run passed. On this evidence B0-29's 600 ms line passes or fails on how often the leader's storage stalls in a run. The new code does not decide it:
- the leader served 303 req/s with no `outcome` traffic;
- commit p50 was 184 to 199 ms, as in every pass;
- the budget staged 2 and 3 pushes.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-29 (seventh pass) | Near (ATL, LHR, HKG): commit p99 378 ms, merge p99 466 ms. Far (ATL, CDG, NRT): commit p99 485 ms, merge p99 379 ms. Both 0 `Stale`, 0 changes, leader 303 req/s, 0 feed polls on the leader | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Pass** (2 of 2) |

### Seventh-pass verdicts

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 | 3 of 3 far-flung runs: 0 leader changes, 0 `suspect`, Human+MQ p99 746 to 943 ms, about 253 agent/s admitted, token Agent, no refusals, no overload errors; agent inline per entry never over 128 KiB | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s, token Agent, `ng` or 429 | **Pass** |
| B0-29 | Near: commit p99 378 ms. Far: 485 ms. Merges at most 466 ms, 0 `Stale`, 0 changes, leader 303 req/s | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale`, leader under 600 req/s, 0 feed polls on the leader | **Pass** |

**Not measured in this pass:** a `STEP_DOWN` under the revised target rule, because no run tripped. Neither did B0-31 or B0-32 run on the seventh-pass code. B0-32's burst of 500 pushes of 256 KiB now stages every one of them under the budget, so it should be rerun.

**What is modeled and what is real.** This is the same as in earlier passes:
- Real: the members, the load clients, R2 staging, storage, and the links between colos.
- Modeled: packs are random bytes, and `decide` checks only the ref's tip.
- `COMMIT_HOLD` (1 s) is this bed's guess.
- Every run used a fresh group placed by the filter.

### Proposed design edits (seventh pass)

41. **`design/DESIGN.md`, Shape, `STEP_DOWN` doc comment, the target sentence.** Write M0-WP4's rule as landed in `src/raft/member.ts`, not as first described.
    - On a commit-error trip, the leader's own calls are over `commit_errors` to every follower by the trip's own condition. So the bar there is the other follower's probe of the target, and a probe that cannot run disqualifies it.
    - On a confirm trip, the bar is the leader's calls to the target.
    - The described form, with both bars on every trip, leaves no target on any commit-error trip, as the fifth pass's 26 holds and 0 transfers showed.
    - The seventh pass ran on the described form, but no run tripped, so it measured neither form under load. B0-06 in M0-WP4 is the row that measures it.
42. **`design/DESIGN.md`, Milestone 0 table, B0-33 row, and Scale "Agent fleet, group commands".** Record the pass. Replace the sixth pass's evidence with: "With `outcome` folded into `commit`, `AGENT_INLINE_ENTRY` on every group, and `hot` from the log, three far-flung runs (ATL leader, weur and apac followers, fresh groups) held their leader with 0 `suspect`, Human and MergeQueue p99 746 to 943 ms, 253 agent commands/s admitted, and the leader at about 500 req/s [measured, `tests/m0/results/m0-wp9.md`, seventh pass]." The 0-leader-change line can move from `mitigated` to measured on this evidence, with the margin stated: 57 to 254 ms under 1 s, least in the first 30 s.
43. **`design/DESIGN.md`, Milestone 0 table, B0-29 row.** Add, beside the 600 ms line: "Commit p99 on this row is set by storage stalls on the leader. A both-follower AppendEntries miss and its slow confirmations add a 1 to 1.7 s window. Across the seven fourth-to-seventh-pass B0-29 runs, the five with 0 to 4 failed AppendEntries in 600 s (0 or 1 of them to both followers at once) passed at 378 to 531 ms. The two with 10 and 12 failed AppendEntries (4 and 2 to both followers at once) and 22 and 7 storage confirmations over 500 ms missed at 809 and 798 ms [measured, `tests/m0/results/m0-wp9.md`, fourth to seventh passes]."
    - Either accept that the line can fail on a bad 10 minutes, or gate the row on the median of three runs, as B0-01 does with placements.
    - This is an operator call. The seventh pass's two runs both passed.

## Teardown (seventh pass)

On 2026-10-31, after the last run, `node run/wp9.ts delete` removed `graft-wp9-a/b/c` and `graft-wp9-load`, and emptied and deleted the buckets `graft-wp9-stage` and `graft-wp9-stage-c`. Checks after teardown:

- every `graft-wp9-*` script, including `graft-wp9-git`, answers 404;
- `wrangler r2 bucket list` and `wrangler containers list` show nothing named `graft-wp9`;
- no `wp9-*` orb service is running.

Nothing with the `graft-wp9` prefix is deployed.


## Eighth pass (2026-10-31): `commit_fail_min` 3, on the final rules

**What changed in the bed** (`src/wp9/`, `run/wp9.ts`, and a new `run/wp9-rescue.ts`; `src/raft/` was not edited):

- **`commit_fail_min` = 3, adopted** (`STEP_DOWN.commitFailMin` in `src/wp9/types.ts`). The commit-error rule now also needs at least 3 failed calls to every follower in the 10 s window, as well as the 5% rate. `src/raft/member.ts` has this behind the `min3-` prefix, and here it is the default.
  - When the rate passes but the count does not, the leader logs `commit errors ... but failed calls x/y: under commit_fail_min, no trip`, at most 50 times.
  - The target rule is the one landed in `src/raft/member.ts`, as ported at the end of the seventh pass. On a commit-error trip the other follower's probe decides; on a confirm trip, the leader's calls decide.
- **Everything else is the seventh pass's code:** `outcome` folded into `commit` (`COMMIT_HOLD` 1 s), `AGENT_INLINE_ENTRY` 128 KiB on every group, `hot` from the log, `INLINE_MAX_HOT` 64 KiB, and the outage rule.
- **Harness:**
  - Load clients record each stage's start, duration, and size. Collection reports B0-32's burst stage latency (`burstStageMs`) and every merge's wait (`mergeWait`).
  - `/start` gets 150 s. If it fails, the runner logs it and carries on to collect, instead of throwing. In `b0-31-p8up`, `/start` timed out after 120 s with 29 of 30 clients started: client 16, an enam client and not a pusher, never answered. The runner then died. `run/wp9-rescue.ts` finished the run's snapshots and collection from the same spec while the clients ran on.

The bed was deployed fresh at 02:25Z, and every run used a fresh group. Raw files are `results/raw/wp9-b0-31-p8up`, `wp9-b0-31o-p8nop`, `wp9-b0-32-p8r1`, and `wp9-b0-33-p8r1`.

### B0-31, buckets up (`b0-31-p8up`: EWR, CDG, NRT; leader c on NRT, apac; every agent push 1 MiB; 600 s)

| Quantity | `p8up` | Fourth pass `p4up2` (SIN leader) |
| --- | --- | --- |
| Leader on apac for the whole run | **Yes**: 0 leader changes, 0 `STEP_DOWN` | No: stepped down at 9.9 s on commit errors 0.60/0.70 |
| Commit p50 / p99 / max, whole run | 314 / **598** / 1,058 ms | 168 / 547 / 2,411 ms |
| Commit p99 from 30 s | 599 ms | 479 ms |
| Human+MQ commit p99 | 601 ms | 523 ms |
| Merges ok of 100, leader p99, landed in | 100, 713 ms, all in the next entry | 100, 416 ms, all in the next entry |
| Agent pushes ok of 2,500 / `Stale` / errors | 2,500 / 0 / 0 | 2,488 / 3 / 9 |
| Agent push leader commit p99; client p99 | 586 ms; 1,417 ms | 492 ms; 7,815 ms |
| Stage p50 / p99 / max | 296 / 1,035 / 1,886 ms | 295 / 870 / 1,827 ms |
| Inline pack bytes per entry, max | **0 agent** (4 KB of Human packs) | 1,048 KB: one agent 1 MiB pack at 1.0 s |
| Entries/s; bytes p50 / max | 4.79; 5.6 / 17 KB | 8.47; 9.9 KB |
| `suspect`; overload / deadline errors | 0; 0 / 0 | 286; around the transfer |
| Leader req/s; feed polls on the leader | 288 (read 262, commit 26, outcome 0); 0 (44.1/s on learners) | 331; 0 |
| AppendEntries deadline misses | 2 (351 s and 453 s, recovered in 270 and 405 ms) | many in the first 10 s |
| Leader storage confirmations over 500 ms | 0 of 2,874 (max 253 ms) | 1 (2,166 ms) |

`p8up` ran 29 of 30 clients, at 26.5 commands/s offered against 27.5. The missing client was not a pusher, so the run lost 1/30 of side commands and reads, and none of its pushes.

**For the first time, the apac leader held through B0-31, and the row passed on it.** Before the budget applied on every group, the first 1 MiB agent packs of a fresh group went inline before the group was hot. Those 1 MB entries made AppendEntries from apac to enam and weur miss `CALL_DEADLINE` for 10 s, and the commit-error rule moved the leader within 9.5 to 11.1 s in every earlier run that allowed it.

In `p4up2` that was one entry, 1,052 KB at 1.0 s, and nothing on that leader committed again until 11.6 s, after the transfer. Now all 2,500 pushes were staged, no entry carried agent inline bytes, and the largest entry was 17 KB. AppendEntries from NRT missed the deadline twice in 600 s. `commit_fail_min` was never reached, and no trip happened.

The apac leader costs latency: commit p50 314 ms, against 168 ms on the enam leader that `p4up2` ended with, and p99 598 ms. That is the same as the second pass's held apac leader (`b0-31-hold`, p50 318 ms, p99 641 ms after 30 s).

### B0-31, placement failing (`b0-31o-p8nop`: ATL, CDG, SIN; leader c on SIN, apac; outage rule; 600 s)

The load was journey 9 at 500 agents with every agent push 1 MiB, plus Human pushes at 0.3/s and merges at 10 a minute, each pack 64 KiB to 1 MiB. Placement failed for every bucket throughout (`nop-`).

| Quantity | `p8nop` | Fourth pass `p4nop2` |
| --- | --- | --- |
| Agent pushes over the cap refused in the `ng` shape | **2,500 of 2,500**, client p50 273 ms, p99 494 ms, max 2.1 s; none queued (queued agent peak 31 side commands, 35 KB) | 2,498 of 2,500, p99 856 ms |
| Human and MergeQueue pushes refused | **0** (187 Human pushes ok; 100 of 100 merges ok, 0 `Stale`) | 0 |
| Human+MQ commit p99, whole / from 30 s | **1,112 / 953 ms** | 1,252 / 1,196 ms |
| Human push p99; merge p99 (leader) | 1,519 ms; 3,899 ms (first merge) | 1,346 ms; 2,585 ms (first merge) |
| Merges landing in the next entry | 99 of 100 (1 in the second) | 98 of 100 |
| Leader changes | 1: `STEP_DOWN` at 8.9 s on commit errors 0.60/0.18 with 6/3 failed calls, so `commit_fail_min` was met. Probes b>a 0.00 and a>b 0.00. Transferred to b (CDG), which won term 4 at 10.6 s and held | 2 (a transfer, then an election) |
| `suspect`; client give-ups | 42; 5 | 58; 8 |
| Leader req/s; feed polls on the leader | 298 (b); 0 (34.5/s on learners) | 326; 0 |

**Why the apac leader moved here and not in `p8up`.** Under the outage rule, Human and MergeQueue packs keep `INLINE_MAX` (1 MiB) inline, and `AGENT_INLINE_ENTRY` does not count them. The run's first Human push carried a 931 KiB pack at 0.9 s, and a 729 KB Human and merge entry followed at 3.9 s.
- c's AppendEntries from SIN to ATL and CDG then missed `CALL_DEADLINE` 6 and 3 times in 9 s ("2.0 append to a failed", "2.2 append to b failed", ...).
- That met `commit_fail_min`. Both probes were clean, so the landed target rule moved the lead to CDG.
- The first merge, admitted at 2.0 s during the stall, took 3.9 s. The second, at 8.0 s, took 1.6 s. After the transfer, merges took 445 to 1,077 ms.

This is the same mechanism as every earlier apac-leader step-down, carried now by Human packs instead of agent packs. The design allows it: the half's gate is Human and MergeQueue at most 3 s with none refused, and it does not require an apac leader.

**Verdict on the split gate:** pass.
- Human+MQ p99 was 1.11 s against the 3 s line, and 0 Human or MergeQueue pushes were refused.
- Every agent push over the cap was refused at once in the `ng` shape and never queued.
- The one merge over 3 s is within the p99, because Human and MergeQueue together had 1,067 commits.

### B0-32 (`b0-32-p8r1`: ATL, CDG, HKG; burst of 500 agent pushes of 256 KiB in 1 s at 60 s, 5 merges queued; 180 s)

| Quantity | `p8r1` | First pass `r1` / `r2` (256 KiB `INLINE_MAX_HOT`) | `ih64r1` (64 KiB) |
| --- | --- | --- | --- |
| Burst pushes ok within 5 s | **505 of 505** (0 refused, 0 errors, 0 `Stale`) | 505 / 505 | 505 |
| Burst push client p50 / p99 / max | 579 / **1,400** / 1,506 ms | p99 3,320 / 2,460 ms | p99 1,381 ms |
| Burst push leader commit p99 | 366 ms | 2,780 / 1,863 ms | 310 ms |
| Burst packs staged | 500 of 505 | 488 to 493 | 500 |
| **Burst stage latency** (500 stages started in the burst second) p50 / p99 / max | **345 / 1,189 / 1,288 ms** | not split out | not split out |
| Stage latency, whole run, p50 / p99 | 299 / 1,169 ms (604 stages) | 506 / 1,677 ms, 451 / 2,118 ms | 368 / 7,060 ms |
| **Burst merges**, leader / client ms | **129, 180, 197, 227, 227 / 163 to 274 ms**, each in the next entry | 1,961 to 2,295 (r1), 842 to 1,459 (r2) at the leader | 149 to 266 ms |
| **Merge wait, whole run** (35 merges) | leader p50 197 ms, p99 259 ms; client p99 281 ms; all 35 in the next entry | not split out | not split out |
| Queued agent peak | 61 commands, 119 KB (0.2% of `AGENT_QUEUE_MAX` 64 MiB) | 556 and 359 commands, 1.67 and 1.68 MB | 59 commands, 1.05 MB |
| Burst entries (60 to 63.5 s) | 41 to 62 commands, at most 80 KB, at most 64 KB inline | about 1 MB | not split out |
| Leader changes, `suspect` | 0, 0 | 0, 0 | 0, 0 |
| Leader req/s; overload errors | 299 (read 270, commit 29, outcome 0); 0 | not split out | not split out |

**The burst no longer touches the merge path.** The edge staged every 256 KiB burst pack before sending it: journey 9's 4.2 pushes/s keeps the group hot (over `HOT_PUSHES`), and 256 KiB is over `INLINE_MAX_HOT` (64 KiB). The leader's per-entry budget therefore had to stage only 2 pushes in the whole run.
- Burst entries carried 41 to 62 commands and at most 80 KB each, against about 1 MB inline per entry in the first pass.
- So the 5 queued merges committed in 129 to 227 ms, each in the next entry. The first pass's `r1` failed at 1.96 to 2.30 s.
- The cost of staging moves to the edge: stage p50 345 ms and p99 1.19 s during the burst, against 299 ms and 1.17 s over the whole run. Burst push client p99 was 1.40 s, inside the 5 s line with 3.6 s to spare.

**What `AGENT_INLINE_ENTRY` adds over `INLINE_MAX_HOT` here: nothing on a hot group.** Both rules stage these packs, and `INLINE_MAX_HOT` acts first. The budget is the rule that covers a burst on a group that is *not* yet hot. The seventh pass measured that (fresh groups' first entries capped at 128 KiB), and this run does not.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-32 (eighth pass) | 505 of 505 burst pushes within 5 s (client p99 1.40 s), 0 refused; queued agent peak 119 KB; burst merges 129 to 227 ms, every merge in the next entry; burst stage p99 1.19 s; 0 leader changes, 0 `suspect` | Every push within 5 s or refused in the `ng` shape, queued bytes under `AGENT_QUEUE_MAX`, every merge in one of the next 2 entries, 0 leader changes, 0 `suspect`; fail on a merge over 2 s | **Pass** |

### B0-33 (`b0-33-p8r1g1`: ATL, CDG, SIN; far-flung, fresh group; 300 s, 60 clients)

| Quantity | `p8r1` | Seventh pass `p7r1` to `p7r3` |
| --- | --- | --- |
| Leader changes; `suspect` received by members | **0; 0** | 0; 0 |
| Human+MQ commit p99, whole / from 30 s | **674 / 606 ms** | 746 to 943 / 652 to 799 ms |
| Merges ok of 50, leader p99, landed in | 50, 718 ms, all in the next entry | 50, 795 to 881 ms |
| Agent committed / refused per s | 250.1 / 0 | 252.6 to 253.7 / 0 |
| Entries/s; commands p50; bytes p50 / p99 | 5.60; 45; 165 / 215 KB | 3.95 to 5.50; 44 to 61; 163 to 181 / 220 to 276 KB |
| Agent inline bytes per entry, max | 131,045 | 131,070 to 131,072 |
| Leader req/s | 507 (commit 273, read 234, outcome 0) | 494 to 504 |
| Held commit calls, peak; holds expired | 219; 0 | 246 to 354; 63 to 527 |
| Overload / deadline client errors | 0 / 0 | 0 / 0 to 5 |
| User token classed Agent after its first minute | 399 of 399 | 399 of 399 |
| Refusals | none | none |
| AppendEntries deadline episodes to SIN | 15 (first at 3.2 s, 7 failures before 11.1 s; then 1 to 3 a minute) | 3 to 14 failures in the first 30 s, then a few a minute |
| Commit-error trips | none: under 5% and under `commit_fail_min` | none |

**`commit_fail_min` did not change B0-33.** No window in the run passed the 5% rate, so the minimum count was never consulted. Every line matches the seventh pass.

**One harness artifact: 205 agent pushes came back `Stale`.**
- **The cause:** load client 6 (IAD, a pusher owning 209 agents) reset about 188 s into the run. Its alarm was retried with an empty tip map. The retry started 202.8 s late and skipped 318 slots, and it lost the first attempt's rows.
- **The evidence:**
  - The first-round pushes total 4,791, which is 5,000 minus that client's 209.
  - All 205 pushes the retry made named no tip, for refs the lost first attempt had already pushed.
  - The members answered each one `Stale`, as the compare-and-set should.
- **No effect on the members:** no apply error, no boot, and every member at the same applied index. The gate's `suspect` count comes from the members' own counter, which is 0.
- **What the lost rows hide:** the commit times of that client's first 188 s. Its share was 1/60 of side commands and 1/24 of pushes.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-33 (eighth pass) | 0 leader changes, 0 `suspect`, Human+MQ p99 674 ms, 250.1 agent/s admitted, token Agent, no refusals, leader 507 req/s; 205 `Stale` from a load-client reset (harness) | 0 leader changes, 0 `suspect`, Human+MQ p99 at most 1 s, at least 140 agent/s, user token Agent, `ng` or 429 shapes | **Pass** |

### Eighth-pass verdicts

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-31, buckets up | Apac leader (NRT) held all 600 s: commit p99 598 ms, Human+MQ p99 601 ms, merges p99 713 ms, 0 leader changes, 0 `suspect`, 2,500 of 2,500 pushes, every one staged | Commit p99 at most 1 s | **Pass**, on an apac leader for the first time |
| B0-31, placement failing | 0 of 287 Human and MergeQueue pushes refused; Human+MQ p99 1.11 s; 2,500 of 2,500 agent pushes refused in the `ng` shape at once (p99 494 ms), none queued; one `STEP_DOWN` from apac at 8.9 s on inline Human packs | Human+MQ p99 at most 3 s, 0 refused, agent over the cap refused at once | **Pass** |
| B0-32 | 505 of 505 burst pushes within 5 s (p99 1.40 s), burst stage p99 1.19 s, burst merges 129 to 227 ms, all 35 merges in the next entry (p99 259 ms), queued peak 119 KB, 0 changes, 0 `suspect` | Every push within 5 s, queued bytes under `AGENT_QUEUE_MAX`, every merge in one of the next 2 entries, 0 changes, 0 `suspect`; fail over 2 s | **Pass** |
| B0-33 | 0 changes, 0 `suspect`, Human+MQ p99 674 ms, 250.1 agent/s, token Agent, no refusals | As in the seventh pass | **Pass** |

**What `commit_fail_min` changed.** In these four runs it decided nothing. Only `b0-31o-p8nop` reached the commit-error rate (0.60 and 0.18 at 8.9 s), and it had 6 and 3 failed calls, so it tripped either way. No run logged a rate over 5% with fewer than 3 failures. The rule's effect, holding a leader through one or two failed calls in a quiet 10 s window, belongs to M0-WP4's F37 rows, not to this load. At these rates every window has well over 10 calls per follower.

**What is modeled and what is real.** This is the same as in earlier passes:
- Real: the members, the load clients, R2 staging, storage, and the links between colos.
- Modeled: packs are random bytes, and `decide` checks only the ref's tip.
- `COMMIT_HOLD` (1 s) is this bed's guess.
- Two harness faults are named above: `b0-31-p8up`'s 30th client never started, and `b0-33-p8r1`'s client 6 reset. Neither touched a member.

### Proposed design edits (eighth pass)

44. **`design/DESIGN.md`, Shape, the `STEP_DOWN` doc comment, the B0-31 sentence.** Replace "An apac leader carrying 1 MiB inline entries misses CALL_DEADLINE to both far followers and steps down within 10 s by the commit-error rule when every entry carries one" with: "With `AGENT_INLINE_ENTRY` on every group, an apac leader's agent pushes of 1 MiB are all staged, its entries stay under 20 KB, and it held 600 s with commit p99 598 ms. While placement fails, Human and MergeQueue packs up to 1 MiB still go inline, and the apac leader stepped down once in 9 s by the commit-error rule (6 and 3 failed calls) [measured, `tests/m0/results/m0-wp9.md`, eighth pass]."
45. **`design/DESIGN.md`, Milestone 0 table, B0-31 row.** B0-31's buckets-up half now passes on an apac leader, so `hold-` and every other variant can be dropped from the method. Add, under the placement-failing half: "the apac leader may move once on the Human packs that the outage rule keeps inline; the half gates Human and MergeQueue latency, not the leader's site."
46. **`design/DESIGN.md`, Shape, `AGENT_INLINE_ENTRY` comment, and the Milestone 0 table, B0-32 row.** On a hot group, `INLINE_MAX_HOT` stages the 256 KiB burst before the budget is reached. B0-32 therefore tests `INLINE_MAX_HOT`, and the budget is the rule for a group that is not yet hot. The numbers for the B0-32 row are: burst merges 129 to 227 ms (was up to 2.30 s), burst push p99 1.40 s, burst stage p99 1.19 s [measured, eighth pass].
    - To test the budget itself, add a B0-32 variant that bursts on a quiet group: no journey 9 pushes in the 5 s before. This is optional, and an operator call.
47. **`design/DESIGN.md`, `STEP_DOWN` constants.** Record `commit_fail_min: 3` as adopted. B0-29 to B0-33 under load neither reached nor needed it: the one trip had 6 and 3 failed calls. Its evidence stays with M0-WP4.

## Teardown (eighth pass)

On 2026-10-31, after the last run, `node run/wp9.ts delete` removed `graft-wp9-a/b/c` and `graft-wp9-load`, and emptied and deleted the buckets `graft-wp9-stage` and `graft-wp9-stage-c`. Checks after teardown:

- every `graft-wp9-*` script, including `graft-wp9-git`, answers 404;
- `wrangler r2 bucket list` and `wrangler containers list` show nothing named `graft-wp9`;
- no `wp9-*` orb service is running.

Nothing with the `graft-wp9` prefix is deployed. `run/wp9-rescue.ts` stays in the tree as the recovery for a runner that dies after `/start`.
