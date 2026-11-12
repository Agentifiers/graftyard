# Test results: chaos, stress, linearizability, conformance, and load

This file lists every run that counts toward a gate row or a decision in Milestones 0 and 1. It covers runs on Cloudflare, in the Raft simulator, and through the Oracle relay. It gives each run's gate row, place, dates, load, faults, checks, and result, and cites the results file and section the numbers come from. Smoke runs and discarded runs appear only in "Smoke, local, and discarded runs", labelled as such. Every figure is [measured] unless a row says otherwise.

Raw data paths (`raw/`) are git-ignored and exist only on the machine that ran the test. Results files cite them. Where this file and a results file's prose disagree, this file takes the raw value, and "Prose and raw data that disagree" lists the difference.

## Totals

Each total has one definition. Totals add only counts of the same thing from different runs, never the counts of different checkers on one history.

| Total | Value | Definition |
| --- | --- | --- |
| Operations checked for linearizability on Cloudflare | 12,304,207 | Operations in the histories of the seven chaos runs in "Linearizability and chaos runs on Cloudflare". Each history counts once, though most were checked two or three ways (recorded clocks, widened 100 ms, widened by the measured bound). By verdict: 9,089,215 Ok (`cf-long-1`, `cf-48h` widened 100 ms, `wp4-cf-1`, `wp4-cf-5`, `lin24b`). 475,301 Illegal from a real bug (`cf-long-2`). 2,739,691 not passed (`lin24`: Illegal on the recorded clocks, Ok widened, early ack not ruled out). Excludes the smoke runs, BF-13's 3,680 operations, and every simulator history |
| Hours of chaos on Cloudflare | 104.6 h | Wall-clock length of those seven runs: 2 + 4 + 48 + 0.52 + 2.08 + 24 + 24. The windows include stretches without chaos: four of 68 to 115 min in `cf-48h`, three driver gaps in `cf-long-2`, a fault plan overwritten for part of `cf-long-1`, and call faults on only 39 of 125 min in `wp4-cf-5` |
| Member resets injected | 16,258 | `/reset` steps by the chaos driver in those seven runs: 243 + 138 + 5,729 + 82 + 343 + 5,547 + 4,176. `cf-long-2`'s 138 comes from prose, because its event log was overwritten |
| Leadership transfer attempts injected | 10,680 | Transfer steps the chaos driver sent in those seven runs: 186 + 77 + 3,607 + 62 + 253 + 3,743 + 2,752, including planned abandons and attempts answered "not leader" or refused by a quarantined target. Transfers that moved the leader are not totalled, because M0's driver and M1's report classify outcomes differently (see each row) |
| Acknowledged writes audited against the members' logs | 3,588,424, 0 lost | `cf-48h` 1,814,873, `lin24` 869,257, and `lin24b` 904,294. Each audit reads all three members' stored logs after the run |
| Leader kills after quorum and before the reply | 12,000 landed | BF-12 on Durable Objects: 10,000 (`bf12-full`), 1,000 (`bf12-p3`), and 1,000 on `Push` through the git edge (`p2`). Every one returned its original outcome |
| SSH git sessions | 31,482, 31,362 succeeded | B0-24 (2,637), B0-24a `r4` (28,347), and the Oracle relay run `r1` (249 through the relay plus 249 control sessions through `cloudflared`). Excludes three stopped 24-hour attempts (1,144 sessions) |
| Results of git's own `t/` tests on Cloudflare | 2,667 | Rows in `results.tsv` of the no-fault baseline (1,067), the three BM-1 faulted runs (1,558), and the t5541 recheck (42), skips included. Per version below |
| `clients.sh` runs | 12 runs, 348 of 348 rows | 29 rows each, on git 2.39.5, 2.51.0, and 2.56.0, with and without a site failing |
| Tests in the workspace | 259 Rust, 14 Go | `cargo test --workspace` at `c0c6fb9`: 259 passed, 0 failed, 2 ignored (`long_sweep`, `long_liveness`). Go: `tests/m0/porcupine` 11, `containers/git/server` 3 |
| Simulator long sweep | 40,191,804 steps, 1,000,856 acked writes | One run of `long_sweep` at 2,000 seeds × 10,000 steps, HARSH plus GRAY. Every acked write passed the in-simulator ack check. Earlier sweeps are listed, not added |
| Planted bugs caught | 84 of 85 | Code mutants planted one at a time against a test suite. Planted bad histories fed to a checker are counted apart |

## Linearizability and chaos runs on Cloudflare

Every run used the same fault plan unless its row says otherwise. 5% of member-to-member calls fail from each site, and 5% of AppendEntries replies are dropped. A chaos loop resets a member or transfers leadership every 8 to 16 s, and some transfers are abandoned on purpose. Load clients are pinned in ATL, LHR, and SIN unless the row names the orb (LAX). Porcupine is `tests/m0/porcupine`, run per key with segmented mode (`-seg N`).

`scripts/results_lin.py <run>.report.json` prints the `lin24` row's first seven columns from the run's report and event log. Use it for every later lin run.

| Run | Gate | Where and when | Duration | Operations | Faults | Checks | Result | Source |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `cf-long-1` | B0-11 | Cloudflare, `graft-raft-*`, 2026-10-28T08:42Z to 10:42Z | 2 h | 270,040, 0 failed; p50 218 ms, p99 1,033 ms | 243 resets. 186 transfer attempts: 81 transferred, 46 "not leader", 5 abandoned with the reply lost, and 54 planned abandons (17 of them answered "not leader"). The B0-06 and B0-03 runs overwrote the fault plan for parts of 09:34Z to 10:25Z, and one deploy reset every member at 09:33Z (`raft-chaos-cf-long-1.started`) | Porcupine on the whole history, rechecked with unique write names | **Ok** | `tests/m0/results/m0-wp2.md`, "B0-11: the 4-hour run found a linearizability violation"; `raw/raft-chaos-cf-long-1.json` |
| `cf-long-2` | B0-11 | Cloudflare, `graft-raft-*`, 2026-10-28T13:00Z to 17:00Z | 4 h | 475,301 | 138 resets and 77 transfers (prose only). No resets or transfers in three driver gaps, and one deploy reset every member at 14:44Z (`raft-chaos-cf-long-2.started`) | Porcupine; members' stored logs read back with `/dump`; deterministic repro (`run/raft-ticket.ts`, 3 of 3) | **Illegal.** A real bug: a `Ticket` of `(term, seq)` was reused after a leader reset, so `outcome` answered an old ticket with another command's result. Fixed in the design and the prototype by adding `boot` | `tests/m0/results/m0-wp2.md`, "B0-11: the 4-hour run found a linearizability violation"; `raw/raft-chaos-cf-long-2.json` |
| `cf-48h` | B0-11 | Cloudflare, `graft-raft-*`, 2026-10-28T18:50Z to 2026-10-30T18:50Z | 48 h | 5,742,736, 12 with no reply; p50 284 ms, p99 1,211 ms | 5,729 resets (A 2,323, B 2,292, C 1,114). 3,607 transfer attempts, 1,265 "not leader", so 2,342 from a leader. Of those, 1,431 transferred, 779 abandoned, and 132 transferred with the reply lost. No per-isolate counters. Coverage on site C is thin (F36) | Porcupine `-seg 20000` per key. Lost-write audit of all three logs. Single-clock early-ack test | Recorded clocks **Illegal** on all 6 keys, from 72 cross-colo reads 27 to 94 ms stale, all consistent with clock skew. Widened 100 ms **Ok** on all 6 keys. 0 of 1,814,873 acked writes lost; 984,514 identical entries on 3 members. Early-ack test 0 of 72 and 0 of 5,934 controls. **Pass** with the 100 ms allowance as an assumption, not a measured bound. Rechecked with the fixed segmented checker: unchanged | `tests/m0/results/m0-wp2.md`, "B0-11: the 48-hour run on the fixed `Ticket` code"; `tests/m1/results/porcupine-seg.md`; `raw/raft-chaos-cf-48h.events.jsonl`, `raw/cf-48h-analysis/lost-acked.json` |
| `wp4-cf-1` | B0-11 (fault and transfer rules on the M0-WP4 code) | Cloudflare, `graft-wp4-*`, 2026-10-30T00:09Z to 00:40Z, 8 clients from the orb | 31 min | 41,567 (4 failed at the orb's own `fetch`); p50 287 ms, p99 1,090 ms | 82 resets. 62 transfer attempts: 29 transferred, 2 with the reply lost, 13 planned abandons, 18 "not leader" | Porcupine | **Ok**. 16 operations over 9 s in two stall episodes, cause not established | `tests/m0/results/m0-wp4.md`, "Chaos run on graft-wp4"; `raw/raft-chaos-wp4-cf-1.json` |
| `wp4-cf-5` | B0-11 and B0-06 rules (`STEP_DOWN` count 6, quarantined target) | Cloudflare, `graft-wp4-*`, 2026-10-30T07:25Z to 09:30Z, 8 clients from the orb | 2 h 5 min | 184,349 (3 failed at the orb's own `fetch`); p50 246 ms, p99 1,003 ms | 343 resets. 253 transfer attempts: 119 transferred, 1 with the reply lost, 51 planned abandons, 82 "not leader". 14 slow-storage periods. Call faults on for 39 of 125 min | Porcupine; `STEP_DOWN` trips from 22,979 member log lines | **Ok**, 0 operations over 9 s. 23 confirm trips, all on a slowed site | `tests/m0/results/m0-wp4.md`, "Two-hour chaos run on the final code"; `raw/raft-chaos-wp4-cf-5.json` |
| `lin24` | BM-1, 24 h | Cloudflare, `graft-m1-lin-*`, `repo:915635`, 2026-11-01T18:27:14.621Z to 2026-11-02T18:27:14.621Z | 24 h | 2,739,691, 0 with no reply; p50 326 ms, p99 1,009 ms. 1,096,016 gets, 821,524 puts, 47,733 CAS ok, 774,418 CAS refused | 5,547 resets (3,676 of a leader), 3,743 transfer attempts (1,131 planned to be abandoned, 73 accepted by the leader, 3,663 refused by the target), 77,653 failed calls, 72,130 dropped AppendEntries replies, counters from 88 isolates | Porcupine `-seg 2000` per key, recorded clocks Illegal on k2, k4, k5. Widened 100 ms Ok on all 6 keys. Widened 275 ms (the measured bound, halved) Ok on all 6 keys. No member-side ack evidence. Lost-write audit of all three logs | **Not passed.** The 5 Illegal segments read writes whose history rows were lost in 5 of 12,518 client chunks, and are Ok with the rows restored. 0 of 869,257 acked writes lost, 0 of 774,418 refused CAS in the log, three identical logs of 479,965 entries. An early ack is not ruled out: one ordering needs 93 ms of ATL to LHR skew against a measured 49 ms, and 17 writes were acked faster than a quorum could commit them on their own client's clock (F44) | `tests/m1/results/m1-wp2.md`, "The 24-hour run's result, 2026-11-02: lin24 does not pass"; `tests/m1/lin/raw/lin24.report.json`, `lin24.events.jsonl`, `lin24.jsonl` |
| `lin24b` | BM-1, 24 h | Cloudflare, `graft-m1-lin-*`, `repo:934783`, 2026-11-03T00:55:07.302Z to 2026-11-04T00:55:07.302Z | 24 h | 2,850,523, 0 with no reply; p50 314 ms, p99 952 ms. 1,141,028 gets, 854,483 puts, 49,811 CAS ok, 805,201 CAS refused | 4,176 resets (2,795 of a leader), 2,752 transfer attempts (855 planned to be abandoned, 56 accepted by the leader, 2,690 refused by the target), 83,209 failed calls, 77,604 dropped AppendEntries replies, counters from 97 isolates | Porcupine `-seg 2000` per key, recorded clocks Illegal on k0, k1, k2, k3, k4. Widened 100 ms Ok on all 6 keys. Widened 153 ms (the measured bound, halved) Ok on all 6 keys. Ack check Ok on 904,294 of 904,294 acknowledged writes, 0 violations. Lost-write audit of all three logs. Single-clock early-ack test | **Pass.** Of the 20 recorded-clock Illegal segments, 18 read or hold writes whose history rows were lost in 15 of 12,535 client chunks, and are Ok with the rows restored. 2 are cross-colo ties, Ok with 1 ms (LHR–SIN) and 4 ms (ATL–LHR) of skew against that minute's measured 92 ms and 63 ms. None is same-colo. 0 of 904,294 acked writes lost, 0 of 805,201 refused CAS in the log, three identical logs of 513,232 entries, no request id twice. Early-ack test 0. F44 answered: a client-clock artifact, since 59 writes acked faster than their own quorum proof's round trip on the client's clock all pass the member check | `tests/m1/results/m1-wp2.md`, "The 24-hour rerun's result, 2026-11-04: lin24b passes"; `tests/m1/lin/raw/lin24b.report.json`, `lin24b.events.jsonl`, `lin24b.jsonl` |

## Other gate runs on Cloudflare, Milestone 1

| Gate | Runs | Load and faults | Check | Result | Source |
| --- | --- | --- | --- | --- | --- |
| BF-12 | `bf12-full`, 2026-11-01 02:09Z to 02:44Z, 10 groups | 10,001 leader kills sent, 10,000 landed: `after_quorum` 4,800, `after_quorum_unsynced` 4,800, each plus 200 with the object held down 12 s | Re-send under the same `RequestId`; audit of all 30 members | **Pass.** 10,000 of 10,000 kept their outcome, 0 double applies, 76 answered by another leader | `tests/m1/results/m1-wp2.md`, "BF-12: leader killed after quorum and before the reply"; `tests/m1/wp2/raw/bf12-full.bf12-audit.json` |
| BF-12 | `bf12-p3` regression, 2026-11-01 | 1,000 kills, 1,000 landed | Same audit | **Pass**, 0 changed, 0 double applies | `tests/m1/results/m1-wp2.md`, "BF-12 and BF-13 regression"; `tests/m1/wp2/raw/bf12-p3.bf12-audit.json` |
| BF-12 on `Push` | `p2`, 2026-11-01, 10 repositories | 1,000 kills through the git edge's `commit_hedged` | Re-send outcome and per-ref result; audit of 30 members | **Pass**, 1,000 of 1,000, 0 double applies | `tests/m1/results/m1-wp6.md`, "BF-12 on `Push` commands"; `tests/m1/integ/raw/p2.bf12-report.json` |
| BF-13 | `bf13-full` (400 rounds), `bf13-p3` (400), `bf13-p3e` with early AppendEntries (120) | Leader killed before commit on an entry with a `Stale` refusal caused by an earlier command in the same entry | Porcupine on 1,600, 1,600, and 480 operations; a planted refusal made the `bf13-full` and `bf13-p3` histories Illegal | **Pass.** 0 refusals returned from uncommitted entries, Porcupine Ok on all three | `tests/m1/results/m1-wp2.md`, "BF-13" and "BF-12 and BF-13 regression"; `tests/m1/wp2/raw/bf13-*.bf13-report.json` |
| BF-14 | `r1` 11:39Z and `r2` 13:03Z, 2026-11-02, hosted edge | 1,200 commands each (600 `Push`, 600 `PutAccount`), every first receipt missed: half held 2.6 s, half dropped (`test-faults`) | Edge trace against every member's `arrival` rows | **Pass.** 2,400 of 2,400 decided ok, 0 forwarded | `tests/m1/results/m1-wp8.md`, "BF-14: receipts that miss the deadline"; `tests/m1/wp8/raw/r1.bf14-report.json`, `r2.bf14-report.json` |
| BF-10 | `g1`, 2026-11-01 | 1,000 chain creations from two edges at once (13.9 min), then 100 rounds during an owner move, plus a 20-round control without the generation claim | `/census` of every member | **Pass.** 1,000 of 1,000 and 100 of 100 with one group. The control made two groups in 20 of 20, which is a design problem found | `tests/m1/results/m1-wp7.md`, "BF-10: one group per chain, and no read or vote without a config" |
| BF-11 | `g2`, 2026-11-01, 30 chains | 1,500 cold-cache requests across a member retry, a `ReplaceMember` stand-in, and an owner move | Every request found the group | **Pass**, 1,500 of 1,500 | `tests/m1/results/m1-wp7.md`, "BF-11" |
| BF-15 | `g1`, then rerun `i1` on the integration site script | 1,000 commands queued behind a token revocation, and 1,000 behind an account suspension, per run | Every command decided after the change | **Pass**, 1,000 of 1,000 refused 401 in each case, in both runs | `tests/m1/results/m1-wp7.md`, "BF-15" and "BF-15 rerun on this pass's site script"; `tests/m1/wp7/raw/i1.bf15.jsonl` |
| BF-16 | Stand-in group, then the real groups, 2026-11-01 | N+1 edge with an unenabled format; forced unknown and known-new formats | Member receive counters; refs and entries unchanged | **Pass** on both | `tests/m1/results/m1-wp6.md`, "BF-16 on Cloudflare" and "BF-16 on the real groups" |
| BF-17 | Stand-in group (git 2.56.0 and 2.39.5), then the real groups | Atomic push of 10,000 tags with a 1 MiB pack; 10,001 tags; 12,000 non-atomic creates over the byte limit | Entry size, refs, clone, fsck | **Pass**, 12 of 12 rows on the real groups | `tests/m1/results/m1-wp6.md`, "BF-17 on Cloudflare" and "BF-17 on the real groups" |
| BF-18 | Bed with a stubbed chain leader: networks 5000001 to 5000003 and the LRU rerun 5000020. Hosted with a real `ChainLog`: run 1 and run 2, 2026-11-02 | Archive a range, kill both shards, rehydrate, a second shard behind its cursor | `git fsck --strict`, 9 clones per run, a fetch | Bed: 4 runs, 0 failures. Hosted run 1 **failed** at rehydrate: a thin stored pack in the promoted range, now a design rule. Hosted run 2 **Pass**: fsck clean, 9 of 9 clones | `tests/m1/results/m1-wp5.md`, "BF-18 on Cloudflare Containers", "BF-18 rerun after the LRU", "Run 1" and "Run 2" |
| BF-19 | `r10` and `r11` on real buckets with a stubbed `request` table; `r1` on the hosted cron | 1,000 failed placements plus 1,000 commits with the marker write killed per run (`r10` and `r11` reached 2,150 of each with retried batches) | HEAD of every pack and marker in all three buckets after each sweep; injected sweeper clock | **Pass.** 0 committed packs deleted, every missing marker repaired, every failed placement deleted after 24 h and none before | `tests/m1/results/m1-wp4.md`, "BF-19 on real buckets" and "BF-19 on the hosted cron"; `tests/m1/wp8/raw/r1.bf19-report.json` |
| B0-01 on the Rust host | `b1` to `b6` (first pass) and `d1` to `d6` (second pass), 60 s each | 6,000 commands per run, 0 failed | Commit p99 on the leader, against M0 controls in the same hour | First pass p99 645 to 791 ms in all 6. Second pass p99 414 to 807 ms: 2 of 6 pass 600 ms, 4 between the lines, none over 1 s. On the current host (`m0-carried`, `graft-m3c`, 2026-11-11): 6 of 6 at p99 244 to 377 ms, **Pass** (`tests/m0/results/carried.md`) | `tests/m1/results/m1-wp2.md`, "Commit latency, B0-01 style" and "B0-01 on the new host" |
| F43 soak | `f43-soak`, 2026-11-02 19:28Z to 20:21Z, 4 repositories | 14 cycles: 4 total faults on a follower site, 4 gray faults (30%) on the leader's site, 3 follower resets, 3 without chaos | Push results; follower `last` against the leader 60 s after each fault | 374 of 374 pushes ok. Found a different defect, F45: restore ran out of memory on a log of large entries, since fixed | `tests/m1/results/m1-f43.md`, "The soak"; `tests/m1/results/raw/f43-20261102/soak/summary.txt` |

## Git conformance on Cloudflare (BM-1, git half)

BM-1's fault is B0-03's simulated region loss: every call to and from site 0's member class fails from 120 s into each `t/` run, or 60 s into each `clients.sh` run, to the end. The gray rerun fails each such call with chance 0.3 instead.

git's own smart-HTTP tests, 16 scripts from `t/t55xx` to `t/t58xx`, counted from each run's `results.tsv`:

| Run | git | Fault | Tests | Pass | Fail | n/a (hosted) | Skip | Baseline passes lost |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `conformance-20261101` | 2.51.0 | none | 535 | 413 | 15 | 99 | 8 | baseline |
| `conformance-20261101` | 2.56.0 | none | 532 | 412 | 15 | 97 | 8 | baseline |
| `t-251`, 2026-11-02 04:03Z to 04:49Z | 2.51.0 | site 0 failing | 535 | 415 | 14 | 98 | 8 | 0 |
| `t-256`, 05:51Z to 06:33Z | 2.56.0 | site 0 failing | 532 | 412 | 14 | 98 | 8 | 1 (t5549 #2, a test that sets a server-side `receive.hiderefs`) |
| `t-239`, 06:33Z to 07:15Z | 2.39.5 | site 0 failing | 491 | 377 | 16 | 90 | 8 | no 2.39.5 baseline. Against `t-251`, only t5541 #20 and t5549 #2 pass on 2.51.0 and not here, both from fixtures the host cannot reproduce |
| `t5541-nofault`, after the t5541 #20 fix | 2.56.0 | none | 42 | 30 | 3 | 6 | 3 | #20 now passes |

The baseline's 30 fails are 15 per version and the same in both. One was a real edge defect (t5541 #20, a member reply the edge could not decode), now fixed. One is a header deviation (t5551 #5). The other 26 follow from tests the host cannot stage.

`clients.sh` runs 29 rows per client: protocol v2 capabilities, atomic and non-atomic pushes, a raced `pre-push`, non-fast-forward, `http.postBuffer`, deletes, clones, partial clones, shallow clones, protocol v0, and refused tokens.

| Runs | Fault | Result |
| --- | --- | --- |
| Stand-in group: 2.39.5, 2.51.0, 2.56.0 | none | 29 of 29 each |
| Real groups: 2.39.5, 2.51.0, 2.56.0 | none | 29 of 29 each |
| `clients-239`, `clients-239-homed`, `clients-251`, `clients-251-homed` | site 0 failing | 29 of 29 each |
| `clients-239-gray`, `clients-251-gray`, 2026-11-02 13:10Z to 13:22Z | site 0's calls failing 30% | 29 of 29 each. Every leader move was an election, and no `STEP_DOWN` fired on any member |

Verdict: BM-1's git half **passes** (`build/BUILD-PLAN.md`, "Milestone 1 status"). Not covered: `t/` under the gray fault, and a no-fault `t/` baseline on 2.39.5.

Sources: `tests/m1/results/m1-wp6.md`, "Git clients", "Run", and "Git clients on the real groups"; `tests/m1/results/m1-wp8.md`, "BM-1 results", "BM-1 gray-fault rerun", and fix 1; `tests/m1/results/raw/conformance-20261101/results.tsv` and `tests/m1/results/raw/bm1-20261102/`.

## SSH git sessions

Each session is one `git clone`, `fetch`, or `push` over SSH through a Cloudflare Tunnel to cloudflared in three containers. Two workers ran one session every 6 s each.

| Run | Gate | Where and when | Sessions | Succeeded | Faults | Pushes | Result | Source |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `b0-24` | B0-24 | Cloudflare, Pro zone, 2026-10-30T23:58:59Z to 2026-10-31T02:10:49Z (2.2 h) | 2,637 | 2,634 (99.886%) | 13 container kills | 878 acked, all in the group log | **Between the lines** (pass 99.9%, fail 99%) | `tests/m0/results/m0-wp7.md`, "Results (run `b0-24` ...)"; `tests/m0/results/raw/wp7/b0-24/summary.json` |
| `b0-24a-r4` | B0-24a | Cloudflare, Pro zone, 2026-10-31T07:36:12Z to 2026-11-01T07:36:07Z (24.0 h) | 28,347 | 28,230 (99.587%) | 143 container kills, one every 10 min | 9,418 acked, all 9,418 in the group log | **Between the lines.** 85 of 117 failures within 2 min of a kill, the rest Tunnel-edge drops. Carried to M6-WP2 | `tests/m0/results/m0-wp7.md`, "The run of record: `b0-24a-r4`"; `tests/m0/results/raw/wp7/b0-24a-r4/summary.json` |
| `r1` | none (the plain-SSH relay the operator allowed until go-live) | Oracle relay to the same Tunnel, 2026-11-03T00:54:33Z to 01:19:25Z (24.9 min) | 249 relay + 249 `cloudflared` control | 249 and 249 | One forwarder restart: sessions open at the stop were cut, new ones worked within about 0.4 s | 166 acked, all in the group log | 0 failures, which cannot tell this path from M0's 99.86% floor at this sample size | `tests/m1/results/oci-ssh.md`, "Measurements (run `r1` ...)"; `tests/m1/results/raw/oci-ssh-20261103/r1/summary.json` |

## Other Milestone 0 fault and load rows on Cloudflare

These rows ran on the M0 prototype. Verdicts are the ones `build/BUILD-PLAN.md`, "Milestone 0 status", carries, or the results file's where that table has no row.

| Row | Runs | Load and faults | Result | Source |
| --- | --- | --- | --- | --- |
| B0-01 | 23 one-minute runs, one entry in flight | 81 to 100 commands/s, 1 KB | p99 399 to 690 ms, median 491 ms. **Pass** in 19 of 23 at the restated 600 ms line, none over 1 s | `tests/m0/results/m0-wp2.md`, first table |
| B0-02 | 3 reruns at 300 commands/s offered | Group commit, one entry in flight | 159 to 280 commands/s carried at p99 750 to 1,288 ms. **Between the lines**, carried forward. Rerun on the Rust host (2026-11-11): 6 of 8 runs at 308/s carried at p99 286 to 509 ms, 2 between. Second rerun with the `STEP_DOWN` fix (2026-11-12): 12 of 16 at 276 to 579 ms, 4 between (678 to 790 ms), every miss on a group with a near follower over 120 ms. **Between** on every-run reading, F68. Third rerun on groups placed within `NEAR_RTT` (2026-11-12): 15 of 16 at 249 to 540 ms (8 of 8 at 300 offered, 7 of 8 at 330), 0 failed, and 1 between at 614 ms from one leader stall. **Between**. Fourth rerun with streamed member-call bodies (`m0-carried-4`, 2026-11-12): 11 of 16 at 342 to 588 ms, 5 between (614 to 738 ms), then 3 of 4; every miss follows a slow leader storage confirm (p99 132 to 399 ms) or one follower stall. **Between**, carried forward | same; `tests/m0/results/carried.md`; `tests/m0/results/carried-2.md`; `tests/m0/results/carried-3.md` |
| B0-03 | 26 runs (13 failing-calls, 13 blackholed) plus 6 reruns on the final code | One site's member class failing or blackholed, about 100 ops/s | RTO 11.0 to 12.9 s, 0 failed operations. **Pass** at 13 s | same, and "B0-03" |
| B0-06 | Three modes, 2 min each, plus reruns | 30% of one site's calls failing; 30% of its storage writes failing; only the edge's calls to it failing | Every group moved within 20 s. **Pass**, 2 minutes per mode, not an hour | same, and "B0-06 and `STEP_DOWN`" |
| B0-18 | 3 runs | 16 learners at 1,000 reads/s each | 16,000 reads/s, 0 failed, learner p99 683 to 743 ms. **Pass** at the restated 1 s | same |
| B0-25 | 9 runs with the `STEP_DOWN` hold, 300 pushes each | 22% of member calls and storage writes failing | 92.3% to 98.7% within three attempts, median 95.0%. **Pass** at the restated 92%. 17 earlier runs on older code gave 79.3% to 98.3% | same, and "B0-25"; `tests/m0/results/raw/b0-25-*.json` |
| B0-26 | 4 runs | Every member of 4 or 8 groups reset inside 60 s under 100 writes/s | Every group committed within 1.2 s of its last reset. **Pass** | same |
| B0-04, B0-05, B0-08, B0-09, B0-10, B0-27 | Site days on 300 or 1,000 groups | 100 writes/s; follower regressions, verify refusals, drain guard | **Pass.** 0 failed writes in every site day | `tests/m0/results/m0-wp4.md`, first table |
| B0-07 | Runs 11 to 16 on the drain-alarm code | Gray site C during B's site day | 179 of 182 groups moved within 20 s as written, 181 of 182 under the 50 s form. Carried. Rerun on the Rust host (2026-11-12) with test-faults stand-ins for `graft-ops`'s drain and `SiteHealth`: 235 of 235 within 20 s over 3 days, max 17.2 s, 0 failed writes. **Pass** on the member path; M5-WP4 reruns it with the real drain | `tests/m0/results/m0-wp4.md`, "B0-07 on the final code"; `tests/m0/results/carried.md`, B0-07 |
| B0-22 | 8 rehydrations | 5.38 GB network from R2 into a new container | At most 118.1 s, median 67.9 s. **Pass**, thin margin | `tests/m0/results/m0-wp6.md`, "B0-22" |
| B0-23 | 8 runs | 33 CI clones/s, tip moving 10 times a minute | **Pass** with a push hook (100% from R2, p99 406 ms) or clones 5 to 60 s after a push (97.5 to 97.8%). Up to 44% refused at the instant of a push without the hook | `tests/m0/results/m0-wp6.md`, "B0-23" |
| B0-29 to B0-35 | Eight passes of the agent-fleet replay | 500 agents, journey 9 | B0-29 passes in 5 of 8 runs. B0-31, B0-32, and B0-33 pass on the final rules. B0-30 passes at the margin (accelerated), and B0-35 passes. B0-34 is a partial pass at reduced scale | `tests/m0/results/m0-wp9.md`, "Verdicts", "Second-pass summary", and "Eighth-pass verdicts"; `build/BUILD-PLAN.md`, "Milestone 0 status" |
| L0-12, LF-12 | L0-12 in the first and third to seventh passes, and LF-12 runs `r2` to `r37` | A1 replay with leases, held feed polls, and leader kills | L0-12's feed clause passes for all five actor kinds in the sixth pass. LF-12 is **between the lines**: 1 pass and 3 between in 4 runs on the final rules | `tests/m0/results/m0-wp12.md`, "Sixth-pass summary" and "Tenth-pass summary" |

## Simulator

`crates/raft/tests/sim.rs` runs three members under one seeded generator per run. HARSH drops 8% of messages, duplicates 4%, delays each 5 to 1,500 ms, crashes members, cuts sites off, and skews clocks. GRAY adds one gray site at a time: failing calls, 1 s storage confirms, or failing writes. After every message and step the simulator checks one leader per term, identical committed entries, two disks per committed index, every `Acked` write in every later leader's log, no `RequestId` at two indexes, BF-14's no-forward rule, and the ack evidence of every acked write (`ack_check`).

The long sweep, run on 2026-11-03 at `c0c6fb9` with `RAFT_SEEDS=2000 RAFT_STEPS=10000` in release mode. It ran twice with identical counts, in 344 s and 347 s:

| Half | Steps | Operations | Acked writes | Faults | Porcupine on the exported history | Result |
| --- | --- | --- | --- | --- | --- | --- |
| HARSH | 20,109,261 | 160,066 (49,629 acked, 18,649 refused, 60,710 reads, 31,078 no reply) | 49,629, all with valid evidence | 226,083 crashes (97,866 mid-input), 31,230 partitions, 696,535 of 2,215,368 messages dropped, 3,524 leaders | Ok on 156,723 operations | **Pass**, 0 forwarded |
| GRAY | 20,082,543 | 2,226,291 (951,227 acked, 384,638 refused, 890,206 reads, 220 no reply) | 951,227, all with valid evidence | 17,108 gray faults, 24,327 write failures, 15,329 partitions, 1,534 `STEP_DOWN` transfers, 12,727 leaders | Ok on 2,226,280 operations | **Pass**, 0 forwarded |

Earlier sweeps at the same seeds and steps, on older code, are not added to the totals: M1-WP2's first pass (20,148,290 steps, 230,479 operations), its second pass (HARSH 186,714 and GRAY 2,294,903 operations), and M1-WP3's (HARSH 206,541 and GRAY 2,255,722 operations). Sources: `tests/m1/results/m1-wp2.md`, "Simulator", "The simulator", and "Checks after these changes"; `tests/m1/results/m1-wp3.md`, "Simulator".

## Unit and property tests

`cargo test --workspace` at `c0c6fb9`, 2026-11-03: 259 passed, 0 failed, 2 ignored, in 25 test binaries.

| Crate | Tests |
| --- | --- |
| `graftyard` (`cli`) | 20 |
| `graftyard-fold` | 9 |
| `graftyard-git` | 87 |
| `graftyard-identity` | 39 |
| `graftyard-lookup` | 1 |
| `graftyard-raft` | 76, plus `long_sweep` and `long_liveness` ignored by default |
| `graftyard-release` | 13 |
| `graftyard-site` | 3 |
| `graftyard-types` | 11 |
| `graftyard-edge`, `graft-m1-wp4`, `graft-m1-wp5`, `graft-m1-wp6-bed` | 0 |
| **Total** | **259** |

Go: `go test` in `tests/m0/porcupine` runs 11 tests, including `TestSegmentedAgreesWithWholeHistory` over 3,000 random histories. `containers/git/server` runs 3.

## Planted bugs

A planted bug is a code mutant applied alone, run against a test suite, and then reverted.

| Where | Planted | Caught | Source |
| --- | --- | --- | --- |
| `crates/raft`, M1-WP2 first pass | 8 | 8 | `tests/m1/results/m1-wp2.md`, "The tests catch real bugs" |
| `crates/raft`, M1-WP2 second pass | 4 | 3. Bug D, a resumed `led_term` sending its `Noop`s before they are durable, was **not caught** | `tests/m1/results/m1-wp2.md`, "The simulator" |
| `crates/raft`, `Member::record` | 1 | 1 | `tests/m1/results/m1-wp2.md`, "Gate-row pass, 2026-11-02" |
| `crates/raft`, the ack mutants A and B | 2 | 2, by the ack check under GRAY (54 of 3,873 acked writes flagged) | `tests/m1/results/m1-wp2.md`, "The planted mutant" |
| `crates/raft`, M1-WP3 | 17 | 17 | `tests/m1/results/m1-wp3.md`, "The tests catch real bugs" |
| `crates/git`, M1-WP4 pack store and sweeper, plus M1-WP5's alarm | 25 | 25, after one plant was rewritten to compile and one miss led to a new assertion | `tests/m1/results/m1-wp4.md`, "Adversarial unit tests and planted bugs" |
| M1-WP5: the shard server's LRU (2) and `sweep_all` (2) | 4 | 4 | `tests/m1/results/m1-wp5.md`, "The LRU of bare repositories" and "The sweeper's cron entry" |
| `crates/git`, M1-WP6 | 6 | 6 | `tests/m1/results/m1-wp6.md`, "Planted bugs" |
| `crates/identity`, M1-WP7 | 18 | 18 | `tests/m1/results/m1-wp7.md`, "Planted bugs" |
| **Total** | **85** | **84** | |

Planted bad histories are counted apart, because they test a checker, not the code. The segmented Porcupine checker caught 8,754 of 8,754 planted violations (`tests/m1/results/porcupine-seg.md`, "Experiments on known answers"). The crate's Wing-Gong checker rejects five planted bad histories, and a planted refusal or a read of a value nobody wrote made the BF-13 and sweep histories Illegal.

## Smoke, local, and discarded runs

None of these runs counts toward a total above.

| Kind | Runs | Counts |
| --- | --- | --- |
| Smoke runs on Cloudflare, Porcupine Ok | `cf1` (150 s), `cf-remote-smoke` (180 s), `cf-remote-2` (300 s), `cf-boot-smoke` (10 min), `lin-smoke1` (9 min), `lin24b-smoke` (30 min) | 112,985 operations: 2,729, 6,658, 11,188, 19,030, 16,243, and 57,137. `lin24b-smoke`'s ack check was Ok on 18,225 of 18,225 acked writes |
| Local chaos runs on `wrangler` | 32 `raft-chaos-local*` runs | 744,258 operations, all Ok. Local workerd, not Cloudflare |
| Discarded as set up wrong | `wp4-cf-2`, `wp4-cf-3`, `wp4-cf-4`; `conf-run2` (stale shard containers) | Not checked or not kept |
| Stopped 24-hour SSH attempts, each on a defect since fixed | `b0-24a-24h`, `b0-24a-r2`, `b0-24a-r3` | 1,144 sessions: 341 (338 ok), 536 (533 ok), and 267 (263 ok) |
| Stopped `t/` run | `t-256-first` | Found fix 4 in `m1-wp8.md`, then rerun as `t-256` |

## Prose and raw data that disagree

- **`cf-48h`'s abandoned transfers.** `m0-wp2.md` says 824 of the 2,342 transfers from a leader were "abandoned or with a lost reply". The event log has 824 results whose text says "abandoned" (690, 78, and 11 "abandoned", plus 45 "transferred, reply lost (abandoned)"). Another 87 transfers have a lost reply without that word, so "abandoned or with a lost reply" counts 911.
- **B0-25's verdict labels.** 7 of the 9 hold runs' JSON say "Between pass and fail", because `tests/m0/run/raft-partial.ts` still compares against the old 96% line. Against the restated 92% line all 9 pass, as the prose says.
- **B0-25's earlier runs.** `m0-wp2.md`, "B0-25 after `STEP_DOWN`", says twelve runs gave 79.3% to 98.3% with 4 under 90%. Its own first table says 14 runs. The raw files `b0-25-percall-3` to `-10` and `b0-25-dist-1` to `-6` are 14 runs with 5 under 90%.
- **`cf-long-2`'s fault counts.** The prose's 138 resets and 77 transfers cannot be checked, because a restarted driver overwrote the run's event log.
- **Workspace test count.** `m1-wp2.md`, "Gate-row pass, 2026-11-02", reports 235 tests in 27 binaries. The count at `c0c6fb9` is 259 in 25.
