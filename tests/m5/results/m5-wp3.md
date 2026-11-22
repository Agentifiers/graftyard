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

**Ending the held intent (13:29Z to 13:37Z, approved).** `graft-m4a-ops` `b6d7cd9f` ran with `FIRST_DAY_MS=60000`, `GATE=pass` and the cron. Its 13:35:13Z run committed `IntentStep` step 6 (the first-day hold) and then `IntentDone` for R2's site c `SiteDay`, both signed by the executor key `d387253b…`, and went `Idle`. The group went from 82 to 84 entries, so those two were the only entries in the window; the run changed no deployment. Then `7cc9b100` with `GATE=pending`, and the Cron Trigger was removed at 13:37Z.

Raw records:
- `raw/bf7/bf7.log`: the driver and the assertions.
- `raw/bf7/answers.jsonl`: every edge answer.
- `raw/bf7/crons.txt`: every `graft-ops` cron run's log lines, from `wrangler tail`. The 11:10:13Z run is missing because the tail dropped it; its `IntentDone` for R1 site a is in the group.

## BF-43 rerun with `graft-ops` executing (2026-11-22)

`build/BUILD-PLAN.md`, M5-WP2, row BF-43: an `EdgePointer` undo under a 1% `EdgeRoll` with the API blocked, then the next `EdgeRoll`. Pass: every request pins to N within `POINTER_TTL`, and the next `EdgeRoll` sets `None`. Fail: any request off N while pinned, or the pin survives.

**Result: pass (done line 16:00:49Z).** `graft-m4a-ops` from main `dce09ec2` ran every step: the uploads and `ReleaseUploaded`, every split, every `IntentStep` and `IntentDone`, and the undo. With `DEPLOY_TOKEN` invalid and `GATE=breach:bf43`, its 15:35:45Z cron run committed the intent's pre-signed `EdgePointer` to N. It then waited with "no deployment of Edge read", because the API was blocked. Below, Workers-clock times have a Z; orb times are 20.75 s behind them (measured against the API's `Date` header).
- **Breach to `EdgePointer` committed:** at most 5.6 s. The run was scheduled at 15:35:44.08Z, and site 0's `/view`, polled every second, first held the entry 4.6 s after the run started.
- **`EdgePointer` committed to the pointer serving N:** 1.16 s to the first pinned request. 297 requests in that 1.16 s went unpinned, and 5 of them ran N+1, the last starting at +1.0 s. From +1.16 s to the API's restore, 36,391 of 36,391 requests ran N, and 36,390 carried the pin. From +5 s to the next roll, 0 of 99,936 requests were off N or unpinned.
- **0 Cloudflare API calls on the request path while blocked.** `graft-front` and `graft-m4a-edge` bind no API token. The only token that could reach the API, `DEPLOY_TOKEN`, was invalid from 15:31:30Z to 15:37:57Z (the API answered it 400). It was restored 128 s after the commit was seen, once the pointer was serving N. In that 387 s window, 107,419 requests were answered (all 405 from the edge, none failed), and the edge's `wrangler tail` logged 0 exceptions. The edge's deployment did not move until the API came back (`5276c3ee`=1, `c343923c`=99 from 15:30:45Z to 15:40:45Z).
- **Return and the next roll.** At 15:40:44Z `graft-ops` deployed N at 100% with N+1 at 0%, committed `IntentStep` and `IntentDone`, and went `Idle`. The next `EdgeRoll`, to e3, got 201 at 15:41:22Z. The last pinned request started 0.35 s after it, and 0 were pinned after +5 s. Under its 1% split, 199 of 19,514 requests ran e3. `graft-ops` then walked e3 at 10, 50 and 100%, and committed `IntentDone` at 16:00:49Z.
- **Final edge:** `dd713c1c` (e3) at 100%, with `c343923c` (e1) at 0%. 0 alarms in 631,663 requests.

The releases are three bundles of the same edge build, main `8561eaea` test-faults (wasm sha256 `712756282d42e336…`, the same as edge day 30's `5aa333b8`). Both edgeboot runs passed on that tree with 0 traps before any deploy. Each bundle's `formats.txt` lists edge day 30's 26 formats plus 65000.1, and each is signed by m4a's release key:

| Bundle | Release id | Edge version | Difference |
| --- | --- | --- | --- |
| e1 (N) | `44c04e9b…` | `c343923c` | none |
| e2 (N+1) | `5af23a25…` | `5276c3ee` | `edge/marker.js` |
| e3 (the next roll) | `28af528e…` | `dd713c1c` | `edge/marker.js`, another label |

| Time (Workers) | Step | Who | Answer |
| --- | --- | --- | --- |
| 14:56:20Z | `graft-m4a-ops` `4dec375f`, `OPS_SCRIPTS` + `edge`, `GATE=pass`, 60 s dwells, cron | deploy | — |
| 15:06:01Z–15:06:43Z | e1, e2, e3 uploaded with `keep_bindings`, then `ReleaseUploaded` ×3 | `graft-ops` | group 84 → 87 |
| 15:07:55Z | `EdgeRoll` e1, no undo (the edge's first roll in the group) | `release roll`, customer key | 201, index 754 |
| 15:10:44Z–15:25:44Z | e1 at 1, 10, 50, 100%, then `IntentDone` | `graft-ops` | `c343923c`=100 |
| 15:27:06Z | `EdgeRoll` e2, undo `EdgePointer { pin: e1 }` | `release roll` | 201, index 762 |
| 15:30:44Z | e2 at 1% | `graft-ops` | `5276c3ee`=1, `c343923c`=99 |
| 15:31:30Z | `DEPLOY_TOKEN` set to an invalid value | Secrets Store PATCH | API answers it 400 |
| 15:31:48Z | `graft-m4a-ops` `cbb689e0`, `GATE=breach:bf43` | deploy | — |
| 15:35:45Z | breach: the pre-signed `EdgePointer` to e1 | `graft-ops` | committed (entry 95); then "no deployment of Edge read" |
| 15:37:57Z | real `DEPLOY_TOKEN` restored; `af566775` with `GATE=pass` | Secrets Store PATCH, deploy | — |
| 15:40:44Z | e1 at 100%, e2 at 0%, `IntentStep`, `IntentDone` | `graft-ops` | `c343923c`=100, `5276c3ee`=0 |
| 15:41:22Z | `EdgeRoll` e3, undo `EdgePointer { pin: e1 }` | `release roll` | 201, index 768; pin cleared |
| 15:45:44Z–16:00:45Z | e3 at 1, 10, 50, 100%, then `IntentDone` | `graft-ops` | `dd713c1c`=100, `c343923c`=0 |
| 16:01:35Z | `graft-m4a-ops` `5c8c0d16`, `GATE=pending`; Cron Trigger removed | deploy, `schedules` `[]` | — |

Notes:
- **The fix this needed.** Before `dce09ec2`, `ops::next` read the deployment before it checked the gate on a gated step. With the API blocked, that read is empty, so it waited and never committed the undo. A breach now goes first (`ops-breach-api-down`; `an_edge_breach_with_the_api_down_commits_the_pointer_undo_and_deploys_later`; plant82 mutant 24).
- **The sampler** read `GET /api/graftyard/v1/release/entries` through a scratch `graft-m4a-front` (`c0c16374`, `EDGE` bound to `graft-m4a-edge`), at 8 at once. That route answers 405 with `x-graftyard-edge-version` for any host, which avoided an edge `HOST` change. The other front paths answer 404 here, because the edge routes by `HOST`. `graft-m4a-front` was deleted at 16:02:59Z.
- **Gaps.** An orb executor restart at 15:35:21Z stopped the sampler, the tails and the view poller for 25 s. The breach run fell in that gap, but all three were back 3 s before the `EdgePointer` reached the view, so the numbers above are unaffected. The tail also dropped the 15:05Z cron, which did the uploads, and the 15:55Z cron; the group and the deployments API hold both.
- **`keep_bindings`** held: each uploaded edge version has the same 42 bindings, compatibility date 2026-10-01 and `FORMATS_ENABLED` as `5aa333b8`.
- **Spend:** 14:55Z to 16:02Z was $0.66 on graft-m4a, against the $10 stop (`tests/fanout/acctcost.py`); the install's day to 16:02Z was $3.67.

Raw records in `raw/bf43/`: `bf43.log` (the driver), `answers.jsonl` (the posts, plus `bf43-undo` derived from `counts.jsonl` and the cron's scheduled time), `counts.jsonl` (site 0's entry count per second), `events.jsonl`, `crons.txt` (`graft-ops` log lines from `wrangler tail`), `per-second.jsonl.gz` (sampled requests per second by status, version and pin), and `report.txt`, which is `python3 tests/m5/wp2-report.py tests/m5/results/raw/bf43 --ok 405 --n c343923c-b7db-4555-a06d-8f137effdb01`. That report needs the per-request rows. They are 5.5 MB compressed, so they were not committed; they stay in the orb as `/tmp/bf43/bf43-rows.jsonl.gz`.
