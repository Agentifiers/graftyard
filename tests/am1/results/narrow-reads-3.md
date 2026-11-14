# narrow-reads-3: the learners' narrow tier read

Branch `narrow-reads-3`, from main 20147db. Decision record `decisions/DECISIONS.md`, "`narrow-reads-3`"; F65. Hosted on `graft-m3e` (this branch's own install, built by LF-13 from main 20147db), 2026-11-13. Its shard containers run `SHARD_INSTANCE=standard-4`, which the operator approved for test installs; that does not touch the learners, which are Durable Objects.

## Summary

**Final tree (main 84d29c2 merged), later the same day: the gate is still not met.** Six 500-agent runs, none with a leader change, none passing; the three-in-a-row count has not started. Main's ENTRY_SOFT puts B0-29's commit p99 at 2.9 s (949 ms with `ENTRY_MAX`), and far learners' `late` answers keep API p99 at 2.5 to 3.1 s. An edge hedge was tried and reverted: it served fewer reads. See "The final tree's gate runs". The earlier runs below are on the pre-merge tree.

The learners' narrow tier read is fixed. At 500 agents with 8 learners, its share of API latency at the edge fell from p99 2,879 ms to 1,669 ms, and the learners' `late` answers fell from 96 to 3,399 per learner to 17 to 263. Every learner read now waits for at most one read-index round, never a second one.

**The B0-29 API gate is not met on a run with no leader change.** The best clean run, after run 4, served 99.9% at p50 559 ms and **p99 3,279 ms**, against 2 s. The p99 there is no longer the tier read. It is cold edge isolates whose identity, name, and `Release` genesis reads take 2 to 4 s before the tier read starts. The fix for that changes how an edge isolate learns the system chains' records, which is a design question and is asked below. After run 2 met the gate's p99 within 112 ms (2,112 ms, 99.97% served) on a run with the same build, so the gate is near but not reliably met.

| `read:api`, 500 agents, 200 reads/s, 300 s | Before (main 20147db) | After run 2 | After run 4 |
| --- | --- | --- | --- |
| Repository | nr3-before1, repo:65537 | nr3-after2, repo:65539 | nr3-after4, repo:65540 |
| Load window | 04:35:42 to 04:40:42Z | 06:12:27 to 06:17:27Z | 06:57:46 to 07:02:46Z |
| 200 / 503 / skipped at the driver's cap | 58,425 / 323 / 52 | 58,701 / 17 / 0 | 58,406 / 70 / 201 |
| Served, of answered | 99.5% | 99.97% | 99.9% |
| **p50 / p99** | **732 / 3,262 ms** | **687 / 2,112 ms** | **559 / 3,279 ms** |
| `tier` stage at the edge, p50 / p99 | 674 / 2,879 ms | 637 / 1,964 ms | 515 / 1,669 ms |
| `tier_grant` (member: arrival to the leader's read index applied), p50 / p99 | not measured | 247 / 964 ms | 265 / 912 ms |
| `tier_wait` (member: waiting for an answer build), p50 / p99 | not measured | 0 / 100 ms | 0 / 97 ms |
| `resolve` / `auth`, p99 | 911 / 728 ms | 577 / 293 ms | 2,153 / 2,078 ms |
| Edge CPU per answer, `/issues` and GraphQL, p50 / p99 | 4 / 58 and 2 / 76 ms | 4 / 55 and 2 / 71 ms | 3 / 59 and 2 / 72 ms |
| Leader change during the load | none (term 2) | none (term 2) | none (term 2) |
| Learners' `late` (300 s, per learner) | 96 to 3,399 | 7 to 2,472 | 17 to 263 |
| Learners' `waited` | 1,002 to 8,390 | 209 to 2,001 | 268 to 791 |
| Agent pushes ok, p50 / p99 | 1,236 of 1,237, 2,240 / 4,550 ms | 1,252 of 1,253, 2,088 / 4,391 ms | 1,257 of 1,263, 2,127 / 5,355 ms |
| `read:ref` served, p99 | 99.99%, 3,753 ms | 99.99%, 3,253 ms | 99.4%, 3,904 ms |
| Read-your-write probe (`tests/nr3/fresh.mjs`), stale | 0 of 150 [see "The probe"] | 0 of 149 | 0 of 150 |

`apiStages` in the rows are the driver's per-reply `server-timing`; stages overlap (auth and resolve run at once), so they do not sum to `total`.

**After runs 1 and 3 are not results.** Each had a leader change during the load. Run 1 (repo:65538, 05:11:35Z) moved to term 10 after site b's re-placed voter and site a's leader ended 1,081 and 1,017 invocations `exceededMemory` and `exception`, peaking at 286 and 281 MiB: 85.6% served, p99 20.3 s. Run 3 reloaded run 2's repository, which had grown to about 1,800 entries, and moved the leader from site 0 to 2 (term 7): 93.7% served, p99 3.95 s, with 501 `exceededMemory` on site a. Runs 2 and 4, on fresh repositories, had no reset and no leader change, and every site outcome was `ok` or `canceled`. The resets are voters, not learners: learners peaked at 59 to 93 MiB in every run. I did not find what drove the voters to 280 MiB in runs 1 and 3; it is F65's open shape, not this change's, since the learner path is the only one that changed, and runs 2 and 4 on the same build peaked at 178 and 216 MiB. Rows: `tests/nr3/rows/a1-b0-29-nr3after.jsonl` and `-nr3after3.jsonl`.

## Where the tier read's time went, and the change

The before run's learner reads waited for an answer build, then redid a full read-index round (a new batch behind the one in flight, a 50 ms batch floor, and a leader round trip). By then the tiers had moved again, so the answer the read waited for was stale and it could wait again, up to `FOLLOWER_WAIT` (1 s), then answer `late`. A narrow build costs 2 to 4 ms natively on 500 PRs: REST `/issues?per_page=30` p50 2.3 ms and max 3.4 ms, 44 KB; GraphQL `pullRequests(last:10)` p50 1.8 ms and max 3.8 ms, 15 KB. The 200 ms spacing therefore protected little, and the repeated read-index cost most of the wait.

The change (commit e07375f, design first in DESIGN "Follower read"):

- **Every grant carries a floor** (`raft::ReadGrant::floor`): the commit index the leader confirmed for that read. That is the leader's read index on a follower or learner, and the applied index on the leader.
- **A cached answer serves any read whose floor it covers.** A member keeps each narrow query's newest answer with the index it was built at, and with whether the metadata tier was loaded. A read whose floor that index covers is served at once, with no wait.
- **A read that waits is answered later at the member's current state, with no second read-index** (`raft::Member::fresh_after`, refused after a restart). `NARROW_EVERY_MS` goes from 200 to 100 ms. Answers shared within one input are keyed by floor.
- **Per-boot read-index batches** (`RemoteReads::starting_at`), for the bug below.
- **Stage timing.** `/stats` has `readStages` (grant, waited, and total, with their counts), `narrowBuilds`, and `narrowBuildMs`. Every read answer carries `"stages": [grant, waited]`, which the edge adds to `server-timing` as `tier_grant` and `tier_wait`. `tests/nr3/trace.mjs` splits each learner's grant into its read-index round trip and its apply wait, from the test-faults trace.

The correctness rule is unchanged: no answer older than the commit index the leader confirmed for that read. An answer built at index `i` is the fold at `i`, a function of the committed log, so one at `i ≥ floor` holds every entry acknowledged before the read arrived. A later answer at the member's current state is also at or above the floor, since within a boot the applied index never moves back.

After the change, the measured split on run 4's learners (`tests/nr3/rows/run-nr3after4.status`):

| Learner (site) | grant p50 / p99 | waited p99 | read-index round trip p50 / p99 | apply wait after it, p99 |
| --- | --- | --- | --- | --- |
| l0 (enam) | 231 / 782 | 93 | 124 / 546 | 104 |
| l3 (enam) | 219 / 807 | 100 | 112 / 568 | 264 |
| l6 (enam) | 175 / 822 | 100 | 88 / 713 | 127 |
| l7 (enam) | 83 / 513 | 58 | 29 / 348 | 0 |
| l1 (weur) | 335 / 887 | 100 | 186 / 646 | 672 |
| l4 (weur) | 349 / 868 | 100 | 195 / 640 | 656 |
| l2 (apac) | 486 / 978 | 99 | 243 / 721 | 1,677 |
| l5 (apac) | 436 / 964 | 98 | 246 / 669 | 1,273 |

The build wait is at most about one `NARROW_EVERY_MS` (100 ms) at p99. Every learner's narrow build measured 0 ms (`narrowBuildMs` max 0, the isolate's millisecond clock). What is left in a learner read is the read-index round trip to the enam leader, which grows with distance, and on weur and apac the wait to apply the leader's index after it answers: the leader's AppendEntries reach those learners later than its `ReadIndexRes` does. Both are bounded by `FOLLOWER_WAIT` (1 s), so the tier stage at the edge is now at most about 1 s plus the edge's own calls. 1.7 s at p99 is a read whose first learner answered `late` and was retried on the next member.

What was not changed, and why:

- **Share one read-index confirm across waiting reads.** A learner already sends one batch for every read waiting at that moment, so this was already the case.
- **Serve a cached answer the read-index covers instead of waiting.** Done: this is the floor rule above.
- **Remove `NARROW_EVERY_MS`.** Lowered to 100 ms. The build is cheap, but builds are 1,300 to 2,000 per learner per 300 s already. With no spacing, a learner rebuilds every query on every entry it applies, and the hosted run should show the build's share first. It shows 0 ms, so a later change can drop the spacing. The p99 is not there.
- **No learners added** (operator).

## The final tree's gate runs

These runs are on the final raft tree. narrow-reads-3 has main 84d29c2 merged in (F67 rules 10 to 13, ENTRY_SOFT, hot-shards-4).

- **Site day** 13:23 to 13:34Z (dc5b200): sites and edge exited 0, and the edge carries `GRAFTYARD_GENESIS_SIGNED`.
- **`tests/nr2/stale.mjs`:** 6 of 6.
- **Widened long sweep:** exited 0. HARSH max 16,027 ms, gray 26,832 ms, live max 22,348 ms over 4,000 seeds (seed 11507). Porcupine was Ok on all three histories.
- **Setup:** `SHARD_INSTANCE=standard-4`. Each run uses a fresh 500-PR repository, 8 learners (enam 4,000, weur 2,000, apac 2,000), and 500 agents for 300 s.

| Run | Edge | Entry cap | Repository, load from | Served | API p50 / p99 | `tier` p99 | Leader commit p50 / p99 | Agent push p99 | Leader change |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gate1 | dc5b200 | soft (60 KB) | 65543, 13:56:01Z | 99.84% | 733 / **2,617** | 2,557 | 626 / 2,902 | 5,489 | none |
| gate2 | dc5b200 | soft | 65544, 14:17:48Z | 98.77% | 884 / **3,146** | 3,043 | 721 / 2,893 | 5,692 | none |
| abmax, diagnostic | dc5b200 | `ENTRY_MAX` | 65545, 14:49:38Z | 99.74% | 807 / 2,519 | 2,436 | 471 / 949 | 5,235 | none |
| gate3 | 8c54adb (hedge) | soft | 65546, 17:09:34Z | 81.9% (98.73% of answered) | 885 / **6,982** | 1,965 | 9,651 / 18,559 | 45,399 | none |
| gate4 | 8c54adb (hedge) | soft | 65547, 17:33:30Z | **98.69%** | 811 / **1,988** | 1,806 | 3,504 / 21,306 | 59,521 | none |
| abmax2, diagnostic | 8c54adb (hedge) | `ENTRY_MAX` | 65548, 17:58Z | 97.11% | 956 / 2,306 | 2,056 | 538 / 2,267 | 7,337 | none |

**No run passes, so the three-in-a-row count has not started.**

- Gates 1 and 2 fail on p99.
- Gate 3 fails on p99 and served share.
- Gate 4 meets the p99 (1,988 ms) but serves 98.69%, under 99%.

The read-your-write probe found 0 stale answers in every run (150, 150, 150, 119, 82, and 85 rounds).

Rows: `tests/nr3/rows/a1-b0-29-{gate1,gate2,abmax,gate3,gate4,abmax2}.jsonl`, with each run's `run-*.status` and `fresh-*.jsonl`. Gate 3's driver-capped share is the commit-path collapse described below. Of the reads that were answered, 98.73% were served at a `server-timing` p99 of 2,233 ms.

### The entry cap sets B0-29's commit line, not its API line

| | Pre-merge tree (2c9329b) | Final tree, ENTRY_SOFT (gate1, gate2) | Final tree, `ENTRY_MAX` (abmax) |
| --- | --- | --- | --- |
| Commit p99 | 820 to 1,166 ms | 2,902 and 2,893 ms | 949 ms |
| Queue p99 | 513 to 791 ms | about 2.7 s | 580 ms |
| `/outcome` calls on the leader | 1 to 42 | 951 and 1,051 | 15 |

`ENTRY_MAX` was set with test-faults `/stats?entry=max` on all three voters. So on this install, main's 60 KB ENTRY_SOFT pushes B0-29's commit p99 past its 1 s fail line. The API p99 barely moved with the cap lifted (2,519 ms). The cap is main's design and B0-29's commit line belongs to LM-1, so I sent the data to LM-1 and m0-carried-8 instead of changing it.

### The tier read's tail, and a hedge that did not work

The edge's 1-in-100 sampled stages over gate1, gate2, and abmax (985 answers):

- the tier read's p50 is 747 ms and its p99 2,629 ms;
- in 22% of answers, more than 900 ms of the tier read is outside the member's grant and build. That is a first learner that answered `late` after `FOLLOWER_WAIT` (1 s), followed by the next member's whole read;
- learners on the leader's site answered `late` to 3 to 9% of their reads, and weur and apac learners to 19 to 49%. A far learner's apply wait after the leader's `ReadIndexRes` reaches 1.4 to 2.1 s at p99.

A replay of those samples put the tier p99 near 1.9 s with a hedge: the faster of two random learners first (by a per-isolate moving average), and the next learner beside it after 400 ms. I built it as 8c54adb, design first, with 7 plants, all caught; full checks passed with 827 tests. Then I ran it hosted.

**It is reverted.** The paired comparison holds the commit path still with `ENTRY_MAX` on both sides (abmax against abmax2):

| | abmax, no hedge | abmax2, hedge |
| --- | --- | --- |
| API p99 | 2,519 ms | 2,306 ms |
| Served | 99.74% | 97.11% |
| Learner reads in 300 s, each | 7,900 to 12,400 | 17,000 to 22,000 |
| Learner `late`, each | 239 to 4,621 | 1,835 to 9,968 |
| Learner shed, each | 1 to 10 | 19 to 450 |
| Commit p99 | 949 ms | 2,267 ms |

The replay assumed a second learner answers as fast as the first did without the extra load. Hosted, the learners' read-index path and apply wait are the bottleneck: twice the reads made far more of them `late`, and the 503s rose from 0.26% to 2.9%. The commit p99 also rose, which fits extra read-index rounds on the leader [inference]. The two hedged runs with ENTRY_SOFT (gate3, gate4) collapsed further: commit p99 18.6 and 21.3 s, agent push p99 45 to 60 s, and gate 3's driver capped. ENTRY_SOFT's queue and the hedge's load together are a plausible cause; I have not separated them.

Members stayed small through all of it. Leaders peaked at 70 to 80 MiB and learners at 58 to 84 MiB. There was no `exceededMemory` and no leader change. The only other outcomes were 1,988 `exception` on site a in abmax and 1 in gate4.

**Where the gate stands on this tree.**

- The read side is about 2.5 to 3.1 s at p99 without the hedge. That is set by far learners' `late` answers.
- The commit side is past its fail line with ENTRY_SOFT.

Without adding learners, the next cut is the far learners' apply wait: the leader's AppendEntries reach weur and apac after its `ReadIndexRes` does. Two options, both changes to Raft or the member, so I am not making them without a decision:

- a learner answers from the commit index carried on the `ReadIndexRes`'s own envelope;
- the leader sends the entries a far learner lacks with its `ReadIndexRes`.

## Learners pipelined (operator's choice (c)): reverted

**Reverted in 6f0a50a5** on the operator's decision: gate 5 showed no gain in the far learners' apply wait, and the leader's peak memory rose to 126 MiB, near the 128 MB isolate. DECISIONS records it as "Not adopted: pipelining AppendEntries to learners", with the numbers below. The kept test and plants 15 and 16 went with it; 9e16a8af's per-peer `sent` and `sentBytes` stay. What follows is the record of what was tried.

The operator chose (c) over piggybacking entries on `ReadIndexRes` or a learner-side fetch. Main 82a0bf9 (m0-carried-8: PIPELINE 4, APPEND_BYTES_MAX and ENTRY_SOFT 100 KB) pipelined only to voters (`voter && !lossy`), so every learner still got one AppendEntries at a time. On a 300 to 700 ms far link, that alone puts a learner's appends behind the leader's `ReadIndexRes`.

The change, 682468de, design first (DESIGN "Commit and read paths", DECISIONS "Learners are pipelined as voters are"):

- `send_append` pipelines to a learner as to a voter: the same `PIPELINE` bound, the same `APPEND_BYTES_MAX`, and one call at a time once `link_lossy` holds for it.
- A learner counts toward no quorum or lease, so no commit or read rule changes.
- 9e16a8af adds the leader's calls and bytes per peer to `/stats` `peers`, with `tests/nr3/egress.mjs` after each run, for egress.

Checks on 9e16a8af (main 82a0bf9 merged at 29041e83):

- `crates/raft/tests/scenarios.rs`, `a_learner_is_pipelined_unless_its_link_is_lossy`: a silent learner on a clean link gets PIPELINE calls with entries, and on a lossy link 1. It **fails on main 82a0bf9** with 1 call where it expects 4.
- `tests/nr3/plant.py` plants 15 (only voters pipelined) and 16 (a lossy learner still pipelined): both caught.
- `cargo test -p graftyard-raft`: all pass.
- Widened long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, live half 4,000 seeds): exited 0. HARSH longest 15,735 ms (seed 11431), gray 27,226 ms (seed 11551), live 28,340 ms (seed 11824), all under the 30 s bound.
- Porcupine: Ok on all three histories (155,506 / 2,226,302 / 5,370,591 operations).
- The simulator models no learners, so the sweep shows the change leaves voters' behaviour intact. It does not exercise the learner path; the kept test and the hosted runs do.
- Site day 18:46 to 18:58Z from 9e16a8af: sites and edge exited 0. Shard image f6d2ef40 is unchanged: main's only container change is the web app's `/ui` routes.

### Gate 5 on 9e16a8af: the gate fails, and pipelining did not cut the far learners' apply wait

gate5: repo:65549, load from 19:15:00Z, 500 agents, 300 s, main 82a0bf9 as built (ENTRY_SOFT 100 KB):

- **API:** 99.92% served (44 × 503), p50 794 ms, **p99 2,475 ms**; tier p99 2,410 ms; auth p99 0 and resolve p99 414 ms.
- **Fails on p99.** It is the first final-tree run, so the three-in-a-row count is still at 0.
- **Commit:** p50 557 ms, p99 1,923 ms (queue p99 1,659 ms); 360 `/outcome` calls.
- **Agent push:** 1,255 of 1,255 ok, p99 5,193 ms.
- **Other:** no leader change; 0 stale in 118 probe rounds.

Far learners' apply wait after their `ReadIndexRes` (`tests/nr3/trace.mjs`), p99 in ms:

| Learner | gate1 (no learner pipeline) | gate5 (pipelined) |
| --- | --- | --- |
| l1 weur | 969 | 1,415 |
| l4 weur | 764 | 878 |
| l2 apac | 1,670 | 1,671 |
| l5 apac | 1,442 | 1,858 |
| l0, l3, l6, l7 enam | 13 to 364 | 7 to 398 |

The far learners still answered `late` to 1,743 to 2,870 of 7,800 to 9,400 reads, against 277 to 618 on enam.

**Why pipelining did not move it [inference, read from the code].** A busy learner almost never receives AppendEntries as a call. While it calls the leader (every 50 to 100 ms with read-index batches), the leader holds its messages for the response to the learner's next call (`workers/site/src/callers.rs`, the subrequest-depth rule). Its appends and its `ReadIndexRes` therefore already travel in the same responses, held messages first. The leader's `/stats` peers show it: in 300 s the leader made 1,119 to 1,129 calls to each voter but only 46 to 60 to each learner. So the far learners' tail is not the leader waiting to send entries. It is the time for those responses, and the applies they carry, to cross a 300 to 700 ms link. Option (a), entries piggybacked on `ReadIndexRes`, sends the same bytes on the same path, so I expect it to change little too.

**Costs.**

- Leader CPU: 52.5 s over the run, against 56.6 s on abmax.
- Leader egress as calls to voters: 139 MB; to learners as calls: about 3 MB. `peers` counts only calls, not messages held for a learner's response.
- **The leader's peak memory rose to 126 MiB**, against 67 to 80 MiB in every earlier final-tree run, near the 128 MB isolate. Up to PIPELINE batches of up to 100 KB, or one entry up to ENTRY_MAX, held per learner for 8 learners fits that rise. I have not proven it.

Rows: `tests/nr3/rows/a1-b0-29-gate5.jsonl`, `run-gate5.status`, `fresh-gate5.jsonl`, `egress-gate5.txt`, `mem-gate5.json`. repo:65549 torn down.

## Where a far learner's wait goes (gate 6)

The operator asked for a per-read split of the far learners' wait before choosing between (a), entries in the leader's `ReadIndexRes`, and learner-side work. a17a9138 adds the traces: every AppendEntries a member steps (`Trace::AppendStepped`, its last entry and commit, and whether it rode the response to the member's own call or came as a call), and on the leader each AppendEntries held for a learner's next call and taken into a response (`AppendHeld`, `AppendTaken`). `tests/nr3/split.mjs` splits each `ReadIndex` batch a learner sent into three parts:

- (i) **rtt**: sent until its `ReadIndexRes` stepped.
- (ii) **hold**: that answer until the learner held the entries through the read index R and a commit at or past R. It is 0 when it already held entry R, which the answer commits by digest.
- (iii) **apply**: from then until its applied index reached R.

gate6: site day from a17a9138 (the reverted tree, learners not pipelined) 19:52 to 20:03Z, then repo:65550 with the load at 20:15:15Z, 500 agents, 300 s, main as built. **It counts as a gate run, and fails**:

- **API:** 98.36% served (988 × 503), p50 811 ms, **p99 3,159 ms**; tier p99 3,083 ms; auth p99 71 ms; resolve p99 443 ms.
- **Commit:** p50 409 ms, p99 1,442 ms. Agent push: 1,247 of 1,249 ok, p99 5.3 s.
- **Other:** no leader change, 0 stale in 147 probe rounds. Leader peak 70 MiB, learners 57 to 61 MiB.

The split, per learner, in ms (p50 / p99):

| Learner | rtt | hold (ii) | apply (iii) | total | how the entries through R arrived (same response / later own call / leader call / already held) |
| --- | --- | --- | --- | --- | --- |
| l1 weur | 174 / 639 | **638 / 1,933** | 0 / 709 | 847 / 2,125 | 31 / 1,309 / 2 / 175 |
| l4 weur | 164 / 603 | **624 / 4,451** | 0 / 504 | 818 / 4,613 | 28 / 1,298 / 5 / 178 |
| l2 apac | 189 / 488 | **574 / 3,111** | 0 / 559 | 797 / 3,454 | 8 / 1,236 / 6 / 296 |
| l5 apac | 191 / 476 | **668 / 2,692** | 0 / 461 | 875 / 2,872 | 16 / 1,267 / 5 / 228 |
| l0 enam | 48 / 908 | 0 / 305 | 39 / 1,197 | 128 / 1,414 | 153 / 170 / 3 / 1,568 |
| l3 enam | 78 / 806 | 0 / 643 | 0 / 1,300 | 164 / 1,436 | 113 / 474 / 2 / 1,282 |
| l6 enam | 62 / 958 | 0 / 993 | 0 / 1,048 | 150 / 1,625 | 132 / 516 / 3 / 1,087 |
| l7 enam | 73 / 838 | 0 / 612 | 0 / 1,326 | 161 / 1,504 | 85 / 434 / 5 / 1,355 |

**(ii) dominates on the far learners.** Their answer comes back at p99 under 640 ms. The entries it names come in a later response to their own call in 82 to 87% of batches, and in the same response in only 0.5 to 2%. Held messages are not capped by APPEND_BYTES_MAX as a queue: the leader sends a learner one AppendEntries call at a time (`send_append`, `voter && !lossy`), and while the learner calls, each is held for its next call's response (`workers/site/src/callers.rs`). So when the leader answers a far learner's batch at index R, the entries through R usually have not been sent to that learner at all: they go out only after the learner's previous append is answered, which is one of its own calls later.

**(iii) is the near learners' wait.** They already held entry R in 58 to 83% of batches, and their apply p99 of 1.0 to 1.3 s sits with storage-confirm p99 of 1.0 to 1.4 s on the same learners, against 59 to 204 ms on apac and weur [measured; that confirms cause it is an inference]. That is learner-side work and the next limit after (ii).

**Why gate 6 is worse than gates 1 to 4.** Main 82a0bf9 cut APPEND_BYTES_MAX from 1.5 MB to 100 KB. Gates 1 to 4 (dc5b200, 8c54adb) ran at 1.5 MB, where one learner call carried its whole backlog. Gate 5 ran at 100 KB with learners pipelined. Gate 6 is the first run at 100 KB with one call at a time, so a far learner behind by more than 100 KB needs several calls, each one of its own round trips later. The far learners' apply p50 was 0 to 495 ms in gates 1 to 5 and 564 to 667 ms here [measured; the cause is an inference].

### The change: (a), entries ahead of the answer

Design first: DESIGN "Follower read" and DECISIONS "A read-index answer follows the entries the asker needs". In `read_resolved` (`crates/raft/src/member/install.rs`), before the leader sends a `ReadIndexRes` at index i, `append_for_read` (`leader.rs`) sends that member the entries through i it was not yet sent:

- It is pipelined as a voter's would be. It follows the member's outstanding calls, within `PIPELINE` and `APPEND_BYTES_MAX`, and not while the link is lossy (F67 rule 13). A catch-up, install, or failed call's retry still out sends nothing; `tick` sends after it.
- On the host the AppendEntries and the answer leave in one `Out`, appends first, so a learner calling the leader gets both in the same response and steps the entries through the normal append path before the answer.
- An answer with no entries ahead of it stays valid. No commit, lease, or grant rule changes.

Checks:

- Kept test `crates/raft/tests/scenarios.rs`, `a_learner_read_index_answer_follows_the_entries_it_needs`: a learner on a clean link that missed two entries, then one, gets an append through the read index with a commit at or past it ahead of each answer; a lossy learner gets none. It **fails without the change** (no such append). Gate 8 below revised where that append starts.
- `tests/nr3/plant.py` read-entries mutants, all caught (see gate 8 for the final set).
- `cargo test -p graftyard-raft`: all pass.

### Gate 7 on f1ab1a61: far learners' wait for entries falls, and the gate still fails

Site day from f1ab1a61 21:37 to 21:48Z (sites and edge exit 0); repo:65551, load from 22:02:12Z, 500 agents, 300 s.

- **API:** 99.29% served (411 × 503), p50 835 ms, **p99 2,784 ms**; tier p99 2,688 ms; auth p99 74 ms; resolve p99 454 ms. **Fails on p99.**
- **Commit:** p50 492 ms, p99 1,688 ms. Merges p99 2,053 ms. Agent push 1,251 of 1,252 ok, p99 5.2 s.
- **Other:** no leader change; 0 stale in 150 probe rounds. Leader 61 MiB; learners 56 to 62 MiB.
- **Leader egress:** calls to learners 14 to 26 each, 0.0 to 0.8 MB, as before. It held 962 to 1,095 AppendEntries per learner for that learner's next call, and they waited p99 241 to 389 ms there.

The split against gate 6, p99 in ms:

| Learner | hold (ii), gate6 → gate7 | apply (iii), gate6 → gate7 | total, gate6 → gate7 |
| --- | --- | --- | --- |
| l1 weur | 1,933 → 861 | 709 → 1,600 | 2,125 → 1,873 |
| l4 weur | 4,451 → 404 | 504 → 1,280 | 4,613 → 1,582 |
| l2 apac | 3,111 → **7,832** | 559 → 1,728 | 3,454 → **8,675** |
| l5 apac | 2,692 → 1,210 | 461 → 1,359 | 2,872 → 2,186 |
| l0, l3, l6, l7 enam | 0 to 993 → 0 to 1,530 | 1,048 to 1,326 → 1,301 to 1,842 | 1,414 to 1,625 → 1,408 to 2,046 |

- **The change worked on three of four far learners.** Entries through the read index were already held, or came with the answer, in 63 to 80% of their batches, against 12 to 20% in gate 6.
- **One apac learner, l2, got worse.** p50 hold 1,152 ms, and 6,799 of its 8,987 reads `late`. Its trace starts at the load and holds 7,065 entries, so its tail is not a restart. I have not traced why.
- **Every learner's own apply is now p99 1.3 to 1.8 s.** It is the larger part on the near learners, and on the far learners once their hold fell. Their storage confirms are p99 0.97 to 1.4 s on enam and 0.02 to 0.34 s on weur and apac, so confirms do not explain the far learners' apply.
- **Sweep and Porcupine on f1ab1a61's raft:** the widened long sweep exited 0. HARSH's longest stall was 15,735 ms, gray's 27,226 ms, and live's 28,340 ms (4,000 seeds), all under 30 s. Porcupine was Ok on all three histories. The totals equal gate 5's tree's to the message. That is expected: a voter is sent every entry as it is cut, so `append_for_read` finds nothing unsent for a voter, and the simulator models no learners. The kept test and the hosted runs cover the learner path.

Rows: `tests/nr3/rows/a1-b0-29-gate7.jsonl`, `run-gate7.status`, `fresh-gate7.jsonl`, `egress-gate7.txt`, `mem-gate7.json`. repo:65551 torn down.

### Gate 8, and why one apac learner still waited

Gate 8 ran on the same f1ab1a61 sites: repo:65552, load from 22:34:35Z.

- **API:** 99.30% served (384 × 503, 24 driver-capped), p50 858 ms, **p99 3,064 ms**; tier p99 2,984 ms. **Fails.**
- **Commit:** p99 2,835 ms.
- **Other:** no leader change, 0 stale in 149 probe rounds. Leader 64 MiB, learners 56 to 61 MiB.
- **The same shape as gate 7, on the other apac learner.** l5's hold reached p99 4,781 ms (5,753 `late` of 8,428 reads), while l2 dropped to 1,733 ms.

`split.mjs` now keeps the raw traces (`RAW=`; `tests/nr3/rows/trace-gate8.json.gz`). In them, l5 had 455 holds over 500 ms, and 313 of those had an AppendEntries arrive behind a later one inside the hold. The far weur and apac learners had 75 to 192 such out-of-order arrivals each, against 20 to 78 on enam.

A learner's call and its hedged copy (`HEDGE`, 500 ms), and its calls to the leader, end in any order on the far link. f1ab1a61 sent the read's entries after the calls in flight, as a voter's pipeline does. When an append that overtook an earlier one arrived first, the learner refused it, because its `prev` was not held. The refusal emptied the leader's pipeline to that learner, and the entries then waited for the next tick's resend after `CALL_DEADLINE` (1 s), sometimes more than once [measured: the arrivals; the chain from refusal to resend is read from `on_append_res` and `tick_leader`].

**The fix** (DESIGN "Follower read", DECISIONS "A read-index answer follows the entries the asker needs", updated): the read's AppendEntries starts from the first entry the learner has not acknowledged (`next`, a forced send), not after the calls in flight. It also goes out right after a refusal emptied the pipeline. It is not sent to voters, or on a lossy link.

- The kept test now also has the learner refuse its older call and checks that both appends start from the acknowledged entry. It **fails on f1ab1a61's version**.
- Plants 15 to 19 are all caught: none sent; after the calls in flight; none while a call is out; off by one; a lossy learner.

Rows: `tests/nr3/rows/a1-b0-29-gate8.jsonl`, `run-gate8.status`, `fresh-gate8.jsonl`, `egress-gate8.txt`, `mem-gate8.json`, `trace-gate8.json.gz`. repo:65552 torn down.

### Gate 9 on c95216cc: re-sending on every read lost the leader

Site day from c95216cc 22:56 to 23:07Z (sites and edge exit 0). The sweep on c95216cc passed: HARSH 15,735 ms, gray 27,226 ms, live 28,340 ms, Porcupine Ok ×3.

gate9: repo:65553, load from 23:20:49Z.

- **API:** **73.50% served** (12,023 × 503, 3,815 driver-capped), p50 1,560 ms, p99 3,518 ms. **Fails.**
- **Commit:** p99 6,327 ms. Agent pushes: 1,185 of 1,241 ok, p99 19.2 s.
- **Leader:** **2 leader changes**, from site 0 to site 2 at term 4. 0 stale in 130 probe rounds (20 failed with 503). Site 0's member peaked at **122 MiB**, against 61 to 70 MiB in gates 6 to 8; learners 57 to 64 MiB. No `exceededMemory` outcome was recorded.

c95216cc's read append was a forced send from `next` on every read that found the learner without its entries. With a call in flight, that re-sent entries already on the wire, up to `APPEND_BYTES_MAX` per read and up to 8 learners × about 20 reads a second. I take that as the cause of the leader's memory and of the change [inference; the timing fits, and nothing else changed between gate8 and gate9].

**The version that stays**: with calls in flight, the read's append follows them within `PIPELINE`, as f1ab1a61's did, so no entry is sent twice. With the pipeline empty after a refusal or failure, it goes from `next` at once, which is what gate8's apac learner lacked.

- The kept test has the learner's tick call in flight, then a refusal of an older call. It asserts the first append follows the call in flight and the second starts from `next`. A lossy learner gets neither.
- The test fails on both earlier builds. Plants 15 to 20 are all caught: none sent; f1ab1a61's nothing after a refusal; c95216cc's re-send on every read; none while a call is out; off by one; a lossy learner.

Rows: `tests/nr3/rows/a1-b0-29-gate9.jsonl`, `run-gate9.status`, `fresh-gate9.jsonl`, `mem-gate9.json`, `trace-gate9.json.gz`. repo:65553 torn down.

### Gate 10 on 617ed5b9: seven learners under 2 s, one apac learner behind

Site day from 617ed5b9 (sites and edge exit 0). The sweep on 617ed5b9 passed: HARSH 15,735 ms, gray 27,226 ms, live 28,340 ms, Porcupine Ok ×3. gate10 is repo:65554, load from 00:08:36Z.

- **API:** 99.38% served (229 × 503, 126 driver-capped), p50 715 ms, **p99 2,717 ms**; tier p99 2,581 ms. **Fails.**
- **Commit:** p99 1,483 ms. Agent push 1,236 of 1,237 ok.
- **Other:** no leader change; 0 stale in 149 probe rounds.
- **Memory:** **leader peak 118 MiB**, against 61 to 70 MiB in gates 6 to 8; learners 58 to 62 MiB. With gate9's 122 MiB, sending from `next` after a refusal goes with higher leader memory. I have not found why [measured; cause unknown].

Total wait from `ReadIndex` sent to applied, p99: l0 1,202, l1 1,201, l2 1,539, l3 1,311, l4 1,929, l6 1,145, l7 1,409, and **l5 (apac) 6,303 ms** (hold p99 5,582 ms, 6,825 of 9,748 reads `late`).

l5's raw trace (`tests/nr3/rows/trace-gate10.json.gz`) shows no refusal and no gap. Every read's response carried one or two AppendEntries, each about one entry, held to `APPEND_BYTES_MAX` (entries near ENTRY_SOFT). A round trip took about 350 ms, the leader cut entries faster, and l5 stayed about 12 entries behind for the run. One call per read cannot catch up a learner whose round trip carries less than the leader commits.

**Change:** the read's AppendEntries now go up to `PIPELINE` calls in all, each within `APPEND_BYTES_MAX`, after the calls in flight. The kept test adds a third read with entries of 60 KB each, three behind, and asserts three calls go ahead of the answer. Plant 21 (617ed5b9's one call per read) is caught, as are 15 to 20.

Rows: `tests/nr3/rows/a1-b0-29-gate10.jsonl`, `run-gate10.status`, `fresh-gate10.jsonl`, `mem-gate10.json`, `trace-gate10.json.gz`. repo:65554 torn down.

### Gate 11 on 875b9542: up to PIPELINE calls per read

Site day from 875b9542 (sites and edge exit 0). The sweep on 875b9542 passed: HARSH 15,735 ms, gray 27,226 ms, live 28,340 ms, Porcupine Ok ×3. gate11 is repo:65555, load from 00:49:05Z.

- **API:** 99.48% served (296 × 503, 6 driver-capped), p50 787 ms, **p99 2,635 ms**; tier p99 2,501 ms. **Fails.**
- **Commit:** p99 3,369 ms. Agent push 1,243 of 1,244 ok.
- **Other:** no leader change; 0 stale in 149 probe rounds.
- **Memory:** **leader peak 119 MiB** (gates 9 and 10: 122 and 118); learners 56 to 61 MiB.

Per learner, from the raw traces, ReadIndex sent to applied (p50 / p99, ms):

| Learner | gate8 (f1ab1a61) | gate10 (617ed5b9) | gate11 (875b9542) |
| --- | --- | --- | --- |
| l1 weur | 268 / 1,771 | 240 / 1,008 | 238 / 938 |
| l4 weur | 225 / 1,323 | 287 / 1,615 | 262 / 1,144 |
| l2 apac | 497 / 2,431 | 297 / 1,516 | 451 / 2,958 |
| l5 apac | 1,194 / 5,335 | 1,277 / 6,303 | 342 / 2,197 |
| enam l0, l3, l6, l7 | 64 to 168 / 736 to 1,327 | 77 to 126 / 676 to 874 | 103 to 162 / 805 to 920 |

- **The catch-up worked.** No learner stays behind for a run: l5's p50 fell from 1.2 s to 342 ms.
- **The apac tail is still 2.2 to 3.0 s at p99.** Its hold p99 is 1.6 to 2.3 s. On the leader, 168 to 250 of about 970 appends held for each apac learner carried nothing past one already held for it, against 2 to 34 on enam. That fits responses that cross the far link out of order, or late [inference].
- **The weur and enam learners total under 1.2 s at p99.** The edge picks a learner at random, so about a quarter of reads go to apac.

Rows: `tests/nr3/rows/a1-b0-29-gate11.jsonl`, `run-gate11.status`, `fresh-gate11.jsonl`, `mem-gate11.json`, `trace-gate11.json.gz`. repo:65555 torn down.

### Gate 12 on fdf363f3: the adopted B0-29 shape, with routing by answer time

The operator adopted LM-1's B0-29 shape (lm1-memory.md, round 12) for every gate run: a fresh repository, 8 learners
and k 4 placed first, then `a1.mjs b0-29 --open-prs 500 --seconds 600`, which opens 500 draft PRs on the agents' own
head branches and waits for every learner to apply the leader's commit before its window. Gates 1 to 11 ran on idle
PR rows at k 1 and stay recorded but do not count. The tree has the leader's held-bytes and log-bytes instruments
(`/stats?summary=1`, sampled every 15 s by `tests/nr3/lmem.mjs`) and the edge's routing by measured answer time.
The first try (repo 65556) exited before setup on a missing `clock.mjs` and ran no load.

Repo 65557, 06:28 to 06:38Z [measured]:

| Line | Gate 12 | B0-29 pass |
| --- | --- | --- |
| API p50 / p99, served | 672 / 2,011 ms, 99.93% (117,092 reads) | p99 ≤ 2,000 ms, ≥ 99% |
| Tier read at the edge, p50 / p99 | 636 / 1,935 ms | |
| Commit p50 / p99 | 997 / 5,140 ms (queue p99 4,798, round p99 493, confirm p99 280) | p99 ≤ 600 ms |
| merge:only p99 | 5,985 ms | ≤ 2 s |
| Leader | site 0, term 2 throughout, 82.5 req/s, 0 feed polls | no change |
| Read-your-write | 150 of 150 rounds, 0 stale | 0 stale |

A miss on API p99 by 11 ms, and on the commit and merge lines, which LM-1 owns.

**Leader memory.** Setup: 500 of 500 PRs opened with no failure, all 8 learners caught up in 2.6 s, leader wasm 11.3
MiB at the window's start. In the window the leader's high-water was 47.6 MiB by `/stats` and 83 MiB by the analytics
API's per-minute peak (76 MiB for most of it), against 118 to 125 MiB on gates 9 to 11. `logBytes` sat at the 8 MB
`LOG_BYTES` cap, held bytes per learner peaked at 0.10 to 0.37 MB, and across all 8 at 214 KB. Learners peaked at 57
to 62 MiB. The jump at learner join that set gates 9 to 11's baseline does not happen when learners join a near-empty
log, so no code change was made for it.

**Routing by answer time.** Reads per learner (`readStages` n): l6 40.2%, l3 30.8%, l1 10.2%, l7 7.4%, l4 6.1%, l0
3.6%, l2 1.0%, l5 0.7% of 121,587. The two apac learners (l2, l5) took 1.7% between them, against about a quarter
before, and their apply wait (p99 2.9 and 5.1 s) no longer reaches many reads. Two enam learners took 71%, but no
learner ran hot: CPU-s per learner in the window did not follow its read share (narrow answers are built at most once
per `NARROW_EVERY_MS` and shared), peaks stayed at 57 to 62 MiB, and no learner shed more than 56 reads.

| Learner | Region | Reads | CPU-s | Peak MiB | late |
| --- | --- | --- | --- | --- | --- |
| l0 | enam | 4,390 | 328.5 | 59 | 595 |
| l1 | weur | 12,393 | 197.9 | 60 | 463 |
| l2 | apac | 1,191 | 119.6 | 61 | 1,252 |
| l3 | enam | 37,418 | 305.6 | 60 | 548 |
| l4 | weur | 7,470 | 142.1 | 61 | 638 |
| l5 | apac | 865 | 91.8 | 61 | 1,704 |
| l6 | enam | 48,923 | 274.7 | 62 | 521 |
| l7 | enam | 8,937 | 349.7 | 57 | 840 |

**What sets the API p99 now: corrected.** The first split here (`tests/nr3/split.mjs` as of f5463e75) left a read
whose index was already applied when its answer arrived open until the next `Applied` row, and counted that gap as
apply. That is 40 to 44% of reads on the near learners. Counted as 0 (the script is fixed), per read on each learner,
p99 in ms, from `trace-gate12.json` [measured]:

| Learner | Read-index round trip | Wait for entries | Own apply | Total | Total over 1 s |
| --- | --- | --- | --- | --- | --- |
| l3, l6 (enam, 71% of reads) | 578 to 615 | 263 to 296 | 0 to 2 | 634 to 667 | 0.1% |
| l7 (enam) | 830 | 310 | 6 | 873 | 0.3% |
| l0 (enam) | 899 | 389 | 329 | 1,003 | 1.1% |
| l1, l4 (weur) | 726 to 789 | 443 to 533 | 266 to 870 | 919 to 1,501 | 0.5 to 2.6% |
| l2, l5 (apac) | 1,005 to 1,007 | 2,516 to 5,101 | 1,518 to 1,695 | 3,199 to 5,588 | 23 to 43% |

So the busy learners' apply is not slow: their storage confirms are p99 84 to 158 ms (l0 and l7 reach 1.0 to 1.2 s),
and their apply is p99 0 to 6 ms. Neither storage confirm, apply CPU, nor the read share sets their read. What does is
the read-index batch: one in flight, at most one every `LEARNER_READ_EVERY` (50 ms). A read that arrives while a batch
is out waits for that batch's answer and then for its own round trip. Sampling the traced batches at a random arrival
time, the grant wait is p99 889 ms on l6, 1,141 on l3, 1,640 on l7, 1,446 on l1 and 2,675 on l0, against round trips of
p99 0.6 to 0.9 s (`readStages` on the members agree: grant p99 894 to 971 ms, capped by `FOLLOWER_WAIT`). The near
learners' round trip is itself p99 0.6 to 0.9 s under load against about 40 ms idle, and the leader takes and answers
each `ReadIndex` in the same millisecond, so that time is outside the leader's step: the network, the learner's isolate
or the leader's input queue [measured; where inside, not measured].

A replay of the traced round trips at 80 reads/s puts the near learners' wait at p99 874 ms (l6) and 884 ms (l3)
with 1 batch in flight, 715 and 747 with 4, and 630 and 709 with 8; l0 goes from 1,239 to 997 at 8 [inference, a model
over measured round trips]. Replaying the edge over the same traces (each read's isolate routing by answer time as
built, three learners at most, a late answer at 1 s, 230 ms of edge stages outside the tier read), API p99 is 1,804 ms
with 1 batch in flight and 1,216 ms with 8; the same replay with gate 12's measured read shares gives 1,963 ms, against
2,011 measured, so it runs about 50 to 200 ms optimistic. Scoring a learner by its mean plus 4 deviations instead of its
mean gives 1,665 and 1,086 ms: a second, smaller lever, not built [inference, same replay, `/tmp`-only scripts
summarized here]. The edge side: tier p99 1,935 ms against member totals under 1 s on the busy learners comes from reads that
went late (`followerLate` 463 to 840 per near learner) and were retried on a second learner (`SPREAD_TRIES`).

**Where a near learner's read-index round trip goes.** The operator asked what the round trip's rise from about 40
ms idle to p99 0.6 to 0.9 s under load is spent on, from one gate's traces (gate 12, `trace-gate12.json`, the 500 s
the leader's 60,000-row trace covers). Batch numbers above 2^53 lost precision in the JSON, so a learner's `IndexSent`
is matched to the leader's `IndexTaken` by number and time (`tests/nr3/legs.py`). Leader and
learner clocks are different isolates', so the legs carry their skew; the learner-side and leader-side rows do not.
p50 / p90 / p99 in ms [measured]:

| Learner | Out: sent to the leader's step | Back: the leader's step to answer stepped | Call on the learner (issue to answer) | Learner turn after the answer |
| --- | --- | --- | --- | --- |
| l3 | 72 / 212 / 326 | 140 / 353 / 716 | 218 / 421 / 1,000 | 0 / 0 / 0 |
| l6 | 69 / 212 / 310 | 135 / 344 / 574 | 209 / 399 / 657 | 0 / 0 / 0 |
| l1 | 83 / 225 / 335 | 156 / 345 / 666 | 250 / 424 / 789 | 0 / 0 / 0 |
| l4 | 93 / 239 / 337 | 163 / 345 / 743 | 270 / 436 / 888 | 0 / 0 / 0 |
| l0, l7 | 66 to 85 / 219 to 287 / 338 to 542 | 129 to 132 / 508 to 605 / 1,377 to 1,800 | 161 to 177 / 601 to 874 / 1,417 to 1,700 | 0 / 34 to 183 / 183 to 859 |

- **Not the learner's own turn queue.** On the near learners the call leaves as soon as the batch does (sent to
  issued p99 0 ms) and the answer is stepped in the same turn it arrives (p99 0 ms). l0 and l7 are the exception, with
  turns up to 0.2 s at p99 behind their own storage confirms (p99 1.0 to 1.2 s).
- **Not the leader's step.** The leader takes each `ReadIndex` and answers it in the same ms, and its ticker never ran
  20 ms late in the window (0 `Lag` rows).
- **Some of it is the leader's output gate.** The answer leaves the leader only once the writes before it are durable.
  For 53 to 65% of batches a leader storage confirm was running when the `ReadIndex` was taken; the wait for that
  confirm to end is p50 21 to 28 ms and p99 243 to 265 ms, and about 20% of the back leg's time in its ≥ 400 ms tail.
- **Most of it is on the wire and in the leader's input queue, both ways.** Out p99 is 310 to 337 ms, the back leg
  minus that gate is p99 470 to 635 ms, and entries riding the answer (`append_for_read`) do not lengthen it (with
  entries p99 663 to 761, without 544 to 686). The trace cannot separate the network from the leader isolate holding
  a request before its handler runs, because the leader's clock moves only across I/O: its `/raft` handler at about
  80 req/s is the same isolate that answers 11 learners and voters, and Cloudflare's per-object analytics give the
  leader's wall p50 63 ms and p99 2.05 s per request in the window [measured; which of the two it is, not measured].

So the round trip is not learner-side, and pipelining the batches (below) takes the queueing on it out of the read's
path without needing to know which. Separating wire from leader input queue needs a timestamp the caller sends with
each call; that is not built.

**The change (narrow-reads-3, after gate 12).** A follower or learner keeps up to `READ_PIPELINE` (8 [guess])
read-index batches in flight, still sent at most once per 50 ms, and each answer grants only its own batch's reads,
whichever order answers arrive in (`crates/raft/src/member/install.rs`). F67 rule 12 holds per batch: a batch whose
every read failed past CAMPAIGN_DEADLINE leaves flight, and a batch unanswered for CALL_DEADLINE is resent. The leader's
load is unchanged: the 50 ms floor still bounds a learner at 20 calls/s.

Checks on the change (0b2b51cc, then merged with main 5c0f06f as 75bb312f): the full checks pass, `tests/nr3/plant.py`
catches 27 of 27, the long sweep passes 2,000 seeds of 10,000 steps (longest healthy-quorum stretch with no request
completing: 15.7 s, seed 11431), and Porcupine finds every history linearizable: plain 156,325 operations, gray
2,230,496, live 5,927,243 [measured].

Rows: `tests/nr3/rows/a1-b0-29-gate12.jsonl`, `run-gate12.status`, `lmem-gate12.jsonl`, `mem-gate12.json`,
`egress-gate12.txt`, `fresh-gate12.jsonl`, `trace-gate12.json.gz`.

### Gate 13 on main 31d19c9: READ_PIPELINE in, the API line passes

The first gate counting toward the streak: main 31d19c9 (narrow-reads-3 merged, with READ_PIPELINE, the read floor
and the entries ahead of a learner's answer), site day 12 from that commit, the adopted B0-29 shape. Repo 65558,
window 09:44:37 to 09:54:37Z, torn down [measured]:

| Line | Gate 13 | Gate 12 (1 batch in flight) | B0-29 pass |
| --- | --- | --- | --- |
| API p50 / p99, served | 499 / 1,575 ms, 99.96% (112,716 reads) | 672 / 2,011 ms, 99.93% | p99 ≤ 2,000 ms, ≥ 99% |
| Tier read at the edge, p50 / p99 | 466 / 1,438 ms (grant p99 933, build wait p99 100) | 636 / 1,935 ms | |
| Commit p50 / p99 | 411 / 1,638 ms (queue p99 1,372, round 341, confirm 112) | 997 / 5,140 ms | p99 ≤ 600 ms (B0-02's) |
| merge:only p99 | 2,169 ms | 5,985 ms | ≤ 2 s |
| Leader | site 0, term 2 throughout | same | no change |
| Read-your-write | 150 of 150, 0 stale | 150 of 150, 0 stale | 0 stale |

A pass on the API line, 425 ms under it. Setup: 500 of 500 PRs opened, 8 learners caught up in 2.5 s, leader 11.7 MiB.

**Leader memory.** 82 MiB by the analytics peak, under the 100 MiB stop line, and 46.9 MiB by `/stats`. `memSplit` at
the `/stats` peak (09:54Z): log 8.0 MB (the `LOG_BYTES` cap), read caches 1.67 MB (dumps 1.61 MB, narrow 54 KB), held
61 KB, in-flight call bodies 163 KB, fold rows buffered 0, statements queued 0. Over the window in-flight call bodies
peaked at 393 KB, read answers streaming at 219 KB and queued statements at 124 KB. So what `memSplit` names stays
under 10 MB, and the remaining 35 to 70 MiB is wasm memory the isolate already grew, which it never returns
(`wasmMemMb` only grows) [measured; that the rest is past allocation high-water, not live data, is an inference].
Learners peaked at 59 to 66 MiB.

**Read share** (`readStages` n): l3 24.7%, l6 23.7%, l7 12.1%, l4 11.7%, l5 9.4%, l0 8.6%, l2 7.4%, l1 2.5%. Reads
spread wider than gate 12's 71% on two learners. Grant p99 851 to 986 ms on every learner, with `late` 487 to 1,381
each and at most 24 shed.

**The round trip** (`tests/nr3/legs.py gate13`, p50 / p90 / p99 ms): out 31 to 74 / 140 to 276 / 286 to 571, and back
77 to 194 / 439 to 1,008 / 1,308 to 1,702. The leg out is shorter at p50 and p99 than on gate 12. The leg back's
p90 and p99 are longer, with 8 batches per learner in flight. Leader confirms overlap 27 to 34% of batches, against
53 to 65%. This tree has no send timestamps, so wire and input queue are still not separated.

Rows: `tests/nr3/rows/a1-b0-29-gate13.jsonl`, `run-gate13.status`, `lmem-gate13.jsonl`, `mem-gate13.json`,
`egress-gate13.txt`, `fresh-gate13.jsonl`, `trace-gate13.json.gz`. Spend to 09:58Z: $21.73 of $40.

### Gate 14 on main 31d19c9: streak 2

Same tree and site day as gate 13 (site day 12, main 31d19c9). Repo 65559, window 10:08:28 to 10:18:28Z, torn down
[measured]:

| Line | Gate 14 | Gate 13 |
| --- | --- | --- |
| API p50 / p99, served | 314 / 1,477 ms, 99.99% (115,023 reads) | 499 / 1,575 ms, 99.96% |
| Tier read at the edge, p50 / p99 | 286 / 1,402 ms (grant p99 578, build wait p99 24) | 466 / 1,438 ms |
| Commit p50 / p99 | 539 / 2,168 ms (queue p99 1,820, round 465, confirm 96) | 411 / 1,638 ms |
| merge:only p99 | 2,853 ms | 2,169 ms |
| Leader | site 0, term 2 throughout | same |
| Read-your-write | 150 of 150, 0 stale | same |

A pass on the API line, 523 ms under it. Setup: 500 of 500 PRs opened, 8 learners caught up in 4.8 s.

**Leader memory.** 85 MiB by the analytics peak, under the 100 MiB stop line; 47.1 MiB by `/stats`. `memSplit` at the
`/stats` peak (10:16:46Z): log 8.0 MB, read caches 1.60 MB, held 0, in-flight call bodies 319 KB. Over the window:
call bodies at most 576 KB, held 281 KB, queued statements 89 KB, read answers streaming 571 bytes. Learners 61 to
70 MiB.

**Read share** (`readStages` n): l0 25.4%, l7 25.4%, l3 25.1%, l6 18.0%, l4 2.1%, l1 1.6%, l5 1.2%, l2 1.2%: the four
enam learners took 94%. Their grant p99 was 320 to 546 ms with 0 to 56 `late` each; weur and apac learners 764 to 976
ms with up to 819 `late`, on 1 to 2% of reads each. At most 3 reads shed on any learner.

**The round trip** (`tests/nr3/legs.py gate14`, p50 / p90 / p99 ms), enam learners: out 13 to 27 / 90 to 124 / 160 to
202, back 49 to 85 / 152 to 210 / 309 to 518, total p99 347 to 564. The far learners' back leg reaches p99 608 to
1,224 ms. Against gate 12's near learners (out p99 310 to 337, back p99 574 to 716 at one batch in flight) both legs
are shorter here, so part of gate 12's round trip was the queue that one batch in flight built up, not the wire
[inference; wire and leader input queue are still not separated on this tree].

Rows: `tests/nr3/rows/a1-b0-29-gate14.jsonl`, `run-gate14.status`, `lmem-gate14.jsonl`, `mem-gate14.json`,
`egress-gate14.txt`, `fresh-gate14.jsonl`, `trace-gate14.json.gz`.

### Gate 15 on main 31d19c9: a miss by 19 ms, and the streak on this tree ends at 2

Same tree and site day as gates 13 and 14. Repo 65560, window 10:44:51 to 10:54:51Z, torn down [measured]:

| Line | Gate 15 | Gate 14 | Gate 13 |
| --- | --- | --- | --- |
| API p50 / p99, served | 365 / 2,019 ms, 99.89% (115,647 reads, 127 answered 503) | 314 / 1,477 ms, 99.99% | 499 / 1,575 ms, 99.96% |
| Tier read at the edge, p50 / p99 | 337 / 1,896 ms (grant p99 900, build wait p99 100) | 286 / 1,402 ms | 466 / 1,438 ms |
| Commit p50 / p99 | 409 / 2,249 ms (queue p99 2,058, round 349, confirm 89) | 539 / 2,168 ms | 411 / 1,638 ms |
| merge:only p99 | 3,328 ms | 2,853 ms | 2,169 ms |
| Leader | site 0, term 2 throughout | same | same |
| Read-your-write | 150 of 150, 0 stale | same | same |

A miss on the API line by 19 ms, so the streak on this tree is gates 13 and 14. The operator had already moved the line
to a confirming streak on current main, which carries the admission pricing fix: before it, admission held agents to
about 30 commands/s, which also lightened read load on these three gates.

**Leader memory.** 84 MiB by the analytics peak, under the 100 MiB stop line; 46.3 MiB by `/stats`, with `memSplit` at
that peak: log 8.0 MB, read caches 1.56 MB, held 0, call bodies 45 KB (at most 317 KB, held at most 149 KB). The
leader answered every `ReadIndex` in the same ms, its ticker never ran late, and its confirms were p99 119 ms.

**Where the tail came from.** Routing put half of all reads on one learner: l7 50.2%, l6 23.3%, l3 12.6%, l0 7.5%, and
1.1 to 1.8% on each weur and apac learner. l7's grant p99 was 747 ms, but it shed 54 reads and answered 720 late. The
far learners' grant p99 was 949 to 1,000 ms (`FOLLOWER_WAIT`) with 425 to 1,567 late each, so a read sent to one
mostly comes back late and is retried. Per learner round trip (`tests/nr3/legs.py`, p50 / p90 / p99 ms): enam out
19 to 44 / 110 to 210 / 220 to 388 and back 56 to 119 / 505 to 1,034 / 1,348 to 1,905; weur and apac back p99 1,395 to
2,005. The back leg's tail is longer than on gate 14 (p99 309 to 518 on enam), on the same tree, so it varies from run
to run with the leader's load rather than with the tree [measured; the cause of the variation is not measured, and
this tree has no send timestamps to split wire from input queue].

Rows: `tests/nr3/rows/a1-b0-29-gate15.jsonl`, `run-gate15.status`, `lmem-gate15.jsonl`, `mem-gate15.json`,
`egress-gate15.txt`, `fresh-gate15.jsonl`, `trace-gate15.json.gz`.

### The herding guard (after gate 15)

Gate 15's miss had one learner, l7, take 50.2% of the reads; it shed 54 and answered 720 late. Routing by the average
alone sends every read to the fastest learner, which those reads then slow, and the one read in 20 sent at random went
to far learners that answered at `FOLLOWER_WAIT` (425 to 1,567 late each). The operator asked for the simplest guard a
replay shows works, before site day 13.

**The rule** (`crates/lookup`, `timed_spread_order` and `Timed`): a learner's score is its moving average plus 4 moving
mean deviations. Eligible are the untimed learners, those within 200 ms of the best timed score, and, while fewer than
3, the next best scored under 1 s. A read goes to one of them at random, then the rest by score. A learner untimed for
10 s counts as untimed again, which replaces the one-in-20 random read. So a learner that mostly answers late
(score past 1 s) is asked only after the eligible ones, which answers the operator's second question with the same rule.

**The replay** (`tests/nr3/route.py`, over each gate's learner traces: a read leaves in the next 50 ms batch, takes
the round trip and read index of the traced batch nearest its send, waits for that learner's applied index, answers
late at 1 s and moves on, three members at most; 4 edge isolates at 200 reads/s, 120 ms of edge stages) [inference]:

| Gate | Routing | Projected API p99 | Reads over 2 s | Late answers | Busiest learner |
| --- | --- | --- | --- | --- | --- |
| 13 | average only, 1 in 20 random (as built) | 1,504 ms | 0.36% | 2.98% | 17.5% |
| 13 | guard | 1,288 ms | 0.22% | 1.64% | 19.1% |
| 14 | as built | 757 ms | 0.02% | 0.40% | 26.2% |
| 14 | guard | 764 ms | 0.02% | 0.31% | 25.2% |
| 15 | as built | 1,444 ms | 0.34% | 2.61% | 36.0% |
| 15 | guard | 1,254 ms | 0.31% | 1.71% | 24.1% |

Also tried, and beaten in the same replay: the deviation score alone, with the random read kept (busiest 33% on gate
15); a random pick between the two best-scored learners (busiest 36% on gate 14, p99 1.64 and 1.58 s on gates 13 and
15); a spread of 4 (no better than 3). The replay holds each learner's answer times as traced, so it cannot show the
herding feeding on itself, which is what gate 15 measured; the gain there is likely larger than projected. It also runs
about 0.6 s under gate 15's measured 2,019 ms, so its absolute numbers are optimistic and only the comparison counts.

**Tests and plants.** Kept tests in `crates/lookup`: the fastest learner gets at most 1 in 3 of the first places while
the others answer in time, and a learner scored at 1 s is never asked; an untimed or stale learner is eligible and
sets no best; the score counts the deviation, so a swinging learner scores worse than a steady one at the same
average; the margin is inclusive at its edge. `tests/nr3/plant.py` adds six mutants of the guard; all six are caught.

### Gate 16 on main a858bfb: the leader's isolate runs out of memory, and the run stops

Site day 13 from main a858bfb (the herding guard, send timestamps under test-faults, main 0f09638 with the admission
pricing fix): site exit 0, edge exit 0. Repo 65561, 500 of 500 PRs opened, learners caught up in 2.5 s with the leader
at 11.6 MiB, window 11:57:48 to 12:07:48Z, torn down [measured]:

| Line | Gate 16 | Gate 15 |
| --- | --- | --- |
| API p50 / p99, served | 348 / 21,789 ms, 65.96% (116,235 reads: 11,349 answered 503, 28,223 skipped at the driver cap) | 365 / 2,019 ms, 99.89% |
| Commit p50 / p99 | 18,002 / 22,760 ms (499 commits, queue p99 22,523) | 409 / 2,249 ms (6,428 commits) |
| Agent pushes ok | 1,779 of 2,500 (400 access-row reads shed, 267 no outcome) | 2,498 of 2,500 |
| Leader | site 0, term 2 to 15 (members reset, see below) | site 0, term 2 throughout |
| Leader peak, analytics | 258 MiB on `repo:65561#1#f0`; 298 and 278 MiB on the voters | 84 MiB |
| exceededMemory, site scripts | a 1,930, b 3,431, c 3,923 in the window | a 13, b 0, c 0 |
| Read-your-write | 150 of 150, 0 stale | same |

**This is the stop line, not an API miss.** Analytics put the leader at 258 MiB, past the operator's 100 MiB line.
Per minute (`durableObjectsPeriodicGroups`, max memory, MiB):

| Minute | Leader f0 | Voter f2 | Voter f1 (gen 7) |
| --- | --- | --- | --- |
| 11:58 to 12:01 | 59 to 67 | 55 to 60 | 29 to 35 |
| 12:02 | 77 | 60 | 88 |
| 12:03 | 257 | 258 | 258 |
| 12:04 to 12:07 | 117 to 258 | 106 to 298 | 259 to 278 |

At 12:03:18.930Z the leader's storage write failed with "Durable Object's isolate exceeded its memory limit and was
reset" (Workers Logs, f0). The host aborted on it as designed (`storage write failed`), and all three voters'
isolates then reset over and over: a 1,930, b 3,431, c 3,923 exceededMemory outcomes from 12:03 to the end, against
13 on gate 15. Commits stalled behind it at 18 to 23 s, and API reads were shed or capped. Until 12:02 the run looked
like gate 15: learners' read-index calls p50 55 to 293 ms, `/stats` wasm 22 to 39 MiB, log 8 MB, no leader change.

**What grew is not named yet.** `memSplit` at 15 s intervals shows the leader's log at its 8 MB cap, read caches
under 1 MB, held at most 8, and `inFlight.calls` (encoded bodies of member calls in flight) at 0.1 to 0.36 MB before
12:03. After the first reset it reached 9.1 MB on site 2 (12:03:53) and `inFlight.answers` 19.6 MB on site 0
(12:04:39). The 15 s samples straddle the jump from 77 to 257 MiB, so none of them catches it. That the in-flight call
and answer bodies fill the isolate during the storm is likely, but not measured [inference]. Gate 15's analytics peak
was 84 MiB on the old tree. This tree adds the admission pricing fix, which lets more commands into each entry, and
LM-1's m27 already saw API p99 4.0 s on it.

**Wire split: not answerable on this gate.** The leader's trace restarted with its isolate at 12:06:29, so the leader
side of the healthy minutes (11:58 to 12:03) is gone. The learners' side survived: from 11:57:48 to 12:03:00 the leg
back (learner arrival minus the leader's `x-graft-left`) has a spread above its median of p90 200 to 444 ms and p99
356 to 839 ms. Its median is −733 to +82 ms, which is clock skew between isolates. Without the leader's `CallArrived`
rows the skew cannot be cancelled, so wire and input queue stay unseparated. The commit path's split (`v1`, `v2`)
covers only 12:07 to 12:09, after the storm.

Rows: `tests/nr3/rows/a1-b0-29-gate16.jsonl`, `run-gate16.status`, `lmem-gate16.jsonl`, `mem-gate16.json`,
`egress-gate16.txt`, `fresh-gate16.jsonl`, `trace-gate16.json.gz`.

### The gate 16 A/B, arm "without": main a858bfb again, the storm reproduces

The operator's A/B after gate 16: gate 16's tree (main a858bfb, site day 13, the double decode in) against main a5a85d9
(decode-once: `x-graft-from`, no second decode). Same shape, run.sh with `memSplit` every 2 s. Locally, one decode of
an append carrying a 1 MB inline pack peaks at 2.33 MB of heap and holds 1.0 MB, natively, and the throwaway decode
was dropped before `raft`'s, so the per-call peak was the same either way [measured, native allocation].

Arm "without", repo 65562, window 12:50:16 to 13:00:16Z, torn down [measured]:

| Line | Arm "without" | Gate 16 |
| --- | --- | --- |
| API p50 / p99, served | 266 / 22,977 ms, 41.03% (14,904 answered 503, 54,188 skipped at the cap) | 348 / 21,789 ms, 65.96% |
| Commit p50 / p99 | 24,534 / 30,609 ms (497 commits) | 18,002 / 22,760 ms |
| Leader | site 0, term 2 to 21 | term 2 to 15 |
| Analytics peak: f0 / f1 (gen 2) / f2 | 285 / 310 / 341 MiB | 258 / 278 / 298 MiB |
| exceededMemory a / b / c | 2,342 / 4,235 / 3,572 | 1,930 / 3,431 / 3,923 |
| Read-your-write | 150 of 150, 0 stale | same |

Per minute (max MiB; f0 the leader, f1 gen 2 and f2 the followers): 12:50 to 12:51 at 64 / 26 to 30 / 53 to 60;
**12:52 at 70 / 264 / 260**; 12:53 at 227 / 310 / 341; to the end 119 to 285 on every voter. So the **followers**
jumped first, a minute before the leader. The leader's `memSplit` at 2 s up to 12:52:38 is flat: wasm 33.3 MiB, log
8.0 MB, held under 84 KB, call bodies under 0.34 MB. It then stopped answering `/stats`. The first sample after
(12:54:00) shows 118.8 MiB, held 9.9 MB for two learners, and `inFlight.calls` 30.6 MB. The first exceededMemory
outcomes are f1 (gen 2) at 12:53:52 on `/commit` and f2 at 12:54:41.

**A lead, not a cause.** Both followers took a `/snapshot_commit` call at 12:52:40 to 12:52:50, in the minute they
jumped. The leader's `/snapshot_commit` calls in the window number 56 (30 on gate 16, 8 on gates 14 and 15, 0 on gate
13), first at 12:50:26. On gate 16 the first came at 12:03:23, four seconds after the leader's first reset. A member
writes a tier snapshot (`crate::snap`, `PART_BYTES` 8 MiB part buffers and a frame batch, uploaded to two buckets)
when `snapshot_due` holds, which on a fresh repo is from its first applied entry, so every fresh gate repo writes a
first snapshot in its first minutes. The repeated calls are likely the writer asking again until the snapshot's row
applies. This fold is about 1.6 MB, so the snapshot's own buffers should be small. Whether the snapshot path is what
fills the followers is not measured, and the follower side has no `memSplit` [inference]. The m3f thread's L0-12 replay, on main without narrow-reads-3 or the timestamps, logged
8,465 and 4,722 resets on two voters, which also rules the double decode out as the whole cause.

Rows: `tests/nr3/rows/a1-b0-29-abwithout.jsonl`, `run-abwithout.status`, `lmem-abwithout.jsonl`,
`mem-abwithout.json`, `egress-abwithout.txt`, `fresh-abwithout.jsonl`, `trace-abwithout.json.gz`.

### The gate 16 A/B, arm "with": main a5a85d9, no storm, API p99 2,304 ms

Site day 14 from main a5a85d9 (decode-once merged): site exit 0, edge exit 0. Repo 65563, 500 of 500 PRs opened,
learners caught up in 2.6 s, window 13:44:47 to 13:54:47Z, torn down. `lmem.mjs` now samples every voter every 2 s,
and `tsave.mjs` saved every voter's trace each minute (54 saves) [measured]:

| Line | Arm "with" (a5a85d9) | Arm "without" (a858bfb) | Gate 15 (31d19c9) |
| --- | --- | --- | --- |
| API p50 / p99, served | 525 / 2,304 ms, 99.92% (116,666 reads, 89 answered 503) | 266 / 22,977 ms, 41.03% | 365 / 2,019 ms, 99.89% |
| Commit p50 / p99 | 538 / 2,225 ms (6,474 commits; queue p99 1,919, confirm 129) | 24,534 / 30,609 ms | 409 / 2,249 ms |
| Agent pushes ok | 2,500 of 2,500, p99 5,762 ms | 1,048 of 2,500 | 2,498 of 2,500 |
| Leader | site 0, term 2 throughout | term 2 to 21 | term 2 throughout |
| Analytics peak: f0 / f1 / f2 | **81 / 62 / 63 MiB** | 285 / 310 / 341 MiB | 84 MiB on f0 |
| exceededMemory a / b / c | 0 / 0 / 0 | 2,342 / 4,235 / 3,572 | 13 / 0 / 0 |
| `/snapshot_commit` calls | 4, first at 13:38:48 (setup) | 56, through the window | 8 |
| Read-your-write | 150 of 150, 0 stale | same | same |

`/stats` agrees: wasm high-water 48.6 MiB on the leader and 24.8 and 25.4 MiB on the followers. Per minute the
leader's analytics peak rose from 59 to 81 MiB, and the followers' from 34 to 62 MiB.

**The A/B's answer.** Without the second decode, no member stormed. With it, every voter stormed on both runs of the
same tree (gate 16 and arm "without"). That makes the double decode the trigger, though the local measurement says
it held no second copy at once, and the m3f thread saw voter resets on a tree without it. The snapshot path differs
too: 4 `/snapshot_commit` calls here against 56 and 30. Whether the storm feeds the snapshot retries or the retries
feed the storm is not measured [inference]. The m3f thread is reproducing its own resets with these instruments.

**The API line misses.** 2,304 ms at 99.92% served is over the 2 s line, on the tree the streak would resume on.
Read shares under the herding guard: l6 28.7%, l7 16.5%, l0 14.1%, l1 14.0%, l2 11.1%, l4 6.7%, l3 6.0%, l5 2.9%, so
the busiest learner took 28.7% against gate 15's 50.2%. Grant p99 was 707 to 1,000 ms on every learner, with 524 to
3,863 late each. The edge tier read p99 was 2,252 ms.

**Wire split** (send timestamps, leader trace from `tsave.mjs`). On every learner the leader's handler, from
`CallArrived` to `x-graft-left`, is p50 0 and p99 98 to 296 ms. The rest of the round trip (both wires, the leader's
input queue, and its output gate) is p50 95 to 302, p90 337 to 1,207, and p99 914 to 2,219 ms. So the slow read-index
round trips are spent outside the leader's handler. Splitting that rest into wire and input queue needs the two
clocks' skew. An estimate from each learner's fastest 2% of trips gives negative leg times on some learners, so it is
not reliable and I report no split [measured for handler versus rest; the wire versus queue split is unresolved].
Commit path, leader to voter: out p99 494 and 607 ms, back p99 673 and 245 ms, both carrying the same skew.

Rows: `tests/nr3/rows/a1-b0-29-abwith.jsonl`, `run-abwith.status`, `lmem-abwith.jsonl.gz`, `mem-abwith.json`,
`egress-abwith.txt`, `fresh-abwith.jsonl`, `trace-abwith.json.gz`, `tsave-abwith.jsonl.gz`.

### Where the round trip's tail goes, from arm "with"'s traces (no new run)

The operator asked, after the A/B, whether the part of a learner's read-index round trip that lies outside the
leader's handler is the Durable Object output gate or the leader's input queue. Arm "with" has every call's four
times: send S and answer R on the learner's clock, handler start A and return L on the leader's. `rest` =
(R − S) − (L − A) needs no skew. `tests/nr3/gates.py abwith` takes each answer's output gates from the confirm
intervals, Confirm (10) as [at − ms, at]: the leader's gate is L to the end of the leader confirm running at L, and
the learner's is S to the end of its own confirm running at S, since its outgoing call waits for its earlier writes
too [measured]:

| Learner | rest p50 / p90 / p99 ms | Leader gate, share of rest ≥ p90 / ≥ p99 | Learner gate, share of rest ≥ p90 / ≥ p99 | Learner confirm p50 / p99 |
| --- | --- | --- | --- | --- |
| l0 | 112 / 691 / 1,738 | 3% / 1% | 17% / 15% | 31 / 268 |
| l1 | 175 / 337 / 914 | 8% / 2% | 12% / 36% | 31 / 189 |
| l2 | 202 / 402 / 1,383 | 4% / 0% | 4% / 9% | 18 / 122 |
| l3 | 188 / 1,207 / 1,722 | 1% / 0% | 49% / 35% | 51 / 1,447 |
| l4 | 212 / 561 / 1,688 | 3% / 0% | 9% / 14% | 41 / 249 |
| l5 | 302 / 1,197 / 2,219 | 1% / 1% | 22% / 39% | 54 / 996 |
| l6 | 97 / 349 / 1,205 | 7% / 3% | 9% / 12% | 33 / 189 |
| l7 | 95 / 796 / 1,609 | 2% / 0% | 20% / 14% | 43 / 669 |

**The leader's output gate does not explain the tail.** It holds 0 to 8% of the slowest tenth and 0 to 3% of the
slowest hundredth on every learner. Leader confirms are p50 50 ms and p99 158 ms, and most answers leave with none
running. **The learner's own output gate** explains 4 to 49% at p90 and 9 to 39% at p99. That is most on l3 and l5,
whose confirms reach p99 1,447 and 996 ms.

**The leader's input queue: some signs, not a measure.** The leader's clock moves only across I/O and the trace has
no "input finished" row, so the gap from the previous input's end to CallArrived cannot be read exactly. The nearest
measure, A minus the leader's last row before it, is p50 6 to 12 ms on both slow and fast trips, because the leader
is always busy. What does tell slow from fast: 43 to 79% of the slow tenth's calls arrive in a millisecond shared
with 3 or more other arrivals, against 7 to 19% of fast trips, and their answers leave bunched the same way. Inputs
released together after waiting is what an input queue looks like [inference]. Answers are also taken on the
learner in heavier turns: 10 to 28 trace rows at R's millisecond on slow trips against 5 to 9 on fast ones. The
learners ran at 41 to 66% of a core (243 to 396 CPU s in 600 s), against 21% on the leader.

So half or more of most learners' tail (everywhere except l3) is left as both wires, the leader's input queue and
the learner's turn. The one-way split stays unresolved, because Workers' frozen clock makes S and L lower bounds and
the per-leg minima drift past the median.

### Options (a) and (b) after the A/B (no new run, no build)

The operator asked for two write-ups before picking. (a) is taking the learner's own output gate off its read-index
calls, with a safety argument. (b) is the learner's CPU split, to see whether read answers are the cheaper lever.

**(a) What the platform allows.**
- The output gate holds every outgoing message, `fetch()` included, "until all previous writes have been confirmed
  flushed to disk" (https://developers.cloudflare.com/durable-objects/api/storage-api/, `put` options) [doc].
- The gate is per object, not per invocation, so no invocation of a learner with an unconfirmed write has a clear
  gate. Sending the ReadIndex call "from an invocation whose gate is clear" is not possible.
- The only opt-out is `allowUnconfirmed`. It exists for the async KV API's `put`, `delete`, and `deleteAll`, which a
  SQLite-backed object also has (stored in the hidden `__cf_kv` table), plus `sync()` to wait for durability [doc].
  SQL `exec` and `transactionSync` have no such option, and the host writes everything through `transactionSync`
  (`workers/site/src/host.rs`, `commit_tx`).
- So the one way out is the first: **a learner writes its log, Raft state, and tier rows through KV with
  `allowUnconfirmed`**, and keeps `sync()` only for its own bookkeeping. The code does not support that today:
  - the tier rows are SQL tables with real columns that the list queries index (`crates/fold/src/store.rs`,
    `workers/site/src/tier.rs` `TierSql`);
  - DECISIONS "`m0-carried-4`" already rejected moving voters' writes to KV as a storage-layer rewrite ("Beat").
- A learner-only KV store is narrower than that, but it is still a second `Store` implementation and a second
  snapshot install path.

**(a) What it would buy [inference].** Removing the learner's own gate from each traced round trip (`gates.py`
`abwith`, `rest − gateS`) moves `rest` over all learners from p90 625 and p99 1,681 ms to p90 512 and p99 1,564 ms:

| Learner | p90 / p99 now (ms) | p90 / p99 without the gate (ms) |
| --- | --- | --- |
| l3 | 1,207 / 1,722 | 900 / 1,609 |
| l5 | 1,197 / 2,219 | 905 / 2,111 |
| l1, l2, l6 | — | 2 to 15% lower |

That is about 120 ms off a round trip's p99, against arm "with"'s API p99 of 2,304 ms and a 2 s line. On its own, (a)
does not close the gap. Learners were mid-confirm 7 to 18% of the run, and l3 26%.

**(a) The safety argument.** It holds only if every one of these rules holds:
1. **A learner never counts toward a quorum.**
   - `quorum::commit` and `quorum::election` count voters only, and `transfer` and `health` hand off only to voters.
   - So a learner ack that leaves before its write is durable cannot commit an entry, elect a leader, or confirm a
     lease or read index.
   - Gap found: `advance_commit` builds `acked` from `lead.matched`, learners included, and `proof_for` writes all of
     them into the `QuorumProof`. Today a learner's AppendRes waits at its gate for its write, so the proof is true.
     Under `allowUnconfirmed` it would name a learner copy that is not durable. **The proof must list voters only.**
     The simulator's `ack_check` asserts that every site the proof counts holds the entry on disk. This matters only
     under `allowUnconfirmed`, so the code is left as it is (operator, with (a) not adopted).
2. **A learner applies only committed entries.** It sets `commit` from `req.commit.min(matched)`
   (`member/follower.rs`), and a leader's `commit`, even a deposed one's, covers only committed entries (log
   matching). So what a learner holds in memory and has not confirmed is committed. Serving it serves no uncommitted
   write, and losing it loses nothing the group needs.
3. **A learner grants a read only at applied ≥ the leader's read index** (`check_remote`, `i <= applied`). The read
   index is a commit index the leader confirmed after the read arrived, by lease or round. So any answer holds every
   write acknowledged before the read, whether or not the learner's copy is durable yet.
4. **Grants and answers die with the boot.**
   - `fresh_after` refuses a grant from another boot, and read-index batch numbers start from the boot's high bits.
   - After a reset that lost applied entries, a new read gets a new read index of at least the commit, which is at
     least every index the learner answered at before. It waits to re-apply up to there or answers `overloaded`.
   - So a client never sees a learner go back in time.
   - The narrow answer cache and the dump cache are `thread_local`, keyed by repository and stamped with the applied
     index they were built at, and they serve only at or above the read's floor. Every cached answer is at a
     committed index, so even a cache that outlives the object's reset in the same isolate serves no stale read.
5. **A lost learner ack costs liveness, not safety.** The leader's `matched` for that learner is now ahead of its
   log. A read-ahead append (`append_for_read`) is skipped because `matched` already covers the index, so the learner
   waits out `FOLLOWER_WAIT` (late answers) until a refusal makes the leader back off. To bound that, a learner's
   first AppendRes of a new boot should reset its `matched` on the leader, keyed by boot as `SelfAck` is.
6. **Term and vote.** A learner that loses its stored term rejoins at an older term. It may take AppendEntries from a
   deposed leader, but by rule 2 those carry only committed entries up to their `commit`. Their uncommitted tail is
   truncated by the next current leader before it can be applied. Learners never vote, so a lost vote does not matter.

**(a) What DESIGN, F65, and F67 would need.**
- DESIGN "Follower read": a learner's log, state, and tier writes do not hold its output gate (KV with
  `allowUnconfirmed`). Its grant stands on its applied index in memory. Its acks count in no quorum and no
  `QuorumProof`. A new boot resets its `matched` on the leader. Stack would need its own line for the KV API on
  SQLite-backed objects.
- F65: the learner's gate becomes a recorded cause of the tail, with its measured share (above).
- F67: no rule changes, since learners take no part in elections. Its liveness bound would need learner resets added
  to the simulator's fault set.
- DECISIONS: a learner-only amendment to "`m0-carried-4`", with its Flip met by this measurement.

**(a) How a plant would catch a stale read.** The raft simulator has no learners today (`crates/raft/tests/sim.rs`,
`common/sim.rs`), so the first step is adding learners. They would serve spread reads and crash with `crash_unconfirmed`
losing their unconfirmed writes, and the run would be checked by the live sweep and Porcupine. Each plant must then
fail:
- `fresh_after` without its boot check: a restarted learner answers a grant from before its reset.
  `leadership.rs` `a_read_index_answer_from_a_previous_boot_grants_nothing` already fails this for followers.
- `check_remote` granting at applied < read index: Porcupine reports a read that misses an acknowledged write.
- `quorum::commit` counting learners: a write acknowledged by the leader plus one learner, then both crash.
  Porcupine reports the write lost.
- `proof_for` listing learners: `ack_check` reports a counted copy that is not on disk.
- The narrow cache serving below its floor: a learner read with no read-index wait; Porcupine reports it stale.

**(b) Learner CPU, arm "with" (600 s, Workers Logs `cpuTimeMs` per invocation) [measured].**

| Learner | CPU s | Cores | `/read` | `/feed` | `/raft` | `/read` invocations ≥ 150 ms: n, CPU s | 50–150 ms: n, CPU s |
| --- | --- | --- | --- | --- | --- | --- | --- |
| l0 | 330 | 0.55 | 98.8% | 1.2% | 0.1% | 837, 169 | 1,291, 135 |
| l1 | 280 | 0.47 | 98.5% | 1.5% | 0.0% | 540, 103 | 1,480, 146 |
| l2 | 279 | 0.46 | 98.2% | 1.6% | 0.2% | 611, 125 | 1,240, 126 |
| l3 | 395 | 0.66 | 98.5% | 0.9% | 0.6% | 1,164, 351 | 295, 29 |
| l4 | 295 | 0.49 | 97.1% | 2.8% | 0.1% | 946, 219 | 548, 54 |
| l5 | 236 | 0.39 | 93.3% | 5.9% | 0.8% | 488, 182 | 246, 23 |
| l6 | 303 | 0.51 | 99.3% | 0.7% | 0.0% | 423, 77 | 1,897, 190 |
| l7 | 349 | 0.58 | 99.2% | 0.7% | 0.2% | 864, 176 | 1,395, 145 |

`/read` invocations under 5 ms are 1,400 to 4,100 per learner and carry 2 to 7 s.

**The split into apply, read answers, and catch-up cannot be read from this data.** A learner's apply, its
read-index calls, and the appends that ride their responses all run inside the invocation that made the call,
mostly a `/read`. So per-invocation CPU puts all three under `/read` [inference from the code, `crate::callers`].
What bounds the read-answer share [measured natively; the wasm factor is a guess]:
- Builds on a 500-PR repository, natively in release: a whole-tier dump 5.9 ms (784 KB), REST `issues?per_page=30`
  2.7 ms (32 KB), GraphQL `pullRequests(last: 10)` 1.9 ms (11 KB).
- Arm "with" built 23,041 narrow answers and 3,218 dumps over 8 learners (`/stats`). That is about 55 s and 19 s
  natively, 3% of the learners' 2,466 CPU s. At a guessed 3 to 5 times slower in wasm it is 9 to 15%.
- Cached answers (`shared`, 2,017 to 23,072 per learner) cost a lookup and a copy.
- The learner's CPU does not follow its reads served (correlation 0.07 over the 8 learners). l6 served 35,569 reads
  on 303 s and l3 served 8,504 on 395 s.
- Answers by kind: `read:api` is half REST `issues?per_page=30` and half GraphQL `pullRequests(last: 10)`, each
  about 97 a second (`tests/am1/rest/a1.mjs`). `/stats` counts builds, not builds by kind. At the native costs above,
  REST would be about 60% of build CPU and GraphQL about 40% [inference].

So read answers are not where most learner CPU goes. The handoff's regression (38 ms per narrow build, 182 ms per
dump) came from collinear counts and does not survive these native timings.
- The dumps (about one per write that reads `Rows::All`: GraphQL mutations, REST label writes, merges) cost about
  19 s natively. Their cost is memory, about 784 KB each, more than CPU.
- **93% of `/read` CPU is in the invocations of 50 ms or more**, which are 32% of them (89 to 98% per learner). On
  l3, 1,164 invocations of 150 ms or more carry 351 of its 395 s. Those are most likely the inputs that applied
  appends riding a call's response: decode, fold apply, and the SQL transaction [inference]. Apply is a learner's
  floor: it runs at the group's write rate, whatever share of reads the learner takes.
- l3, the learner with the most heavy invocations, also had the slowest confirms (p99 1,447 ms) and was mid-confirm
  26% of the run. Busy isolates confirming late fits that pattern, but it is not shown [inference].

**What (b) needs to be settled:** one counter per `/read` invocation, under test-faults, logged with its CPU:
entries applied, append bytes decoded, rows written, and answers built by kind. Regressing over thousands of
invocations, not 32 learner totals, splits apply from answers. That is one gate-shaped run.

**The operator's pick: (b).** (a) is recorded as not adopted (`decisions/DECISIONS.md`, "Not adopted: a learner's
writes clear of its own output gate"). It flips if, after (b), the learner's gate is measured at more than 30% of the
gap between the API p99 and 2 s.

### Instrument run (b) on 55a9b58: heavy learner CPU is fold point reads inside narrow builds

One 300 s run in the arm "with" shape, from f6a99fc1, whose product code is 55a9b58's: main 8f76a54e plus the
per-invocation counter (`workers/site/src/inv.rs`, test-faults). 8 learners, 500 open PRs, 500 agents. The B0-29
window ran 19:18:14Z to 19:23:14Z on repository 65564. Site day 15 and the edge day both exited 0. No stop rule fired:
the watcher checked the voters' analytics peak every 30 s against 120 MiB, and spend against $48.

| | Arm "with" (600 s, a5a85d9) | inv1 (300 s, 55a9b58) |
| --- | --- | --- |
| read:api n | 116,666 | 58,479 |
| p50 / p99 | 525 / 2,304 ms | 540 / 2,687 ms |
| 503 | 89 | 174 (0.30%) |
| served | 99.92% | 99.70% |
| Leader changes | 0 | 0 |
| Voter analytics peak f0 / f1 / f2 | — | 77 / 57 / 55 MiB |
| Learner peak | — | 59 to 64 MiB |

The run is an instrument, not a gate: its p99 of 2,687 ms is over 2 s, as arm "with" was.

**Join [measured].** `tests/nr3/inv.py` joined 41,026 voter and 94,176 learner invocations with Workers Logs CPU by
request id. Learners used 1,198 CPU s, of which `/read` is 93.9%, `/feed` 2.8%, the ticker 3.0%, and `/raft` 0.4%.
Invocations of 50 ms or more (6,775) carry 1,097 s, 92%.

**Heavy learner CPU by cause [measured, counts per invocation].** Each heavy invocation goes to the first cause it
holds, in this order:

| Cause | Share of heavy learner CPU |
| --- | --- |
| A narrow answer build (REST or GraphQL) | 76.7% |
| No counted cause: CPU from tasks the counter did not tag | 13.2% |
| Cached answer only | 3.9% |
| A whole-tier dump build | 3.4% |
| Apply only (entries applied, no build) | 2.8% |

Per learner, invocations with a build carry 71 to 82% of heavy CPU. The least-squares fit of CPU on the counts gives
the fold SQL reads (`TierSql::query`) 52 to 68% of heavy CPU on every learner, with 12 to 25% unexplained. Entries
applied take 0 to 8%, decoded bytes 1 to 5%, rows and row KB written 0 to 8%, and confirm wait none.
- Confirm wait is wall time, not CPU: 192 s of confirm wall in 1,921 heavy invocations, with pendStart p50 0.
- The premise holds that CPU follows the invocation that started the work: a REST or GraphQL build costs 0.043 ms
  of CPU per fold read, both kinds alike, over 7,188 builds.

**Why a build reads so much [measured locally, `cargo test` scratch counter over `narrow::answer`, 500 open PRs
with one comment each].** A build issues 2,605 fold reads on average in the run. Locally:

| Route | Reads for the reply | Reads for `named_accounts` |
| --- | --- | --- |
| `issues?per_page=30` | 1,655 (530 each of `issue`, `pull`, `pull_head` gets) | 1,512 |
| GraphQL `pullRequests(first: 20, states: [OPEN])` | 1,565 (520 each of the same) | 1,512 |
| `pulls/250` | 17 | 1,512 |

Both causes are in the code:
- `view::Repo::list` loads every item of the kind and state (`item`: `issue`, `pull`, and `pull_head` gets) before
  the selection sorts and pages it (`rest.rs` issues, `gql.rs` connections). A 30-item page reads all 500.
- `named_accounts` (`workers/site/src/family.rs`) re-runs `hosted::accounts` over every issue, comment, and
  `pull_head` whenever the tiers' state changes. Its cache is keyed by `dump_key`, which changes with nearly every
  entry at B0-29's write rate.

**The fix it points to [proposal, not built].** Make a narrow build's fold reads scale with the page, not the
repository:
1. **Page from the order index first.** When the order is `issue_order`'s (GraphQL connections, newest number first)
   and the filter needs only kind and state, scan `issue_order` keys alone, which is a few batched scans. Count from
   the keys and load only the page's items. For REST `sort=created|updated|comments`, keep sort keys in an index the
   fold maintains (`created` already exists as a table), or load only the `issue` row for sorting, not `pull` and
   `pull_head`. Filters that need more (labels, assignee, head, base) keep today's path.
2. **Maintain `named` in the fold.** A metadata table counting references per account, keyed by name shard, written
   at apply. Then `named` is a prefix read of the lowest LOGIN_SHARDS_MAX shards plus point gets for the asked
   accounts, in place of a scan of every row.

Together these cut a build from about 2,600 reads to tens. At 0.043 ms a read, that is about 750 of the learners'
1,198 CPU s in this window, from about 0.5 to about 0.2 cores per learner [inference]. Whether that closes the p99
gap (687 ms here) is not shown. A busy learner answers late, but the round-trip split of arm "with" put the tail on
the leg back. Item 2 adds a fold table, so it needs a DESIGN edit first ("Follower read", the narrow-read rows), and
replies must stay byte-identical: the `narrow.rs` oracle tests cover both items.

Rows: `tests/nr3/rows/a1-b0-29-inv1.jsonl`, `run-inv1.status`, `watch-inv1.jsonl`, `inv-inv1.jsonl.gz` (one line
per invocation: CPU, wall, path, counts).

## The bug the simulator found

With deferred answers in the simulator (30% of granted follower reads answered later through `fresh_after`), the live sweep found a non-linearizable history on seed 11435. A `ReadIndexRes` the leader sent a follower's previous boot, either late on the network or held for a learner's next call, carried batch number 1, the same as the new boot's first batch. The restarted follower took it, and granted a read below a write acknowledged before the read arrived. Batches are now numbered from the boot's high bits, so an old boot's answer names a batch the new boot never sends. The regression test `a_read_index_answer_from_a_previous_boot_grants_nothing` (`crates/raft/tests/leadership.rs`) **fails on main 20147db**, so this is a live bug on `main`, independent of this change. A learner whose isolate resets mid-load, which runs 1 and 3 showed is common, is exactly this path.

## Tests, plants, sweep

- `crates/raft/tests/leadership.rs`:
  - `a_grant_answers_later_only_at_or_above_the_leaders_read_index`: the floor on a learner is the leader's read index, and `fresh_after` answers at or above it and refuses after a restart;
  - the previous-boot test above.
- `workers/site/src/fanout.rs`, `a_narrow_read_is_served_at_or_above_its_floor_and_spaced`. It checks four things:
  - an answer below the floor is never served;
  - a covered floor never waits;
  - answers built with the metadata tier loading are kept apart from loaded ones;
  - another query's answer never serves.
- `workers/site/src/host.rs`: answers shared within an input are keyed by floor.
- `python3 tests/nr3/plant.py`: **11 of 11 caught**. The plants:
  - a follower grant's floor is its own applied index, not the leader's read index;
  - `fresh_after` answers after a restart;
  - `fresh_after` answers at the grant's index, not the state now;
  - any cached answer serves, below the read's floor too;
  - a floor one above the answer is still served (off by one);
  - an answer built while the metadata tier loaded serves once it is loaded;
  - another query's answer serves;
  - the answer is stamped with the floor, not the index it was built at;
  - a read whose floor the cache covers waits for a build;
  - never spaced;
  - answers in one input are shared across floors.
- The previous-boot batch bug is gated by its regression test, which fails on main, rather than by a plant.
- `tests/nr/plant.py` mutants 22 to 28 updated for the new signatures: 7 of 7 caught.
- Long sweep, 2,000 seeds × 10,000 steps (`RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -p graftyard-raft --release --test sim -- --ignored long_sweep`):
  - HARSH and GRAY pass every invariant;
  - Porcupine passes on all three histories: HARSH Ok (156,414 ops), GRAY Ok (2,244,942 ops), live Ok (2,438,823 ops).
- **The live half fails the 30 s liveness bound** (F67's `LIVE_BOUND_MS`):
  - seed 10336 reaches 30,742 ms with deferred reads, against 15.1 s without them; the maximum over 2,000 live seeds is 32,736 ms (seed 11577);
  - in both, the stretch is a leader in a transfer that does not complete (seed 11577: the leader at term 5 starts transfer 3791… at 122.1 s, a new term only at 134.8 s, then the same at term 6 to 142.5 s), during which no request completes;
  - counting a deferred grant as progress does not change either number, so the deferred answer is not what holds the stretch;
  - the control without deferral reaches 29,296 ms on seeds 10,000 to 11,999 and 29,347 ms past 12,000 (partial), with 5 stretches over 25 s against 10 with deferral.

  My reading is that the 30 s bound has about 1 s of headroom, and deferral moves which seeds land past it. That is an inference, not a root cause. It is asked below.

## Full checks

On the final tree:

- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy: clean for site and edge, each with and without `test-faults`, and for ops.
- `cargo test --workspace`: 76 suites, 761 passed, 0 failed.
- `go test ./...` in `containers/git/server`: ok.
- The three `scripts/check_*.py`: pass. `check_links` flags only the untracked `.worktrees/target-rows` build directory.

## The probe

`tests/nr3/fresh.mjs` comments on an issue, then reads its count by REST (the page holding the comment) and by GraphQL, 150 rounds during the load. It found 0 stale answers on every after run. The before run's "stale" rows were the probe's own bug: `a1.mjs` comments on the same issue, so the REST page it read was the wrong one. GraphQL counts were never low. It was fixed in 5dbc1d3, before the after runs.

## The remaining p99: cold edge isolates

Run 4's sampled requests (1 in 100, 570 at status 200, `tests/nr2/stages.mjs`):

- **The tier read.** p50 495, p90 995, p99 1,791 ms.
- **The slow requests.** Of the 43 requests over 1.5 s total, most had `resolve` or `auth` over 1 s: 1.2 to 3.8 s, on isolates that read the token, the login, the `Placement`, the owner `Authz`, and the `Release` genesis cold. Their tier reads were 0.4 to 1.8 s.
- **One read sits under every system read on a cold isolate.** That is the `Release` genesis, which `graftyard_lookup::genesis` reads once per isolate, on the `Release` group's leader through `call`. During run 4, `release#1#f2` answered 3,566 `/read`s at wall p50 5 ms and p99 1,867 ms, against 0 to a few hundred in the other runs: a burst of cold isolates.
- **Install `Authz`.** `authz:1`, the install's `Authz`, is read by `install_authz` for every GraphQL query and every REST `GET` that renders the directory. It answered at p99 1.6 to 1.8 s on both followers in runs 2 and 4. It already goes through the same per-isolate `AUTHZ` cache as the owner's, so this tail is cold isolates too.

Run 2's isolates were warmer: `resolve` p99 577 ms and `auth` p99 293 ms. That is the whole difference between 2.1 and 3.3 s, since the tier read was 1.96 and 1.67 s.

So the next cut is not the learners. It is the first reads of a cold edge isolate.

**Candidate cut.** Have the edge accept the `Release` genesis from the edge script's own binding, as test installs' `GRAFTYARD_GENESIS` stands in for, instead of reading it from the `Release` group's leader. Then a burst of new isolates no longer queues on one member before any other read can start.

That changes how an isolate learns the system chains' records, which DESIGN, Shape, "The `Release` view", states. It is the design question below.

## F67 at setup

All four fresh repositories this window came up with three followers at term 2 and no leader after setup. Learner placement failed "no leader" until a test-only `/campaign` on site 0 answered `resumed`:

- repo:65538 at 05:08:28Z;
- repo:65539 at 06:09:38Z;
- repo:65540 at 06:54:56Z;
- repo:65537 for the before run, at 04:32Z.

In each, site 1's original voter (`#1#f1`) stayed at commit 1, while a re-placed `#N#f1` held the slot. This is the same shape narrow-reads-2 sent to f67-liveness.

## Cost

Hosted spend on `graft-m3e` from 04:17Z to 19:25Z, the whole thread on the $40 cap, is **$11.09**, containers about $0.9 included. Every repository from the final-tree runs (65543 to 65548) is torn down. The sites and edge on `graft-m3e` run 9e16a8af (learners pipelined). Earlier, hosted spend from 04:17Z to 07:10Z was **$4.64**, containers $0.54 included (`INSTALL=graft-m3e python3 tests/fanout/m2bcost.py 2026-11-13T04:17:00Z 2026-11-13T07:10:00Z`). That is under the $25 cap. Every shard container is torn down (`teardown.py --all-nets --pools`).

No design cost moves:

- Learner count and read rate are unchanged.
- A learner now does at most one read-index round per read, where it did up to several. That lowers learner-to-leader calls; `scripts/cost.py` prices learner reads as one Durable Object request each and does not count retries.
- The cost scripts were not rerun.

After the instrument run (b): spend from 2026-11-13T04:17Z to 2026-11-14T19:45Z is **$41.08** (`m2bcost.py`),
under the $48 stop and the $55 cap. Repository 65564 and its pool shards are torn down (`teardown.py --pools`).

## Push p99

The raw push timing from narrow-reads-2's window went to hot-shards-2, with the per-minute series in `tests/nr3/raw/git-by-minute.jsonl`. Nothing in it is specific to learners or reads: git requests never take the read-class caches, and learners serve no git path. In this window's clean runs, agent push p99 was 4.4 and 5.4 s.

## Design questions

1. **May an edge isolate take the `Release` genesis from its own script binding** instead of reading it from the `Release` group's leader? The trust anchor already sits in the script, and acceptance against it would not change. Or should the genesis stay a group read and be spread to followers as `Authz` reads are? This is the cut I would make next for the cold-isolate p99.
2. **The 30 s liveness bound.** The long sweep's live half passes linearizability, but exceeds the bound by 0.7 to 2.7 s on 2 seeds of 2,000 with deferred reads, from transfers that do not complete. Should F67's bound carry more headroom, or is a stalled transfer at 10 to 12 s a liveness bug to chase on f67-liveness?
