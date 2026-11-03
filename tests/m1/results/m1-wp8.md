# M1-WP8: the Milestone 1 gate (BM-1)

BM-1 (`build/BUILD-PLAN.md`, M1-WP8): "The `git` conformance suite and `git clone` / `fetch` / `push --atomic` from unmodified git 2.4x pass with one site's member class failing mid-run. Linearizability rerun at 24 hours." The suite is SHARED's `git-http` and `git-protocol-v2` oracle rows on git 2.39 and 2.51 (`tests/m1/wp6/clients.sh`), plus git's own remote-capable smart-HTTP tests from `t/` (`tests/m1/wp6/conformance/run.sh`).

Everything below ran on 2026-11-02 against `graft-m1-edge` on the real groups of the test install `graft-m1-site-a/b/c` (enam, weur, apac; site ids 0, 1, 2), built with `test-faults`. The 24-hour run on `graft-m1-lin-*` was not touched.

| Half | Status |
| --- | --- |
| Git conformance and clients with one site's member class failing mid-run | See "BM-1 results" |
| Linearizability rerun at 24 hours | **Not passed** (2026-11-02; see "The 24-hour half" at the end). It was pending until 2026-11-02T18:27Z. Run `lin24` on `graft-m1-lin-a/b/c` (`m1-wp2.md`, "The 24-hour linearizability rerun"). Its finish service writes `tests/m1/lin/raw/lin24.report.json` after the end |

## Fixes before the run

### 1. A member reply the edge cannot read, and the replay of a refused push (t5541 #20)

`m1-wp6.md`, "t5541 #20", found two defects in one failure. The deployed edge predated `RejectReason::NameConflict` and `AtomicTransaction`, so it could not decode the member's reply and took the lost-reply path. The member then answered `/outcome_by_request` from the push's `request` row, and `Push::replay` reported every ref the stored events had not applied as `Stale`. So git printed `fetch first`.

- **The replay.** The defect was real. A `request` row holds the entry's events, and a push whose refs were all refused stored no event at all. A push that applied some refs stored only the applied updates. `Push::replay` could not recover `NameConflict`, `AtomicTransaction`, `Atomic`, or `Hidden`, so it reported each of those as `Stale`.
  - Fix (`crates/git/src/repo.rs`): a new event, `RepoEvent::Refused { push, refs: Vec<(RefName, RejectReason)> }`. `decide` emits it beside `Refs` whenever any ref is refused. It changes no ref: `RepoFold::apply`, `RepoScratch::apply`, and `check` skip it, and `packs_of` names no pack for it.
  - `replay` answers each ref from the stored events: `Ok` if a `Refs` applied it, else the reason the `Refused` stored. A ref that appears in neither (a row written before this change) still reads `Stale`.
  - Encoding: `Refused` is event tag 1 in `Record::encode`, after the push id. Each ref is written as its name and a fixed reason byte (`reason_tag`: Stale 0, NonFastForward 1, Hidden 2, TooManyRefs 3, Atomic 4, NameConflict 5, AtomicTransaction 6), so reordering the enum cannot change a digest. `Refs` keeps tag 0 and its bytes are unchanged, so every existing entry's digest is the same.
  - `RepoEvent::updates()` gives a consumer the applied updates (none for `Refused`). The site's `Repo` family (`store_fold`, `entry_json`) and the stand-in use it.
  - This is consistent with SHARED: "A refusal inside `Push::decide` still commits an entry with the push's `request` row, so a re-send is answered from it". It is not in SHARED's event list, though (see "Design problems found").
  - Test: `a_replay_reports_each_refused_refs_own_reason` (`crates/git/tests/push.rs`). It commits four pushes and checks each one's `outcome_by_request` and same-id re-send against the first answer, including the report-status lines: the t5541 #20 shape (`AtomicTransaction` on both refs, nothing applied), a per-ref `NameConflict` beside an `Ok`, an atomic push with one stale ref (`Atomic`, `Stale`), and a `Hidden` ref beside a stale one. On the old code it fails at the first: `left: [Rejected(Stale), Rejected(Stale)] right: [Rejected(AtomicTransaction), Rejected(AtomicTransaction)]`.
- **The edge.** It is rebuilt on the current `crates/git`, and the deployed wasm contains `NameConflict`.
  - `workers/edge`'s `commit` already mapped a `done` it could not decode as `PushOutcome`, and an `error` it could not decode as `CallErrorWire`, to `Unsupported` (503 with `Retry-After`). It did that through `unreadable_reply()`.
  - One path was left. `graftyard_lookup::commit_hedged` treated a 200 answer that had no `done`, `admitted`, or `error` as no outcome, and sent the command again under the same request id. The next send could then be answered from the row. Now a 200 answer to `/commit` must be `done` or `admitted`, and a 200 answer to `/outcome` must be `done` or `pending` (`readable`). Any other answer returns `Err(Ok({"unsupported": {"family": 0, "version": 0}}))`, which the edge renders as 503 with `Retry-After`. An `error` answer to `/outcome` is returned too, not dropped.
  - Test: `only_the_commit_shapes_are_readable` (`crates/lookup`). It covers a body that is not JSON (read as `null`), `{}`, `{"done": null}`, a newer shape `{"decided": ..}`, and an array.
- **On Cloudflare.** t5541 on git 2.56.0, no fault, after the redeploy (`raw/bm1-20261102/t5541-nofault/`): 39 run, 30 pass, 3 fail, 6 n/a, 3 skip. **#20 passes**: `ok 20 - push --atomic fails on server-side errors`. The no-fault baseline had 29 passes, and #20 was the difference. The 3 fails (#29, #35, #36) are the same as the baseline's, with the same causes.

### 2. The sweeper's Cron Trigger

- `workers/site/src/sweep.rs` is the site's `#[event(scheduled)]` handler. It calls `sweep_all(&[(SiteId, &R2)…], &RequestSource, now, CronBudget::SITE_CRON, stored_next, |n| home_site(n) == me)` over every site's `BUCKET_<n>`.
- **`RequestSource`** is the real Raft `request` table.
  - It calls `/read` with `{"q": {"request": "<ulid>"}}` on the chain that the pack's metadata names. The chain is found by the lookup rule from the repository's `RepoOwner` row and the owner's record.
  - The leader answers from `GroupFold::request` inside the same `fresh` resolution as any read (`workers/site/src/host.rs`). The answer is the row's index and the stored packs its events name (`Family::row_packs`; for `Repo`, `packs_of`), turned into `Marker`s.
  - No row reads `null`, which is `NoRow`. A `NotLeader`, a busy read, or an unexpected shape is an error, which is `Unavailable`. So a lookup that fails can never let the sweeper delete a pack (test `a_row_names_its_stored_packs_as_markers`).
- **`home_site(n)`**: a network id is its root repository's id in M1. The network's home is that repository's owner record's `home`. All of the networks' homes are resolved before `sweep_all` runs, because `mine` is synchronous. A network whose home can't be resolved is left to the next run and listed as `unresolved`.
- **`next`** is stored in this site's own `Registry` object `sweep` (`GET`/`PUT /sweep`), together with the run's report. No new Durable Object class.
- **Schedule.** `*/15 * * * *` [guess]: deletes wait 24 h (`ORPHAN_AFTER`), so the cadence only bounds how late a missing marker or third copy is repaired. `workers/site/deploy.mjs` sets it as `triggers.crons`, and the CLI sets it too (item 3). The cron is part of the site-script version, so on a product install it ships only on a site day. On this test install the redeploy was the new version.
- **On Cloudflare.** The cron fired at 02:45Z on all three sites; each site's report was read through the edge's test route `/_bed/sweep?site=N`.
  - Site 0 is home for every network on this install (every owner record is Tri with home 0). It swept network 196609 (`wp6/big`) and ran 1 action, with 0 failed and 0 alarms.
  - That action was a repaired `Committed` marker. Bucket b's `committed/pack/196609/01M6CNBMK11A7Y6YB1H3B5AEW9` is dated 02:45:26Z. Buckets a and c had had the marker since 22:58Z on 2026-11-01.
  - Sites 1 and 2 swept nothing, since `mine` excluded every network for them, and left `next` null.
  - The pack's request row named it, so the marker came from the row: the M1-WP5 rule "a row whose events name this pack id proves the commit".

### 3. `graftyard sites add` deploys the git cache shard

`cli/src/sites.rs` now deploys what `workers/site/deploy.mjs` deploys:

- the `SHARD_<own id>` binding to the script's own `GitShard` class. No other site's shard class is bound, because Cloudflare attaches the container config to the first binding of the class;
- `containers: [{name: "<script>-gitshard", class_name: "GitShard", images: {git: <--image>}}]` in the upload metadata;
- migration `v2`. A new script gets `steps: [Member, Registry] then [GitShard]`. A script at `v1` gets `old_tag: v1, steps: [GitShard]`. Any other tag is refused;
- `limits.cpu_ms = 300000`;
- the sweeper's cron (`PUT …/schedules`).

After the upload, the CLI finds the `GitShard` namespace id in the version's bindings and creates the Durable Object-managed container application (`POST /containers/applications` with `{name, scheduling_policy: "durable_object", durable_objects: {namespace_id}}`) if `GET /containers/applications/<namespace id>` finds none. This is what wrangler does. An application under another name is an error.

`--image` is a new required flag on `sites add` and `sites add-spare`, and it must be digest-pinned (`@sha256:`). The image is hashed into `GRAFTYARD_BUNDLE`, so a new image is a new version and an unchanged rerun is not.

Unit tests: `a_site_binds_only_its_own_git_shard_and_uploads_its_container` and `a_v1_script_migrates_to_v2_by_adding_only_the_git_shard`. `cargo test -p graftyard`: 20 passed. `tests/m1/integ/throwaway.sh` passes `--image`, drops `--no-shard` for the gen install's edge, and checks each site's shard binding, container application, cron, and migration tag. It ran end to end from 04:00 to 04:04Z and passed, then deleted the install (`m1-wp7.md`, "2026-11-02 (M1-WP8): `sites add` deploys the git cache shard"; `tests/m1/integ/raw/throwaway-20261102.log`).

### 4. Found by the first t/ run on 2.56.0: a follower and a shard that could not catch up

The first `t-256` (04:51 to 05:16Z, stopped, raw output in `raw/bm1-20261102/t-256-first/`) failed tests that pass in the baseline. Every mirror into `wp6/main` (`repo:196608`) answered `remote unpack failed: Error: repo:196608: repo:196608#1#f2: not leader` or `(unpacker error)`, so 11 of t5541's first 11 tests failed, and so did t5551's and t5702's tests on that slot. `wp6/bf16` was unaffected. The fault was not the cause:

- `repo:196608`'s member on site 0 had stopped at index 235 at 04:49Z, before the fault, while the leader on site 1 was at 237. It stayed at 235 for the next 25 minutes, with the fault on and off, through six `home.sh` transfers that were each abandoned (`Busy`) because the target never caught up. Every AppendEntries from the leader to it failed (`failedCalls` rose by about 1 a second with the fault off). The member's own state was sound (`/audit`: no duplicate rows, log 1 to 235 committed, digests equal to the leader's).
- The edge's pack check calls the network's home shard (site 0's `GitShard`), which first catches up from the group: `/read`, then `/entries` through the lookup rule. `/entries` answered all 239 entries, 4.6 MB of JSON with up to 1 MB inline packs in each, in 7.8 s. The lookup's `CALL` is 3.5 s, so every attempt timed out, the lookup moved on to site 2's member, which answered `NotLeader`, and the shard's rehydrate failed with that error. The edge mapped the failed check to `unpack failed`.

Fixes:

- **`workers/site` `/entries` pages by bytes** (`ENTRIES_PAGE_BYTES` = 600,000 stored bytes, at least one entry). The shard already asks again from the next index. After the redeploy, `/_bed/catchup?repo=196608` rehydrated 161 inline packs (3.48 MB) in 16.8 s and answered 200, where it had answered 500 after 27 to 30 s.
- **`crates/raft` caps each AppendEntries by bytes** (`APPEND_BYTES_MAX` = 1.5 MB past its first entry, as well as `CATCHUP_MAX` = 64 entries), on both the in-window path and `Member::catchup`. 64 entries of up to `ENTRY_MAX` each could make a single call of up to 96 MB, which no member answers within `CALL_DEADLINE`, and the leader would resend the same call forever. Test: `a_lagging_follower_catches_up_in_calls_under_the_byte_cap` (`crates/raft/tests/scenarios.rs`, ten 400 kB entries). On the old code it fails with `one AppendEntries carried 4001260 bytes`.
- **What these fixes do not explain.** The stuck follower's missing entries 236 to 239 were 836 bytes in all, so the byte cap does not explain that member. Its calls failed for another reason, which was not found before the redeploy reset it. After the redeploy (05:47Z) it caught up to the leader's 241 within 16 s. If it recurs, look at the leader's `call_peer` errors to that member [unverified].
- **Correction (2026-11-02, evening).** The cause was found later (`m1-f43.md`). The stuck follower's failed calls were not the 836-byte calls for 236 to 239, which were never sent. They were the 1.08 MB resend of entries 172 to 235. The follower answered each in about 2 s, and the leader's host dropped each answer after `CALL_DEADLINE`, so `next` never moved past 172. The host now keeps a late answer for up to 10 s and steps it (`m1-f43.md`, "The fix").

The redeploy is a new site-script version, so it reset every member of the test install, as any site-script change does. `t-256` was then run again from the start (below).

## The fault: one site's member class failing every call

This is B0-03's "simulated region loss" (`tests/m0/src/faults.ts`, rules `">a:"` and `"a>"` at rate 1). The `GRAFTYARD_FAULTS` plan of the 24-hour run is read from a variable once per isolate, so changing it would need a new version, which resets every member. This fault therefore lives in storage:

- **The plan:** `SiteFault { site, from, until }` (`workers/site/src/faults.rs`, `test-faults` only), kept in each site's own `Registry` object `faults` (`GET`/`PUT /site_fault`).
- **Reading it:** a member isolate rereads the plan at most once a second, in the background after its first read, as M0's `FaultControl` did. The read never sits on a measured call.
- **On the faulted site:**
  - Every Raft and edge call to a member answers 503 (`member class failing (injected site fault)`). Only `/status`, `/stats`, `/ping`, `/rtt`, and the harness's read routes still answer, so the run can be observed.
  - Every member-to-member call a member there sends fails before it leaves and is reported with `call_failed`. The intent was that a leader there sees every commit-path call fail and its `Health` window trips `STEP_DOWN` (`CommitErrors`) through the M1-WP3 path. That did not happen, because no command reaches it (design problem 4).
  - The isolate counters (`/isolate`) count `site_fault:in` and `site_fault:out>N`.
- **Control:** the edge's test route `/_bed/fault?site=N[&after_ms=A][&for_ms=F]` or `?off=1` writes the plan to all three sites' objects. `tests/m1/wp8/fault.sh on <site>` and `off` wrap it, and are run.sh's `--fault-cmd` and `--restore-cmd`. `tests/m1/wp8/clients-fault.sh` does the same around clients.sh.
- **Clock:** the plan's `from` is on Cloudflare's clock. The orb's clock read 11.06 s behind the members' (three probes, 301 to 324 ms round trips), so all times below are on member clocks.
- **Leaders:** before each faulted step, `tests/m1/wp8/home.sh 0` transfers every watched group's leader to site 0, so the fault moves all of them. The watched groups are the two slot repositories (`repo:196608` wp6/main and `repo:196610` wp6/bf16) and every system group their requests read: `names:174` (the owner's account), `names:987` (login `wp6`), `names:938` and `names:806` (the slots' `RepoOwner` rows), `authz:7000001`, `tokens:1563` (writer), and `tokens:1153` (read-only). The first two client runs (`clients-239`, `clients-251` below) ran before `home.sh` existed, and most leaders were already on site 1 then.
- **Observer:** `tests/m1/wp8/observe.ts` samples every watched group's `/status` (role, term, leader, member clock) every 2 s, and reads each member's `/stats` (`commitMs`, failed calls, `STEP_DOWN` alarms) at the fault and at the end. `tests/m1/wp8/summarize.py` turns those into the moves and latencies below.
- **The test-faults proof**, extended with the new strings (`tests/m1/lin/grep-faults.sh` now has 34 needles, adding `/site_fault`, `site_fault:`, `injected site fault`, `::site_down`; `tests/m1/integ/grep-edge-faults.sh` 14, adding `/_bed/fault`, `/site_fault`):
  - product site build (`worker-build --release`, no feature): 0 of 34;
  - deployed site test build: 34 of 34;
  - product edge build: 0 of 14;
  - deployed edge test build: 14 of 14.

The sweeper's cron is product code and is in both builds.

## BM-1 results

`tests/m1/wp8/bm1.sh ~/.graft-wp8/bm1 0 120` ran each step below as an orb service: `wp8-bm1` through `t-251`, then `wp8-bm1c` for `t-256` and `t-239` after fix 4 (the script resumes at the first unfinished step). Before each step it cleared the fault, tore down the slots' shard containers (as the baseline's run did), and moved the leaders to site 0. The raw output, with tokens redacted, is in `raw/bm1-20261102/`.

### The results against the no-fault baseline

| Part | git | No fault (`m1-wp6.md`) | Site 0's member class failing | Baseline passes lost |
| --- | --- | --- | --- | --- |
| clients.sh, 29 rows | 2.39.5 | 29 of 29 | 29 of 29 (`clients-239`), 29 of 29 (`clients-239-homed`) | 0 |
| clients.sh, 29 rows | 2.51.0 | 29 of 29 | 29 of 29 (`clients-251`), 29 of 29 (`clients-251-homed`) | 0 |
| t/ suite, 16 scripts, 535 tests | 2.51.0 | 413 pass, 15 fail, 99 n/a, 8 skip | 415 pass, 14 fail, 98 n/a, 8 skip (`t-251`) | 0 |
| t/ suite, 16 scripts, 532 tests | 2.56.0 | 412 pass, 15 fail, 97 n/a, 8 skip | 412 pass, 14 fail, 98 n/a, 8 skip (`t-256`) | 1 (t5549 #2, below) |
| t/ suite, 16 scripts, 491 tests | 2.39.5 | no baseline (m1-wp6 ran 2.51.0 and 2.56.0) | 377 pass, 16 fail, 90 n/a, 8 skip (`t-239`) | n/a; against this run's own 2.51.0, see below |

In each client run the fault started 60 s in and lasted to the end of the run (about 4.5 to 5 minutes of fault). In each t/ run it started 120 s in and lasted to the end. Every row after the fault's start ran with every call to site 0's members failing.

**Differences from the baseline, per test** (`tests/m1/wp8/compare.py`, which compares each test's category with `raw/conformance-20261101/results.tsv`; output in `raw/bm1-20261102/t-*/compare.txt`):

- git 2.51.0 (`t-251`, 04:03 to 04:49Z, fault from 04:05:24Z (orb clock), on the 02:38Z site version, before fix 4):
  - t5541 #20 `push --atomic fails on server-side errors`: baseline fail, now pass. This is the t5541 #20 fix (item 1).
  - t5549 #2 `push with negotiation`: baseline n/a, now pass. The test sets `receive.hiderefs` on the server, which report.py counts as a setting a hosted repository cannot carry. In the baseline it failed and so was n/a; here it passed, and a pass counts as a pass whatever the test touched. Why it passes now was not established. The baseline ran on the stand-in slot `bed/main` and this run on the real group's `wp6/main`, so the slot's history differed.
  - No other test changed category. The 14 fails are the baseline's 15 minus #20, with the baseline's causes (`m1-wp6.md`, "2026-11-01 (later)").
- git 2.56.0 (`t-256`, 05:51 to 06:33Z, fault from 05:53:56Z (orb clock), on the 05:47Z site version with fix 4):
  - t5541 #20: baseline fail, now pass (item 1).
  - t5549 #2 `push with negotiation`: baseline pass, now n/a. Its check is `grep_wrote 3`, meaning the client's negotiated push wrote 3 objects. The test also sets `receive.hiderefs`, so whenever it fails, report.py classes it n/a. This test is the one that went n/a to pass on 2.51.0. In the no-fault baseline it was n/a on 2.51.0 and passed on 2.56.0, so it already differed between two runs with no fault. In both faulted runs the push itself succeeded, and only the client's object count differed. That count depends on which commits the server advertises and acknowledges in push negotiation. The host does not apply the server-side `receive.hiderefs` the test sets, and the slot carries other refs between runs, so the count is not stable from run to run [inference]. None of this involves the fault. The test's pushes all answered 200 under it (19.3 s, 27.5 s and 22.5 s).
  - No other test changed category. The 14 fails are the baseline's 15 minus #20.
- git 2.39.5 (`t-239`, 06:33 to 07:15Z, fault from 06:35:49Z (orb clock), on the 05:47Z site version). It builds with `-std=gnu17` (run.sh's `CSTD`). Its scripts are older than 2.51.0's, so they hold 491 tests against 535. Compared by script and test name with this run's 2.51.0 results, only two tests that pass on 2.51.0 do not pass on 2.39.5:
  - t5541 #20 `push --atomic fails on server-side errors`: fail. 2.39.5's test breaks the server ref with `mkdir refs/heads/other.lock`, a lock file on the conformance front's local mirror, which the hosted slot cannot see. 2.56.0's test uses a d/f conflict (`refs/heads/other/conflict`), which the slot does carry. So the hosted push succeeded, and `test_must_fail` failed. The front logged `Another git process seems to be running in this repository` when it mirrored the slot back. This is a fixture the host cannot reproduce, not a defect, and not the fault.
  - t5549 #2: n/a, as on the baseline's 2.51.0 (above).
  - t5548 #16 `non-fastforward push (HTTP protocol)` failed on 2.39.5 only. It has no 2.51.0 counterpart by name, because 2.51.0 numbers the HTTP tests differently and passes its own. The cause is test #14 just before it (`pre-receive hook declined`), which installs a server-side hook (n/a). The front still mirrored the hosted slot back, which deleted `baz`, so #16's `:baz` delete found no remote ref. A fixture interaction [inference from the front's `mirror-back` line]. No 5xx and no failed push was logged.
  - 16 fails: the 2.51.0 baseline's 15 (by name, t5550 and t5551 numbered lower in 2.39.5), plus t5548 #16 above. #20 is among them, for the reason above.

### Leader moves

Times are from the fault's start, on member clocks, and are the first 2-second observer sample in which a member off site 0 reported itself leader of a newer term.

| Group | clients-239 (leaders mostly on site 1 already) | clients-251 | clients-239-homed | clients-251-homed |
| --- | --- | --- | --- | --- |
| `repo:196608` (wp6/main) | already off site 0 | 8.1 s | 26.1 s | 10.6 s |
| `names:174` (owner account) | already off | 4.1 s | 12.1 s | 8.6 s |
| `authz:7000001` | already off | 12.1 s | 14.1 s | 14.6 s |
| `names:938` (`RepoOwner` row) | already off | 14.2 s | 34.2 s | 16.6 s |
| `names:987` (login `wp6`) | 16.1 s | 20.1 s | 30.1 s | 22.6 s |
| `tokens:1563` (writer token) | already off | 36.2 s | 52.2 s | 38.7 s |
| `tokens:1153` (read-only token) | 247.2 s | 272.3 s | 284.2 s | 272.8 s |
| `repo:196610`, `names:806` (wp6/bf16) | not used by clients.sh | not used | not used | not used |

- t-251 (t/ on 2.51.0): `names:174` 11.7 s, `authz:7000001` 13.7 s, `names:938` 15.7 s, `repo:196608` 17.7 s, `names:987` 21.7 s, `tokens:1563` 51.7 s, `repo:196610` 57.7 s, `names:806` 73.7 s. `tokens:1153` never moved, because the t/ run uses only the writer token. The `[] t0` rows at +608, +1,351 and +2,093 s in `moves.txt` are samples where the observer's own fetch failed for every member (`TypeError: fetch failed`), not leader changes.
- t-256 (t/ on 2.56.0): `names:174` 9.7 s, `authz:7000001` 13.7 s, `names:938` 15.7 s, `repo:196608` 21.7 s, `names:987` 23.7 s, `tokens:1563` 53.7 s, `repo:196610` 55.7 s, `names:806` 69.7 s. `tokens:1153` never moved. The observer's own fetch failed in 3 samples.
- t-239 (t/ on 2.39.5): `names:174` 13.7 s, `names:987` 15.7 s, `authz:7000001` 19.7 s, `names:938` 19.7 s, `repo:196608` 25.7 s, `tokens:1563` 55.8 s, `repo:196610` 59.7 s, `names:806` 77.8 s. `tokens:1153` never moved. No `STEP_DOWN` alarm fired in any t/ run.
- Every move was an election, a term higher than the one before the fault, on site 1. The order follows the order of each group's first request after the fault: a clients.sh row reads the token, the owner's names and `Authz`, then the repository.
- `tokens:1153` is used only by the read-only-token rows near the end, so it moved only when they ran. Campaigns are request-driven, so a group with no request does not move (see "Design problems found", 3).
- `STEP_DOWN`: one alarm in the four runs, on `names:938`'s new leader on site 1 during clients-239 (`CommitErrors { calls: [(SiteId(0), 3, 3), (SiteId(2), 3, 3)] }, to: None`). No leader on site 0 stepped down, and no move came from a `STEP_DOWN` transfer (see "Design problems found", 4).
- The deposed members on site 0 kept reporting `leader` of their old term until the fault cleared; at the clear (+281 to +311 s) they took the new term as followers, which is the second batch of changes in `moves.txt`.
- Each run's moves, failed-call counts, and alarms are in `raw/bm1-20261102/<run>/moves.txt`.

### Commit latency during the fault

The leader's `commitMs` (`/stats`, from the command's admission to its commit), split at the fault's start:

| Run | Before the fault | From the fault to the end |
| --- | --- | --- |
| clients-239 | not split (this run predates `FAULT_STATS`) | n 9, p50 2,501 ms, p99 8,575 ms (leader on site 1) |
| clients-251 | n 2, p50 2,406 ms (leader on site 0) | n 7, p50 3,049 ms, p99 4,274 ms (leader on site 1) |
| clients-239-homed | n 2, p50 2,846 ms (site 0) | n 7, p50 2,898 ms, p99 4,861 ms (site 1) |
| clients-251-homed | n 2, p50 2,653 ms (site 0) | n 7, p50 2,822 ms, p99 4,529 ms (site 1) |
| t-251 (t/ on 2.51.0, about 44 minutes of fault) | n 4, p50 2,659 ms, max 2,694 ms (site 0) | n 89, p50 2,959 ms, p99 28,368 ms (`repo:196608` n 65, p50 3,039 ms; `repo:196610` n 24, p50 2,598 ms, p99 4,917 ms; all on site 1). The 28,368 ms is one entry, the third on the new leader, whose replication round took 25,024 ms; its cause was not established |
| t-256 (t/ on 2.56.0, about 39 minutes of fault) | n 3, p50 2,149 ms, max 2,466 ms (site 0) | n 82, p50 3,040 ms, p99 7,120 ms (`repo:196608` n 61, p50 3,242 ms; `repo:196610` n 21, p50 2,552 ms, p99 3,076 ms; all on site 1) |
| t-239 (t/ on 2.39.5, about 40 minutes of fault) | n 4, p50 2,451 ms, max 2,644 ms (site 0) | n 74, p50 3,221 ms, p99 11,547 ms (`repo:196608` n 55, p50 3,312 ms, p99 6,723 ms; `repo:196610` n 19, p50 2,837 ms, p99 11,547 ms; all on site 1) |

These are `Push` commands, not B0-01's 1 KB register commands. `commitMs` runs from the member admitting the command to its outcome, which includes the entry's dependency reads through `fresh` (`Names`, `Authz`) as well as the replication round. The replication round alone (`roundMs`) was much shorter: on `repo:196608`'s site-1 leader after t-251's first scripts, 26 rounds had a p50 of 191 ms, plus one 25,024 ms round. With the leader on weur (site 1), the quorum is weur plus apac (site 2), because enam (site 0) is failing. Before the fault it was enam plus weur.

A probe before the runs (40 pushes of one ref to `repo:196611`, the fault from push 6) showed the outage as one request sees it: pushes 1 to 5 took 4.3 to 4.4 s end to end (2.8 to 4.3 s of commit), push 6 took 16.8 s (15.2 s commit; the new leader was elected about 11 s after the fault), push 7 took 11.1 s, and pushes 8 to 40 took 5.4 to 6.3 s (p50 commit 3.6 s) with the quorum `[1, 2]`. The 16.8 s is the design's "about 11.5 s" plus a push.

In the t/ runs the request times the front recorded show the fault's cost per request. In t5541 on 2.51.0, `git-receive-pack` POSTs took 10.7 to 22.9 s (22 of them, mean 20.3 s) under the fault, against 7.5 to 18.2 s (mean 16.0 s) in this pass's no-fault t5541 on 2.56.0. The commit is about 3 s of each (above). The rest is outside the commit (the edge, the shard, and the front's mirror step) and was not broken down.

## What is short of the gate

- **The 24-hour half is pending** until 2026-11-02T18:27Z (run `lin24`, `m1-wp2.md`).
- **The fault is a total loss of one site's member class, not a gray one.** Every call fails, so this shows the clean-loss path (`suspect` and election). A partial error rate, which is where `STEP_DOWN` matters, was not run in this pass.
- **Groups move only when requested** (design problem 3). In these runs every group a request needed did move, but an idle group with its leader on the lost site keeps it there until its first request, which then waits about 11 to 15 s.
- **t/ under the fault is slow.** A receive-pack request through the front took 11 to 27 s under the fault (16 s mean with no fault), so a t/ version took 42 to 46 minutes. No test timed out.
- **git 2.39.5 has no no-fault t/ baseline.** `m1-wp6.md` ran 2.51.0 and 2.56.0 only. 2.39.5's t/ result is compared with this run's 2.51.0 by test name instead.
- **Only one site was failed (site 0, enam).** It led every watched group at the fault's start. Sites 1 and 2 were not failed.
- **`STEP_DOWN` moved no leader** (design problem 4).
- **A follower stuck before the fault, cause not found** (fix 4). `repo:196608`'s site-0 member stopped taking AppendEntries at 04:49Z with the fault off. The fix 4 redeploy cleared it, and the byte caps make one cause impossible. But the calls that failed carried only 836 bytes, so the cause seen here is unexplained. It did not recur in `t-256` or `t-239`: `repo:196608` committed 55 to 61 entries under the fault in each, and every mirror into `wp6/main` succeeded apart from the SHA-256 ones.
- **`t-251` ran on the site version before fix 4.** `t-256` and `t-239` ran after it. The client runs also ran before fix 4. Those runs lost no baseline pass, so fix 4 cannot change what they show. A rerun of them on the new version was not done.

## Design problems found (quoted, not edited)

1. **`replay`'s doc in DESIGN, "Shape".** "The output for a duplicate request, rebuilt from the events its first commit stored. `Push` reports a ref that is absent from them as `Stale`." That rule caused t5541 #20's `fetch first`: a ref refused for `AtomicTransaction`, `NameConflict`, `Atomic`, or `Hidden` is absent from the stored events and was replayed as `Stale`. With `RepoEvent::Refused` the stored events carry every refusal, and only a row written before the change still replays an absent ref as `Stale`. Evidence: `a_replay_reports_each_refused_refs_own_reason` fails on the old rule.
2. **SHARED, "Event types", `pub enum RepoEvent`.** The list has `Genesis`, `Refs`, `PrOpened`, `Merged`, and the rest, and no event for a refused ref. The fix adds `RepoEvent::Refused { push, refs: Vec<(RefName, RejectReason)> }` (tag 1), which changes no ref and names no pack. The list, the stored formats ("Stored formats of the `RepoEvent` payloads"), and `tier_of` need a row for it (`Refused` is Core, as `Refs` is).
3. **The "about 11.5 s" RTO does not hold for a group with no traffic.** DESIGN, Summary: "If a site is lost outright, groups led there stop reads and writes for at most about 11.5 s", with "Campaigns are request-driven through this call, never alarm-driven" (DESIGN, "Shape", the read-index doc). Measured: `tokens:1153` (the read-only token, used by two rows near the end of clients.sh) moved off the faulted site 247 to 284 s after the fault in all four client runs, which is when its first request arrived. Groups that had requests moved 4 to 78 s after the fault (below). So the bound is "about 11.5 s after the first request", and a request that arrives then waits that long [measured]. This is the design's stated choice, not a defect in the code, but the Summary sentence reads as a bound from the loss.
4. **Moves under total call failure come from `suspect`, not `STEP_DOWN`.** DESIGN's failure table for a region: "A gray region is left by step-down, `suspect`, or a signed drain." In 9 watched groups across four faulted client runs, one `STEP_DOWN` fired (`names:938`, clients-239, `CommitErrors { calls: [(SiteId(0), 3, 3), (SiteId(2), 3, 3)] }, to: None`), and it fired on the new leader on site 1 while site 0 still failed, not on a leader on site 0. The leader on the faulted site cannot step down from a commit-error window, because no edge call reaches it (every call answers 503 before it reaches Raft) and its own sends fail before leaving, so it never sees a commit attempt. Every move was an election started by `suspect` from an edge that got a 503. A deposed leader on site 0 still reports itself leader of its old term (`/status`, observed through the fault), which is safe (it holds no promise anyone honours past its term) but shows in the moves below as two sites both reporting `leader` until the fault clears. For a clean loss this is the expected path. Whether a gray region (some calls succeed) trips `STEP_DOWN` was not measured here.

## Checks

Run as the orb services `wp8-checks3` and `wp8-raft2` at 05:50 to 05:58Z, after every code edit of this pass (the last were fix 4's `crates/raft` and `workers/site` changes):

| Check | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| wasm32 clippy `-D warnings`, `workers/site` and `workers/edge`, each with and without `test-faults` | clean (4 of 4) |
| `cargo test --workspace` | 234 passed, 0 failed, 2 ignored (27 test binaries); the new test is the 234th |
| `crates/raft` long sweep, `RAFT_SEEDS=2000 RAFT_STEPS=10000`, release | ok, 329.75 s |
| Porcupine (`tests/m0/porcupine`) on that sweep's history, HARSH | `{"droppedFailedReads":3422,"noReplyWrites":25525,"operations":165184,"result":"Ok"}` |
| Porcupine, GRAY | `{"droppedFailedReads":6,"noReplyWrites":164,"operations":2259445,"result":"Ok"}` |
| test-faults needles, product builds | site 0 of 34, edge 0 of 14 |
| test-faults needles, deployed test builds | site 34 of 34, edge 14 of 14 |

The Porcupine counts equal the earlier run's to the operation, as they should: the simulator's AppendEntries never reach 1.5 MB, so the byte cap changes no simulated step. The new scenario test covers the cap. The needle counts were measured before fix 4, which added no test-only string.

## Deployed and left in place

- `graft-m1-site-a`, `graft-m1-site-b`, `graft-m1-site-c`: the test install, a `test-faults` build with migration `v2` (`Member`, `Registry`, `GitShard`), the cron `*/15 * * * *`. Deployed at 02:38Z on 2026-11-02 for the client runs and `t-251`, and again at 05:47Z with fix 4 (versions `1affe1f7`, `6bf41b70`, `16c3d7f3`) for `t-256` and `t-239`.
- `graft-m1-edge`: the git edge on the real groups, a `test-faults` build (version `303bcd62`, 02:56Z).
- Left from earlier packages, unchanged: `graft-m1-wp7-edge` and `graft-m1-wp7-edge-b` (the observer and `home.sh` reach members through `/member`), `graft-m1-wp2-edge`, `graft-m1-driver`, `graft-m1-wp6-bed`, `graft-m1-wp4`, `graft-m1-wp5`.
- Not touched: `graft-m1-lin-a/b/c`, `graft-m1-lin-tools`, and the services `lin24-watch` and `lin24-finish`.
- Deleted: the throwaway install `graft-m1-gen-*` (scripts, container applications, buckets, secrets, store).
- The site fault is off at the end of every step (`fault.sh off`; `bm1.sh` clears it once more after the last step).

## Gate rows closed on the hosted install, 2026-11-02 (afternoon)

The "Milestone 1 status (2026-11-02)" table in `build/BUILD-PLAN.md` left BF-14, BF-18 and BF-19 open. All three ran on the hosted test install `graft-m1-site-a/b/c` and `graft-m1-edge`, both `test-faults` builds. `lin24` was not touched.

| Row | Result | Verdict | Where |
| --- | --- | --- | --- |
| BF-14 | 2,400 commands, each with a missed receipt (two runs of 600 `Push` and 600 identity commands). **0 forwarded**: 0 sends to another member that a redirect did not explain, and 0 arrivals beyond the edge's own sends | **Pass** | below |
| BF-18 | Hosted run with a real `ChainLog`, real `Promoted`/`Archived` entries and a truncated log on every member. fsck clean on both shards, 9 of 9 clones, the newer pushes served by the second shard. One earlier run failed on a design gap (thin stored pack in the promoted range), fixed in the archive step | **Pass** (run 2) | `m1-wp5.md`, "BF-18 on the hosted install with a real `ChainLog`" |
| BF-19 | 1,000 of 1,000 failed placements deleted after 24 h and none before. 0 of 1,000 committed packs deleted. 1,000 of 1,000 missing markers repaired from the real `request` rows, with the cron body's clock advanced | **Pass** | `m1-wp4.md`, "BF-19 on the hosted cron with the real `request` table" |

### BF-14: receipts that miss the deadline

BF-14: "Receipts that miss `CALL_DEADLINE`. Pass: zero commands forwarded to a second member."

**Faults (workers/site, `test-faults`).** The edge's first `/commit` send of each command carries a `receipt` fault. Later sends do not.

- **`delay`:** the member answers as usual but holds the reply 2.6 s, past the edge's `COMMIT_RECEIPT` (2 s, which is `COMMIT_HOLD` + `CALL_DEADLINE`, DESIGN, Shape).
- **`drop`:** the member admits the command, and its reply is replaced by a 503 (`reply dropped (injected)`). The edge sees a lost reply for a command the leader holds.

Each member writes every `/commit` that reaches it to an `arrival` table, with its role on arrival, the fault, and its answer. `/arrivals?since=` reads the table.

**Commands.** Driver: `tests/m1/wp8/bf14.ts --push 600 --identity 600 --par 10`. Faults alternate `delay` and `drop`.

- **`Push`:** 600 single-ref pushes with no pack, on repositories `k0` to `k9`, under the writer's token, through `/_bed/push`. That is the edge's `commit` path, with `Names`/`Authz` dependency reads on the leader.
- **Identity:** 600 `PutAccount` commands on four `Names` shards (`names:174`, `965`, `689`, `292`), through `/_bed/identity`. That is the same `graftyard_lookup::commit_hedged`, with the operator credential.

**Count.** For each command there are two sources:

- the edge's trace (`commit_hedged`'s `Hooks.trace`): each `/commit` send with its target and the reason for it, each missed receipt, and each `suspect` answer;
- every member's `arrival` rows: the repositories' members, and the `Names` shards' members.

A command is **forwarded** in either of two cases:

- The edge sent it to a member other than its previous target, and the move did not follow a redirect. The redirects are: a `NotLeader` naming a leader (`notLeader`); a voter answering `suspect` as leader (`suspectLed`); a `NotLeader` naming no leader, after which the next voter is tried and refuses unless it leads (`leaderless`); and a new lookup after one that found no leader (`relookup`).
- A member got the command more times than the edge sent it there, so a member passed it on.

| Run | Edge version | Commands | Ok | Missed receipts in trace | Faults landed on a member | Sends by reason | Commands at a second member | Forwarded |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| r1 (11:39Z) | `4a53b0af`; sites before the `call_opt` fix (site-a `7125d157`) | 1,200 (600 + 600) | 1,200 | 1,200 | 1,200 (300 delay + 300 drop per family) | lookup 1,200, same 1,196, suspectLed 4, leaderless 1 | Push 0, identity 5, all explained (4 `suspectLed`, 1 `leaderless`) | **0** |
| r2 (13:03Z) | `2997a641`; sites after it (site-a `2e434cc0`) | 1,200 (600 + 600) | 1,200 | 1,200 | 1,200 (300 + 300 per family) | lookup 1,200, same 1,200 | 0 | **0** |

- **Totals:** 2,400 missed receipts, 2,400 commands decided ok, 0 forwarded, 0 unexplained sends, 0 extra arrivals.
- **r1's five second-member commands.** In each, the edge's `suspect` after the missed receipt reached a site-1 member that answered as leader (`suspectLed`), or the original leader answered `NotLeader` naming no leader (`leaderless`). The edge then sent the same request id to that member, which answered `done` (`roles` in `raw/r1.bf14-report.json`, for example `0:leader:done`, `1:leader:done:delay`). That is the design's hedge: a re-send under the same request id after a redirect, not a forward. This count does not check double applies. BF-12 covers those (`m1-wp2.md`, `m1-wp6.md`).
- **End-to-end time per command (r2):** p50 2,826 ms, p99 4,673 ms, max 6,481 ms. By family and fault: identity `drop` p50 1,117 ms, identity `delay` p50 2,908 ms, Push `drop` p50 2,708 ms, Push `delay` p50 3,205 ms. A `delay` costs the 2 s receipt deadline plus a second send.
- **Raw data:** `tests/m1/wp8/raw/r1.bf14.jsonl` and `r2.bf14.jsonl` hold per-command traces. `r1.bf14-report.json` and `r2.bf14-report.json` hold the counts and r1's second-member examples.

### BM-1 gray-fault rerun: site 0's member calls failing 30%

`clients.sh` (29 rows) on git 2.39.5 and 2.51.0, as in "BM-1 results". Two things differ. The site fault has `rate: 0.3`: each call into a site-0 member, and each call a site-0 member sends, fails on its own with chance 0.3 (`SiteFault.rate`, rolled per call), instead of every call. And the run used the current versions, edge `2997a641` and sites from 11:49Z.

Setup per run, the same as `bm1.sh`'s client step (the scripts as run are copied in `raw/bm1-gray-20261102/bin/`):

1. Clear the fault.
2. Tear down the slots' shards.
3. Move every watched group's leader to site 0 (`home.sh 0`).
4. Run `FAULT_RATE=0.3 tests/m1/wp8/clients-fault.sh <out> <image> 0 60`, which turns the fault on 60 s into the run and keeps it on to the end.

`fault.sh` gained `FAULT_RATE`. Raw data is in `tests/m1/results/raw/bm1-gray-20261102/`.

| Run | Rows | Fault on (UTC) | Leader moves (first sample off site 0, from the fault's start) | `STEP_DOWN` alarms | Commit latency, before → during |
| --- | --- | --- | --- | --- | --- |
| clients-239-gray (2.39.5) | **29 of 29** | 13:10:37 to 13:15:28 | `names:987` 18.3 s, `names:174` 20.3 s, `repo:196608` 54.3 s, `names:938` 96.3 s, `tokens:1563` 104.3 s, `authz:7000001` 126.3 s. `tokens:1153`, `repo:196610` and `names:806` stayed on site 0 (unused, or used only by read-only rows that passed through the 70% of calls that succeed) | 0 on every member | n 2, p50 2,748 ms (site 0) → n 7, p50 2,874 ms, p99 5,778 ms (1 on site 0, 6 on site 1) |
| clients-251-gray (2.51.0) | **29 of 29** | 13:17:08 to 13:22:14 | `names:174` 12.2 s, `names:987` 22.1 s, `authz:7000001` 34.2 s, `names:938` 58.2 s (one failed election first: site 0 won term 21 at 46 s, then lost it to site 1 at term 22), `tokens:1563` 66.2 s, `repo:196608` 112.2 s. The same three stayed | 0 on every member | n 2, p50 2,843 ms (site 0) → n 7, p50 3,388 ms, p99 5,370 ms (2 on site 0, 5 on site 1) |

- **Every move was an election, not a `STEP_DOWN` transfer.** The observer shows the site-1 member as `candidate` of a new term, then `leader`. This matches `names:174` in clients-239-gray: `0:leader:t18 1:candidate:t19`, then `0:follower:t19 1:leader:t19` 2 s later. Each election followed a 503 the edge got from a site-0 member and its `suspect` report.
- **No `STEP_DOWN` fired on any member,** in either run, before or during the fault (`stats-at-fault.jsonl` and `stats.jsonl`: every `alarms` list is empty). `STEP_DOWN`'s commit-error rule needs, within a 10 s window, an error rate above 5% **and** at least 3 failed calls to **every** follower (`STEP_DOWN = Health { window_s: 10, commit_errors: 0.05, commit_fail_min: 3, … }`). With 30% of a site-0 leader's sends failing, 3 failures to each of 2 followers inside 10 s takes about 10 or more appends in that window. In these runs `repo:196608` committed 7 entries in about 5 minutes, and the identity groups committed none, because clients.sh only reads them. So the window never filled before an edge's `suspect` had moved the leader. This is an inference from the rule and the commit counts; the window's contents were not logged.
- **Groups that stayed on site 0 still served.** `tokens:1153` (the read-only token's group) answered the read-only rows from site 0. 70% of its calls succeed, and the edge's lookup retries the others, so the rows passed without a move.
- Each run's moves, failed calls and alarms are in `<run>.moves.txt`, from `summarize.py`. The leader on site 0 reports `failedCalls: 0` because a gray-failed send is dropped before it leaves and reaches Raft as `call_failed`. The host's `failedCalls` counter counts only calls that left and missed their deadline.

Against design problem 4 above: under a gray fault too, the leader moves through `suspect`, not `STEP_DOWN`. In two runs with 30% failure, no `STEP_DOWN` fired on any member. DESIGN's region row says "A gray region is left by step-down, `suspect`, or a signed drain." Both runs left the gray region, through `suspect`. Whether `STEP_DOWN` would leave it for a group with enough traffic to fill its window was not measured. clients.sh is too sparse to show it.

### What is stubbed or short in this pass

- **BF-14:**
  - The receipt faults are test-faults injections: a held reply and a replaced reply. Real network loss of a reply was not produced.
  - The identity commands are `PutAccount` only, through the bed's `/_bed/identity` route.
- **BF-18:**
  - A test route triggers the archive. No leader policy exists.
  - A key held in only one bucket is not re-put; the member alarms and truncates nothing.
  - `last_promoted`'s index is an upper bound.
  - `consumed` is empty.
  - `RepoFold::check` requires every archived range to be promoted. That is stricter than the design (`m1-wp5.md`).
- **BF-19:**
  - The 24 h comes from the sweeper's injected clock, not a day of waiting.
  - Each cron run reads one `request` row per unmarked pack through the leader, which the cron budget does not count (`m1-wp4.md`).
- **Gray rerun:**
  - clients.sh only, on 2.39.5 and 2.51.0. The t/ suite was not rerun under the gray fault.
  - Only site 0, only at 30%.

### Checks (this pass)

| Check | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, after `commit_hedged`'s hooks became one `Hooks` argument |
| `cargo test --workspace` | 235 passed, 0 failed, 2 ignored (27 binaries), rerun after the last edit. clippy, fmt, and the 4 wasm32 clippy runs (site and edge, with and without `test-faults`) were also clean after it |
| `crates/raft` long sweep, 2,000 seeds × 10,000 steps, release | ok, 333.25 s |
| Porcupine on its histories | HARSH `{"droppedFailedReads":3343,"noReplyWrites":27735,"operations":156723,"result":"Ok"}`; GRAY `{"droppedFailedReads":11,"noReplyWrites":209,"operations":2226280,"result":"Ok"}` |
| `worker-build --release` of `workers/site` and `workers/edge`, with and without `test-faults` | 4 of 4 built |
| test-faults needles, product builds | site **0 of 42**, edge **0 of 20** |
| test-faults needles, test builds | site 42 of 42, edge 20 of 20 |

New needles this pass:

- **Site:** `/arrivals`, `::receipt_fault`, `receipt_delay`, `receipt_drop`, `reply dropped (injected)`, `no_marker`, `/sweep_run`, `::RecordReq`.
- **Edge:** `/_bed/identity`, `x-graft-shard`, `::bed_archive`, `::bed_place`, `fail_place`, `no_marker`.

The product build carries `Member::record` and the truncate-after-`Archived` path. The route that calls `record`, `/record`, is test-only. Outputs are in `tests/m1/results/raw/checks-20261102b/`.

### Deployed in this pass and left in place

- **`graft-m1-site-a/b/c`:** `test-faults` builds from 11:49Z on 2026-11-02 (versions a `2e434cc0`, b `8762cdea`, c `29ad23a2`). They carry the receipt faults, `arrival`, `/record`, `no_marker`, truncation, `/sweep_run`, the gray `SiteFault.rate`, and the `call_opt` fix. Every BF-18, BF-19, BF-14 r2 and gray-run step ran on these.
- **`graft-m1-edge`:** `test-faults` build `2997a641` (12:27Z). Its only change from `be69c632` (12:04Z) is `/_bed/heads` running its HEADs concurrently. BF-14 r1 ran on `4a53b0af` (11:38Z).
- **Repositories seeded by this pass on the install:** `wp6/bf18a` (196621, run 1), `wp6/bf18b` (196623, run 2), `wp6/bf19a` (196622, BF-19's 1,000 committed refs under `bf19/r1/`). Their objects are left in the buckets.
- **The site fault is off** (`fault.sh off` at 13:22:25Z).
- **Services:** the scratch orb services of this pass (`wp8b-*`, `wp8c-*`) are stopped. `graft-m1-lin-*` was not touched.

## The 24-hour half, 2026-11-02: lin24 does not pass

BM-1's second half is "Linearizability rerun at 24 hours". Its rows set the pass line at zero violations and zero lost acknowledged entries (B0-11). Run `lin24` ran on `graft-m1-lin-a/b/c` from 2026-11-01T18:27:14.621Z to 2026-11-02T18:27:14.621Z. Its report is `tests/m1/lin/raw/lin24.report.json`, and the full analysis is in `m1-wp2.md`, "The 24-hour run's result".

| Check | Result |
| --- | --- |
| Operations | 2,739,691, 0 with no reply; p50 326 ms, p99 1,009 ms |
| Chaos | 5,547 resets (3,676 of a leader), 3,743 transfers (1,131 abandoned), 88 isolates with fault counters |
| Porcupine, widened 100 ms and widened 275 ms (the measured LHR–SIN bound of 550 ms, halved) | Ok on all 6 keys |
| Porcupine, recorded clocks | Illegal on k2, k4 and k5, 5 segments. All 5 come from writes whose history rows were lost when 5 of 12,518 client chunks were cut short. With those writes restored: Ok, no skew needed |
| Lost acknowledged writes | 0 of 869,257. The three logs are identical over 479,965 entries |
| Early ack | **Not ruled out.** Entry 398,032: an LHR put returned 52 ms before an ATL put was called, and both committed in the same entry. That needs 93 ms of skew against that minute's measured ATL–LHR bound of 49 ms. 17 writes were acknowledged, on their own client's clock, faster than any quorum could commit them |

**Verdict: BM-1's 24-hour half is not passed.** The history cannot rule out an early ack. The members recorded no commit time and no send time, and a client-clock artifact explains the two observations as well as an early ack does. The finish step first crashed with `ERR_STRING_TOO_LONG` on the 584 MB widened history. `check()` in `tests/m1/lin/run.ts` now splits each history by key in one streaming pass. BM-1's 24-hour half is rerun as `lin24b`, with member-side commit and send times checked for every acknowledged write (`m1-wp2.md`, "Member-side ack evidence").

## The 24-hour half, 2026-11-03: member-side ack evidence, and lin24b

The early-ack question that lin24 left open is now checked from member clocks. Every acknowledged write carries its answering member's `QuorumProof` timings, plus its host's own storage-confirm time. `tests/m1/lin/ackcheck.ts` asserts that the reply left at or after the commit, that a quorum of distinct sites stored the entry durably before it, and that the host's own confirm covered it. A planted early ack on the leader's own append was caught: 54 of 3,873 acknowledged writes failed. It was caught again when the crate's proof was made to hide it (`m1-wp2.md`, "Member-side ack evidence, 2026-11-03").

Cloudflare's rule that `Date.now()` advances only on I/O explains neither the 17 fast acks nor the 4 sub-physics clock rows, because a frozen clock only lengthens intervals. They stay unexplained by client clocks. lin24b decides them with the member check.

F45 is fixed, so `load` and the in-memory log are bounded by bytes (`LOG_BYTES`), and `request` rows hold no inline pack bytes. `repo:196624` restored on all three members and took an 800 kB push (same section).

`lin24b` started on `graft-m1-lin-a/b/c` at 2026-11-03T00:55:07.302Z and ends at 2026-11-04T00:55:07.302Z (`m1-wp2.md`, "lin24b: the smoke and the start"). BM-1's 24-hour half stays pending until its report. It passes only if Porcupine finds zero violations, no acknowledged entry is lost, and the member-side ack check finds zero violations.
