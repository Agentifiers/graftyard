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

## Gate check row 4: the 10% step judged on the real floor (2026-11-24, main `05aace32`)

**Result: pass.** The judged `SiteDay{c, R10}` ran to `IntentDone` with no breach. Its 10% step was judged by the share rule on the real floor and passed on all 8 judged reads, which settles R38's 10% case. 1% passed on the override only, so R38's 1% case stays open. Slow-apply (R11) was not run, because it would have passed the $8 stop. The lead accepted the row. The done line was 03:11Z. Spend was $7.67 on graft-m4a from 02:10Z, against the $8 row stop; the install's UTC day stood at $7.85. Raw records are in `raw/ck4/`. Times are Cloudflare's; `ck.log` stamps lines with the orb's clock.

**Setup** (all in `tests/am1/results/install-m4a.md`):
- **Starting point:** BF-47's R9 (`e23f69e9…`, main `05aace32` test-faults plus a marker) on all three sites, the edge and `graft-ops`, with no site day.
- **Bundles R10 `1a0f0e50…` (label `ck-r10`) and R11 `e592682c…` (`ck-r11`):** R9's `site/index.js` and `site/index_bg.wasm` (the same SHA-256 as R9's objects in `graft-m4a-releases/r9/`) and R9's `formats.txt`, plus a `site/marker.js` each, signed with the install's release key (`openssl pkeyutl -sign -rawin`). `graft-ops` uploaded both from the bucket on its own: `ReleaseUploaded` at entries 274 and 275. N+1 and N are R9's code.
- **Token:** `graft-m4a-ops-deploy-6` (`df3894a8…`), minted 02:14:38Z, revoked 03:09:18Z.
- **Cron:** a new ops cron fired only after a second remove and re-add; the first run came at 02:23:36Z.
- **Load:** the scratch Worker `graft-m4a-ck-edge` ran 2x row 1's load. It was cut to 1x at 02:35Z in the override-only 1% step, back to 2x at 02:41Z, and to 1x again at 02:44Z to hold the stop.

**The judged day:** `SiteDay{c, R10}` with undo `Rollback{c, R9}` and `GATE` unset. N+1 is R10 `54bbb423`, N is R9 `4600b130`.

| Index | Group time | Entry |
| --- | --- | --- |
| 276 | 02:28:13Z | `Intent` |
| 277, 278 | 02:28:39Z, 02:28:41Z | `SiteMode` Drain c, `IntentStep` 0 (1% deployed) |
| 279 | 02:41:39Z | `IntentStep` 1 (1% passed; 10% deployed) |
| 280 | 02:54:40Z | `IntentStep` 2 (10% passed; 50% deployed) |
| 281 | 02:56:41Z | `IntentStep` 3 (50% passed) |
| 282 to 285 | 02:56:46Z to 02:56:53Z | `SiteRelease` c R10, step 4 (100%), `SiteMode` Normal c, step 5 (the first day starts) |
| 286, 287 | 02:58:39Z, 02:58:40Z | step 6, `IntentDone` |

**`graft-ops` redeploys.** All four used the same `05aace32` test-faults build, and each was made on the prior step's commit (the runbook in `tests/am1/results/install-m4a.md`). After each one, the next run fired within a minute, read the same intent, and re-ran no committed step.

| Config | Version | Deployed (orb) | Vars | On |
| --- | --- | --- | --- | --- |
| a | `fa472388` | 02:15:09Z | `OPS_STEP_MIN_ENTRIES` 50, `STEP_DWELL_MS` 720000, `FIRST_DAY_MS` 60000 | the row's start |
| b | `8796e3b4` | 02:41:51Z | no override, `STEP_DWELL_MS` 720000, `FIRST_DAY_MS` 60000 | 12 s after step 1 (279) |
| c | `466e44cc` | 02:55:12Z | no override, `STEP_DWELL_MS` 60000, `FIRST_DAY_MS` 60000 | 32 s after step 2 (280) |
| parked | `ac4cd9a5` | 03:09:17Z | `GATE=pending`, no cron | cleanup |

Configs a and b share the 12-minute dwell, so the redeploy's timing could not shorten 10%.

**`judge_site`'s real verdicts** (`graft-ops`' own lines, `raw/ck4/verdicts.txt`). Counts are entries / answers / refused:

| Step | Where N came from | N+1 | N | Over N's p99 / over 5 s | Ack z | Verdict | Judged by |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1%, 12 min | what c runs (R9) | 0 for about 5 min, 560 at 02:40:41Z, 589 / 945 / 0 at 02:41:38Z | 72,399 / 110,169 / 56 | 0 / 0 bp | −22.2 | `Pass` | **override only** (R38's 1% case) |
| **10%, 12 min** | what c runs | `Starved` at 112 to 880, then **1,171 / 1,973 / 0** (02:47:41Z) up to **3,973 / 6,808 / 0** (02:54:39Z) | 30,095 to 53,376 / 89,285 / 31 | **8 to 5 / 0 bp** | −15.7 to −21.6 | **`Pass` on all 8 judged reads** | **share, ack and backstop on the real floor** |
| 50%, 60 s | what c runs | 1,618 / 2,853 / 1 | 7,891 / 13,563 / 6 | 6 / 0 bp | −2.77 | `Pass` | share, ack and backstop |
| 100% | N's step window | 692 to 878 | 1,815 to 2,200 | 14 to 11 / 0 bp | | `Starved`, then committed | not judged (dwell 0, below) |
| first day, 60 s | the `Rollback`'s R9 (after `SiteRelease`, the day's own release) | 1,938 / 3,331 / 0, then 3,593 / 6,156 / 0 | 4,027 / 7,020 / 5, then 5,554 / 9,618 / 5 | 5 / 0 bp | −5.4 to −6.5 | `Pass` | share, ack and backstop |

- **R38's 10% case is settled.** With the 12-minute dwell on the real floor and no override, N+1 passed `STEP_MIN_ENTRIES` 6 minutes into the step. Every read from then to the commit judged the share rule against a real N and passed.
- **100% is not judged by the share rule, as built.** The 100% step has dwell 0, and `next` dwells on a `Starved` gate only when the step's dwell is over 0 (`crates/release/src/ops.rs`, `Step::Deploy`), so 100% commits on its first run unless the gate breaches. Here N+1 had 878 entries, under the floor. The first day after it is judged against a real N and passed (recorded with R38).
- In the 02:41:36Z run, the 02:41:38Z read is the 1% step's verdict. The 02:41:39Z and 02:41:41Z reads, at 42 and 52 entries, are the 10% step's first window in the same run (`ck.log` has a correction line).

**Not run.** Slow-apply (R11) was uploaded and ready, but at 02:49Z the row was at $6.09 with 10% still in its dwell. A third site day plus cleanup would have passed the $8 stop, so it was skipped, as the lead ruled. `r11/` stays in `graft-m4a-releases`.

**Cleanup.**
- Benches stopped at 02:58:10Z. The ops cron was reset at 02:59:21Z and again at 03:05:55Z, and fired `Idle` runs until 03:04:36Z.
- Idle to 03:08:49Z. Then `graft-m4a-ops` `ac4cd9a5` (R9's build, `GATE=pending`, schedules `[]`). Token revoked, scratch Worker deleted.
- End state at 03:11Z:
  - count 288, nothing current, `drained []`;
  - a `75bdd60d`, b `627ed36c`, c `54bbb423` (R10; the group's `site_release` for c is R10);
  - edge `eef95142` at 100% and `f4e193b9` at 0%, `/api/v3/meta` answers 200;
  - c leads 6 of 6 sampled home chains, and every schedule is `[]`.
- `graft-ops` went to AM2-WP3 with `~/.graft-m4a` staged.

Raw records in `raw/ck4/`:
- `ck.log`: the driver's milestones, with corrections.
- `answers.txt`: the intent post.
- `entries.txt`: entries 273 to 287, decoded with their signers.
- `verdicts.txt`: each `graft-ops` gate line with the recomputed share and backstop, and every action.
- `crons.txt`: the Workers Logs lines.
- `gate.txt`: the scratch mirror. Its `judge(...)` lines are not verdicts.
- `health.txt`: `site_health` each minute.

## Rows A and B: the pooled 1% floor with slow-apply, and the drain during a site day (2026-11-24, main `dc880f1d`)

Both rows ran on graft-m4a from merged main `dc880f1d`, which carries `gate-pool` (`f9e13961`), cleared by the lead: row A with a $5 stop and row B with a $6 stop. Both used `graft-m4a-ops`, built test-faults, with 1x load from the scratch Worker `graft-m4a-ck-edge` (two c-benches and one a/b bench, as in row 4). Token `graft-m4a-ops-deploy-7` (`625f764f…`) was minted at 05:54:43Z. Times are Cloudflare's; the orb's clock ran 25.7 s behind it, and `ck.log` stamps lines with the orb's. The raw records are in `raw/ckA/` and `raw/ckB/`.

### Row A: R11 slow-apply on the pooled floor

**Result: pass.** The lead accepted it. All three lines held: 1% committed `Starved` after its dwell with no page; 10% was judged from the 1% step's start and breached on the share rule once N+1's pooled count passed 1,000; and the `Rollback` ran to `IntentDone`. This closes R38's 1% case hosted. The done line was 06:20Z, and the row cost $3.61, against a $5 stop.

**Setup.**
- `graft-m4a-ops` was `62a8ed72`, deployed with `exec-ckA.json`: no `OPS_STEP_MIN_ENTRIES`, `STEP_DWELL_MS` 720000, `FIRST_DAY_MS` 60000, and a 1-minute cron.
- The cron did not fire after the deploy. One remove and re-add fixed it, and the first run came at 06:00:07Z (R52).
- `SiteDay{c, R11}` carried the pre-signed `Rollback{c, R10}`. N+1 was c `bf647730` (R11's version as the fold records it at entry 275; c also holds an unrecorded `62a04785` from an overlapping upload run). N was c `54bbb423` (R10, which the group's `site_release` for c names). Both are R9's code plus a marker.
- The plant was `slow_ms` 600 on `bf647730` through the scratch Worker's `/fault`, set at 06:00:53.9Z, before the intent. Every c member running that version holds each inbound `/raft` 600 ms before it steps it.

| Index | Group time | Entry |
| --- | --- | --- |
| 354 | 06:00:57Z | `Intent` |
| 355, 356 | 06:01:14Z, 06:01:17Z | `SiteMode` Drain c, `IntentStep` 0 (1% deployed at 06:01:20Z) |
| 357 | 06:14:12Z | `IntentStep` 1: 1% committed `Starved(499)`, no page; 10% deployed |
| 358 | 06:18:15Z | `Rollback{c, R10}`, on the 10% breach |
| 359 to 363 | 06:18:16Z to 06:18:26Z | return steps 0 and 1 (c to `54bbb423` at 100%), `SiteMode` Normal c, step 2, `IntentDone` |

**`judge_site`'s verdicts** (`graft-ops`' own lines, `raw/ckA/verdicts.txt`). Counts are entries / answers / refused:

| Step | N+1 | N | Over N's p99 (1,500 ms) / over 5 s | Verdict |
| --- | --- | --- | --- | --- |
| 1%, 12 min | 0 until 06:05Z, then 11 to 499 | up to 47,512 / 80,310 / 35 | 7,379 to 9,090 / 0 bp | `Starved` throughout; committed at `Starved(499)` after the dwell (`gate-pool`) |
| 10%, from the 1% step's start | 618 (06:15:14Z), 737, 867 | up to 58,423 | 7,443 to 7,647 / 0 bp | `Starved` |
| 10%, first read past 1,000 | **1,068 / 1,843 / 2** (06:18:14Z) | 61,821 / 104,941 / 37 | **7,696 / 0 bp** | **`Breach`: `GATE_APPLY_LAG: 7696 bp of N+1 entries over N's p99 1500 ms`** |

- The 10% count went on from the 1% step's 499 rather than restarting at 0, which is `window_start` reading `Current::before`.
- No read put an entry over 5 s, so the backstop never fired. The share rule did.

### Row B: B0-07 hosted, a gray site during another site's day

**Result: the drain half passed; the automatic undrain could not happen during the day, so c was undrained by hand on the lead's ruling.** After the hand undrain the day resumed and ran to `IntentDone`, with no breach. **B0-07's per-group 20 s line is unproven hosted, not met** (corrected 2026-11-25): `leaders.txt` polled every 12 s, and no chain's activity before the drain was recorded, so the row cannot tell which chains the line judges or when each moved. On Cloudflare time c led 14 chains 3.3 s after the drain, 2 at 15.4 s and 0 at 27.4 s, so those 2 moved between 15.4 and 27.4 s. B0-07 r2 (`tests/m5/b07r2/`) times each chain from its members' election traces. The done line was 07:27Z. The row cost $5.27, against a $6 stop; graft-m4a's UTC day stood at $16.85.

**Setup.**
- `graft-m4a-ops` was `9249fa8b`, the same `dc880f1d` build, deployed with `exec-ckB.json`: `OPS_STEP_MIN_ENTRIES` 50 (b's own gate is not under test), `STEP_DWELL_MS` 300000, `FIRST_DAY_MS` 60000, and a 1-minute cron. This time the cron fired without a re-add; the first run was at 06:25:08Z.
- `SiteDay{b, R10}` carried `Rollback{b, R9}`. N+1 was b `847a6465` (R10, entry 274), and N was b `627ed36c` (R9).
- At b's 10% step, while b was drained, a gray fault went on c at 06:32:29.6Z: rate 0.3, 3 s hangs (`hang_ms` 3000). It was lifted at 06:39:42.6Z.

| Index | Group time | Entry |
| --- | --- | --- |
| 364 | 06:25:54Z | `Intent` |
| 365, 366 | 06:26:14Z, 06:26:15Z | `SiteMode` Drain b, step 0 (1%) |
| 367 | 06:32:11Z | step 1 (10% deployed at 06:32:13Z) |
| 368 | 06:37:14.354Z | `SiteMode` Normal b, drain key: **b undrained under the 1% lead limit**, 284.8 s after the fault |
| 369 | 06:37:16.339Z | `SiteMode` Drain c, drain key, 286.7 s after the fault, in the same run |
| 370 | 07:06:20.238Z | `SiteMode` Normal c, **signed by hand** with the drain key (the lead's ruling) |
| 371 | 07:07:11Z | step 2 (10% passed: N+1 6,702 / 11,797 / 20 against N 81,505, 58 bp over N's p99, 2 bp over 5 s; 50% deployed) |
| 372 to 375 | 07:13:11Z to 07:13:18Z | step 3 (50%), `SiteRelease` b R10, step 4 (100%), step 5 (undrain, b already Normal) |
| 376, 377 | 07:15:11Z, 07:15:13Z | step 6 (the first day passed: 11 bp over N's p99, 5 bp over 5 s), `IntentDone` |

**What B0-07's line asked, and what was seen.**
- **Two sites drained at once:** never. b's `Normal` and c's `Drain` committed in the same run, 2.0 s apart, in that order (`ops::auto_drain`).
- **The day holds while c is drained:** each run from 06:37Z to 07:06Z answered `Wait`, "site 2 is drained; the site day waits".
- **Leaders move off c** (`raw/ckB/leaders.txt`, every 10 s from site a's members, orb time):
  - Before the fault, c led 145 to 150 of the 160 c-homed chains.
  - Under the fault, before the drain, a took most of them: c led 14 to 32.
  - After the drain, the 06:37:19.6Z poll (3 s after the drain) read 14 on c, 06:37:31.7Z read 2, and 06:37:43.7Z read 0. c then led none until the hand undrain. b led none either: it is lead-limited, and the one sampled chain in its 1% (`repo:7390455`) is c-homed.
- **c's undrain after the lift: not reached, a deadlock (R53).** c was healthy: over 15 minutes, o0→c read 67 of 78,637 calls missed (0.09%). But calm needs both other voters to report at least `MIN_CALLS` (300) over `UNDRAIN_AFTER`, and o1→c read 71. `SiteHealth` counts member-to-member calls, so with c drained and b lead-limited by the running day, a led every chain and b's members only followed, sending c 15 to 45 calls per 5 minutes. The day's wait on another drained site has no bound, so neither could move.
  - Restarting observer b's bench at 06:46Z did not help, because the bench's own counts do not feed `site_health`.
  - The row was held at 07:03Z for the lead, at $4.23.
  - Row 2's undrain worked only because no intent was current, so b led its own chains.
- **The hand undrain:** the lead ruled (a). `SiteMode Normal c` was signed with graft-m4a's drain key, as at entry 135, and posted at 07:06:20Z (digest `17e9c253…`). Just before, the group held 370 entries, `SiteDay{b, R10}` was current at step 2, and c was the only drained site. The day resumed on its next run. After the undrain, c led 134 to 138 of the c-homed chains again.

**Cleanup.**
- Benches stopped at 07:14:40Z and the fault was off.
- The ops cron was reset at 07:15:54Z and again at 07:18:28Z. It fired `Idle` runs until 07:19:07Z.
- `graft-m4a-ops` was parked on `ac4cd9a5` by version id; the token was revoked and the scratch Worker deleted (the end state below).

Raw records in `raw/ckA/` and `raw/ckB/`:
- `ck.log`: the driver's milestones.
- `answers.txt`: the posts.
- `entries.txt` (row A): entries 353 to 363, decoded with their signers.
- `verdicts.txt` and `crons.txt`: `graft-ops`' Workers Logs lines with the recomputed shares.
- `gate.txt` and `gate-b.txt`: the scratch mirror. Their `judge(...)` lines are not verdicts.
- `health.txt`: every voter's `site_health` each minute.
- `leaders.txt` (row B): the leader site of the 160 c-homed chains every 10 s.

## B0-07 r2: the plan (2026-11-25, driver `tests/m5/b07r2/`; run the same day, result below)

Row B's shape on graft-m4a, with `graft-ops` from merged main so `calm-one` (R53's fix) is in, and the per-group line
measured from each chain's own members. The lead approved the driver at $0 on 2026-11-25; it runs only after the lead
has the head and this plan, after BF-71, and before or after AM2's Sandbox kill row by agreement with AM2.

**Pass lines**, every one on Cloudflare time (the group's entry times, the members' clocks, and the scratch Worker's
`Date.now()`; the orb's clock is only offset-checked against Cloudflare's `Date` header at the start):
- never two sites drained, and `graft-ops` undrains b under the lead limit, then drains c;
- c undrains with no hand `Normal`: the driver posts no `SiteMode`, and `graft-ops` logs `Committed("SiteMode")` for
  c's `Normal` in the run that committed it;
- the day (`SiteDay{b, N+1}`, undo `Rollback{b, N}`) reaches `IntentDone`;
- every c-homed chain's leader at the drain is known (a baseline `/status` before the fault, moved by the election
  trace's role rows);
- the result names how many c-led chains were active (a bench write served or committed) in the 50 s before the drain,
  and at least one was: both c-benches write every c-homed chain through the drain, so the 20 s line is exercised;
- every chain led on c at the drain moves within 20 s: an active one of the drain, an idle one of its next request.
  The move is the first role row in which a member on a or b becomes leader, read from each member's `/elections`
  (test-faults; `[at, kind, term, peer, index, extra]` on the member's clock), every 20 s through the row, so no poll
  interval bounds it.

`judge.py` computes every line from the raw records alone. Checked at $0: on a synthetic record with an active chain
at 5 s, an idle one at 5 s after its next request and an active one at 25 s it fails the last line naming that chain,
and passes once that chain moves at 15 s; on row B's own `Release` entries and `graft-ops` lines it fails "no hand
Normal" (entry 370 was signed by hand), and the two per-chain lines for want of traces.

**Steps**, each a phase of `row.sh`:
1. `state`: the group, every deployment and schedule, the day's spend, and the clock offset.
2. `build` and `siteday`: the site from merged main at run time, test-faults, with BF-70 r2's flags; crons `[]`;
   reretire. One site day.
3. `bundles`, `setup`: N and N+1 are that build plus a marker each, with the group's last `formats.txt`; the deploy
   token `graft-m4a-b07r2-deploy`; the scratch Worker `graft-m4a-b07r2-edge`; 160 c-homed and 40 a-homed chains.
4. `ops 60000`, `roll`: `graft-ops` from main with 60 s dwells; `SiteDay{b, N}` with no undo, so N is b's record.
5. `ops 420000`, `run` (supervised): 7-minute dwells so c drains inside b's 10% step (row B drained 286.7 s after the
   fault); `SiteDay{b, N+1}` undo `Rollback{b, N}`; load as row B's 1x; the fault (rate 0.3, 3 s hangs) once b is
   drained at 10%; the lift 60 s after c's drain; load halved for c's 15-minute calm; the day to `IntentDone`.
6. `judge`, then `teardown`: load and loggers off, the fault off; `graft-ops`' cron reset, parked on `ac4cd9a5` by
   version id, cron `[]` again; the deploy token revoked and shredded; the scratch Worker deleted. `lists` checks it.

**Cost and stop.** Row B cost $5.27 for the same load over about an hour. The stop is $6 from `setup`, checked every
minute by `roll` and `run`; at it the load and fault go off and the row is held for the lead. One site day; no edge day.

**Not covered.** The election trace lives in each member's memory (`ELECT_MAX` 2,000 rows), so an isolate reset between
polls loses rows; the 20 s poll keeps the loss to rows written in the 20 s before a reset, and `judge.py` counts each
member it could not read (`members_missed`).

## B0-07 r2: result (2026-11-25, main `f08c570e`, graft-m4a)

**Result: PASS on every line.** `graft-ops` undrained b under the lead limit and drained c, then undrained c on its own
after the lift, with no hand `Normal`. The day reached `IntentDone`. All 32 chains led on c at the drain moved within
20 s: 31 were active in the 50 s before it and 1 was idle. The active max was 18,621 ms and the p50 5,044 ms. R53's fix
(`calm-one`) has now run hosted, so B0-07 closes. Done line 14:47:04Z (Cloudflare time). The lead had the verdict at
about 14:52Z.

**Setup.**
- Site day from `f08c570e` (test-faults, BF-70 r2's flags) at 13:21Z: a `2f8a1178`, b `dd18f535`, c `b9960b91`. Crons
  `[]`, reretire OK. The edge stayed on `3c453149`.
- N `38b34a9b…` and N+1 `387034ad…` are that build plus a marker each, with 27 formats.
- From 13:26:57Z: 160 c-homed chains from `repo:7634000` and 40 a-homed from `repo:7634500`. Voters were known for 200
  of 200; 9 of 200 warm writes missed.
- `graft-m4a-ops` `914cb38f` (60 s dwells) ran `SiteDay{b, N}` to `IntentDone` (entry 561), so b reported N. Then
  `cbbdcd6d` (420 s dwells, 1-minute cron) ran the measured day.

**Incident, the deploy token (A54).** The row's first deploy token held Workers Scripts Write and Workers Containers
Write only. `graft-ops`' first version upload answered 10021 "Secrets store binding authorization failed", the case
DECISIONS's A54 amendment records. The token was re-minted with Secrets Store Write and PATCHed into
`graft-m4a-ops-deploy-token`, the old one was revoked, and the uploads then committed. The driver now mints that
permission (`row.sh setup`).

**The measured day** (intent `01M89GE5NFWA3EJ2RBMPSE3GMX`; the group's times):

| Index | Time | Entry |
| --- | --- | --- |
| 562 | 14:06:32.817Z | `Intent` `SiteDay{b, N+1}`, undo `Rollback{b, N}` |
| 563, 564 | 14:07:04.353Z, 14:07:06.166Z | `SiteMode` Drain b, step 0 (1%) |
| 565 | 14:15:07.631Z | step 1 (10%) |
| | 14:15:12.762Z | the gray fault on c: rate 0.3, 3 s hangs (the scratch Worker's clock) |
| 566 | 14:18:08.575Z | `SiteMode` Normal b, drain key, 175.8 s after the fault |
| 567 | 14:18:11.753Z | `SiteMode` Drain c, drain key, 179.0 s after the fault, in the same run |
| | 14:19:38.344Z | the fault lifted, 86.6 s after the drain |
| 568 | 14:35:08.882Z | `SiteMode` Normal c, drain key, **committed by `graft-ops`** (its `Committed("SiteMode")` at 14:35:18.986Z). The driver posted no `SiteMode` |
| 569 to 574 | 14:35:11.719Z to 14:45:08.981Z | steps 2 to 6, with `SiteRelease` b N+1 at 571 |
| 575 | 14:45:13.013Z | `IntentDone` |

c's calm came from observer a alone while b was lead-limited (`calm: limited Some(SiteId(1))`, first `calm: true` for
c at 14:35:08.716Z), as `calm-one` says. Afterwards the group held 576 entries, with no intent current, nothing drained,
and site_release b = `387034ad`.

**Lines** (`judge.py` on the committed raws, `raw/b07r2/m4ar2/verdict.jsonl`):

| Line | Result |
| --- | --- |
| Never two sites drained | pass: at most 1 |
| `graft-ops` undrains b under the lead limit, then drains c | pass: 566, then 567 |
| c undrains with no hand `Normal` | pass: 568 by `graft-ops`; hand posts none |
| The day reaches `IntentDone` | pass: 575 |
| Every c-homed chain's leader at the drain is known | pass: 160 of 160 |
| c-led chains active in the 50 s before the drain | **32 led on c at the drain: 31 active, 1 idle**. The benches wrote every c-homed chain, so no extra load was needed |
| Every group led on c moves within 20 s | pass: **32 of 32**. Active max 18,621 ms (`repo:7634050`), p50 5,044 ms. 4 moved after 10 s and 2 after 15 s. 13 member reads missed |

**Not measured.** Member reads that missed lose only the role rows written in the 20 s before an isolate reset (the
plan's "Not covered"). The 1 idle chain (`repo:7634040`) moved 1.6 s after the drain and 0.56 s after its next
request.

### The judge's four fixes, and that none turned a fail into a pass

`judge.py` was changed during the run, after the raws began to arrive and before the verdict. On the same raws, the
pre-run judge (`196c2ffa`) answers FAIL (`raw/b07r2/m4ar2/verdict-196c2ffa.jsonl`). It judges the wrong day: it takes
the group's first c drain, entry 36 on 2026-11-22. From there:
- "no hand Normal" fails, because that day's Normal c (entry 43) has no `graft-ops` line in this row's log;
- it finds 37 chains on c, 0 active, with moves of about 268,000 s;
- "leader known" fails with 95 unplaced.

| Fix | Line it changes | Why the old rule was wrong, not strict | Effect on these raws | Case that still fails a late move (`test_judge.py`) |
| --- | --- | --- | --- | --- |
| 1. Judge only entries from the measured intent on; a missing intent fails | every line that reads entries | The file holds the group's whole history (576 entries), so "first c drain" found another day's drain three days earlier. Judging that day is a different row, not a stricter one | the drain becomes 567 and the Normal 568. With this fix alone the per-group line reads 31 of 31, p50 4,690 ms, max 18,621 ms | an earlier day drained c and the chain moved 5 s after it; on the measured day it moves at 25 s: FAIL, naming it. Also: an intent missing from the group fails |
| 2. Baseline is the newest pre-fault `/status` poll naming a leader, not the first poll | "leader known", and which chains count as led on c | The first poll (14:06:50.8Z) named a leader for only 65 of 160 c-chains, because a group restored from storage reports no leader until a request wakes it (F128). Polls 2 to 12 named 158 to 160. The old rule then also dropped every role row before the first poll, so a chain whose last election predates the poll could never be placed | places `repo:7634079`: c won term 2 at 14:06:42.6Z, before the first poll, and it moved 5,776 ms after the drain. Led on c goes from 31 to 32; nothing else changes | the first poll names no leader, a later pre-fault poll names c, and the chain moves at 25 s: FAIL. Under the old rule the same chain is unplaced and the row still fails |
| 3. Role rows from `POLL_MS` (30 s) before a poll apply on top of it | which chains count as led on c | A poll's `at` is when it ends; it reads 480 member calls at 6 in flight over up to 30 s. A role row inside that span may be newer than what the poll read, and dropping it misplaces the leader. Term order keeps an older row from undoing a newer leader | none alone (32 of 32 with `POLL_MS` 0); without fix 4 as well, 2 chains are misplaced on c (below) | the poll read a leading at term 2, c won term 3 5 s before the poll's `at`, and moves at 25 s: FAIL |
| 4. The baseline stops at the fault (`fault.jsonl`, written by `row.sh run`) | which chains count as led on c | With fix 2, "newest before X" has to stop where members start to hang. Polls under the fault read c's members timing out and keep stale claims | none alone (32 of 32 with the drain as the cut); without fix 3 as well, the same 2 chains | a poll under the fault keeps a's stale term-1 claim, and the chain led on c moves at 25 s: FAIL |

Every case also has a control. The same chain moved at 15 s passes, and moved at 25 s with all four fixes, it fails.
Removing any one fix makes exactly its own case go wrong. The pre-run judge answers FAIL on cases 1 to 3 too, but not
because it saw the late move:
- case 1: it judges the earlier day (a move at 3,625 s);
- case 2: it calls the chain unplaced;
- case 3: it finds no chain led on c.

It judges case 4 correctly, because its first poll is before the fault. Checked by swapping each rule back in a scratch
copy and running `python3 tests/m5/b07r2/test_judge.py`.

**What the fixes did to the verdict.**
- The pre-run judge's FAIL on these raws is a verdict on 2026-11-22's day, not on this one.
- With fix 1 held, all eight on/off combinations of fixes 2 to 4 were run on these raws. Seven read 32 of 32 within
  20 s, max 18,621 ms, 0 unplaced.
- Fix 2 alone adds `repo:7634079` (moved at 5.8 s) next to the pre-run baseline, and closes "leader known".
- The eighth combination has fix 2 with neither 3 nor 4. It reads 34 led on c, with 2 "late": `repo:7634098` at
  1,259 s and `repo:7634104` at 1,043 s. Neither moved late. Each member's own role rows show a won them before the
  drain: a leader at terms 13 and 11 at 14:17:37.293Z and 14:17:34.048Z, with c follower at 14:17:37.380Z, 34 to 38 s
  before 14:18:11.753Z. The 14:18:22.5Z poll reads a leading both.
- In that combination the baseline is the 14:17:39.8Z poll, which still named c as leader at terms 12 and 10. It read
  c's members before their step-down, at least 2.4 s before its `at`, and with `POLL_MS` 0 the rows that correct it
  are dropped. That is the poll-span error fix 3 removes. Fix 4 avoids it too, because it keeps the baseline out of the
  fault, where elections run.
- So no chain the measured day moved late was dropped or judged on time by a fix. The only verdicts the fixes change
  are the pre-run judge's other-day verdict and two stale-poll misplacements.

**Cost.** $4.09 on graft-m4a from setup (13:26:57Z) to teardown, against the $6 stop. That is $4.36 including the
13:20Z site day. m4a's UTC day was $7.45 at 14:50Z.

**Teardown (14:51Z to 14:56Z).**
- The fault is off (200 from each site).
- `graft-m4a-ops` is parked on `ac4cd9a5` by version id, with schedules `[]`. The first cron reset's `PUT` hung for 4
  minutes with no answer; it was killed, and the second reset after the park read `[]`.
- `graft-m4a-b07r2-deploy` is revoked, so `graft-m4a-ops-deploy-token` holds a revoked token, and the next `graft-ops`
  upload mints its own. `graft-m4a-b07r2-edge` is deleted (404). The row's token files are shredded.
- Left running: sites a `2f8a1178`, b `421db7d6` (`graft-ops`' N+1 day), c `b9960b91`; edge `3c453149`; every cron
  `[]`. No intent is current and nothing is drained.

**Raw records** in `raw/b07r2/m4ar2/`:
- the phase logs (`state`, `siteday`, `bundles`, `setup`, `ops`, `roll`, `run`, `judge` and `teardown`);
- `entries.jsonl`, `ops-log.jsonl`, `chains.json`, `voters-*.json`, `answers.txt`, `fault.jsonl` and `intent.id`;
- `ops.jsonl.gz`, `elections.jsonl.gz` and `status.jsonl.gz`, which `judge.py` reads compressed;
- `verdict.jsonl` (this judge) and `verdict-196c2ffa.jsonl` (the pre-run judge, on the same raws).
