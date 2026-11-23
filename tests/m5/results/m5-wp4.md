# M5-WP4 hosted: BF-39 on graft-m4a with `graft-ops` killed in every run

`build/BUILD-PLAN.md`, M5-WP4, row BF-39: kill `graft-ops` at each step of a `SiteDay` and an `EdgeRoll`, 100 times. Pass: it resumes after the last `IntentStep`, no step is skipped, and there is one `IntentDone`. Fail: a skipped or doubled step.

**Result: pass (2026-11-23, done line 03:05:30Z, the last `IntentDone`).** `graft-ops` ran three intents with `OPS_KILL=2`. Each cron run drew 0, 1 or 2 calls to live and then died, so 2 of 3 runs died before or between a deploy and its `IntentStep`. 119 kills fell inside the three intents and 134 in the row, against 100 asked. Every `IntentStep` of each intent committed once and in order, from 0 up, and each intent has one `IntentDone`. No entry was refused (no `Refused` line in the row's logs): a restarted run never resent a step the view already held. Times below are the group's commit times, which run about 22 s ahead of the orb's clock.

| Intent | Gate | Kills in it (view / commit / deploy) | Steps committed | `IntentDone` |
| --- | --- | --- | --- | --- |
| `SiteDay{c, R3}`, index 105, undo `Rollback{c, R2}` | `GATE=pass` | 60 (37 / 22 / 1), 63 runs | 0 to 6, once each, plus drain, `SiteRelease`, undrain | 01:36:32Z |
| `EdgeRoll{e4}`, index 117, undo `EdgePointer{e3}` | `GATE=pass` | 43 (28 / 13 / 2), 43 runs | 0 to 3, once each | 02:20:32Z |
| `SiteDay{c, R4}`, index 125, undo `Rollback{c, R2}` | real gate (`GATE` unset) | 16 (12 / 1 / 3), 19 runs | 0, then the `Rollback`'s return steps 0 to 2, once each, plus `SiteRelease` to R2 and undrain | 03:05:30Z |

Ending state (corrected 2026-11-23: c was drained again after the done line, see "After the done line: a false drain of c and its hand undrain"):
- Site c runs R2's `39439670` at 100%, and the group's `site_release` for c is R2 `a3a1f620…`. `deploy.mjs` site days leave the group's `site_release` on R2, the last release `graft-ops` rolled; that is a test-install artifact, not a product bug.
- c was drained by the drain key at entry 134 (03:09:34Z) with no fault, and undrained by hand with the drain key at entry 135 (about 03:54:47Z). Since then the group reads `drained []`.
- The edge runs e4 `f4e193b9` at 100%, with e3 `dd713c1c` at 0%. It serves today's traffic: `/api/v3/meta` answers 200 `installed_version 3.22.0`.
- Sites a and b run day 69's `c3be75f2` and `627c8882`.
- `graft-m4a-ops` is `32f9d175`, with `GATE=pending` and no Cron Trigger. The scoped token is revoked, and no shard container is running.

**Spend:** the row cost $6.26 on graft-m4a from 23:44Z to 03:07Z, against the $10 stop (`tests/fanout/acctcost.py`). $4.22 of it is Durable Object rows written, mostly from the bench load. The account's whole day to 03:07Z was $9.42.

## Setup

All of it is recorded in `tests/am1/results/install-m4a.md`.

- **Token.** `graft-m4a-ops-deploy-2`, Workers Scripts Write and Workers Containers Write on this account, minted 23:44:19Z and PATCHed into `graft-m4a-ops-deploy-token`. Revoked at 03:06Z.
- **Site day 69 (`workers/site/deploy.mjs`, test-faults, main `cf7571a7`, `--image-prev m4a60`), 23:56:33Z to 23:57:34Z.** Each site's version carries the `VERSION` binding, which the gate counters need: a `c3be75f2`, b `627c8882`, c `69b87309`. The crons it puts back were removed at 00:13:58Z.
- **Bundles** in `graft-m4a-releases`, signed by m4a's release key:
  - R3 `00ed8e77…`: the same site build, with 27 formats including 65000.1.
  - R4 `308769e7…`: R3 plus `site/marker.js`.
  - e4 `63644eb6…`: the `8561eaea` test-faults edge wasm that BF-43 used, plus a marker.

  `graft-ops` uploaded each one and committed `ReleaseUploaded` for it.
- **`graft-m4a-ops`, test-faults, from `cf7571a7`.**
  - Legs 1 and 2: `e51410f1`, with `GATE=pass`, `OPS_KILL=2`, `OPS_STEP_MIN_ENTRIES=50`, both dwells at 60 s, and a 1-minute cron. `OPS_SCRIPTS` names the three sites and `edge`.
  - Leg 3: `8047b005`, the same but with `GATE` unset (02:21Z).
- **Load and fault.** A scratch Worker, `graft-m4a-b39-edge`, ran B0-07's bench and gray fault and read the registry objects. It is not a site binding, and it was deleted at 03:07Z. It made 60 chains homed on c (`repo:7390000`–`059`), 38 homed on a and b (`7390100`–`7390219`), and 88 more homed on c for leg 3 (`7390400`–`7390499`), and drove about 15 to 30 puts/s.
- **A test-install artifact, not a product bug.** `deploy.mjs` site days do not commit to the group, so before the row the group recorded a, b and c on R2 while they ran day 69's build. That is why leg 1's N, and the `Rollback`'s target, is R2's `39439670`, which has no `VERSION` binding and counts no gate inputs.

## Leg 1: `SiteDay{c, R3}` with `GATE=pass`, under kills

| Index | Group time | Entry | Step | Signer |
| --- | --- | --- | --- | --- |
| 105 | 00:33:52Z | `Intent` SiteDay c R3 | | customer |
| 106 | 00:35:30Z | `SiteMode` Drain c | | drain |
| 107 | 00:44:29Z | `IntentStep` | 0 (drain) | executor |
| 108 | 00:47:32Z | `IntentStep` | 1 (1%, deployed 00:46:31Z) | executor |
| 109 | 00:50:31Z | `IntentStep` | 2 (10%, deployed 00:49:30Z) | executor |
| 110 | 00:55:31Z | `IntentStep` | 3 (50%, deployed 00:52:32Z) | executor |
| 111 | 01:04:30Z | `SiteRelease` c to R3 | (100% deployed 00:56:31Z) | executor |
| 112 | 01:06:30Z | `IntentStep` | 4 (100%) | executor |
| 113 | 01:23:30Z | `SiteMode` Normal c | | drain |
| 114 | 01:32:32Z | `IntentStep` | 5 (undrain) | executor |
| 115 | 01:35:30Z | `IntentStep` | 6 (first day) | executor |
| 116 | 01:36:32Z | `IntentDone` | | executor |

Several kills fell between an action and its `IntentStep`: between the drain and step 0, the 100% deploy and `SiteRelease`, and the undrain and step 5. Each time the next run found the action done in the view or the deployment and committed only the `IntentStep`.

**What `judge_site` would have decided** (`raw/bf39/gate.txt`), from the real `gate` object, once a minute, with N+1 = R3's `8e86f2c4`:
- At 1%, 10% and 50%: no reading. N+1's `gate` object answered 404, because R2's `39439670` registry, which held most of c's objects, has no `/gate` route. `graft-ops` would have waited (`Gate::Pending`).
- At 100% and on the first day, every minute from 00:59Z: `Breach("GATE_APPLY_LAG: N+1 p99 1500 ms, N 0 ms")`. N+1 had 1,348 to 29,396 entries, 0 exceptions, 0 `STEP_DOWN`, and an ack rate of 99.85 to 100%. With no N baseline, only the 500 ms cap applies, and c was drained, so all its members were followers. See R40.

## Leg 2: `EdgeRoll{e4}` with `GATE=pass`, under kills

| Index | Group time | Entry | Step |
| --- | --- | --- | --- |
| 117 | 01:37:32Z | `Intent` EdgeRoll e4 | |
| 118 | 01:42:29Z | `IntentStep` | 0 (1%) |
| 119 | 01:48:32Z | `IntentStep` | 1 (10%) |
| 120 | 01:52:31Z | `SiteMode` Drain c, drain key | (the drain leg, below) |
| 121 | 01:55:29Z | `IntentStep` | 2 (50%) |
| 122 | 02:04:34Z | `ReleaseUploaded` R4 | |
| 123 | 02:19:30Z | `IntentStep` | 3 (100%, deployed 01:57:32Z) |
| 124 | 02:20:32Z | `IntentDone` | |

The automatic drain ran inside this intent and did not disturb its steps. The 100% `IntentStep` took 22 minutes from the deploy. The runs that lived first uploaded R4's three site versions and committed its `ReleaseUploaded`, and the others died before the commit.

## Drain leg, on c's R3 build (`site_health` counted)

- **No fault first (01:38Z to 01:48Z), with load on chains homed on a and b.**
  - c did not drain on its own. Over 5 minutes, a's burn on c read 2.6 to 19.8 x, but b's read 3.7 to 7.6 x, never 10 x at the same time.
  - c's own reports were under `MIN_CALLS`: 3 to 28 calls, because c, drained, leads no chain.
  - `raw/bf39/health.txt` has every minute of both reports.
- **Fault.** Gray fault on c (`rate 0.3`, `hang_ms 3000`), from 01:48:52.5Z on the Worker clock.
  - **Burning.** `graft-ops`' first run after the fault read c `burning` at 01:49:31Z: a→c 64 of 2,859 missed (44.8 x) and b→c 45 of 2,993 (30.1 x). That run's drain commit was killed, and the next two runs died earlier.
  - **Drain committed** at 01:52:31.8Z, entry 120, by the drain key.
  - **Fault start to drain committed: 3 min 39 s.** Of that, 39 s is fault start to the first burning verdict. The other 3 min are kills.
  - At the drain, the reports read a→c 655 of 2,970 (441 x) and b→c 642 of 3,180 (404 x).
- **Lift** at 01:54:47Z on the Worker clock (`{off: true}`).
- **Undrain not reached.** By 02:27Z, more than 30 minutes after the lift, no faulted bucket was left in the window. Even so, c's `UNDRAIN_AFTER` reports read a→c 30 of 16,635 (3.6 x) and b→c 64 of 18,716 (6.8 x), both over `UNDRAIN_BURN` (1 x), so c was not calm. Steady load alone keeps c over 1 x (R41).
- **Undrain by leg 3.** Leg 3 needed c undrained to roll it, and its own drain step then held c drained by its intent. The undrain after leg 3's `Rollback` was the day's step, entry 131 at 02:59:32Z, not the automatic one. So **recovery to automatic undrain: not reached.** The $10 stop was not the reason. The cause is that c's calm verdict never came, even with no fault.

| Threshold | Value | Exercised | Reading |
| --- | --- | --- | --- |
| `DRAIN_WINDOW` / `DRAIN_BURN` | 5 min / 10 x from both | yes | No fault: a→c up to 19.8 x, b→c up to 7.6 x, no drain. Fault: 44.8 x and 30.1 x at the first run, then the drain |
| `MIN_CALLS` | 100 | yes | c's reports of a and b (3 to 28 calls) counted as no evidence, so c judged nobody. Before the row, b→c read 70 calls (under the floor) and a→c 582 calls with 3 missed (10.3 x, counted) |
| `UNDRAIN_AFTER` / `UNDRAIN_BURN` | 30 min / under 1 x from both | yes, not met | 3.6 x and 6.8 x 30 min after the lift |
| `STUCK_DAY` | 1 h | no | No day sat on one drained step for an hour. Leg 1's longest step was the undrain, 01:06Z to 01:23Z |
| `GATE_APPLY_LAG` | 500 ms cap, 2 x N | yes (leg 3, and leg 1 offline) | Breached with N+1 1,500 ms and N 1,500 ms: on the cap, not on the 2 x rule |
| `GATE_EXCEPTIONS`, `GATE_STEP_DOWN`, `GATE_ACK_RATE` | 0, 0, 0.1 points | read | 0, 0, and N+1 100% against N 99.93% in leg 3 |
| `STEP_MIN_ENTRIES` | 1,000 (test override 50) | yes | Leg 3's 1% read 0 N+1 entries for 2 dwells and paged at 02:34:32Z and 02:35:33Z (R38). Leg 3 then added load on 88 more chains homed on c |
| `STEP_DWELL` | test value 60 s | yes | |

## Leg 3: `SiteDay{c, R4}` with the real gate, under kills

The row was at $4.96 when the intent posted, under the $6 bar.

| Index | Group time | Entry | Step |
| --- | --- | --- | --- |
| 125 | 02:28:14Z | `Intent` SiteDay c R4, undo `Rollback{c, R2}` | |
| 126 | 02:28:32Z | `IntentStep` | 0 (drain; c already drained by entry 120, so no `SiteMode`) |
| | 02:32:30Z | R4 `e2b04384` at 1%, R3 `8e86f2c4` at 99% | |
| | 02:33Z to 02:38Z | `Starved(0)`, then `PAGE graft-ops: … 1% step applied 0 entries in 2 x its dwell` | |
| 127 | 02:39:32Z | `Rollback{c, R2}`, the pre-signed undo (customer signature) | breach: `GATE_APPLY_LAG: N+1 p99 1500 ms, N 1500 ms`, 20 N+1 entries |
| 128 | 02:42:31Z | `IntentStep` | 0 (drain, already held) |
| | 02:50:31Z | R2 `39439670` at 100% | |
| 129 | 02:52:30Z | `SiteRelease` c to R2 | |
| 130 | 02:58:31Z | `IntentStep` | 1 (R2 at 100%) |
| 131 | 02:59:32Z | `SiteMode` Normal c, drain key | |
| 132 | 03:03:29Z | `IntentStep` | 2 (undrain) |
| 133 | 03:05:30Z | `IntentDone` | |

The undo restarted the step count at 0, as `IntentState::Undoing` does. Kills fell before each return deploy (3) and between the deploy and its `SiteRelease`, and each return step still committed once. This is the "a breach runs the pre-signed `Rollback` with no human signature" half of BF-39, here under kills. It breached for the reason R40 gives, not for a planted fault.

## After the done line: a false drain of c and its hand undrain

- **False drain, entry 134, 03:09:34Z, drain key `477de526…`.** No fault was set. The reports read a→c 11 of 1,812 missed (12.1 x on 1 in 2,000) and b→c 10 of 1,949 (10.3 x), both over `DRAIN_BURN` 10 x with more than `MIN_CALLS`, so `verdict` called c burning. The run came after the `GATE=pending` redeploy and before the Cron Trigger's removal took effect: the trigger was removed at 03:06Z, and `graft-ops` still ran until 03:12:31Z, its last run. Automatic drain under `GATE=pending` is correct, since drain is the safety path; the cause is R41's thresholds. Under the R41 rule now in code (1 in 100, 5 x) the same reports read 0.61 x and 0.51 x and drain nothing.
- **Customer-key `Normal` refused.** A customer-signed `SiteMode Normal c` posted at 03:52:55Z answered 422 `WrongSigner`: the group ends a drain-key drain only with a drain-key `Normal` (`crates/release/src/group.rs`, `by_drain_key`), and steady load kept c over `UNDRAIN_BURN` 1 x, so `graft-ops` would never have undrained it (OPEN-QUESTIONS R42).
- **Hand undrain, entry 135, posted 03:54:25Z on the orb's clock (group time about 03:54:47Z), digest `bb3f81eb…`.** Signed by hand with graft-m4a's drain key, on the lead's instruction: the drain was false, B0-29 needs c leading, and this is the test install's own key. Just before signing, the group read 135 entries, no intent current, and c the only drained site. The seed was read only into the signing step and never printed.
- **After.** The fold reads `count 136`, `current None`, `drained []`, and all three members are at 136. c leads its home groups again: at 04:04Z, after light writes, 6 of 6 sampled c-homed chains (`repo:7390000`, 011, 023, 037, 059, 433) had c's member as leader in a new term. `graft-ops` has run 0 times since 03:12:31Z, and its schedules read `[]`, so the old rules cannot drain c again.

## Gate check row: `gate-relative` and `drain-budget` hosted (2026-11-23, main `bb64b0ff`)

**Result: the judged day ran the real gate and breached on chance, so the share rule was never judged hosted; the drain leg and the slow-apply leg were not run.** Done line 07:00:33Z (end state read). Spend $4.50 on graft-m4a from 06:03Z to 07:00Z (`tests/fanout/acctcost.py`, PREFIX), against the $5 stop. Raw records in `raw/ck/`.

**Setup** (all in `tests/am1/results/install-m4a.md`):
- Token `graft-m4a-ops-deploy-3` minted 06:03Z, revoked 06:59Z.
- Site day 70 (**test-faults**, setup, `deploy.mjs`, main `bb64b0ff`) 06:03Z to 06:04:43Z, exit 0: a `31436419`, b `ef13b1dc`, c `9861b487` (`tests/m4/raw/wp4/siteday70.log`). Its crons were removed at 06:04:43Z.
- Bundles R5 `5d0ba4e9…` (the day-70 build), R6 `761a720d…` and R7 `44f1ddfe…` (R5 plus a marker each), `ReleaseUploaded` at entries 136 to 138.
- `graft-ops` test-faults, `OPS_STEP_MIN_ENTRIES=50`, no `OPS_KILL`, 60 s dwells, a 1-minute cron. Its first run came about 8 minutes after the deploy, as in BF-39.
- Load from the scratch Worker `graft-m4a-ck-edge` (deleted 06:59Z): 160 chains homed on c from observers a and b, about 25 puts/s each, and 40 chains homed on a and b.

| Index | Group time | Entry | |
| --- | --- | --- | --- |
| 139 | 06:32:54Z | `Intent` SiteDay c R5, undo `Rollback{c, R2}` | **setup, unjudged (`GATE=pass`)**: moves c to a build with the `VERSION` binding, so R5 can be a real N |
| 140 to 150 | 06:33:18Z to 06:40:20Z | drain, 1%, 10%, 50%, `SiteRelease` c R5, 100%, undrain, first day, `IntentDone` | c on R5 `1704e715` |
| 151 | 06:41:46Z | `Intent` SiteDay c R6, undo `Rollback{c, R5}` | **judged (`GATE` unset)**, N+1 `c7687c36`, N `1704e715` |
| 152, 153 | 06:42:20Z, 06:42:22Z | `SiteMode` Drain c, `IntentStep` 0 | |
| 154 | 06:46:19Z | `IntentStep` 1 (1%) | passed only on the test override |
| 155 | 06:47:19Z | `Rollback{c, R5}`, pre-signed | 10% breached `GATE_ACK_RATE` |
| 156 to 160 | 06:47:21Z to 06:47:32Z | return steps 0 and 1 (R5 at 100%), `SiteMode` Normal c, step 2, `IntentDone` | |

**`judge_site`'s real verdicts on the judged day** (`graft-ops`' own log lines, `raw/ck/crons.txt`; N over the same step window):

| Step | Run | N+1 entries | N entries | N+1 over N's p99 / over 5 s | Ack N+1 / N | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| 1% | 06:43:14Z | 0 | 3,625 | — | 100% / 99.55% | `Starved(0)` |
| 1% | 06:45:14Z | 13 | | | | `Starved`, then `PAGE … applied 13 entries in 2 x its dwell` |
| 1% | 06:46:14Z | 58 | 14,166 | 0 / 0 bp | 100% / 99.88% | `Starved(58)`, **passed only on the override** (`OPS_STEP_MIN_ENTRIES` 50) |
| 1% to 10% | 06:46:14Z, two later reads in the same run | 22, 31 | 1,357, 1,510 | 0 / 0 bp | 100% / 100% | `Starved` |
| 10% | 06:47:14Z | 100 | 4,814 | 0 / 0 bp | **99.44% / 100%** | **`Breach("GATE_ACK_RATE: N+1 9944 bp, N 10000 bp")`** → `Rollback` |

- **The share rule (`gate-relative`) was judged on no step.** No N+1 window reached `STEP_MIN_ENTRIES` (1,000): 0 to 58 at 1% and 22 to 100 at 10%, while N applied 1,357 to 14,166 in the same windows. It is not confirmed hosted. On the windows it saw, N+1 put 0 bp over N's p99 (1,500 ms) and 0 bp over 5 s, which is the healthy reading, but under the floor it counts for nothing.
- **The breach was chance, not a regression.** R6 is R5's build plus one unused module, so N+1 and N run the same code. 99.44% is about 1 refused AppendEntries answer in about 180 [inference from the rate: the log gives the rate, not the count]. N's own ack rate, the same code, read 97.44% to 100% from one read to the next in the judged day's runs. `GATE_ACK_RATE`'s 0.1-point margin is far inside that noise and is judged at any count, so a step's N+1 breaches on chance often: even at a steady 0.12 to 0.33% of refused answers, an N+1 of 180 to 1,000 answers reads 0.1 points under N 12 to 45% of the time [inference, binomial]. This is the floor problem `gate-floor` fixed for the lag shares; the lead's ruling kept the ack rate judged at any count, and this row refutes that (OPEN-QUESTIONS R43).
- **N's p99 from the intent's start at 100% and on the first day: not exercised.** The judged day ended at 10%. In the setup day N was R2's `39439670`, which counts nothing, so its fallback read 0 entries and only the backstop held, as designed.
- **The pre-signed `Rollback` ran with the real gate**, and c ended on R5 at 100% with its undrain.
- `raw/ck/gate.txt` holds a scratch mirror of each minute's counts. Its `judge(...)` lines from 06:47:23Z judge R5 against itself after the rollback and are not verdicts.

**Not run.** At 06:48Z the row was at $4.20. The drain leg (about $2 in BF-39) would have passed the $5 stop, so it and the slow-apply leg after it were not run: the drain at about 126 s and the undrain at about 15 min 40 s stay replays, and no slow-apply breach was observed.

**Cleanup.**
- `graft-ops`' cron was removed at 06:47:56Z but fired `Idle` runs every minute until 06:56:14Z. A reset (a yearly cron, then `[]`) at 06:56:11Z stopped it; no run came from 06:56:14Z to 06:59:02Z.
- Then `graft-m4a-ops` `8e90328d`, `GATE=pending`, schedules `[]`. Token revoked (the API answers it 1000), the scratch Worker deleted.
- End state at 07:00Z: count 161, `drained []`; a `31436419`, b `ef13b1dc`, c `1704e715` (R5, the group's `site_release` for c); edge `f4e193b9` 100%, `dd713c1c` 0%, `/api/v3/meta` 200; 6 of 6 sampled c-homed chains led by c.

## Records this row changes

- `decisions/OPEN-QUESTIONS.md`:
  - R40, the apply-lag gate on a drained site.
  - R41, `DRAIN_BURN`'s margin.
- `design/DESIGN.md`, Site day's table: `GATE_APPLY_LAG`'s cap, and `DRAIN_BURN` with `MIN_CALLS`, marked as [guess] values this row refutes. No threshold changed in code.
- `decisions/DECISIONS.md`, "The hosted half's inputs are counted in the site's registry objects": its hosted evidence.
- Later, at $0 (2026-11-23): R40 closed by `gate-relative` (N+1's share over N's p99, a 5 s backstop) and R41's thresholds replayed on `health.txt` (1 in 100, 5 x over 5 min, `MIN_CALLS` 300, calm under 2 x over 15 min), both in `decisions/DECISIONS.md`; R41 stays open, and R42, the missing override for a false automatic drain, is new.

Raw records in `raw/bf39/`:
- `bf39.log`: the driver's milestones.
- `answers.txt`: the three intent posts.
- `entries.txt`: the group's entries 105 to 133, decoded with their signers.
- `state1.txt` to `state3.txt`: the fold each minute.
- `gate.txt`: `judge_site` each minute during legs 1 and 2, with its inputs.
- `health.txt`: every voter's `site_health`, over 5 and 30 minutes, each minute.
- `crons.txt`: `graft-ops` Workers Logs lines.
- `opslog.json.gz`: every `graft-m4a-ops` log line from 00:17Z, which the kill counts come from.

## Gate check row 2: the share and ack rules judged, and `drain-budget` hosted (2026-11-23, main `cd10ac82`)

**Result: the first hosted step judged by the share and ack rules passed (50%), and the drain leg drained in 134.8 s and undrained 15 min 34.0 s after the lift, with no false drain. At 100% and on the first day N read as nothing, so only the 5 s backstop judged them (R44). The slow-apply leg was not run.** The lead accepted it. Done line 08:52Z (end state read). Spend $6.82 on graft-m4a from 07:55Z (`tests/fanout/acctcost.py`, PREFIX), against the $8 stop. Times are Cloudflare's: the orb's clock ran 22.7 s behind it, and `raw/ck2/ck.log` stamps lines with the orb's. Raw records in `raw/ck2/`.

**Setup** (all in `tests/am1/results/install-m4a.md`):
- Token `graft-m4a-ops-deploy-4` was minted at 07:56:16Z and revoked at 08:52:06Z.
- No site day. The sites ran B0-29's site day 71 (main `ecd685ac`, test-faults): a `9fb2121a`, b `247e0365`, c `4aa58601`. The group's `site_release` for c was R5 from row 1.
- `graft-ops` (`4ef7d70c`) was the test-faults build from `cd10ac82`, with `GATE` unset, `OPS_STEP_MIN_ENTRIES=50`, 60 s dwells and a 1-minute cron. The cron was removed and re-added at once, and it first fired at 08:00:03Z.
- Load came from a redeployed scratch Worker `graft-m4a-ck-edge`, at 2x row 1: 160 c-homed chains from four benches (two from observer a, two from b) and 40 chains homed on a and b. That is about 3,250 c-homed writes/min, against row 1's 2,850.

| Index | Group time | Entry | |
| --- | --- | --- | --- |
| 161 | 08:01:49Z | `Intent` SiteDay c R6, undo `Rollback{c, R5}` | **judged (`GATE` unset)**, N+1 `c7687c36` |
| 162, 163 | 08:02:07Z, 08:02:08Z | `SiteMode` Drain c, `IntentStep` 0 | the 1% split is `c7687c36` 1%, **`1704e715` (R5) 99%**: N is the group's `site_release` for c, and day 71's `4aa58601` is replaced |
| 164 | 08:08:08Z | `IntentStep` 1 (1%) | passed only on the override |
| 165 | 08:10:05Z | `IntentStep` 2 (10%) | passed only on the override |
| 166 | 08:14:08Z | `IntentStep` 3 (50%) | **judged by the share and ack rules**, with no override |
| 167 to 171 | 08:14:11Z to 08:16:07Z | `SiteRelease` c R6, step 4 (100%), `SiteMode` Normal c, step 5, step 6 (first day) | only the backstop judged (R44) |
| 172 | 08:16:08Z | `IntentDone` | |
| 173 | 08:22:06.847Z | `SiteMode` Drain c, drain key | drain leg |
| 174 | 08:39:06.070Z | `SiteMode` Normal c, drain key | drain leg |

**Mid-day redeploy.** At 08:12:01Z, during the 50% step, `graft-m4a-ops` was redeployed as `87367b76`: the same `cd10ac82` build and executor file without `OPS_STEP_MIN_ENTRIES`. This was a test-ops change, not a load or spend change, and the lead accepted it.
- Why: the 1% and 10% steps had passed only on the override. At 2x load, 10% gave N+1 at most 256 entries, which was the row's stop condition. Load was not raised.
- Effect: 50%, 100% and the first day were judged on the real `STEP_MIN_ENTRIES` floor.

**`judge_site`'s real verdicts on the judged day** (`graft-ops`' own lines, `raw/ck2/verdicts.txt`, where the share and the backstop are recomputed from the logged histogram). Counts are entries / answers / refused:

| Step | Run | N+1 | N | Over N's p99 / over 5 s | Ack z | Verdict | Judged by |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1% | 08:03:08Z to 08:07:05Z | 0 / 0 / 0 | 95 to 1,982 | — | — | `Starved(0)`, paged at twice its dwell | — |
| 1% | 08:08:07Z | 71 / 102 / 1 | 5,959 / 9,538 / 28 | 0 / 0 bp | 0.60 | `Pass` | **override only** |
| 10% | 08:09:05Z | 133 / 194 / 0 | 6,845 / 10,542 / 0 | 0 / 0 bp | — | `Pass` | override only (step not yet committed) |
| 10% | 08:10:05Z | 256 / 378 / 0 | 13,726 / 21,124 / 1 | 0 / 0 bp | −22.1 | `Pass` | **override only** |
| 50% | 08:11:05Z | 200 / 321 / 0 | 6,887 / 10,532 / 3 | 0 / 0 bp | −7.8 | `Pass` | override (before the redeploy) |
| 50% | 08:12:05Z, 08:13:06Z | 458, 847 | 13,691, 20,385 | 0 / 0 bp | | `Starved`, paged at twice its dwell | — |
| 50% | 08:14:07Z | **3,141 / 4,935 / 3** | **23,191 / 35,345 / 5** | **22 / 0 bp** | **−1.50** | **`Pass`** | **share rule (cap 200 bp), ack rule (z 3.09), backstop** |
| 100% | 08:14:08Z, 08:14:10Z | 109, 125 | **207** (N's new window) | — | | `Starved` | — |
| 100% | 08:14:12Z to 08:14:17Z | 125 to 1,130 | **0** (no N after `SiteRelease`) | — / 0 bp | none | `Pass` at 1,130 | **backstop only** |
| first day | 08:15:08Z, 08:16:06Z | 4,098, 8,097 / 12,385 / 1 | 0 | — / 0 bp | none | `Pass` | **backstop only** |

- **The share and ack rules judged a hosted step for the first time, at 50%, and passed it:** 22 bp of N+1's entries over N's p99 (1,500 ms), against the 200 bp cap, at an ack z of −1.50. N+1 and N are the same code plus one unused module, so this is the healthy reading.
- **1% and 10% passed only on the override.** No 60 s window at 10% reached 1,000 N+1 entries even at 2x load. The rules are not confirmed on those steps (R38).
- **N at 100% and on the first day did not come from the intent's start** (R44). `ops::Exec::gate` falls back to the intent's start only when N's window counts 0 entries:
  - at 100% before `SiteRelease`, N still counted 207 entries in the new window, under the floor, so there was no share baseline;
  - after `SiteRelease c R6` (entry 167), `running` equals the day's release, so `day_versions` gives N as `None` and its counters as empty.
  
  The first day's passes at 1,130 to 8,097 N+1 entries were judged by the backstop alone.

**Drain leg** (no intent current; load cut to 1x at 08:18:50Z; gray fault on c at rate 0.3 with 3 s hangs):

| Event | Time | From the previous event | Evidence |
| --- | --- | --- | --- |
| Fault set | 08:19:52.087Z | | scratch edge `now` |
| 08:21 run: not burning | 08:21:03Z | | o0→c 594 of 13,067 = 4.5%, under 5 x of 1 in 100 |
| **Drain**, entry 173 | 08:22:06.847Z | **134.8 s** (replay: 126 s) | 08:22 run: o0→c 1,071 of 7,606 = 14.1%, o1→c 398 of 2,958 = 13.5%, both at least 5 x over 5 min and over `MIN_CALLS` 300 |
| Fault lifted | 08:23:32.118Z | | |
| **Undrain**, entry 174 | 08:39:06.070Z | **15 min 34.0 s** (replay: 15 min 40 s) | c's 15-minute rate o0→c 0.90%, o1→c 0.80%, under 2 x; a minute earlier o0→c read 3.6% |

- Before the fault, under 2x and 1x load, c read 0.07 to 0.17 x on the 1-in-100 budget from each observer, and no site was drained apart from the judged day's own drain step. `STUCK_DAY` was not reached.
- Load was cut to 0.5x at 08:24:52Z to stay under the stop. One c-bench and the a/b bench kept both observers over `MIN_CALLS`.

**Not run.** At the undrain the row was at $6.65. A third site day for slow-apply would have passed the $8 stop, so no slow-apply breach was observed.

**Cleanup.**
- Benches stopped. The `graft-m4a-ops` cron was reset (a yearly cron, then `[]`) at 08:40:05Z, but it fired `Idle` runs until 08:48:03Z, so it was reset again at 08:44:48Z. There were no runs from 08:48:03Z to 08:50:43Z.
- Then, at 08:51:03Z, `graft-m4a-ops` `8b6fe2e3`, `GATE=pending`, with schedules `[]`.
- Token revoked at 08:52:06Z. The first `DELETE` was sent without the account base, and the token was found by name and revoked.
- The scratch Worker was deleted.
- End state at 08:52Z:
  - count 175, `drained []`;
  - a `9fb2121a`, b `247e0365`, c `c7687c36` (R6, and the group's `site_release` for c is R6);
  - edge `f4e193b9` at 100% and `dd713c1c` at 0%, and `/api/v3/meta` answers 200;
  - c leads 6 of 6 of its home chains;
  - every site's schedules are `[]`.

**Records this row changes:**
- `decisions/DECISIONS.md`: hosted evidence for `gate-relative`, `gate-ack` and `drain-budget`.
- `decisions/OPEN-QUESTIONS.md`: R38 (counting across the dwell, with 10 to 15 minute hosted dwells at 10% and 50%; 1% stays open), R41 (confirmed on one install), R43 (judged once, hosted), and the new R44, N at 100% and on the first day.

Raw records in `raw/ck2/`:
- `ck.log`: the driver's milestones, with orb times.
- `answers.txt`: the intent post.
- `entries.txt`: the group's entries 160 to 174, decoded with their signers.
- `verdicts.txt`: each `graft-ops` gate line, with the recomputed share and backstop.
- `gate.txt`: a scratch mirror of each minute's counts. Its `judge(...)` lines are the mirror's, not verdicts.
- `health.txt`: every voter's `site_health` over 5 and 15 minutes, each minute.
- `crons.txt`: `graft-ops` Workers Logs lines from 07:55Z.

## Gate check row 3: a real N at 100% and on the first day (2026-11-23, main `ad8fd52f`)

**Result: R44's fix is shown hosted.** N was real at every step. The share rule judged 50%, 100% and the first day, and a first-day `GATE_APPLY_LAG` breach against a real N ran the pre-signed `Rollback` to `IntentDone`. Each step committed once and in order. The breach is most likely chance (R46). The lead accepted the row.
- **Not exercised:** 10% was judged on the override only, because of the dwell slip below, so 10% judged by the share rule at the real floor (R38's open case) is still to come.
- **Not run:** slow-apply, which the lead's install cap ruled out for the day.

The done line was 11:34Z. Spend was $6.07 on graft-m4a from 10:50Z against the $8 row stop. The install's UTC day reached $35.23, against the lead's $39.5 stop under the $40 cap. Raw records are in `raw/ck3/`. Times are Cloudflare's; `ck.log` stamps lines on the orb's clock, which runs about 23 s behind.

**Setup** (all in `tests/am1/results/install-m4a.md`):
- Token `graft-m4a-ops-deploy-5` (`56b96bd6…`) was minted at 10:50:50Z and revoked at 11:33:15Z.
- There was no site day. The sites ran B0-29's site day 74 (main `f129cd16`): a `7a2abd5d`, b `0de60a36`, c `7d9f70f9`. The group's `site_release` for c was R6 from row 2.
- Load came from the scratch Worker `graft-m4a-ck-edge`. It ran at 2x row 1 until 11:13Z, then 1x to protect the install cap.
- The orb's executor was replaced at about 11:14Z. That stopped the load and loggers for about a minute, and they were restarted. `graft-ops` on Cloudflare was not touched.

**The judged day:** `SiteDay{c, R5}` with undo `Rollback{c, R6}` and `GATE` unset. N+1 is R5 `1704e715` and N is R6 `c7687c36`; they are the same code plus a marker. The 1% split replaced day 74's `7d9f70f9`.

| Index | Group time | Entry |
| --- | --- | --- |
| 175 | 10:57:03Z | `Intent` |
| 176, 177 | 10:57:50Z, 10:57:52Z | `SiteMode` Drain c, `IntentStep` 0 (1% deployed) |
| 178 | 11:03:51Z | `IntentStep` 1 (1% passed; 10% deployed) |
| 179 | 11:05:49Z | `IntentStep` 2 (10% passed; 50% deployed) |
| 180 | 11:18:52Z | `IntentStep` 3 (50% passed) |
| 181 to 184 | 11:18:56Z to 11:19:02Z | `SiteRelease` c R5, step 4 (100%), `SiteMode` Normal c, step 5 (the first day starts) |
| 185 | 11:19:53Z | `Rollback{c, R6}`, pre-signed, on the first day's breach |
| 186 to 191 | 11:19:56Z to 11:20:09Z | the return steps: Drain c, R6 at 100%, `SiteRelease` c R6, Normal c |
| 192 | 11:20:11Z | `IntentDone` |

**`graft-ops` redeploys.** All three used the same `ad8fd52f` test-faults build. After each one the next cron run fired within a minute, read the same intent, and re-ran no committed step.

| Config | Version | Deployed (orb) | Vars | For |
| --- | --- | --- | --- | --- |
| a | `75b79e2e` | 10:51:21Z | `OPS_STEP_MIN_ENTRIES` 50, `STEP_DWELL_MS` 60000, `FIRST_DAY_MS` 60000 | 1% |
| b | `673fffbf` | 11:05:41Z | no override, `STEP_DWELL_MS` 720000, `FIRST_DAY_MS` 60000 | meant for 10%; it fell on 50% |
| pending | `4bbfd9e5` | 11:33:14Z | `GATE=pending`, no cron | cleanup |

**The dwell slip.** 10% was judged on the override only, and the 12-minute dwell fell on 50%.
- Config b was deployed once 1% had committed, on a guess that 10% would need its full minute.
- 10% committed at 11:05:49Z on config a's 60 s dwell, by the override at 316 N+1 entries, before b's version served the next run.
- The runbook now waits on the prior step's commit: deploy the long-dwell config as soon as the step before the target step commits, and confirm the target step's run is on the new version before its dwell can end (`tests/am1/results/install-m4a.md`, the cleanup recipe's paragraph). For a 10% step that means redeploying once step 0 (the drain) commits, so 1% runs on the long dwell too.

**`judge_site`'s real verdicts** (`graft-ops`' own lines, `raw/ck3/verdicts.txt`). Counts are entries / answers / refused:

| Step | Where N came from | N+1 | N | Over N's p99 / over 5 s | Verdict | Judged by |
| --- | --- | --- | --- | --- | --- | --- |
| 1% | what c runs (R6) | 0 for 5 min, then 69 / 100 / 0 | 6,119 / 9,901 / 19 | 144 / 0 bp | `Starved(0)`, paged, then `Pass` | **override only** (R38) |
| 10% | what c runs | 316 / 489 / 0 | 17,670 / 27,082 / 9 | 0 / 0 bp | `Pass` | **override only** (the slip) |
| 50%, 12 min | what c runs | 3,134 to 27,590 / 43,987 / 18 | 28,774 to 47,469 / 74,001 / 6 | 35 to 7 bp / 0 to 3 bp | `Pass` on every read from 11:09:52Z | **share, ack and backstop** |
| 100% | N's step window | 1,961 to 2,182 | 1,535 to 1,689 (members not yet moved) | 0 / 0 bp | `Pass` | share, ack and backstop |
| first day, 60 s | the `Rollback`'s R6 (after `SiteRelease`, the day's own release) | **1,901 / 3,233 / 2** | **1,222 / 2,114 / 0** | **215 / 57 bp** | **`Breach("GATE_APPLY_LAG: 215 bp of N+1 entries over N's p99 1500 ms")`** | **share** (cap 200 bp; backstop cap 100) |

- **R44 hosted.** Row 2 read no N at 100% and on the first day. Here, on the first day, `day_versions` gave the `Rollback`'s R6 after `SiteRelease`, and at 100% N's own window had over `STEP_MIN_ENTRIES`. The gate read N from the intent's start on no step of this day.
- **The first-day breach is most likely chance** [inference]. Of N+1's 1,901 entries, 37 sat in the 5 s and 10 s buckets (26 and 11), all in the first 60 s after the full-site version change. N had none there. R5 and R6 are the same code. The share was 215 bp against the 200 cap. Recorded as OPEN-QUESTIONS R46; no threshold changed.
- **The ack rule** judged 50%, 100% and the first day and never breached. z was at most 1.91 against 3.09 (50%'s first judged read, 12 refused of 4,832 against N's 5 of 43,906), then −0.9 to −6.6 as the 50% counts grew, and −0.87 on the first day.

**Cleanup.**
- Benches stopped. The ops cron was reset at 11:22:38Z and again at 11:26:52Z, and it fired `Idle` runs until 11:29:47Z.
- Idle to 11:32:51Z. Then `graft-m4a-ops` `4bbfd9e5`, `GATE=pending`, with schedules `[]`.
- Token revoked at 11:33:15Z, and the scratch Worker deleted.
- End state at 11:34Z:
  - count 193, `drained []`;
  - a `7a2abd5d`, b `0de60a36`, c `c7687c36` (R6, and the group's `site_release` for c is R6);
  - edge `f4e193b9` at 100%, `/api/v3/meta` answers 200;
  - c leads 6 of 6 sampled home chains, and every schedule is `[]`.
- `graft-ops` was handed to BF-46, which the lead moved to after 00:00Z on 2026-11-24. The next check row (a 12-minute 10% step at 2x, plus slow-apply if money allows) runs after BF-46 restores `4bbfd9e5`.

Raw records in `raw/ck3/`:
- `ck.log`: the driver's milestones, with corrections.
- `answers.txt`: the intent post.
- `entries.txt`: entries 174 to 192, decoded with their signers.
- `verdicts.txt`: each `graft-ops` gate line with the recomputed share and backstop, and every action.
- `crons.txt`: the Workers Logs lines.
- `gate.txt`: the scratch mirror. Its `judge(...)` lines are not verdicts.
- `health.txt`: `site_health` each minute.
