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

## Push p99

The raw push timing from narrow-reads-2's window went to hot-shards-2, with the per-minute series in `tests/nr3/raw/git-by-minute.jsonl`. Nothing in it is specific to learners or reads: git requests never take the read-class caches, and learners serve no git path. In this window's clean runs, agent push p99 was 4.4 and 5.4 s.

## Design questions

1. **May an edge isolate take the `Release` genesis from its own script binding** instead of reading it from the `Release` group's leader? The trust anchor already sits in the script, and acceptance against it would not change. Or should the genesis stay a group read and be spread to followers as `Authz` reads are? This is the cut I would make next for the cold-isolate p99.
2. **The 30 s liveness bound.** The long sweep's live half passes linearizability, but exceeds the bound by 0.7 to 2.7 s on 2 seeds of 2,000 with deferred reads, from transfers that do not complete. Should F67's bound carry more headroom, or is a stalled transfer at 10 to 12 s a liveness bug to chase on f67-liveness?
