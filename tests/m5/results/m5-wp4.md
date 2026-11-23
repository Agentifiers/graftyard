# M5-WP4 hosted: BF-39 on graft-m4a with `graft-ops` killed in every run

`build/BUILD-PLAN.md`, M5-WP4, row BF-39: kill `graft-ops` at each step of a `SiteDay` and an `EdgeRoll`, 100 times. Pass: it resumes after the last `IntentStep`, no step is skipped, and there is one `IntentDone`. Fail: a skipped or doubled step.

**Result: pass (2026-11-23, done line 03:05:30Z, the last `IntentDone`).** `graft-ops` ran three intents with `OPS_KILL=2`. Each cron run drew 0, 1 or 2 calls to live and then died, so 2 of 3 runs died before or between a deploy and its `IntentStep`. 119 kills fell inside the three intents and 134 in the row, against 100 asked. Every `IntentStep` of each intent committed once and in order, from 0 up, and each intent has one `IntentDone`. No entry was refused (no `Refused` line in the row's logs): a restarted run never resent a step the view already held. Times below are the group's commit times, which run about 22 s ahead of the orb's clock.

| Intent | Gate | Kills in it (view / commit / deploy) | Steps committed | `IntentDone` |
| --- | --- | --- | --- | --- |
| `SiteDay{c, R3}`, index 105, undo `Rollback{c, R2}` | `GATE=pass` | 60 (37 / 22 / 1), 63 runs | 0 to 6, once each, plus drain, `SiteRelease`, undrain | 01:36:32Z |
| `EdgeRoll{e4}`, index 117, undo `EdgePointer{e3}` | `GATE=pass` | 43 (28 / 13 / 2), 43 runs | 0 to 3, once each | 02:20:32Z |
| `SiteDay{c, R4}`, index 125, undo `Rollback{c, R2}` | real gate (`GATE` unset) | 16 (12 / 1 / 3), 19 runs | 0, then the `Rollback`'s return steps 0 to 2, once each, plus `SiteRelease` to R2 and undrain | 03:05:30Z |

Ending state:
- Site c runs R2's `39439670` at 100%, and the group's `site_release` for c is R2 `a3a1f620…`.
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

## Records this row changes

- `decisions/OPEN-QUESTIONS.md`:
  - R40, the apply-lag gate on a drained site.
  - R41, `DRAIN_BURN`'s margin.
- `design/DESIGN.md`, Site day's table: `GATE_APPLY_LAG`'s cap, and `DRAIN_BURN` with `MIN_CALLS`, marked as [guess] values this row refutes. No threshold changed in code.
- `decisions/DECISIONS.md`, "The hosted half's inputs are counted in the site's registry objects": its hosted evidence.

Raw records in `raw/bf39/`:
- `bf39.log`: the driver's milestones.
- `answers.txt`: the three intent posts.
- `entries.txt`: the group's entries 105 to 133, decoded with their signers.
- `state1.txt` to `state3.txt`: the fold each minute.
- `gate.txt`: `judge_site` each minute during legs 1 and 2, with its inputs.
- `health.txt`: every voter's `site_health`, over 5 and 30 minutes, each minute.
- `crons.txt`: `graft-ops` Workers Logs lines.
- `opslog.json.gz`: every `graft-m4a-ops` log line from 00:17Z, which the kill counts come from.
