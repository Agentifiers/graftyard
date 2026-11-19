# held-grant on graft-m3f: B0-29 row m3fhg (2026-11-19)

This is the hosted check of `held-grant` (`decisions/DECISIONS.md`, "`held-grant`"), granted by the orchestrator
(T-01a17bda).

- **Build.** Site day and edge day m3fhg from main `ab6086b4`: `held-grant` on the sites and `shed-retry` on the edge.
  - Sites a/b/c serve `83d73101` / `f479ebab` / `53e3f203`; graft-m3f-edge serves `e25b8638`.
  - The image is `sha256:15bc0e2d…` (m3fg12's). `containers` is unchanged since `f466a39e`.
- **Row.** One B0-29 row on run 7's script (`tests/fanout/b029-m3f.sh`, which wraps `b029.sh`), fresh repo:65595, a1 run
  `mwnmao4d`. Load ran 14:07:29.7–14:17:29.7Z through the egress proxy, under the harness-outage rule.
- **Stop.** $6 on graft-m3f, counted from 13:42:08Z. The install spent $2.44 (`acctcost.py`, `prefix_usd`, read at
  15:03Z).
- **Raw files.**
  - `tests/fanout/raw/rows-m3fhg.status`, `tsave-m3fhg.jsonl.xz`, `terms-m3fhg.jsonl`, `arrivals-m3fhg.jsonl` and
    `edge500guard-m3fhg.status`.
  - a1's `a1-b0-29-mwnmao4d.jsonl` and `a1-samples-b0-29-mwnmao4d.jsonl.gz` are under the ignored `tests/am1/raw/` on
    this thread's orb.

Since main `88ae0051`, B0-29's commit line is 800 ms, 880 at 90% (`FRAME.md`, "Operator decision, 2026-11-19, B0-29's
commit line"). The row is judged against that line.

## Verdict lines

| Line | m3fhg | Against |
|---|---|---|
| held-grant's flip: `SentBy::Walk` within 2 s of a held-and-stepped leader read (`heldgrant.py`) | **0 of 179** `ranges` and **0 of 3** merge rows. Held but not stepped: 0 of 112. Not held: 0 of 4,212 and 0 of 65 | Traces before the fix: 234 of 244, 91 of 92, 193 of 195 (about 95%). **Pass** |
| Slow merge lookups (`tier_ahead` over 1 s on a REST merge's rows read) | **1 of 69** (2,298 ms) | Before the fix: 1, 1 and 3 per trace at 928–2,168 ms. The one here is not held-grant's case (below) |
| `/stats` commit p99 | **1,325 ms** (p50 406, n 10,576) | **Fail**: over the 1 s fail line. The tail is the leader change (below) |
| Round p50 / p99 (`/stats`), for F98 | 178 / 349; cycle 221 / 465; `queue:slot` 177 / 814; confirm 32 / 82; `deps:all` 55 / 333 | Run 7: 166 / 325 |
| Leader changes | **2**: site 0 → site 1 (term 3) at about 14:12:25, then back to site 0 (term 4) at 14:12:40 | B0-29 has no leader-change line. B0-33's is zero |
| Edge 500s or exceptions | **0**. All 150,097 edge invocations from 14:00 to 14:19 ended `ok`. `fivexx.py` found no 500 | One would have stopped the row |
| Harness outages | 8 s, 1.3%, **valid**. Every line reads the same with them removed | Over 5% is invalid |

Other lines:
- `merge:only` p99 19,352 ms (62 × 200, 7 × 409, 1 × 503).
- API reads p99 2,377 with 97.65% served.
- Agent pushes: 2,429 of 2,501 ok.
- Leader at 188 req/s.
- Colos: LAX 1,208, no DFW.

**Held-grant's change works as designed.** The edge's leader reads no longer come back `notLeader` from a leader that
holds the read. Before the fix about 95% of held-and-stepped reads were followed by a walk; on this row none were. The
row walked 100 times in all: 76 posts on site 0, outside 14:12 about one a minute, and the rest bunched at 14:12, during
the change.

## 1. Why site 0 went 24 s without confirming

### Measured

- **One storage confirm was outstanding for 24 s.**
  - At 14:12:11.546 site 0 (leader, term 2) appended entry 1961 (159,090 B) and issued its AppendEntries to both
    followers (`AppendIssued`, kind 13).
  - The confirm of that input's transaction (27 statements) ended only at 14:12:35.503: 23,957 ms.
  - All 12 slow confirm rows end at that same instant, 14:12:35.503. Their starts run from 14:12:11.546 to
    14:12:35.163, and they cover applied index 1965. So one flush covered every write queued in those 24 s.
  - Whole run: 3,755 leader confirms, p50 40, p99 107. The next slowest outside this window were 900 ms (14:12:41.7)
    and 694 ms (14:13:59.9).
- **The isolate was not blocked.**
  - The trace has 0 event-loop lag rows (kind 8) in the whole run.
  - Site 0 kept taking and finishing requests through the stall: `Entered` (kind 25) and handler-returned rows
    (`CallLeft`, kind 24) every second from 14:12:12 to 14:12:34.
  - The isolate id did not change (`2e1da20400c8` throughout), and Workers Logs show no reset, exception, `mem high`
    or member alarm on site a between 14:11:30 and 14:12:45.
- **What the isolate did not do was send.**
  - Site 0's answers to its callers reached them only at 14:12:34–35: 214 answers, with arrival minus site 0's
    handler-return stamp at p50 21.65 s.
  - Its AppendEntries reached neither follower after 14:12:11 (`CallArrived` from site 0: 4 at each follower at
    14:12:11, then none until 14:12:35).
  - Site 0 logged no `AppendEnded` for any call to a voter between 14:12:11.628 and 14:12:40.627.
  - This is the output gate: a Durable Object holds "outgoing network messages … until all previous writes have been
    confirmed flushed to disk" (`research/cloudflare.md`; `design/DESIGN.md`, Scale, "Commit on `Tri`").
- **The followers elected without it.**
  - The edge's `/suspect` reports about site 0 began at 14:12:14. The followers answered `promised` (they withhold
    while the leader's promise stands) and then `busy` from 14:12:21.
  - Site 2's `ReadIndex` calls to site 0 failed at the 1 s deadline from 14:12:13.
  - Site 1 appended as leader at 14:12:25.5, about 14 s after site 0's last send. That is `ELECTION_AFTER` (10 s) plus
    the election.
- **Site 0 took the group back.**
  - Site 0 stepped after its flush at 14:12:35.5, was a candidate at term 4 at 14:12:39.757 and leader at 14:12:40.758.
  - `terms.mjs` caught both changes. Its sample at 14:12:12 took 23,752 ms because site 0's `/status` answer was held
    too.
- **The load did not differ from run 7's before the stall.**
  - Site 0's inputs in the 4 min before: 154 requests/s and requests open at each entry p50 8 / p99 21. Run 7 had 159/s
    and 8 / 22.
  - Second by second, the rows logged at 14:12:00–14:12:11 look like every earlier second.
  - Entry 1961's 159 KB is one of 132 entries over 150 KB in this load (run 7: 117).
  - Its flush of 27 statements is under the run's p99 of 67; the largest was 281.
  - The rise in `/read`s at site 0 at 14:12:14 (307 `Entered` that second) came after the stall started. The edge
    retried and walked because site 0's answers were not coming back.

### Not the storage-stall shape of m0-wp9

That pass's B0-29 misses (`p5r1`, `p6r1`) had 10 and 12 failed AppendEntries in 600 s, and 22 and 7 confirms over
500 ms, each adding a 1–1.7 s window (`tests/m0/results/m0-wp9.md`, seventh pass).

m3fhg had **0** failed AppendEntries to a voter in the whole run, and one 24 s stall. Nothing that m0-wp9 counted would
have warned of it.

`STEP_DOWN`'s confirm rule could not act either:
- A confirm is judged only when it ends, and none ended for 24 s.
- Any transfer it started would have waited at the same output gate.

The followers' election was the only way out, and it took 14 s. What the leader should do instead is `decisions/OPEN-QUESTIONS.md`, F99.

### Not the new code [measured for what each change touches; inference that nothing else did]

- **`held-grant`** (`Host::release` → `released_view` → `Member::fresh_after`) chooses which fold a held read answers
  from. It adds no storage write and no await. Its whole diff against `f466a39e` is that call, its function and its
  test (`git diff f466a39e ab6086b4 -- workers/site`: `host.rs` and `lib.rs` only, plus the test).
- **`shed-retry`** is edge-only. The edge's traffic to site 0 was normal until the stall had already begun.
- Neither can hold a DO's storage flush. The isolate's own loop kept running, which a blocking change would have
  stopped.

### Inference

The cause is a platform storage stall: the Durable Object's `sync()` for one transaction did not complete for 24 s.

- **Evidence from inside:** the confirm row, the output gate holding every send, and the isolate otherwise live.
- **What the code can see:** nothing below `sync()`, so this is not proven further from inside.

Two nearby events are recorded and are not shown to be the cause:
- Site a's script ran a promotion from about 14:11:52 to 14:12:03.2. It was in-band through the `SITE` binding, a
  separate invocation and not the member: 1,463 packs, 25.5 MB in 10,809 ms. It ended 8 s before the stall.
- At 14:11:54 a 1 s dip showed on all three sites (trace rows thin and a 988 ms round). Its cause is not known.

## 2. merge:only p99 19 s and the 409s

**The three slow merges all started inside the change** [measured: a1 samples, `mergeSteps`, `walkseq`]:

| started | ms | answer | where the time went |
|---|---|---|---|
| 14:12:11.209 | 19,352 | 200 | `commit` 18,931: admitted by site 0 as the stall began, and answered after it |
| 14:12:35.822 | 15,583 | 503 `git cache … warming` | The shard warmed once site 0 led again (`shard warm 65595 site 0 597ms`, 14:12:41.3) |
| 14:12:40.338 | 3,936 | 200 | `tier_ahead` 2,298: lookup `s1L s0W`. The edge held site 1, leader at term 3, which had just handed back, and walked to site 0 |

`merge:only` outside 14:12:07–14:12:41 was n 67, p50 973, **p99 1,781**, under the 2 s line.

The `tier_ahead` over 1 s is a correct walk off a deposed leader. It is not held-grant's case: that read was not held at
site 1.

**The 7 × 409 are `BaseModified`** ("Base branch was modified"), all `merge:only` [measured]:
- Each overlaps another merge, started 0.2–0.8 s before it, that answered 200.
- Two at 14:14:02 overlap each other as well as a 200.
- So a1's merges race on the same base, and the git lease refuses the stale one. This is correct.
- 2 were before the change and 5 after, none in it.
- Earlier rows: run 7 had 5 and run 6 had 14 (`tests/am1/results/b029-m2b.md`).

## 3. Round time outside the change, and whether 800 ms holds

Rounds are leader append to apply, from site 0's trace (kinds 15 and 4), outside 14:11:50–14:13:00 [measured]:

| | n | round p50 | round p99 | entry bytes p50 / p90 |
|---|---|---|---|---|
| m3fhg before (14:07:30–14:11:50) | 1,092 | 180 | 323 | 34,068 / 121,327 |
| m3fhg after (14:13:00–14:17:30) | 1,169 | 182 | 322 | 29,991 / 121,024 |
| run 7 (`tsave-m2bb29h`) | 2,746 | 167 | 316 | 28,129 / 116,464 |

So round p99 is the same as run 7's (+6 to 7 ms), and p50 is 13–15 ms higher. `/stats`'s 349 includes the change.

Commit, modelled per command through the cuts (`bycut.py`, which ran 70 ms under `/stats` on run 7: 747 against 817):

| window | modelled commit p50 / p99 |
|---|---|
| 14:07:30–14:11:50 (before) | 394 / **670** |
| 14:13:00–14:17:30 (after) | 401 / **1,105** |
| whole row without 14:11:50–14:13:00 | 398 / **859** |
| run 7, whole row | 383 / 747 |

Per minute: 713, 639, 663, 642, (1,261, 2,027 in the change), 1,057, 1,091, 797, 1,514. Run 7's 2-minute windows read
688, 623, 1,190, 723 and 975.

**Answer: 800 ms holds before the change and not after it.**

- **Before the change:** modelled p99 670. With run 7's +70 offset that is about 740 at `/stats` [inference].
- **After the change:**
  - The worst minute (14:16:30) has one 1,036 ms round. Site 0's AppendEntries for entries 3000 and 3001 reached both
    followers 0.9 and 1.1 s after it sent them (`CallArrived` stamps against `CallSent`).
  - No slow confirm was logged then (the confirms around it were 40–169 ms).
  - Requests open at site 0 reached p99 51 that minute, against 21 before the change.
  - 14:13:30's tail is `post` (deps and decide, p99 526), not rounds.
  - Run 7 had the same kind of minute (18:28:33, one 913 ms round, 1,190).
- **Without the change** the row is at the line: 859 modelled, which is under 880. But it would read about 930 at
  `/stats` [inference]. The ~1 s held-send rounds decide it, as storage stalls decided m0-wp9's runs.
  - Whether those short holds are short versions of the 24 s stall is not shown. The 14:16:34 one had no slow confirm
    logged.

## Driver fix in this branch

- **The fault.** `tests/fanout/terms.mjs` exited `ENOENT` at the start of m3fhg. A fresh install's state has no
  `am1-runs-repos.txt` until `rows.sh` creates the first repository, and terms.mjs read it before that.
- **The fix.** A missing file now waits like a missing line.
- **Checked with a scratch harness of `idOf`:**
  - missing file → `undefined`, so it waits;
  - written file → `65595`.
- **During the row.** It started terms.mjs again by hand from 14:11:27Z with `repo:65595`, so this row's
  `terms-m3fhg.jsonl` starts there.
- **Two other fixes to the record:**
  - `outages.sh`'s first two Workers Logs reads came back degraded (a 500, then sparse 10,000-count buckets marking
    7.5%). The status file keeps them, with a third full read appended (8 s, 1.3%).
  - The status file's traceback paths are masked to `<worktree>/`.

## Close-out

- `retirenets.mjs 65595`: 12 names, all not joined.
- `purge.py` twice: 2,442 objects, then 0.
- `joined.mjs 65595`: "none joined of 1".
- `teardown.py --sweep` twice: no shard container running.
- Services `row-m3fhg` and `terms2-m3fhg` are stopped.
