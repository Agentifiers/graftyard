# B0-29 on graft-m3f: the `merge-front` three-run set (2026-11-23)

**Verdict on the median of three valid runs (`FRAME.md`, "Operator decision, 2026-11-20"): B0-29 passes on every line.** Commit p99 756 ms against 800; merge:only p99 1,663 ms against 2 s, with the pooled p99 at 1,406 ms (n 252) and no merge over 2 s. API read, `Stale`, leader changes, leader load and feed polls pass on every valid run.

The set measures `merge-front` (`decisions/DECISIONS.md`): a REST merge's identity reads run alone, outside their key's
write-class batch, and it guesses its repository at any age or from the data center's link entry. The lead ruled run 1
invalid: a site cron's fold audit refolded the repository on its leader inside the load window, a harness fault. Runs
2, 3 and 4 are the valid three. Run 1's merge-front flip data stands as data.

| Run | Site day, main | Commit p99 (p50, n) | merge:only p99 (p50, n) | Setup CPU a commit (hostguard) | Loaded leader | deps:all p99 | Frozen wall share | API read p99, served | Stale |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1, b9m3f1 (`mwto0w7z`, repo 65625), **invalid** | day 1, `327ac2bb` | 2,188 (375, 11,248) | 4,928 (869, 91) | 41.8 | 0.24 vCPU, 2.09 ms an event, 7 ticker lags up to 708 ms, all in the refold minute | 514 | 43% | 1,468, 100% | 25 of 25 pass (rerun with the fixed `stalecheck.mjs`) |
| 2, b9m3f2 (`mwtq6v8x`, repo 65626) | day 2, `327ac2bb` (main `d1c47f80`, no runtime change) | **756** (432, 11,440) | **1,254** (907, 87) | 32.9 | 0.13 vCPU, 1.23 ms an event, 0 ticker lags | 247 | 39% | 1,945, 100% | 13 of 13 |
| 3, b9m3f3 (`mwtra5w1`, repo 65627) | day 3, `53d4b6d7` | **797** (432, 11,292) | **1,663** (933, 82) | 31.1 | 0.13 vCPU, 1.27 ms an event, 0 ticker lags | 257 | 39% | 1,892, 100% | 22 of 22 |
| 4, b9m3f4 (`mwtsyums`, repo 65628) | day 4, `53d4b6d7` | **651** (344, 11,476) | **1,664** (782, 83) | 32.8 | 0.15 vCPU, 1.43 ms an event, 0 ticker lags | 226 | 37% | 1,216, 100% | 22 of 22 |
| **Median of runs 2 to 4** | | **756** | **1,663** | | | | | | |

**Merge, pooled:** runs 2 to 4 together give n 252, p95 1,135 ms and p99 1,406 ms, with 0 merges over 2 s, so the median of the run p99s (1,663) and the pooled p99 both pass the 2 s line. Each run's p99 is its slowest merge, since n is under 100.

All runs led from site a at term 2 with 0 leader changes, had 0 edge status 500s and 0 exceptions, and were valid
(harness outages 1 to 2 s, shares 0.0017 to 0.0034). Leader load 148 to 150 req/s with 0 feed polls on every run. Every
leader drew a host over hostguard's 28.4 line (31.1 to 41.8 ms a setup commit), and loaded at 0.13 to 0.15 vCPU on the valid
runs, under b9m4a3's 0.16.

## The merge-front flips

`decisions/DECISIONS.md`, "`merge-front`", "Flips", per run [measured]:

| Flip | Bound | Run 1 (data) | Run 2 | Run 3 | Run 4 |
| --- | --- | --- | --- | --- | --- |
| Merges with no guess (`authz_ahead` absent) | under run 3's 5 of 90 (b9m4a3r3) | 0 of 91 | 0 of 87 | 0 of 82 | 0 of 83 |
| `rcwait` on a merge, p90 / max | | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| Merges that shared a batched read (`rcshare` > 0) | | 0 | 0 | 0 | 0 |
| Merge front p50 / p90 / max (ms) | b9m4a3r3 124 / 363 / 640 | 89 / 218 / 644 | 82 / 172 / 453 | 82 / 144 / 222 | 79 / 174 / 431 |
| Owner `Authz` leader (`authz:7100001`) reads shed: `shedReads`, `readsMax` | 0 | 0, 0 | 0, 0 | 0, 0 | 0, 0 |
| Owner `Authz` leader `/read`s in the row (dep index / dep plain / repo) | | 845 / 4,818 / 161 | 794 / 4,743 / 139 | 862 / 4,616 / 128 | 798 / 5,095 / 136 |
| A kept read ahead naming another repository | none | none | none | none | none |

- **No guess, no wait.** Every merge in all four runs read ahead on a guess, and no merge waited in a batch. The
  front is under a quarter of a second at p90 on every valid run, against 363 ms on b9m4a3r3.
- **The leader reads.** `tests/fanout/authzreads.mjs` sampled the owner `Authz` group's leader every 30 s through each
  row. It shed nothing on any run and stayed the same leader (site 0) throughout. Merges are about 90 a row, so their
  own leader reads are a few hundred of the 5,600 to 5,900 the leader answered; the rest are the identity `dep` reads
  of every write [inference: the counts are by read kind, not by route].
- **A kept read on another repository.** The merge path does not log `beside_bad` (`kept_ahead` is the rule, and its
  test `a_write_keeps_its_ahead_reads_only_for_the_repository_it_resolved` holds it). Every merge here resolved the
  repository its guess named, so no read ahead was dropped and none was kept on a mismatch [measured: no merge carried
  a `tier` or `authz` stage after `tier_ahead`/`authz_ahead`].
- **What sets the merge tail now.** The commit, on every run: the slowest tenth is led by `commit` (9 of 9, 7 of 8 and
  8 of 8). Each run's slowest merge: run 2 1,254 = front 76 + plan 130 + commit 566 + client leg 482; run 3 1,663 =
  front 105 + plan 809 (shard `merge` waiting behind a test merge) + commit 726 + 23; run 4 1,664 = front 431 + plan 149 + commit 1,059 + 25.

## The commit line

Commit p99 **756** on the median of runs 2 to 4 (756, 797, 651), against the 800 ms line and the 1 s fail
line. It passes: all three valid runs are under 800, on hosts at 31.1 to 32.9 ms a setup commit, all over hostguard's 28.4 line. Against b9m4a3's median 744, the commit line holds with `merge-front` on main; `merge-front` does not touch the commit path, and none of these runs drew a host as slow as b9m4a3 run 2 (94.4 ms), so the slow-host case stays open, as ruled.

## The merge's commit stage on the leader (2026-11-23, $0, for the record)

After `merge-front` the commit stage leads the merge tail (1,059 of run 4's 1,664 ms). `tests/fanout/mergecommit.py`
splits each merge's edge `commit` stage on the leader's trace the way `tests/fanout/b033probes/humansplit.py` splits a
Human push: freeze wait (the edge's send to the cut that took it), held (frozen behind the entry in flight), own round
(first AppendQueued to the first ok voter), apply, the storage confirm after it, and the rest (the outcome's return and
the clock offset's error). A merge's send instant is estimated from a1's start, the client time and half the client
leg, so a single merge's split carries tens of ms of error [inference]; every merge joined (87, 82 and 83).

| Slowest tenth of merges | n | Client | Commit stage | Freeze wait | Held | Own round | Apply | Confirm | Rest |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Run 2, b9m3f2 | 9 | 1,134 | 703 | 156 (22%) | 105 (15%) | 226 (32%) | 0 | 65 (9%) | 150 (21%) |
| Run 3, b9m3f3 | 8 | 1,312 | 833 | 160 (19%) | 182 (22%) | 198 (24%) | 0 | 69 (8%) | 225 (27%) |
| Run 4, b9m3f4 | 8 | 1,046 | 668 | 129 (19%) | 127 (19%) | 187 (28%) | 0 | 47 (7%) | 178 (27%) |
| All 25 | 25 | 1,163 | 734 | 148 (20%) | 137 (19%) | 205 (28%) | 0 | 61 (8%) | 183 (25%) |

Means in ms [measured, the leader's trace and a1's `server-timing`]. Over all merges the queue (freeze wait and held)
is 37 to 40% too, and `rest` is smaller (14 to 20%). Run 4's slowest merge, 1,664: commit 1,059 = freeze wait 430 + held 113 + own round 178 + confirm 75 + rest 264.

- **Apply reads 0.** Applied is traced right after the ack's turn, and a busy isolate's clock reads its last I/O, so
  apply's CPU (p50 6, p90 13 ms an entry on run 4, `tests/fanout/b029join.py`) lands in the confirm and rest columns [inference].
  The leader ran at 0.13 to 0.15 vCPU with 0 ticker lags; apply and the deferred transaction were 5.9 to 13.4 ms of
  CPU in a near round of 139 to 325 ms.
- **The own round is the wire.** Near rounds p50 / p90 / p99 164 / 212 / 304 ms on run 4; the merges' own rounds sit
  inside that.
- **The queue is 39%.** Freeze wait and held together are 285 ms of the slowest tenth's 734: a merge waits for the next
  cut and then for the entry in flight, as `reask-read` left it.

**What-ifs** on the same merges (`mergecommit.py`), merge:only p99 as the run's slowest merge:

| Change | Run 2 | Run 3 | Run 4 | Median of run p99s |
| --- | --- | --- | --- | --- |
| Now | 1,254 | 1,663 | 1,664 | 1,663 |
| No held (the bound on any freeze-rule change) | 1,222 | 1,527 | 1,551 | 1,527 |
| No held and no freeze wait | 1,072 | 1,378 | 1,122 | 1,122 |
| Own round × 0.9 (a generous leader CPU lever) | 1,228 | 1,638 | 1,646 | 1,638 |

**What already merged or priced moves it:**

- **G39 (`catchup-lag`, merged):** no. It is the git shard's catch-up; every merge here read `catchup` 0 in the shard's
  timing, and the shard's work sits in `merge_plan`, outside the commit stage.
- **F113 (`removed-read`, merged):** no. It answers a read-index read at a removed voter; a merge's commit is a
  `/commit` to the leader, and these groups ran their first-generation voters with 0 leader changes.
- **LM-1's levers:** `read-memo` already ran in every one of these runs (main `327ac2bb` and `53d4b6d7` contain it).
  `encode-once` (3 to 11% of leader CPU an entry), `seal-batch` (2 to 3%) and `learner-relay` (2 to 12%) are priced
  and not built; on these hosts the leader's whole joined CPU in a slow round is 24 to 28.5 ms of 252 to 325, so even
  the generous ×0.9 on the own round moves the median 25 ms [inference]. They are slow-host levers (A41), not this tail.
- **What would move it** is the queue: only a freeze-rule change reaches held (at most 1,663 → 1,527), and nothing
  merged or priced removes the freeze wait. The last attempt at that window, `human-join`, stays reverted
  (`decisions/DECISIONS.md`, "`human-join`"). B0-29 passes without it, so this is recorded, not proposed.

```
python3 tests/fanout/mergecommit.py b9m3f4 2026-11-23T21:59:44Z
```

## Run 1, ruled invalid: the cron refold

- **The fault.** An orb service reran the site and edge day script when it exited. Its second pass (from 19:32:05Z)
  redeployed the sites, which restored each site's `*/15` Cron Trigger, and the service was stopped before that pass
  reset them. Site a's cron ran at 19:46:57Z, inside the load (19:41:29 to 19:51:29Z), and its fold audit started a
  refold of repo 65625 on its leader: from 19:48:30Z, `repo:65625#1#f0 alarm step load scans stored 814..=2649: 81399738
  bytes`, then on down to 1534..=2649 by 19:48:49Z.
- **What it did.** Minute 7 of the load ran the leader at 0.588 vCPU, against 0.193 to 0.228 in every other minute.
  Learner `/raft` turns took up to 570 ms of CPU (19,279 ms in the minute, against 7,000 to 8,400). Rounds to both
  followers reached 2.47 s and 2.58 s. The ticker lagged 7 times, up to 708 ms. The member asked to step down
  (`StepDown { why: LoopLag { clamped: 2 } }` at 19:48:42Z and 19:49:09Z), with no leader change.
- **Its numbers.** Commit p99 2,188 ms; merge:only p99 4,928 (its slowest merge: commit 2,480 and a test merge that
  waited 1,798 ms in the shard, inside the refold minute). An agent push's commit stage p99 is 2,247 over the run and
  1,366 outside the refold minute and the archive spike below.
- **The fix.** The day script now runs once and then holds. Before each row, one `*/15` tick passed with 0 scheduled
  runs on all four scripts (`graft-m3f-ops` and the three sites), and each load window had 0: 20:24 to 20:55Z across run
  2, 20:55 to 21:26Z across run 3, and 21:29 to 22:13Z across run 4.
- **`Stale`.** `tests/fanout/stalecheck.mjs` called one of run 1's 25 stale pushes a FAIL. On `a1h-mwto0w7z-19`, push
  1012 was acked ok (`24a6` to `48a9`) at 993,832; push 1016, naming `24a6`, overlapped it and was answered stale;
  push 1024, naming `24a6` again, started at 994,726, after 1012's ack, and was answered stale correctly. The check
  compared 1024 only with 1016's `old`. It now judges a stale answer that follows refused pushes against the newest
  push acked ok before it started, when every push since that ack was refused; a lost reply since the ack falls back
  to the refused push's own `old`, as before. Run 1 now reads 25 of 25 PASS; b9m4a1 and the three b9m4a3 runs read
  as before (20, 19, 13 and 16 PASS). Tests in `tests/fanout/stalecheck.test.mjs`; `tests/m0carried/plant45.py`
  mutants 0 to 6, 7 of 7 caught.

## The leader's archive step: real leader work

This is not harness. Each minute the repository leader's archive alarm sums its stored log's bytes past the archive
point (`workers/site/src/archive.rs`, `View::view`: `SELECT COALESCE(SUM(length(row)), 0) FROM log WHERE idx > ?`,
then a walk of the short rows to `ahead_bound`), and probe-steps' scan line (`alarm_steps::note_scan`) names its range.
The range starts at index 2 and grows with the log, since nothing in a 600 s row reaches the archive trigger
(`LOG_MAX_BYTES` 2 GiB / `ARCHIVE_SLACK`):

- run 1: `repo:65625#1#f0 alarm step archive scans stored 2..=N` at 19:41:42 (3.6 MB), 19:42:42 (16.3 MB), 19:43:42
  (27.4 MB), 19:44:42 (40.4 MB), **19:45:42.948Z, `2..=1916: 51922711 bytes`**, 19:47:24 (72.6 MB) and 19:49:02 (91.1
  MB);
- run 2: 20:42:57 (4.5 MB), 20:43:57 (17.0), 20:44:57 (29.2), 20:45:57 (41.2), **20:46:57.540Z, `2..=1718: 52909021
  bytes`**, 20:48:34 (72.2), 20:50:22 (93.8) and 20:51:53 (111.2 MB).

On run 1 the 19:45:42.948Z step is the start of minute 4's round spike: the leader's next `/raft` turn took 161 ms of
CPU (19:45:42.954Z), and rounds to site 1 queued from 19:45:46.789 to 19:45:50.808Z took 2,157, 1,343, 617 and 887 ms.
Minute 4 (19:45:29 to 19:46:29Z) ran the leader at **0.228 vCPU**, its highest outside the refold minute (0.193 to
0.214 elsewhere); 10 s buckets from 19:45:00Z read 0.16 to 0.27 vCPU, so the cost is a turn or two, not a minute
[measured, `tests/fanout/raw/wl-29/b9m3f1-f0.json.xz`]. On run 2 the 52.9 MB step at 20:46:57.540Z is likewise
followed by its two slowest rounds to site 1 (queued 20:47:00 and 20:47:01Z, 2,237 and 1,237 ms), in a minute at 0.132
vCPU [measured, `wl-29/b9m3f2-f0.json.xz`]. Neither run's commit line moved on it, but the scan grows about 10 MB a
minute at journey 9's rate and is summed in one statement inside the alarm, so a longer row would pay more of it
[inference]. That is LM-1's to weigh.

## The set's setup

- **Builds.** The site with `worker-build --release --features test-faults` from `327ac2bb` in a detached worktree;
  runs 3 and 4 deployed the same site build from `53d4b6d7`, whose `workers/site`, `crates` and `containers` are
  unchanged since `327ac2bb`, and an edge rebuilt from it (`edge-shed-429`). A new git shard image,
  `graft-m3f-gitshard@sha256:ff661983…`, built from `327ac2bb`'s `containers/git` (`maint-yield`'s 6 min defer); sshfront
  `sha256:fc6eda91…` reused.
- **Each day** (`tests/m4/raw/wp4/b029m3f-day{1,2,3,4}.log`): `workers/site/deploy.mjs --test-faults --anchor --image
  … --vars --ssh <store> --ssh-image … --edge graft-m3f-edge --export-bucket graft-m3f-export --skip-build`, then
  `tests/m1/integ/deploy-edge.mjs … --test-faults --anchor --ops graft-m3f-ops --ssh hostkeys.json --var
  FORMATS_ENABLED=<through 32.26, 20.4, 17.4> --export-bucket graft-m3f-export`, then the site crons removed and reset
  (a yearly cron, then `[]`) until all three read `[]`.
- **Rows** `tests/fanout/b029-m3f.sh` as orb services, `RUN_FROM=19:28:30Z`, the set's stop `RUN_STOP_USD=11.5` (the
  lead raised it from 10 after run 1); run 4 with `ROW_USD=b0-29:2.7`, since the default 4 would have stopped it at
  $7.74 + 4 against 11.5.
- **Cost:** the set was $10.34 from 19:28:30Z to the end of run 4, under the raised $11.50 set stop; each row stayed under $10. graft-m3f's UTC day was $15.30 of its $40.

## Raw data

`tests/fanout/raw/`: `{rows,edge500guard}-b9m3f{1,2,3,4}.status`, `{terms,arrivals,authzreads}-b9m3f*.jsonl`,
`shardgate-b9m3f*-b0-29-{0,1,2}.jsonl`, `tsave-b9m3f*.jsonl.xz`, `mergestarts-b9m3f{2,3,4}.jsonl`, a1's summary line as `a1-b9m3f*.jsonl`, and the
leaders' Workers Logs in `wl-29/b9m3f*-f0.json.xz`.

```
python3 tests/fanout/b029join.py b9m3f2 2026-11-23T20:42:40Z
python3 tests/fanout/mergefront.py b9m3f2 b9m3f3 b9m3f4
node tests/fanout/stalecheck.mjs tests/am1/raw/a1-pushes-b0-29-mwto0w7z.jsonl
python3 tests/fanout/hostguard.py graft-m3f-site-a 'repo:65626#1#f0' 2026-11-23T20:35:57Z 2026-11-23T20:42:40Z
```
