# Milestone 0's carried rows on the current code (2026-11-11 and -12)

Rerun on `main` at `9a6d955` and branch `m0-carried`, on the `graft-m3c` test install
(`tests/am1/results/install-m3c.md`) in the Milestone 0 account. No gate number changed. Raw rows are in
`raw-carried/`.

| Row | Gate | Result | Verdict |
| --- | --- | --- | --- |
| B0-01 | Commit p99 on `Tri`, leader enam, 1 KB, 100 commands/s: pass at most 600 ms, fail over 1 s | 6 of 6 one-minute runs at p99 244 to 377 ms, 6,000 commands each, 0 failed | **Pass** |
| B0-02 | At least 300 commands/s at commit p99 at most 600 ms, one entry in flight; fail under 100/s | At 330 offered, 308/s carried in 8 runs: 6 at p99 286 to 509 ms, 2 between the lines (656 and 1,000 ms). 0 failed commands | **Pass** (6 of 8 runs; see below) |
| B0-07 | See its section | See its section | See its section |
| LF-12 | Commit p99 ≤ 600 ms placed, ≤ 1 s before; zero `overloaded` AppendEntries replies; ≤ 1,000 held per member, the rest 429 with `Retry-After`; zero polls on the leader; zero on followers after placement | `am1-fix` lf1-6: 474 ms placed, 710 ms before; 0 failed AppendEntries calls of any kind in 165 samples | **Pass**, already recorded; the Milestone 0 status table was stale |
| B0-29 | Not run here | LM-1's rerun (thread T-01a1c694-6112-72b4-83ca-dc9337a9646e) owns it | Unchanged |

## B0-01 and B0-02

`tests/m1/wp2/run.ts bench --rate R --seconds 60`, the M1-WP2 load: `R / 5` callers from this orb (LAX) to the test
edge, each running 5 workers at 1 command/s of 1 KB through the edge's caller (one RequestId per write, a hedge only
by `suspect`) on one new `Tri` register group led from site 0 (enam). The test edge is `tests/m1/wp2/edge.ts` bound to
`graft-m3c-site-{a,b,c}` (`tests/m0carried/edge.jsonc`, script `graft-m3c-wp2-edge`, a separate script, so no site
version). Commit is measured on the leader from admission to the ticket's resolution, as M0 and M1-WP2 measured it.
Site versions were merge-budget's site day of 16:30Z on 2026-11-11 (`9684ae63` / `c58c97c9` / `eadf3310`, a
`test-faults` build, as the install's thread reported them); this file did not check which commit that build was
from. No other thread ran on `graft-m3c` during the window (23:22Z to 23:49Z); each agreed to hold
load.

| Run | At (UTC) | Commands | Failed | Carried /s | Per entry | Commit p50 | Commit p99 (ms) | Round p50 / p99 | Leader confirm p50 / p99 | Append to site p50/p99 | Leader |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| pilot1 (15 s) | 23:22 | 1,500 | 0 | 93 | 16.7 | 255 | **349** | 168 / 238 | 25 / 39 | 1 168/238 / 2 296/338 | 0 |
| b01-1 | 23:24 | 6,000 | 0 | 94 | 13.3 | 204 | **281** | 134 / 160 | 23 / 28 | 1 134/160 / 2 184/226 | 0 |
| b01-2 | 23:25 | 6,000 | 0 | 93 | 14.6 | 224 | **313** | 146 / 188 | 32 / 38 | 1 146/188 / 2 295/479 | 0 |
| b01-3 | 23:26 | 6,000 | 0 | 94 | 18 | 280 | **377** | 182 / 207 | 43 / 51 | 1 182/207 / 2 359/453 | 0 |
| b01-4 | 23:28 | 6,000 | 0 | 93 | 10.8 | 165 | **244** | 108 / 135 | 26 / 31 | 1 108/135 / 2 300/513 | 0 |
| b01-5 | 23:29 | 6,000 | 0 | 93 | 11.9 | 183 | **259** | 119 / 152 | 29 / 76 | 1 119/148 / 2 245/304 | 0 |
| b01-6 | 23:30 | 6,000 | 0 | 94 | 13.3 | 204 | **294** | 133 / 206 | 20 / 27 | 1 133/206 / 2 208/372 | 0 |
| b02-1 (300 offered) | 23:31 | 18,000 | 0 | 280 | 44.9 | 233 | **360** | 151 / 214 | 38 / 52 | 1 151/214 / 2 232/434 | 0 |
| b02-2 (300) | 23:33 | 18,000 | 0 | 280 | 63.8 | 331 | **641** | 203 / 322 | 32 / 52 | 1 203/322 / 2 325/852 | 0 |
| b02-3 (300) | 23:34 | 18,000 | 0 | 280 | 58.1 | 304 | **566** | 187 / 301 | 47 / 92 | 1 187/301 / 2 294/626 | 0 |
| b02-r330-1 | 23:35 | 19,800 | 0 | 308 | 73.1 | 348 | **656** | 211 / 369 | 28 / 58 | 1 211/369 / 2 244/600 | 0 |
| b02-r330-2 | 23:37 | 19,800 | 0 | 308 | 46.3 | 216 | **367** | 139 / 223 | 24 / 41 | 1 139/223 / 2 248/518 | 0 |
| b02-r330-3 | 23:38 | 19,800 | 0 | 308 | 40.6 | 192 | **311** | 123 / 180 | 27 / 50 | 1 123/180 / 2 364/948 | 0 |
| b02-r400-4 | 23:39 | 24,000 | 0 | 334 | 80.7 | 328 | **608** | 199 / 351 | 17 / 44 | 0 199/351 / 2 232/592 | 1 |
| b02-r400-5 | 23:41 | 24,000 | 0 | 371 | 93 | 370 | **832** | 218 / 477 | 43 / 107 | 1 218/477 / 2 301/811 | 0 |
| b02-r330-6 | 23:42 | 19,800 | 0 | 308 | 52.1 | 247 | **470** | 150 / 253 | 35 / 57 | 1 150/253 / 2 343/759 | 0 |
| b02-r330-7 | 23:43 | 19,800 | 0 | 308 | 42.6 | 202 | **286** | 131 / 175 | 35 / 42 | 1 131/175 / 2 368/944 | 0 |
| b02-r330-8 | 23:45 | 19,800 | 0 | 308 | 55.6 | 264 | **509** | 162 / 288 | 34 / 53 | 1 162/288 / 2 356/808 | 0 |
| b02-r330-9 | 23:46 | 14,850 | 0 | 233 | 94.6 | 622 | **1,000** | 300 / 1,000 | 166 / 800 | 1 300/908 / 2 559/1,000 | 0 |
| b02-r330-10 | 23:47 | 19,800 | 0 | 308 | 43 | 205 | **290** | 132 / 162 | 36 / 52 | 1 132/162 / 2 407/920 | 0 |

"Carried /s" is commands over the run's wall time including its last 15 s chunk's tail, so 300 offered shows as 280
and 330 offered as 308; the leader's own rate inside the minute is the offered rate. The gate's "at least 300
commands/s" is read on carried commands, so B0-02 is judged on the eight 330-offered runs.

- **B0-01: Pass, 6 of 6** (p99 244 to 377 ms; with the pilot, 7 of 7). M1-WP2's two passes had 2 of 6 and 0 of 6
  under 600 ms. Round p50 is 108 to 182 ms, as before (108 to 153 ms in the second pass); the tail is what changed.
  Leader confirm p99 is 27 to 76 ms, against 98 to 613 ms in M1-WP2's second pass, and round p99 is 135 to 207 ms
  against 281 to 613 ms. So the second pass's tail, which it traced to the leader's own storage confirm
  (`max(confirm, round)`), did not occur here. What changed is not proven: the host's commit path has changed since
  (`m1-wp2.md`; the B0-11 member memory work, `lm1-memory`'s commit split), and this install's sites are placed
  differently (site 1, weur, answers at p50 108 to 182 ms). No same-hour M0 control ran; the M0 prototype scripts
  `graft-raft-*` no longer exist in the account.
- **B0-02: Pass** on the gate as written, read as M0 read B0-01 when it passed 19 of 23: 6 of the 8 runs at 308/s
  carried are at or under 600 ms (286 to 509 ms), 2 are between the lines (656 ms, and r330-9 at 1,000 ms), and none
  is under 100 commands/s. At 300 offered: 2 of 3 under 600 ms. At 400 offered (334 and 371/s carried): 608 and 832 ms,
  between the lines.
- **The two misses at 330.** r330-9 is the leader's storage: its own confirm went to p90 458 and p99 800 ms, the round
  followed it (p99 1,000 ms, the sample cap), and the leader tripped `STEP_DOWN` on `Confirm { p99_ms: 1042, slow: 6,
  of: 21 }`, held with no target (`to: None`), and kept leading. That is the Durable Object storage stall that LF-12,
  B0-29, and M0's B0-02 each recorded (`tests/m0/results/m0-wp12.md`, LF-12, tenth pass). r330-1's p99 656 ms is the
  entry size: 73 commands an entry against 41 to 56 in the passing runs, with round p99 369 ms; its confirm was 28 / 58
  ms. Neither is a code path the host controls (one entry in flight is the design's), so no code change follows
  [inference].
- **Why the verdict moved from "between" in M0.** M0's three 300-offered runs carried 159 to 280/s with 60 to 82
  commands an entry and p99 750 to 1,288 ms. Here the entries are 41 to 73 commands at 308/s, with round p50 123 to
  211 ms. The M0 prototype's round was p50 101 to 135 ms at 100/s and grew under load (739 ms at 1,000 offered,
  `m0-wp2.md`); the Rust host's grows less [measured here, cause not isolated].

## LF-12: the record

`build/BUILD-PLAN.md`, AM1 status: "**Pass after `am1-fix`** (day 7, lf1-6)". The Milestone 0 status table still
listed it as carried, "Between pass and fail on the final rules". Against each gate line, on `am1-fix`'s lf1-6 run
(`tests/am1/results/am1-fix.md`, \"After the fix\"):

| Line | lf1-6 | Verdict |
| --- | --- | --- |
| Commit p99 at the leader ≤ 600 ms once `place_learners` has run | 474 ms (n 50), p50 217 ms | Pass |
| ≤ 1 s before it | 710 ms (n 50) | Pass |
| Zero `overloaded` replies to AppendEntries | 0 failed AppendEntries calls of any kind: 165 `/stats` samples of every member from 03:47:12Z to 03:52:42Z on 2026-11-09, `failedCalls` 0 in all, `calls.append` 967 ok and 0 failed; 11 learners after placement (`raw-carried/lf12watch-repo-1048583.jsonl`, copied from the parent thread's orb). `failedCalls` resets on each read and counts every call that errored, missed CALL_DEADLINE, or was injected (`workers/site/src/host.rs`), so a Durable Object `overloaded` error is among them | Pass, measured after the fact from the watch file |
| ≤ 1,000 held per member, the rest 429 with `Retry-After` | B: 1,000 held on each follower, 3,000 `full`; A: 360 × 429, each with `Retry-After` | Pass |
| Zero polls on the leader | `leaderPolls` 0 | Pass |
| Zero on followers after placement | voter polls 0 more after placement | Pass |

The client's p99 through the edge (about 2.7 s with no polls) is the client path, not the commit, which the row
measures at the leader (`decisions/DECISIONS.md`, A24's close). LF-12 is done; the Milestone 0 table now says so.
The gate is one run, as B0-29's is.

## B0-07

See "B0-07 on the hosted code" below once run.
