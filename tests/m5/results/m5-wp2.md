# M5-WP2: `graft-front`, pin-only pointers, and gradual edge deploys (2026-11-14)

BF-43 and BF-48 pass. **BF-8 fails one clause.** The `EdgeRoll` that drops an undo's version was refused, and the pointer outside the deployment alarmed within 465 ms with no silent mismatch. No request blocked on the blackholed group. But "requests keep the last pointers" held only for `graft-front` isolates that started before the blackhole. The 126 isolates that Workers started during it held no view, and they served the deployment split unpinned. This needs an operator decision (`decisions/OPEN-QUESTIONS.md`, R20). 23 of 23 plants were caught. Hosted cost for the window 14:30Z to 18:10Z was $1.76, of which $1.23 was graft-m3c's idle shard containers.

BF-43's first run failed and found a real bug. An idle `Release` group's followers apply a commit only when the next entry's round carries it. Isolates that read a follower therefore stayed unpinned for 59 s after the undo, until the next entry committed. The fix: a view refresh reads on until a member that names itself leader answers (`b52fdd78`). BF-43 run 2 and BF-48 ran on that code after site day 3.

## What was built

- **`crates/release/src/view.rs`:**
  - `Replica` is the isolate's replica of the group. It replays committed entries, read by position, through the group's own `check` and `apply` against the isolate's anchor. It never moves backward and refuses a gap.
  - `Freshness` decides when a request waits. A refresh runs beside requests at POINTER_TTL − 1 s. A request waits at most 1 s, and only when the isolate is cold or idle and the last refresh did not fail.
  - Uncommitted `StaleMode`: `verify_stale` and `offer_pending`, with a flag honored until min(`until`, earliest arrival + STALE_MAX).
  - Also `covers`, the release endpoint's signer `precheck`, `mismatch`, and the override header.
- **`crates/release` group:**
  - `roll_steps` (`PastLastStep`).
  - A `FrontRoll` with an undo is refused.
  - A pointer undo moves `deployed[script]` to its pin, so the next roll keeps the pin beside it.
- **`crates/lookup/src/view.rs`:** the per-isolate view.
  - It reads `/view` from `Release` members, the last-named leader first, until a member that leads answers.
  - It feeds `graft-front`, the edge's `OpsPointer` override and release endpoint, and every site member's stale check.
- **Site:**
  - The `Release` family answers `/view` (entries with their times, plus `pending` rows) and `/pending`.
  - While a flag covers a member's chain, `/read` answers from `stale()` with `"stale": {"until"}`, and `/commit*` answers `{"error": {"stale": {"until"}}}`.
  - Test faults: `SiteFault.chains` and `group_only`, and `GRAFTYARD_STALE_MAX_MS`.
- **Edge:**
  - The release endpoint prechecks signers against the view.
  - A `StaleMode` commit is bounded at 10 s, then goes to every member's `/pending` and answers 202.
  - The edge reports its version (`x-graftyard-edge-version`) only to requests carrying `x-graftyard-front`.
  - The `OpsPointer` override applies on the `OPS` binding.
- **`workers/front` (`graftyard-front`):**
  - Forwards every request over the `EDGE` `fetch()` binding, adding the version override while pinned.
  - Alarms (`console_error` "ALARM graft-front…") and counts every pinned request that ran another version or none.
  - `GET /_front/status` behind `FRONT_TOKEN`.
  - Deployed by `workers/front/deploy.mjs`.
- **CLI:** `StaleMode` specs. `tests/m1/integ/deploy-edge.mjs` gained `--upload-only` and `OPS_SCRIPT`.

## Gates

| Row | Pass line | Result |
| --- | --- | --- |
| BF-8 | Requests keep the last pointers; the mismatch alarms within `POINTER_TTL` and the `EdgeRoll` that dropped it was refused | **Fail on one clause.** (A) Every `Release` member was blackholed (calls hang 10 s, then fail) for 601 s, with the edge pinned to N under a split of N@99 and N+2@1. All 168,705 front requests answered 200: p50 23 ms, p99 73 ms, max 1,451 ms. 1,110 of 1,110 repository reads answered 200. No request blocked on the group. The 18 isolates that started before the blackhole kept the pin on 15,451 of 15,451 requests, all on N. The 126 that started during it began with no view: 122,791 of their requests went out unpinned, and 1,214 of those ran N+2. **This misses "requests keep the last pointers".** (B) After the blackhole, an `EdgeRoll` to N+1 whose undo names N+2, which its deployment drops, was refused 422 `DropsUndoVersion`. Then an accepted `EdgeRoll` ran at 1%, a deploy dropped N (N+1@100), and the roll's undo set `EdgePointer` to N, outside the deployment. The first alarm came 465 ms after the undo's 201. 21,310 of the 21,886 requests until N returned alarmed. 617 requests in that window did not alarm. All of them carried no pin, the last at +3.7 s, before their isolates' views refreshed. There were 0 silent pinned mismatches, and 198 ALARM lines in the front's log tail |
| BF-43 | Every request pins to N within `POINTER_TTL`; the next `EdgeRoll` sets `None` | **Pass (run 2).** Under the 1% `EdgeRoll`, 250 of 40,518 requests (0.62%) ran N+1. The undo was posted from a shell whose Cloudflare token was invalid: `wrangler deployments list` failed with an API error in that shell. The undo committed 201, and the front read it. The last request off N started at +2.2 s and the last unpinned one at +3.2 s. From +5 s to the next roll, 0 of 23,869 requests were off N or unpinned. During the 64 s with the API blocked, 17,641 of 17,648 requests ran N; the 7 others were the ones before +2.2 s. After the next `EdgeRoll`'s 201 (to N+2, undo to N), the last pinned request was at +3.3 s, with 0 pinned after +5 s. 173 of 16,232 requests then ran N+2 under its 1% split. 0 requests failed and 0 alarmed. **Run 1 failed:** 2,272 requests after +5 s were unpinned, the last unpinned at +53 s and the last off N at +57.9 s. The cause was follower reads of an idle group (above), fixed in `b52fdd78` |
| BF-48 | Scoped groups serve `stale()` within `POINTER_TTL`; the flag expires at `until`; no flag past `STALE_MAX` | **Pass (run 3).** The `Release` group's member-to-member Raft calls were blackholed (`group_only`), so it could neither commit nor lead. Two customer-signed `StaleMode`s on repository 65537 each answered 202 after the 10 s commit bound plus the `/pending` writes (21.3 s and 21.7 s end to end). Flag A (`until` +100 s): all three members served `stale()` within 0.5 to 3.1 s, below 5 s. The last stale answer was at +76.1 s and the first non-stale read was sent at +76.4 s. The members' `until` falls at +76.1 s on the orb's clock: the orb ran 2.4 to 2.9 s behind the Workers clock, measured against `/_front/status`, and its 21 s round trip used up the rest. Flag B (`until` +900 s, STALE_MAX lowered to 120 s on this install): `stale()` arrived within 0.8 to 3.5 s. The last stale answer was at +119.7 s and the first non-stale read at +120.0 s, so the flag ended at its earliest arrival plus STALE_MAX, not at `until`. A write probe at +15 s under each flag got `{"error": {"stale": {"until"}}}` from all three members. The control repository (65538) served 0 stale reads out of 654. Run 2's write probe ran before the members' views held the flag, so it saw none; run 3 probes at +15 s |

## Plants

`tests/m5/plant_wp2.py` breaks one rule at a time in `crates/release/src/{group,view}.rs` and runs the crate's tests. 23 of 23 plants were caught (`raw/plant_wp2.log`). They cover:

- replica order, gaps, and the group check;
- the pin kept by the next roll, and the deployed release after an undo;
- `ROLL_STEPS` and the `FrontRoll` undo;
- the alarm on a missing version, and the override header's format;
- requests that wait on a failing or hung refresh, and the refresh time;
- the pending flag's key, `until`, parent, install, STALE_MAX bound, and arrival;
- committed flags, and `Repo` and `Owner` scope;
- the endpoint precheck's signer and signature checks.

## Hosted runs

Install graft-m3c (`tests/am1/results/install-m3c.md`, "Site and edge days `m5-wp2`"). The edge and graft-front under test were scratch scripts, deleted afterwards. `graft-m3c-edge` was not touched.

- **Site day 3** (16:48Z to 17:00Z) from `b52fdd78`: sites a/b/c `64d8afcf` / `9933fafc` / `9379827a`, test-faults, `GRAFTYARD_STALE_MAX_MS=120000`.
- **`graft-m3c-m5-edge`:** a test-faults edge with m3c's anchor, `--ops graft-m3c-ops`, and `HOST` set to the front's host. N `3ba676ce` was deployed. N+1 `d42016e1` and N+2 `0ea53421` are the same build with another `EDGE_TAG`, uploaded with `--upload-only`. Run 1 used the day-2 versions `a4d1ec8e` / `ef096f81` / `c6969335`.
- **`graft-m3c-front`:** `71c00065` (run 1: `9680a8dd`), with the `EDGE` binding to the edge above.
- **Release group:**
  - The rows used `Keys` generation 1. Its executor and drain keys are held in `~/.graft-m3c/m5wp2`, because m3c's genesis keys were discarded.
  - Each run's three `ReleaseUploaded` and a roll to N went through graft-front's release endpoint. The endpoint's precheck accepted the new executor key after one refresh.
  - The group ended with nothing current and every pointer `None` (31 entries).
- **Who deployed the splits:** M5-WP3 builds `graft-ops` executing intents. Until then, the driver (`tests/m5/wp2-hosted.sh`) plays the executor. It deploys each split with `wrangler versions deploy` from the orb, which is the ops path, and commits the executor-signed `IntentStep` and `IntentDone`. No request path calls the Cloudflare API: the front and edge hold no API token (`decisions/DECISIONS.md`, "M5-WP2", "The run stands in for `graft-ops`'s executor").
- **Raw data:**
  - The samplers (`tests/m5/wp2-sample.mjs`) sent about 300 front requests per second at `/api/v3`, plus one stream of repository reads. Each request recorded the version that ran, its pin, its alarm, and the front isolate's view.
  - BF-48 read every member of `repo:65537` every 500 ms through the tools edge.
  - Raw rows are in `raw/wp2-hosted/run{1,2,3}`, and `tests/m5/wp2-report.py` prints the numbers above (`run2 ` with no flags, `run3 --offset 2600`, `run1 --n a4d1ec8e-…`).
- **Teardown:** faults were cleared, then `graft-m3c-front` and `graft-m3c-m5-edge` were deleted, the `tests/m0/.m0-token` symlink was removed, and the front token file was deleted. The sites keep `GRAFTYARD_STALE_MAX_MS` until their next site day. The 17:52Z orb executor restart killed run 2's BF-48 driver mid-row. Its fault was cleared by hand at 17:52:49Z, and BF-48 was rerun as run 3.
- **Cost:** `INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-14T14:30:00Z 2026-11-14T18:10:00Z` gives $1.76: 903,136 Workers requests, 253,069 Durable Object requests, and $1.23 of shard containers.

## What BF-8 needs

An isolate learns the pins only from a `Release` member. One that starts while no member answers has nothing to keep. The design says each isolate caches the view and that no request waits on the group, and the code does both. Neither gives a new isolate the last pointers. Options for the operator:

1. **A copy of the last pins outside the group,** readable by a cold isolate: for example a site `Registry` row each member writes on apply. It is one more read on a cold start, served by objects that are not the blackholed members. A fault that takes the registry too still leaves the gap.
2. **Hold cold requests until a member answers.** This breaks "no request waits on the group", which BF-8's fail column names.
3. **Accept the gap and narrow the row:** "requests on isolates that held a view keep the last pointers". This would need an operator decision, because the gate is written as is.

I recommend option 1. It keeps both invariants and needs no new failure domain (the registry objects are already on every site). It is not built.
