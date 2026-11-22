# M5-WP3 hosted: BF-7 on graft-m4a with `graft-ops` executing

`build/BUILD-PLAN.md`, M5-WP3, row BF-7: after `FormatsEnable`, submit the pre-signed `Rollback` to N and `Abort { intent, target: N }`. Pass: both refused, and both page. Fail: either applied.

**Result: pass (2026-11-22, done line 13:20:16Z).** `graft-ops` ran every roll step on graft-m4a from merged main. After `FormatsEnable 65000.1` the customer's `Abort` to R1 got 422 `UnreadableFormat` with a `PAGE release` line. With the gate breached, `graft-ops` submitted the intent's pre-signed `Rollback` to R1; the group refused it `UnreadableFormat`, and `graft-ops` wrote `PAGE graft-ops`. No split moved, and the group holds neither entry. Spend from the ops deploy at 10:18Z to 13:21Z was $0.71 on graft-m4a (`tests/fanout/acctcost.py`); the install's day to 13:21Z was $2.50.

**65000.1 is enabled for good on graft-m4a's `Release` group.** Every later m4a release's `formats.txt` must list it, or the group refuses that release's rollbacks and every intent to it (`UnreadableFormat`). It is a dummy format: no family 65000 exists in `crates/` (checked before the run).

## Setup

All of it is recorded in `tests/am1/results/install-m4a.md`, "Resources".

- `Keys` generation 2 at `Release` index 601 (`f2e30821…`), signed by the customer key. It names a new executor key and a new drain key.
- The customer deploy token `graft-m4a-ops-deploy`: Workers Scripts Write and Workers Containers Write, on this account only. It is stored as the Secrets Store secret `graft-m4a-ops-deploy-token`, next to `graft-m4a-ops-executor-key` and `graft-m4a-ops-drain-key`.
- The release bucket `graft-m4a-releases`, which holds two bundles signed by m4a's release key:
  - R1 `bdf8547f…`: the test-faults site build from main `2d7429a2`, whose `formats.txt` holds the edge's 26 enabled formats.
  - R2 `a3a1f620…`: the same `index.js` and wasm plus `site/marker.js`, so the roll does not skip it as unchanged, and `65000.1` in `formats.txt`.

  Neither bundle carries site code past `2d7429a2`.
- `graft-m4a-ops` from main `70c4f7ca`, with `--executor`, `GATE=pass`, `STEP_DWELL_MS=60000` and `FIRST_DAY_MS=60000`, on the 5-minute cron. Before R2's site c day it was redeployed with `FIRST_DAY_MS=21600000`, so site c held at its first day.

Two host bugs were found and fixed before any intent ran, each merged before it was used:
- `ops-upload-runtime`, main `2d7429a2`. A version upload kept no runtime settings, which would have dropped the git container map and the CPU limit.
- `ops-view-retry`, main `70c4f7ca`. A cold cron isolate's first view refresh missed `VIEW_CALL_MS`: 1,014 and 1,008 ms on the first two runs.

## Run

`graftyard release roll --entries --key customer.key` signed each next intent. `/tmp`'s driver posted them, and `graft-ops` did everything else: uploads, `ReleaseUploaded`, drain-key `SiteMode`, deploys at 1, 10, 50 and 100%, `SiteRelease`, `IntentStep`, undrain, first day and `IntentDone`. Each site day took 25 minutes at these dwells. Every graft-ops upload kept `compatibility_date` 2026-10-01, `cpu_ms` 300000, the GitShard image `…1368b1d21d41`, and the 42 bindings.

| Release | Site | Intent (index) | R at 1% | R at 100%, `SiteRelease` | `IntentDone` |
| --- | --- | --- | --- | --- | --- |
| R1 | a | 607 | 10:50:13Z | 11:05:13Z | 11:10:13Z (not in the tail; site b's intent posted 11:10:34Z) |
| R1 | b | 630 | 11:15:13Z | 11:30:13Z | 11:35:13Z |
| R1 | c | 653 | 11:40:13Z | 11:55:13Z | 12:00:13Z |
| R2 | a | 675, undo `Rollback { Site(0), R1 }` | 12:05:13Z | 12:20:13Z | 12:25:13Z |
| R2 | b | 697, undo to R1 | 12:30:13Z | 12:45:13Z | 12:50:13Z |
| R2 | c | 719, undo `Rollback { Site(2), R1 }` | 12:55:13Z | 13:10:13Z | held at first day |

At 13:15Z, site c was at its first-day hold, and sites a, b and c ran R2 at 100%: `6331c773`, `afdaf2a9`, `39439670`. Every site's `SiteRelease` named R2.

| Time | Step | Answer |
| --- | --- | --- |
| 13:15:51Z | `FormatsEnable 65000.1`, signed by the customer key over the entry and the release key over `formats_enable_message` | 201, index 740 |
| 13:15:51Z | `Abort { intent 01M81MXN44YHGEK4WKPR1CDEP9, target: R1 }`, customer | 422 `UnreadableFormat`: "release bdf8547f… cannot read enabled format 65000.1". The edge logged `PAGE release UnreadableFormat: …` |
| 13:16:01Z | `graft-m4a-ops` `c378b478` with `GATE=breach:bf7` | deployed |
| 13:20:13Z | `graft-ops` cron: breach on the first-day hold, so it submits the intent's first unused pre-signed undo, `Rollback { Site(2), R1 }` | refused `UnreadableFormat`. `PAGE graft-ops: Rollback refused, UnreadableFormat: release bdf8547f… cannot read enabled format 65000.1`, then `Refused { kind: "Rollback" }`, and the run stopped |

After the run:
- Splits: site a `6331c773`=100 (12:20:17Z), site b `afdaf2a9`=100 (12:45:18Z), site c `39439670`=100 (13:10:17Z). These are the same deployments as before `FormatsEnable`.
- The group holds 82 entries. The last is `FormatsEnable`, after site c's `IntentStep` for undrain. No `Abort` and no `Rollback` is committed.
- `m3org/j4` answers 200.

Then `graft-m4a-ops` `31251d43` was deployed with `GATE=pending`, and its Cron Trigger was removed (`schedules` = `[]`) at 13:21Z, so `graft-ops` cannot step on its own. R2's site c intent stays current, held at its first day.

Raw records:
- `raw/bf7/bf7.log`: the driver and the assertions.
- `raw/bf7/answers.jsonl`: every edge answer.
- `raw/bf7/crons.txt`: every `graft-ops` cron run's log lines, from `wrangler tail`. The 11:10:13Z run is missing because the tail dropped it; its `IntentDone` for R1 site a is in the group.
