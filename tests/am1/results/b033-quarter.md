# b033-quarter: one group at a quarter of B0-33's agents (1,250), three runs on main `e31c8c1b` (graft-m2b)

LM-1, 2026-11-22. Step 2 of FRAME, "Operator decision, 2026-11-22, B0-33 measured before a design flip, and a $50
research budget": one group at about 63 agent-class commands/s offered (1,250 agents), on three fresh placements,
otherwise as in the baseline set (`tests/am1/results/b033-baseline.md`). This is a measurement, not a judged B0-33 set.
Tags follow `design/SHARED.md`, "Read this first".

**Short answer.** A single group holds a quarter load easily on both MIA and EWR leaders [measured]:
- cycle p50 230 to 262 ms on the near voter's clock;
- 0 `suspect` reports in the load window and 0 leader changes;
- 0 merges over 2 s;
- Human and merge p99 1.0 to 1.3 s.

The leader's CPU per command cut stays at 4.4 to 4.9 ms, the same as the baseline's 4.5 to 5.5 ms, so it does not fall
with load [measured]. The load actually offered was about 42 agent-class commands/s, not 63 (see "Offered load").

## Setup

- One site day from `e31c8c1b`, siteday exit 0. Edge `520bb6e4` serves 100% (rev `e31c8c1b9`).
- Before each run, `reretire.sh` printed RERETIRE OK graft-m2b (`tests/fanout/raw/reretire-graft-m2b-ql{A,B,C}.log`).
- Each run was B0-33 with `--agents 1250` on its own fresh repository, with four learners (`enam:2000,weur:1000,apac:1000`),
  the flag off, and a $10 run stop.
- Harness: qlA ran a1 as on `e31c8c1b`. qlB and qlC ran with two a1 changes made after qlA, which the run starts record
  as `4dfefa0b1`:
  - `side:label`'s label is now in the agent label allowlist. On qlA every label add answered 403 and every remove 404.
  - Draft PRs open only on branches first pushed at least 10 s earlier. This did not help (see "Draft PRs").
- Harness outages were 1 s on each run (0.4%), so all three are valid. The watchdog logged 0 edge 500s, exceptions or
  traps.

| Run | Repo, a1 run | Load (UTC) | Done line | Run cost |
|---|---|---|---|---|
| qlA | 131389, `mws1o3x0` | 16:24:36 to 16:29:36 | 16:32:07Z | $1.16 |
| qlB | 131390, `mws3a00p` | 17:09:28 to 17:14:28 | 17:17:09Z | $1.16 |
| qlC | 131391, `mws4t6zp` | 17:52:14 to 17:57:14 | 18:00:00Z | $1.34 |

At 18:01Z graft-m2b stood at $11.61 for 2026-11-22 from 00:00Z, $3.67 above the $7.94 the research budget starts from.

## Per run

[measured]

| Line | qlA | qlB | qlC |
|---|---|---|---|
| Leader colo (`#1#f0`, DO analytics) | MIA | MIA | EWR |
| Near voter, far voter | CDG, SIN | LHR, NRT | MAD, SIN |
| Cycle, near voter's clock, p50 (mean) ms (`second.py`) | 262 (270) | 261 (273) | 230 (241) |
| Leader excess p50 / p90 ms | 111 / 178 | 108 / 177 | 92 / 164 |
| Entries/s; commands an entry | 3.70; 12 | 3.66; 12 | 4.16; 10 |
| Agent commands/s cut (`cuts.py`) | 35.7 | 37.0 | 35.1 |
| Agent-class commands answered ok/s (a1) | 37.2 | 41.0 | 37.6 |
| Agent queue left after a cut, p50 / max | 0 / 0 | 0 / 0 | 0 / 0 |
| Human commit stage p99, push log (n) | 1,040 (182) | 1,045 (159) | 1,232 (164) |
| Merge commit stage p99, push log (n) | 1,262 (48) | 1,020 (57) | 1,100 (56) |
| B0-29's sampler: bycut two rounds p99 + entries.py round p50 | 755 + 233 = 988 | 751 + 234 = 985 | 804 + 205 = 1,009 |
| Merges over 2 s at the commit stage | 0 of 48 | 0 of 57 | 0 of 56 |
| Voters' `/suspect` Reported rows in the load window | 0 | 0 | 0 |
| Leader changes (terms) | 0 | 0 | 0 |
| Ticker stalls ≥ 500 ms | 0 | 0 | 0 |
| Edge 503s | 95 | 3 | 14 |
| `ERR_HTTP2_STREAM_ERROR` | 0 | 0 | 0 |
| Leader clock behind the near voter, p50 ms (`stale.py`) | 347 | 341 | 328 |
| TORNALARM | OK | OK | OK |

- Each run's trace has 3 Reported rows, all from setup's first election at term 1 (16:20:11, 17:04:57, 17:49:04Z),
  minutes before load. None is in a load window [measured].
- Against the baseline's EWR run blB (98.0 agent commands/s cut, cycle p50 491 ms), a quarter of the agents gives about
  half the cycle and 0 suspect on both a MIA and an EWR leader.

## The leader's CPU per command

`cycsplit.py` on the leader's and near voter's Workers Logs (`tests/fanout/raw/wl-ql/`), 90 to 300 s [measured]:

| Run (colo) | Entries/s | Commands an entry | Cycle mean | Wire (idle RT) | Voter handling | Leader on CPU | Leader off CPU | CPU per command cut | Leader CPU, share of wall |
|---|---|---|---|---|---|---|---|---|---|
| qlA (MIA) | 3.70 | 12 | 270 | 123 (46%) | 22 (8%) | 57 (21%) | 67 (25%) | 4.86 ms | 21% |
| qlB (MIA) | 3.66 | 12 | 273 | 122 (45%) | 25 (9%) | 54 (20%) | 72 (26%) | 4.36 ms | 20% |
| qlC (EWR) | 4.16 | 10 | 241 | 113 (47%) | 20 (8%) | 50 (21%) | 58 (24%) | 4.94 ms | 21% |
| blB (EWR, baseline) | 1.95 | 61 | 514 | 94 (18%) | 19 (4%) | 273 (53%) | 128 (25%) | 4.50 ms | 53% |

- **CPU per command holds at a quarter load.** A least-squares fit of leader CPU-ms/s over the five runs (blA, blB, blC2,
  qlA, qlB) gives 4.81 ms per command cut and −2.4 ms per entry. So the cost scales with commands, not with entries.
- **Where it is spent shifts.** The leader's CPU per command cut, by route [measured, Workers Logs]:

| Run | Commands/s | CPU ms/s | `/commit` | `/raft` | `/entries` | `/outcome` | `/outcome_by_request` | `/read` | other |
|---|---|---|---|---|---|---|---|---|---|
| blB | 118.3 | 533 | 2.36 | 0.84 | 0.71 | 0.30 | 0.08 | 0.13 | 0.09 |
| blA | 84.0 | 390 | 2.45 | 0.80 | 0.15 | 0.23 | 0.62 | 0.26 | 0.14 |
| blC2 | 82.7 | 451 | 2.52 | 1.44 | 0.34 | 0.23 | 0.50 | 0.25 | 0.18 |
| qlA | 43.7 | 212 | 1.45 | 1.73 | 0.91 | 0.00 | 0.00 | 0.49 | 0.27 |
| qlB | 45.1 | 196 | 1.40 | 1.67 | 0.66 | 0.00 | 0.00 | 0.37 | 0.25 |
| qlC | 41.8 | 206 | 1.49 | 1.84 | 0.79 | 0.01 | 0.00 | 0.51 | 0.30 |

  At a quarter load, `/commit`'s share per command falls from 2.4 to 2.5 ms to 1.4 to 1.5 ms. Learner `/raft` (about
  47 of the 50 `/raft` a second come from learners) rises from 0.8 to 1.4 ms to 1.7 to 1.8 ms, and `/entries` to 0.7 to
  0.9 ms. The likely reason is that entries carry 10 to 12 commands instead of 58 to 62, so each entry's per-entry and
  per-member costs are spread over fewer commands [inference]. `/outcome` and `/outcome_by_request` disappear because
  every send is answered inside COMMIT_HOLD.
- At 20 to 21% of wall, the leader is mostly idle. 46% of the cycle is the idle wire round trip.

## Offered load: 42 agent-class commands/s, not 63

- a1's plan at 1,250 agents offers 42.19 agent-class commands/s from the client: 10.42 pushes/s and 31.77 side
  commands/s. Its 63/s total adds 20.85/s host-made commands (each head push's PrSync and test-merge records) [measured,
  `a1.mjs --dry`].
- B0-33 runs without `--open-prs`, so agents push their own branches, not PR heads, and the host makes almost none of
  those commands. The baseline's 252 has the same gap. That keeps the quarter set comparable to the baseline, so
  `--open-prs` stayed off.
- The agent queue was empty after every cut on all three runs, so the leader admitted everything offered. The gap
  between offered and answered is client-side refusals, not admission:
  - qlA: labels 650 × 403 (not in the allowlist) and 656 × 404 (removing a label never added), fixed for qlB and qlC;
  - draft PRs, below;
  - 503 read sheds, below.

## Draft PRs: "Head sha can't be blank"

70 of 298 (qlA), 79 of 313 (qlB) and 62 of 298 (qlC) draft PR opens answered "Head sha can't be blank, Base sha can't be
blank". `IssueErr::HeadMissing`, `BaseMissing` and `HeadMoved` all render this message
(`crates/api/src/graphql.rs`, `refused_field`).
- The 10 s branch-age wait added for qlB changed nothing, so a ref read behind a fresh head push is not the cause.
- Failures follow merges. When a merge was acked in the 5 s before the open's answer, 41%, 37% and 36% failed (67 of
  164, 72 of 197, 56 of 154). Otherwise 2%, 6% and 4% failed (3 of 122, 7 of 109, 6 of 134) [measured, push log against
  a1 samples].
- So this is most likely `BaseMissing`: the edge read `main` before a merge moved it, and `decide` refused the stale
  `base_sha` [inference]. It is a harness issue (a1 does not retry the open), not fixed here.

## Why 503s at a quarter load

The edge answered 95 / 3 / 14 × 503, every one "every member asked shed the read: overloaded", on GraphQL mutations,
REST label writes and one task write. Those plans read the whole tier (F107). The leader shed none of these reads.
- **Which member shed** (memguard's `shedBy`, load window) [measured]:

| Run | Leader (v0) | Near voter (v1) | Far voter (v2) | apac learner (194) | Other learners |
|---|---|---|---|---|---|
| qlA | `leaderRefused` 73 | `rebuild` 165 | `late` 717 | `late` 696 | 193: `late` 24 |
| qlB | `leaderRefused` 33 | none | `rebuild` 8 | `late` 1,352 | 193: `late` 51; 195: `late` 43 |
| qlC | `leaderRefused` 72 | none | `late` 39, `rebuild` 9 | `late` 336 | 193: `late` 6 |

- `leaderRefused` is the leader turning away whole-tier and `ls-refs` reads, which it never serves by design
  (`graftyard_lookup::leader_never_serves`, `decisions/DECISIONS.md`, "`tier-off-leader`"). `late` is a member that had
  not applied the leader's read index within FOLLOWER_WAIT (1 s). `rebuild` is a voter's whole-tier read arriving before
  its next dump rebuild (DUMP_EVERY_MS 1 s).
- **When.** On qlA, the 503s ran from 16:24:50 to 16:27:20 and again at 16:29:10. Over the same span, the near voter
  served 1,278 whole-tier, core and `ls-refs` reads (75 to 110 per 10 s) until 16:27:16, then none until 16:29:06. On qlB
  it served none of these, and qlB had 3 × 503. Per 10 s on qlA, the 503s correlate 0.52 with the far voter's and the
  apac learner's `late` sheds, and 0.37 with the near voter's `rebuild` sheds [measured].
- **Cause.** A spread of three members (SPREAD_TRIES) sometimes picks the far voter or the apac learner, whose apply
  trails by more than 1 s. When the third pick is a voter that sheds a dump read for its rebuild spacing, all three shed
  and the edge answers 503 [inference from the above]. It is read routing plus far-member apply lag, not leader overload.
  The far voter's read-index call took p50 256 ms and its applied index moved p90 812 ms apart on qlA [measured].

## Raws

- Traces: `tests/fanout/raw/tsave-ql{A,B,C}.jsonl.xz`. memguard: `memguard-ql{A,B,C}.jsonl.xz`.
- Statuses: `rows-`, `costrow-`, `terms-`, `arrivals-` and `shardgate-` files for each tag; `siteday-ql.status`.
- Leader (f0) and near voter (f1) Workers Logs, from `wlobj.py`: `tests/fanout/raw/wl-ql/ql{A,B,C}-f{0,1}.json.xz`.
- Run logs: `tests/am1/raw/runs/a1-b0-33-fql{A,B,C}-b033.log` and `fanout-ql-{edge,image,siteday}.log`. Rows:
  `tests/am1/raw/a1-b0-33-{mws1o3x0,mws3a00p,mws4t6zp}.jsonl`.
- Probes: the tags are added to `tests/fanout/b033probes/host/common.py`. `second.py` takes tags to run alone.
