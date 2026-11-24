# LF-7 hosted on graft-m4a

`build/BUILD-PLAN.md`, AM3-WP1, LF-7: "Bad `graft-controller` at 1% with the deployments API blocked. Submit the
pre-signed `ControllerPointer` undo, then blackhole `GET release/entries` for 10 minutes." Pass: "Every
`graft-agents-front` isolate pins N within `POINTER_TTL` of its next request. Webhooks keep flowing on the last view."
Fail: "Any webhook reaches N+1 after the pin, or blocks on the read."

Cleared by the lead with one change: the blackhole is the bed's fault hook on graft-m4a's `Release` members
(`site=255&chains=release&hang_ms=10000&for_ms=600000`, as BF-8 used it in `tests/m5/wp2-hosted.sh`), not a WAF rule
or a custom domain on graftyard.dev. The edge from main served the hook, so the fallback (a fault in
`graft-agents-front`'s own fetch) was not used. **Stand-in:** the runner account stands in as the agents account
(`FRAME.md`, "Operator decision, 2026-11-24, AM1 closed and the runner account stands in as the agents account"); every
resource there was named `graft-agents-m4a-`. Stops: $3 for the row, $35 for graft-m4a's UTC day.

Run r1 from main `65c10004` (the green head after `a1d04ad4`), driver `tests/am3/hosted/lf7-day.sh`, logs in
`tests/am3/raw/lf7-r1/`. Start sweep 19:05:36Z; **done line 20:06:33Z** (end sweep). **Fail.**

## Verdict

| Line | Result |
| --- | --- |
| Pointer with the API blocked | the pre-signed `ControllerPointer` to c1 posted by hand with the agents deploy token invalid (probe 400): 201 at entry 475, 2.8 s from post to answer; the intent went to `Undoing` [measured] |
| Pins N within `POINTER_TTL` (before the blackhole) | every webhook from 4.1 s after the 201 carried the pin c1 `298b89f0`: 4,556 of 4,562 in the 133 s before the fault; the 6 others came from one new isolate 73 to 78 s in, before its first read finished; 0 reached c2 [measured, `hooks.jsonl.gz`] |
| Webhooks flow during the 10-minute blackhole | 31,576 of 31,580 answered (4 client `fetch failed` in one 30 ms burst), p50 35 ms, p99 65 ms, max 1,308 ms; none waited on the read [measured] |
| **No webhook reaches N+1 after the pin** | **fail: 313 of 31,580 reached c2 (`04c3be7f`, the `test-bad` build, all answered 500) during the blackhole, the first 36.7 s after it began.** From 23 s in, the sampler's requests landed on isolates with no view (29,250 unpinned), whose forwards follow the 1% split. 6 more reached c2 in the 47 s after the fault was cleared [measured] |
| The return | after `unblock`, the agents `graft-ops` deployed c1 100 / c2 0 and committed `IntentStep` and `IntentDone` (entries 476, 477) at 20:03:16Z; count 478, no intent current, `controller_pin` c1, `rolled.Controller` c1 [measured, `final-view.json`] |

**Why it failed [measured, with the cause an inference].** Isolates that started during the blackhole never read a
view. Every edge read in the window ended `canceled` after 861 to 3,847 ms (`edge-reads-fault.txt`). The front's status
from those isolates read `index 0`, `controller null`, and 81 to 84 failed refreshes
(`census-fault.jsonl`). An isolate with no view sets no override, so its forwards take the deployment's split, 1% c2.
The view is fail-static only for an isolate that already holds one. The install's `graft-front` has a second source
for a cold isolate, the members' copies in the `Release` chain's `MemberRegistry` objects (`COPY_MS`). The agents
account has no binding to them, and `GET release/entries` reads only the members' `/view`
(`workers/edge/src/release.rs`, `read`). So a cold `graft-agents-front` isolate has nothing to pin from while the
read is down. The same gap shows without a fault: a cold isolate's first read of all 478 entries is five pages, and the
first page often passed `VIEW_CALL_MS` (`after=0: no answer within 1000 ms` on every first refresh before the row), so
new isolates ran unpinned for 4 to 8 s at ordinary times. This is `decisions/OPEN-QUESTIONS.md`, A53.

## Setup and the steps

1. **Edge day** (test-faults, main `65c10004`, the live version's `FORMATS_ENABLED` and `RUN_SERVICE_PUBLIC`, AM2-WP3's
   web build): `e9197bf3` at 100% from 19:09:50Z, replacing `eef95142`/`f4e193b9`. **R57:** the newest upload
   (`06329970`, `bm5m-edge`) had the same plain-text and JSON vars as the deployed `eef95142`, and `e9197bf3` the same
   as `eef95142`: both diffs empty. `GET release/entries` with no JWT answered 401 after propagation (405 from the old
   version in the first seconds). `/_bed/fault?off=1` answered on all three sites.
2. **`agents init`** in the runner account, store `graft-runners`, prefix `graft-agents-m4a-`: the four secrets, and
   the agents executor `35df5147…`. The webhook secret was then replaced with a value the row held in a 0600 file, so
   the sampler could sign deliveries. Claims and the registration through `tests/am2/hosted/apps.mjs` (new `agents`
   prefix argument): `graftyard-agent` App 3801088, `graftyard-workspace` App 3866624, all 8 commands ok.
3. **Keys:** a customer-signed `Keys` adding `agents_executor`, entry 467 (generation 5).
4. **Agents account:** bucket `graft-agents-m4a-releases`; token `graft-agents-m4a-deploy`; `graft-agents-m4a-controller`
   c0 `b7125cca`, `graft-agents-m4a-front` `91b302e7`, `graft-agents-m4a-ops` (`deploy.mjs --agents`, `GATE` unset,
   60 s dwell, 5-minute `FIRST_DAY`, 1-minute cron, which fired only after a second yearly-then-minute reset, R52).
5. **Two fixes during setup:**
   - The agents `graft-ops`'s first read of each run timed out on its first page (five runs, each canceled at 782 to
     877 ms at the edge). `workers/ops/src/agents.rs` now reads a timed-out page up to `VIEW_TRIES` (4) times, as the
     install's `graft-ops` retries its refresh; redeployed as `39a03439`, after which every run read the view.
   - The deploy token with Workers Scripts Write alone (and with Containers Write, or Secrets Store Read, added) got
     10021 "Secrets store binding authorization failed" on a version upload of a script with a Secrets Store binding.
     With Secrets Store Write it uploaded (`token-scope.log`). The row's token was reminted with Workers Scripts Write
     and Secrets Store Write. The install's deploy token (Scripts Write, Containers Write) uploaded `graft-mcp`, which
     has Secrets Store bindings, in the h1 row, so the difference between the accounts is not explained
     (OPEN-QUESTIONS A54).
6. **Bundles:** c1 `3452f14a…` (main's controller with a marker) and c2 `6c2e44e7…` (`--features test-bad`), signed
   with the release key; the agents `graft-ops` uploaded both (`ReleaseUploaded` 468 and 469, signed `35df51`).
7. **N:** with the token blocked, `release roll c1` (entry 470, no undo: no N yet). The step-0 check passed (the
   controller was still `b7125cca` 100). `Abort` to c1 (471), then unblock: c1 `298b89f0` at 100%, `IntentDone` at 473.
8. **The row:** `release roll c2` (entry 474, one undo, the `ControllerPointer` to c1). c2 at 1% from 19:46:13Z: 69 of
   7,527 webhooks reached it, all 500. Block, then the pointer at 19:48:36Z; the fault plan from 19:50:58Z to
   20:01:07Z, cleared at 20:01:28Z; unblock at 20:02:40Z.

The sampler (`tests/am3/hosted/lf7-sample.mjs`, 2 clients) sent 119,184 signed `issues` deliveries through
`graft-agents-front`'s workers.dev route with `x-graftyard-report-version`, from 19:27:30Z to 20:03:56Z.

## Teardown, state left, cost

- Agents `graft-ops` cron off. `graft-agents-m4a-ops`, `-front` and `-controller` deleted. The five
  `graft-agents-m4a-` store secrets deleted. Token revoked. The bucket's 12 objects deleted, then the bucket. The row's
  token and secret files shredded. The runner account is left with `graft-runners` alone, as before.
- graft-m4a: edge `e9197bf3` at 100%; `graft-ops` parked on `ac4cd9a5`, schedules `[]`, untouched through the row; the
  `Release` group at count 478, no intent current. It now holds `Keys` generation 5 with an `agents_executor` whose seed
  was deleted with the store secret, and `rolled.Controller` c1. The next `ControllerRoll` needs a new `Keys`. The fault
  plan is cleared, and the Apps 3801088 and 3866624 stay registered.
- End sweep 20:06:33Z: no shard container running.
- Cost: the row $0.11 on graft-m4a (19:04Z to 20:07Z) and $0.00 on the runner account's `graft-agents-m4a-` scripts;
  graft-m4a's UTC day $18.23 [measured, `acctcost.py`].

## Run r2: pass

Run r2 from main `6dc3aed7`, which carries A53's fix (DECISIONS "`controller-default`", ruling (c)): while a
`ControllerRoll`'s split is deployed, `graft-agents-front` carries N as `CONTROLLER_DEFAULT`, and an isolate whose view
does not show the roll pins it. Same driver, same blackhole, same stops; logs in `tests/am3/raw/lf7-r2/`. Start sweep
21:59Z; **done line 22:49:09Z**. **Pass.** Times below are Cloudflare's; the orb's clock ran 8.6 s behind it, and the
sampler's rows are compared on the orb's own clock.

| Line | Result |
| --- | --- |
| Pointer with the API blocked | the `ControllerPointer` to c1 posted by hand with the agents deploy token invalid (probe 400): 201 at entry 486, 1.6 s from post to answer; the intent went to `Undoing` [measured] |
| Pins N within `POINTER_TTL` | the first webhook pinned to c1 `d8b368a1` came 125 ms after the 201; 5 unpinned ones in the next 2.2 s also ran on c1; from then on every webhook carried the pin [measured, `hooks.jsonl.gz`] |
| Webhooks flow during the 10-minute blackhole | 36,404 of 36,406 answered 202 (2 client `fetch failed`), p50 28 ms, p99 63 ms, p99.9 1,086 ms, max 4,462 ms. 48 took 1.0 to 1.3 s: new isolates waiting their bounded first read (`VIEW_CALL_MS`, 1 s), then forwarding pinned. Two took 4.3 and 4.5 s, 389 s in, pinned to c1; their cause is not known [unverified] |
| **No webhook reaches N+1 after the pin** | **0 of 50,217 reached c2 (`8996a56e`, `test-bad`)** from the pointer's 201 to the end of the run, 36,406 of them during the blackhole. r1 sent 313 there in the same window [measured] |
| The return | after `unblock`, the agents `graft-ops` deployed c1 100 / c2 0 at 22:46:48Z, removed `CONTROLLER_DEFAULT` (front version `1d05fc0e`, 22:46:50Z), and committed `IntentStep` and `IntentDone` (487, 488); count 489, no intent current, `rolled.Controller` c1 [measured, `final-view.json`] |

**The default, hosted.**
- **Set before the split:** at the roll's first step the agents `graft-ops` set `CONTROLLER_DEFAULT` to c1. The settings PATCH deployed front version `87adfbaa` at 22:31:48.96Z, then the 1%/99% split at 22:31:49.89Z.
- **The secret survived:** the PATCH re-sent `FRONT_TOKEN` as the read returns it, with no value, and the token still worked. `/_agents_front/status` with it answered 200 after the set (showing the default) and after the clear.
- **The canary still ran:** before the pointer, the 1% split sent 50 webhooks to c2 (all 500), from isolates whose verified view showed the roll.
- **Cleared after:** once the default was removed, webhooks went unpinned to c1, which then served 100% [measured].

**Setup, as r1 except:**
- **No edge day.** `e9197bf3` was kept: main's edge changes since `65c10004` are ssh meta and the `/_bed/feed` hold, and the row reads neither.
- **A54's probe** ran in both accounts (`a54.log`). A fresh Workers-Scripts-Write-only token got 10021 on a version upload of a script with a Secrets Store binding, and each account's own token uploaded it.
- **`agents init`** drew a new executor, `400908f0…`. Re-registering Apps 3801088 and 3866624 committed all 8 commands with the new keys. `Keys` generation 6 is at entry 478.
- **Scripts:** controller c0 `2ebf755e`, front `ff855802`, agents `graft-ops` `6f220757`.
- **The agents `graft-ops` cron** fired only after a third schedule reset (empty, yearly, then the minute cron, R52); two resets were not enough this time.
- **Releases:** c1 `3c7fe311…` and c2 `94260ec8…` uploaded at 479 and 480. N is c1: rolled (481), aborted to itself with the token blocked (482), then c1 `d8b368a1` at 100% and `IntentDone` (484).
- **The row:**
  - `release roll c2` committed at 485;
  - c2 went to 1% at 22:31:49Z;
  - block, then the pointer at 22:33:34Z;
  - the fault plan ran from 22:34:51Z to 22:45:33Z;
  - unblock at 22:45:56Z.
- **The sampler** sent 60,442 signed `issues` deliveries with 2 clients, from 22:30:14Z to 22:47:56Z.

**Teardown, state left, cost.**
- **The runner account:** the agents cron is off. The three `graft-agents-m4a-` scripts, their five store secrets, the deploy token, and the bucket with its 12 objects are deleted, and the row's 0600 files are shredded. Only `graft-runners` is left.
- **graft-m4a:**
  - edge `e9197bf3` and `graft-ops` `ac4cd9a5` (schedules `[]`) are as before;
  - the `Release` group is at count 489, no intent current, `Keys` generation 6, whose `agents_executor` seed is deleted;
  - the fault plan is cleared.
- **End sweep:** no shard container running.
- **Cost:** the row $0.08 on graft-m4a (21:55Z to 22:53Z) and $0.00 on the runner account; graft-m4a's UTC day is $18.40 [measured, `acctcost.py`].
