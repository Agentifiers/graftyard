# M5-WP2: `graft-front`, pin-only pointers, and gradual edge deploys (2026-11-14)

**BF-8, BF-43 and BF-48 pass.** BF-8 failed in run 2 on one clause, "requests keep the last pointers". `graft-front` isolates that started during the blackhole held no view, so they served the deployment split unpinned (`decisions/OPEN-QUESTIONS.md`, R20). The operator chose option 1: a copy of the group's entries outside the group, written by every member into an existing per-site object. That was built, and BF-8 run 4, as written with the same 601 s blackhole, passed after site day 4. All 200,416 requests during the blackhole carried the pin and ran N: 33,642 of 33,642 on the 37 isolates started before it, and 166,774 of 166,774 on the 240 started during it. The `EdgeRoll` that drops an undo's version was refused, and the pointer outside the deployment alarmed after 331 ms with no silent mismatch. BF-43 was rerun on the final code and passed. 25 of 25 plants were caught. Hosted cost was $3.05: $1.76 from 14:30Z to 18:10Z, and $1.29 from 18:10Z to 21:32Z.

BF-43's first run failed and found a real bug. An idle `Release` group's followers apply a commit only when the next entry's round carries it. Isolates that read a follower therefore stayed unpinned for 59 s after the undo, until the next entry committed. The fix: a view refresh reads on until a member that names itself leader answers (`b52fdd78`). BF-43 run 2 and BF-48 ran on that code after site day 3.

## The copy outside the group (R20)

- **Where the copy lives.** Each site's copy is in the `Release` chain's own `MemberRegistry` object on that site, `registry#36` of the existing `Registry` class, at the route `/release_copy` (`workers/site/src/registry.rs`). This is the object that already holds the chain's registry row. No new class or store was added. The registry is not one of the member classes the site fault hits, and a blackholed group leaves it up.
- **The row.** The row holds the group's committed entries exactly as signed, each with its entry time, by position. Pins derive from many entries (a later intent clears one), so a single pointer row could not prove the current pin; the history can. It only grows: a write adds the positions it lacks, and a write past a gap changes nothing.
- **Who writes it.** Every member writes it on the alarm right after it applies entries, and again on restore. The write is bounded at 1 s and sends only what the copy lacks (`workers/site/src/release.rs`, `copy`). Site day 4's restores filled all three copies before the group's next entry.
- **How an isolate takes it.** It goes through `Replica::adopt`, with every entry the isolate lacks checked by the group's `check` against the isolate's own anchor, all or none. An unsigned, forged, or other-install copy is ignored whole. A copy no longer than the view held adds nothing, so an older copy cannot roll an isolate back. Once a member answers, the group's view replaces the copy through the normal `extend` (`a_cold_isolate_takes_a_sites_copy_whole_and_an_older_copy_never_rolls_its_view_back`; plants 2 and 3).
- **The bounded read.** A refresh that requests wait for reads every site's copy at once, alongside the group. That happens on a cold isolate, or on one idle past POINTER_TTL. The requests go out when the group answers or when every copy has answered: at most `COPY_MS` = 500 ms from arrival for a cold isolate, and 1 s for an idle one. They go out unpinned only if nothing came, as before the copy existed. The bound sits beside POINTER_TTL in DESIGN. It rests on M0's idle cross-region Durable Object round trip (111 to 135 ms p50, p99 within 10 ms) and on run 4's reads (81 ms p50, 139 ms max over 277 isolates).

## BF-8 run 3: two holes, fixed before run 4

Run 3 ran on the first build of the copy (site day 4, front `362f703b`, raw `run4`). Every isolate started during the blackhole took a copy, and 151,558 of 151,717 of their requests were pinned. Two holes remained:

- **The first copy released the wait.** A follower site's copy of an idle group holds one entry fewer than the leader's, the same lag as BF-43 run 1. Requests released on the first copy to answer sometimes carried the copy from before the pin: 159 requests on 38 isolates, each within 1.4 s of the isolate's start.
- **Idle isolates never read a copy.** Six isolates had sat idle from BF-43 to BF-8. Each held a view from before the pin, and its refreshes failed through the whole blackhole: 4,059 requests unpinned.

The fix (`393f8950`): every waited refresh reads the copies, cold or idle, and waiting requests go out once every copy has answered, so they carry the longest. Run 3's driver also exited on one failed fault-off call, after the plan had run out at its `until`. The fault was cleared by hand at 21:00:34Z, and run 3's part B was closed unmeasured. `fault()` now retries.

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
| BF-8 | Requests keep the last pointers; the mismatch alarms within `POINTER_TTL` and the `EdgeRoll` that dropped it was refused | **Pass (run 4).** (A) Every `Release` member was blackholed (calls hang 10 s, then fail) for 601 s, with the edge pinned to N under a split of N@99 and N+2@1. All 200,416 requests through the front answered 200 and carried the pin, and all ran N. Latency was p50 22 ms, p99 56 ms, max 1,060 ms. 1,060 of 1,060 repository reads answered 200. No request blocked on the group. By isolate start, isolates started before the blackhole served 33,642 requests on 37 isolates, all pinned. Those started during it served 166,774 on 240, all pinned. Isolates that started in each minute 0 to 9 of the blackhole pinned 100%: 21,376, 18,542, 15,952, 17,459, 20,651, 21,217, 8,648, 15,497, 15,655, and 11,777 requests. All 277 took a site's copy, from sites 0 and 2, read in 81 ms p50 and 139 ms max. A cold isolate's first-600-ms requests ran 28 ms p50, 508 ms p99, 674 ms max. (B) After the blackhole, an `EdgeRoll` to N+1 whose undo names N+2, which its deployment drops, was refused 422 `DropsUndoVersion`. An accepted `EdgeRoll` then ran at 1%, a deploy dropped N (N+1@100), and the roll's undo set `EdgePointer` to N, outside the deployment. The first alarm came 331 ms after the undo's 201. 19,616 of the 20,339 requests until N returned alarmed. The other 723 did not. 675 of them carried no pin and ran N+1, which is the split, not a pinned mismatch: 669 were from isolates whose views had not refreshed yet, the last at +4.0 s, and 6 were from one isolate in the 1 s its view moved through the copy at +52.5 s. The other 48 were pinned to N and ran N, after N's redeploy at +60 s. 0 silent pinned mismatches. 195 ALARM lines were in the front's log tail. One request at 0.26 s after the N@99/N+1@1 deploy answered 500 from the edge with no front headers; it is outside the row. **Run 2 failed** on isolates started during the blackhole: 30,463 of their 153,254 requests were pinned, against 15,451 of 15,451 on the 18 started before. **Run 3**, on the first build of the copy, had the two holes above |
| BF-43 | Every request pins to N within `POINTER_TTL`; the next `EdgeRoll` sets `None` | **Pass (runs 2 and 4).** Run 4, on the final code with front `04cbdcac`: 12 requests off N after the undo, the last at +3.3 s, and 0 of 26,874 off N or unpinned from +5 s to the next roll. With the API blocked (62 s), 20,093 of 20,105 requests ran N. After the next `EdgeRoll`, the last pinned request was at +0.7 s, and 216 of 19,618 then ran N+2 under its 1% split. 0 alarms. Run 2: Under the 1% `EdgeRoll`, 250 of 40,518 requests (0.62%) ran N+1. The undo was posted from a shell whose Cloudflare token was invalid: `wrangler deployments list` failed with an API error in that shell. The undo committed 201, and the front read it. The last request off N started at +2.2 s and the last unpinned one at +3.2 s. From +5 s to the next roll, 0 of 23,869 requests were off N or unpinned. During the 64 s with the API blocked, 17,641 of 17,648 requests ran N; the 7 others were the ones before +2.2 s. After the next `EdgeRoll`'s 201 (to N+2, undo to N), the last pinned request was at +3.3 s, with 0 pinned after +5 s. 173 of 16,232 requests then ran N+2 under its 1% split. 0 requests failed and 0 alarmed. **Run 1 failed:** 2,272 requests after +5 s were unpinned, the last unpinned at +53 s and the last off N at +57.9 s. The cause was follower reads of an idle group (above), fixed in `b52fdd78` |
| BF-48 | Scoped groups serve `stale()` within `POINTER_TTL`; the flag expires at `until`; no flag past `STALE_MAX` | **Pass (run 3).** The `Release` group's member-to-member Raft calls were blackholed (`group_only`), so it could neither commit nor lead. Two customer-signed `StaleMode`s on repository 65537 each answered 202 after the 10 s commit bound plus the `/pending` writes (21.3 s and 21.7 s end to end). Flag A (`until` +100 s): all three members served `stale()` within 0.5 to 3.1 s, below 5 s. The last stale answer was at +76.1 s and the first non-stale read was sent at +76.4 s. The members' `until` falls at +76.1 s on the orb's clock: the orb ran 2.4 to 2.9 s behind the Workers clock, measured against `/_front/status`, and its 21 s round trip used up the rest. Flag B (`until` +900 s, STALE_MAX lowered to 120 s on this install): `stale()` arrived within 0.8 to 3.5 s. The last stale answer was at +119.7 s and the first non-stale read at +120.0 s, so the flag ended at its earliest arrival plus STALE_MAX, not at `until`. A write probe at +15 s under each flag got `{"error": {"stale": {"until"}}}` from all three members. The control repository (65538) served 0 stale reads out of 654. Run 2's write probe ran before the members' views held the flag, so it saw none; run 3 probes at +15 s |

## Plants

`tests/m5/plant_wp2.py` breaks one rule at a time in `crates/release/src/{group,view}.rs` and runs the crate's tests. 25 of 25 plants were caught (`raw/plant_wp2.log`). They cover:

- replica order, gaps, and the group check;
- a site copy taken in part, past a bad entry, and an older copy that rolls a newer view back;
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
- **Site day 4** (20:24Z to 20:34Z) from `4417ceaa`, the copy: sites a/b/c `779d2b47` / `b8998062` / `2f04e1af`, the same flags. Runs 3 and 4 of BF-8 ran on it; `393f8950` changed only `graft-front`'s view code (`crates/lookup`), so it needed no site day.
- **`graft-m3c-m5-edge`:** a test-faults edge with m3c's anchor, `--ops graft-m3c-ops`, and `HOST` set to the front's host. N `3ba676ce` was deployed. N+1 `d42016e1` and N+2 `0ea53421` are the same build with another `EDGE_TAG`, uploaded with `--upload-only`. Run 1 used the day-2 versions `a4d1ec8e` / `ef096f81` / `c6969335`. After site day 4 the edge was made again: N `52f05950`, N+1 `5cfcecc3`, N+2 `a02b9c73`.
- **`graft-m3c-front`:** `71c00065` (run 1: `9680a8dd`), then `362f703b` (BF-8 run 3) and `04cbdcac` (BF-43 and BF-8 run 4, from `393f8950`), with the `EDGE` binding to the edge above.
- **Release group:**
  - The rows used `Keys` generation 1. Its executor and drain keys are held in `~/.graft-m3c/m5wp2`, because m3c's genesis keys were discarded.
  - Each run's three `ReleaseUploaded` and a roll to N went through graft-front's release endpoint. The endpoint's precheck accepted the new executor key after one refresh.
  - The group ended with nothing current and every pointer `None`. It held 31 entries after run 2 and 75 after run 4. Runs 3 and 4 used `RUN=4` and `RUN=5`, so their intent ids and release records are fresh.
- **Who deployed the splits:** M5-WP3 builds `graft-ops` executing intents. Until then, the driver (`tests/m5/wp2-hosted.sh`) plays the executor. It deploys each split with `wrangler versions deploy` from the orb, which is the ops path, and commits the executor-signed `IntentStep` and `IntentDone`. No request path calls the Cloudflare API: the front and edge hold no API token (`decisions/DECISIONS.md`, "M5-WP2", "The run stands in for `graft-ops`'s executor").
- **Raw data:**
  - The samplers (`tests/m5/wp2-sample.mjs`) sent about 300 front requests per second at `/api/v3`, plus one stream of repository reads. Each request recorded the version that ran, its pin, its alarm, and the front isolate's view.
  - BF-48 read every member of `repo:65537` every 500 ms through the tools edge.
  - Raw rows are in `raw/wp2-hosted/run{1,2,3,4,5}`. `run4` is BF-8 run 3, with its unmeasured part B in `run4/close`, and `run5` is BF-43 run 4 and BF-8 run 4. `tests/m5/wp2-report.py` prints the numbers above: `run2` with no flags, `run3 --offset 2600`, `run1 --n a4d1ec8e-…`, and `run4` and `run5 --n 52f05950-b49c-42b9-a4d2-7d0aafb6c894`. The printouts are in `raw/wp2-hosted/run*.txt`. Run 4's copies and pins per isolate come from the `copy=<site>@<ms>` field of each request's view.
- **Teardown:** faults were cleared, then `graft-m3c-front` and `graft-m3c-m5-edge` were deleted, the `tests/m0/.m0-token` symlink was removed, and the front token file was deleted. This was done after day 3 and again at 21:31Z after BF-8 run 4. The orb's CLI reached the front only over IPv4. Its `ureq` tried the AAAA record first, and the orb has no IPv6 route. A `precedence ::ffff:0:0/96 100` line in `/etc/gai.conf` was added for runs 3 and 4 and removed after. The sites keep `GRAFTYARD_STALE_MAX_MS` until their next site day. The 17:52Z orb executor restart killed run 2's BF-48 driver mid-row. Its fault was cleared by hand at 17:52:49Z, and BF-48 was rerun as run 3.
- **Cost:** `INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-14T14:30:00Z 2026-11-14T18:10:00Z` gives $1.76: 903,136 Workers requests, 253,069 Durable Object requests, and $1.23 of shard containers. From 18:10Z to 21:32Z it gives $1.29: 245,878 Durable Object requests, 59 CPU-s of Workers, and $1.14 of shard containers. Its Workers request count, 40, misses the scratch scripts, whose requests the analytics list under `__unknown__`; 726,047 sampler requests at $0.30 per million add about $0.22 at most. M5-WP2's total is $3.05, or $3.27 with that.

## What BF-8 needed (R20, before run 3)

An isolate learned the pins only from a `Release` member, so one that started while no member answered had nothing to keep. Run 2 listed three options: a copy of the last pins outside the group, holding cold requests until a member answers, or narrowing the row. The operator chose the first and rejected the other two: holding requests breaks "no request waits on the group", and narrowing the row is a substitution. It is built as "The copy outside the group" above. A fault class that takes every site's registry along with the members would still leave the gap (`decisions/DECISIONS.md`, the copy's Flip).
