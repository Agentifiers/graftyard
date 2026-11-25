# M6-WP1, region loss and re-protection

This file records M6-WP1's work (`build/BUILD-PLAN.md`, M6-WP1; BF-50). Status: **the pure half is merged** (main 3bf43a16), and **the hosted glue is built** on branch `m6-wp1-glue`. BF-50 on graft-m6a is not started. The operator approved graft-m6a for after the pure half and the glue merge (3 sites plus a wnam spare from genesis, test-faults builds, $20 a UTC day and $40 in total).

## The pure half (2026-11-24)

| Part | Where | Tests |
| --- | --- | --- |
| `graft-ops` signs `ReplaceMember` (executor key) on `SiteLost`, or `ZERO_ACKS` of drain with both other sites silent | `crates/release/src/ops.rs`, `auto_replace`; `gate::loss` | `tests/group/replace.rs` |
| A `SiteLost` site nobody drained is drained first | `ops::auto_drain` | `a_site_lost_that_was_never_drained_is_drained_first` |
| The spare on the lost site's continent, with a page when none exists or it cannot read every enabled format | `SiteTable::spare_for`; `auto_replace` | `a_lost_site_with_no_spare_on_its_continent_pages`, `a_site_lost_replaces_at_once_but_never_onto_a_spare_that_cannot_read_every_enabled_format` |
| One `Move` step per intent: start the coordinator, wait, then done, or a page on failure | `ops::Step::Move`, `Exec::{move_run, start_move}`; `workers/ops/src/exec.rs` (`WORKFLOWS` binding) | `a_failed_*`, the `ZERO_ACKS` test |
| Undrain only on recovered (`RECOVER_AFTER`); `RestorePlacement` refused while drained, then moving back and clearing the loss | `ops::auto_drain`; `group.rs`, `replaced` | `a_replaced_site_undrains_only_once_recovered_*` |
| One group's replace steps | `crates/identity/src/group_move.rs`, `replace_step`, `holds` | `crates/identity/tests/group_move.rs`, on real members |
| The groups to move | `crates/types/src/registry.rs`, `replace_candidates` | `every_chain_on_a_surviving_site_is_a_candidate_once_at_its_highest_generation` |
| The spare's bucket fill | `crates/git/src/sweep.rs`, `refill` | `a_refill_copies_single_copy_packs_first_from_surviving_buckets_only` |

**On real members** (`crates/identity/tests/group_move.rs`):
- **Tri.** The leader's site 0 is blackholed, site 1 is elected, and the steps are `Swap { add: 3, drop: 0 }`, `Leave`, `Home`, `Transfer`. The spare leads at generation 2, and every acknowledged write is on 1, 2 and 3.
- **`*2w`, follower Full member lost.** The steps are `Degrade`, then `Restore`. The spare is Full in slot 1, and every write is kept on both Full members.
- **`*2w`, witness lost.** The steps are `Swap` and `Leave`, with no `Degraded`.
- **`*2w`, leader's Full site lost.** The successor cannot commit its `Noop`, so `reconfigure` answers `Busy`. That is OPEN-QUESTIONS R48, which the operator kept out of the hosted row.

**Plants.** All 11 mutants are caught:
- `tests/m0carried/plant115.py`, 3 mutants: a restore while drained, an undrain on calm, and a `ZERO_ACKS` replace without silence.
- `plant116.py`, 2 mutants: a spare that can't read a format.
- `plant117.py`, 3 mutants: the refill order and its sources.
- `plant118.py`, 3 mutants: a group left out.

Main's `gate-pool` took 114 before this branch, so these are 115 to 118.

**Deferred by ruling.** The `from_record` swap for creates during a loss (R49), and `*2w` hosted (R48).

## The hosted glue (2026-11-24)

| Part | Where | Tests |
| --- | --- | --- |
| The coordinator, group and expiry Workflows, and the `/move/<intent>` handler `graft-ops` calls | `workers/workflows/move.mjs`, `entry.mjs`, `deploy.mjs`; `workers/ops/deploy.mjs` (`"workflows"`, the `WORKFLOWS` service binding) | `workers/workflows/tests/move.test.mjs`, 9 tests |
| Every row of a registry shard, and one budgeted refill pass | `workers/site/src/registry.rs` (`/all_rows`, `/refill`); `replace.rs`; `graftyard_git::refill_run` | `refill_runs_copy_single_copy_packs_first_and_resume_from_their_cursor` |
| One group's replace or restore, asked of its leader | `workers/site/src/member.rs`, `move_step` (`{"replace": {lost, spare}, started, dead}`), the `Degrade` and `Restore` arms; `group_move::replace_plan` | `a_replace_leaves_a_group_that_never_held_the_lost_site_alone`, `a_restore_moves_a_2w_full_member_back_by_joint_without_degrading` |
| The drained-site probe | `registry.rs` (`?probe=` on the `site_health` read, and the object's alarm); `replace::probe` | `a_probe_never_calls_its_own_site` |
| 24 h per-minute `SiteHealth` rollups, and `gate::loss` on the host | `workers/site/src/gatecount.rs` (`site_health_min`); `workers/ops/src/exec.rs`, `Live::health` | `a_day_long_health_read_sums_the_minute_rollups` |
| `deleteAll()` after 35 days | `move.mjs`, `expire`; `member.rs`, `/expire` | `an expiry asks the removed member after 35 days, naming its replacement, until it wipes`; `a_removed_member_is_wiped_only_35_days_after_its_move` |

**How a replace runs.**
- `graft-ops` posts the `MoveSpec` to `/move/<intent>`, which creates the coordinator under the intent's ULID. A second post finds it there and answers 200.
- **The coordinator.**
  - For a lost site it first refills the spare's bucket, single-copy packs only. It calls `/refill` on a surviving site's registry object `refill`, one `CronBudget::SITE_CRON` pass a step, and resumes at the network the last pass stopped on.
  - It reads `/all_rows` from each of the 64 shards on each surviving voter site, one step per shard, and keeps each chain at its highest generation.
  - It starts one group instance per chain with `createBatch`, 100 at a time, under ids derived from the intent and the chain. A replayed start step gets 10405 for each id and starts nothing twice.
  - It then runs the full refill and polls the groups, at most 200 a step, from the first one not yet seen complete. One errored group errors the coordinator, which `graft-ops` pages on.
- **A group instance** asks its row's member `/move` with `{"replace": {lost, spare}, started, dead}`.
  - The leader plans by `replace_plan`. A group whose committed config holds no voter on the lost site is `Done` before any step, so the many chains that never touched it end on their first ask.
  - A `notLeader` names the leader to ask next. For a lost site, a follower naming no leader, or one on the lost site, is asked `/campaign` (`campaign`, below), then asked again. A member that answers nothing, or knows no leader, passes the ask to the next voter its last config names, never one on the lost site.
  - `Done` after a step starts the expiry instance for the removed member, naming the member that replaced it.
- **The leader's new arms.** `Degrade { lost }` commits `Degraded` once the voters it needs are caught up (`voters_ready`). `Restore { add }` joins a member on the spare in the lost member's slot and kind at the next generation, then commits `Normal` with it, the home moving to the spare if it was on the lost site. Neither reads the stored log.
- **The expiry instance** sleeps 35 days, then asks the removed member `/expire` once a day, up to 30 times, until it wipes. The member wipes only if it is removed: by its own committed config, by F113's `superseded`, or by the replacing member's committed config, which names that member and not this one. A member on a lost site never commits the config that drops it, so the last case is the one a replace meets. An unjoined member answers 409, which ends the instance.

**The drained-site probe.**
- A replaced site holds no members, so nothing calls it and `SiteHealth` never reaches `MIN_CALLS`. `gate::loss` could never read it recovered, so it would stay drained and `RestorePlacement` refused for good.
- Each `graft-ops` run already reads every voter site's `site_health` object. The `DRAIN_WINDOW` read now names the lost and replaced sites (`?probe=`), not every drained one: an automatically drained site still holds followers that are called, and healthy pings to an unjoined member would dilute their misses (and cut across R53's undrain).
- The object stores them with an end 15 minutes out, and its alarm pings the unjoined member `probe#0` on each every 2 s within `CALL_DEADLINE`, adding each answer as one member call. A stopped `graft-ops` stops the probe within 15 minutes.
- Under test-faults an unjoined member's `/ping` answers 503 while BM-1's site fault is on, so a faulted site probes silent as a lost one does.
- The cost is 43,200 pings a day per prober while a site is lost or replaced, on the prober's own registry object.

**The rollups.** `site_health` keeps its 10 s buckets for `UNDRAIN_AFTER` and adds a per-minute table kept for `ZERO_ACKS` plus an hour. A read over more than `UNDRAIN_AFTER` sums the minutes. `Live::health` reads four windows per observer (`DRAIN_WINDOW`, `UNDRAIN_AFTER`, `ZERO_ACKS`, and the recovery window) and calls `gate::loss`, so a silent `ZERO_ACKS` now replaces without a `SiteLost`, and a lost site undrains on its own once recovered.

**C3, the site-day question** (OPEN-QUESTIONS C3).
- The Workflows script is a new script. Its own config binds every site's `Member` and `Registry` classes by `script_name`, as `graft-ops` does, and Cloudflare documents a Worker binding a Durable Object class or a Workflow of another script [doc: https://developers.cloudflare.com/workflows/build/workers-api/, "Cross-script calls"]. Deploying it gives no site script a new version, provided the install's `sites.json` lists the spare from genesis, as `workers/site/deploy.mjs` already binds every listed site.
- The glue also changes the site script: `registry.rs` (routes and the probe alarm), `member.rs` (`/move`'s replace, `/expire`, the faulted probe), and `gatecount.rs`. **So it ships on a site day**, graft-m6a's install day, and an install that already exists takes it on its next one.
- `graft-ops` gains a service binding and a variable, a new version of `graft-ops` only.

**Deviation for BF-50.** `graft-ops`' recovery window is `OPS_RECOVER_MS` on a test-faults build (`"recover_ms"` in the executor file), lowered from `RECOVER_AFTER`'s 24 h for the row's recovery half, as the operator allowed. BF-50's lines don't measure the window's length. The drain's 24 h of zero acks runs at full length.

**`campaign` (2026-11-24, before BF-50).** Every system chain's home is site A, so after A is lost its followers on B and C still name A's leader once their promise lapses, and nothing else calls an idle chain to make it campaign (`Host::keep_led` acts only for leader-only work). The group instance now asks the live voter it reached to `/campaign` when its `notLeader` names no leader or the lost site, as `graftyard_lookup`'s create does for a home member. Found reading the glue against m6a's genesis, before any row; one test and one plant added.

**Plants.** `plant117.py`'s and `plant118.py`'s anchors are re-pointed for the `refill` and `replace_step` refactors, and still catch 3 of 3 each. `tests/m0carried/plant130.py`, 9 mutants, all caught:
- a group that never held the lost site moved anyway;
- a restore planned as a dead site's replace;
- a day-long health read summed from the 10 s buckets;
- `deleteAll()` a moment before 35 days;
- a site probing itself;
- the single-copy refill pass copying every pack;
- the coordinator keeping a chain's lowest generation;
- a group asking a member on the lost site;
- a group following a leader named on the lost site instead of asking for a campaign.

## Bootstrap refusals on cold isolates (2026-11-24, before BF-50)

Found building graft-m6a, on its `Tokens` precreate from main 05ea366d. **Origin: M5-WP2's 7e01f24f** ("cached Release view per isolate"), which made `graftyard_lookup::system_record` answer a cold isolate from the chain's `MemberRegistry` row before it reads the genesis.

- **Symptom.** 74 of 384 eight-shard ranges answered 200 across three passes; the rest answered 503, and retries did not converge. Each failed shard was `1 of 3 joined`, with sites 1 and 2 refusing `notTheSourcesRecord`, or `created, but no member leads`. About 2,000 `Tokens` chains were left with a registry row on some site and no working group [measured, `tests/m6/raw/m6a/precreate/`].
- **Cause.** `member.rs`'s `check_source` read a `Source::Genesis` join's record through `system_record`. On a cold isolate, once the chain's site-0 member had written its row, that returned `RegistryRow::record`, whose `at` is 0, and the check compares records for equality, so the genesis record the joiner sent was refused. Shown by hand on m6a: the same `/join` for `tokens:44` sent with `at: 0` joined on site 1, and site 0, which holds the genesis, refused it and named the real `at` [measured].
- **Why m3f and m4a never saw it.** Both were built from 20147db, before 7e01f24f.
- **Fix.** `check_source` takes the record from the genesis alone (`graftyard_lookup::source_record`: the view, else the held genesis, else a genesis read); `system_record` keeps its registry shortcut for reaching existing groups (`decisions/DECISIONS.md`, "`source-record`"). `tests/sourcebed/run.sh` reproduces the refusal on 05ea366d in local workerd and accepts with the fix; `tests/m0carried/plant131.py` plants it. `create`'s own joins on a cold isolate still send the row's record, which only matters for a half-made system group created from a cold isolate (OPEN-QUESTIONS R54).

## Idle leaders of half-made chains (2026-11-24, before BF-50)

Found on graft-m6a with no load running, after the precreate on 05ea366d (above) left half-made `Tokens` chains.

- **Symptom.** The day read $9.42 on `graft-m6a-site-a` by 12:41Z. Site a's `Member` namespace cost $1.81 in the 10Z hour, $4.10 in 11Z and $2.75 in 12Z, nearly all active time. About 586 of its objects were active in every minute from 12:36Z to 12:48Z, and over 12:13Z to 12:42Z 595 of 596 such objects took no inbound request [measured, `durableObjectsPeriodicGroups` per minute and `durableObjectsInvocationsAdaptiveGroups` by object]. Of the 909 half-made chains with a member on site a, that member led in 578 cases, at commit 2 with config index 1. A sample leader, `tokens:2318`, counted `append failed 6764` toward site b, and site a's alarm events ran about 8,300 s of wall time each [measured, `/status` and `/stats`; telemetry]. Registry, `graft-ops` and the git shard were cents.
- **Cause.** A leader's own failed or ended call counted as activity. `Host::call_failed` and `Ended`'s drop stepped the core through `input`, whose `step` runs `kick`, pushing `active_until` 3 s out. `tick_leader` sends a `behind` peer its next AppendEntries every `CALL_DEADLINE`, and a member that is not joined answers each one 409, so the 100 ms ticker never stopped (`decisions/OPEN-QUESTIONS.md`, R55; `decisions/DECISIONS.md`, "`quiet-failures`").
- **First stop.** 578 `POST /abort`s at 12:49:33Z: site a fell to 140 active objects in the 12:49 minute, then 4, 4, 4, 4 and 1 a minute to 12:54. The aborts logged 1,925,568 events in three minutes, 33,751 from one object (OPEN-QUESTIONS R56).
- **It came back.** The site day from 7e673712 at about 13:12Z and the precreate rerun (13:20Z to 13:55Z: 383 of 384 ranges at 200, then range 1488 at 200 on one retry) woke the same leaders, and the deployed build lacks the fix: 598 active objects at 13:55Z.
- **Second stop, by repair.** The registry then held 2,457 `Tokens` chains with rows on all three sites and 615 with rows on two (461 on sites 0 and 1, 154 on 0 and 2). All 615 held a committed Tri config of [0, 1, 2] and an unjoined third member. Each third member was joined with the genesis's `Tokens` record (the `notTheSourcesRecord` a member names): all 615 now read `joined: true`, 566 at commit 4 and 1 at commit 6. Site a fell from 598 active objects at 13:55Z to 256 at 14:05Z, 88 at 14:08Z and 1 at 14:11Z, with no abort.
- **Excluded from BF-50's before-snapshot: 18 chains with no leader**, each with every member joined: `tokens:133`, 169, 281, 417, 476, 518, 600, 628, 741, 745, 819, 919, 1331, 1435, 1598, 2125, 2310 and 2378. Their reason: no member leads, so no working group, and line 2 cannot count them as moved.
- **Spend meters.** The day's stop is read on `PREFIX=graft-m6a tests/fanout/acctcost.py` ($14.67 at 14:14Z), not `tests/fanout/m2bcost.py` ($12.74 at 14:10Z): acctcost also prices Durable Object rows read, at a rate near the bill's own for duration, which m2bcost does not. Neither counts Workers Logs.
- **Fix.** `quiet-failures`: a failed or ended call no longer counts as activity. `tests/quietbed/run.sh` fails on 7e673712 and passes with the fix. It ships on m6a's next site day, before m6aL.

## BF-50 (planned)

On graft-m6a, from merged main, after the glue, with the Workflows script deployed beside the sites (`workers/workflows/deploy.mjs`) and named in the executor file: site A lost with every account API token revoked, drained 24 h with zero acks, `ReplaceMember` by the executor, then recovery and `RestorePlacement`. The recovery window is lowered under test-faults only (the deviation above). The row is not run yet.

## m6aL: loss by SiteLost (2026-11-25), FAIL on line 2

The token handling follows FRAME.md, "Operator decision, 2026-11-24, BF-50's 'every API token revoked' reads the install's own tokens". Sites were deployed from 1d180e5b, and the Workflows script from 05ea366d's glue (version f5e20fc4). The row's full verdict lines, raws and harness go in with the m6aL results; this section records the findings that changed the glue.

- **What happened.** The row created its chains at 00:24Z and loaded them. `before` then judged 3,099 chains (19 excluded). At 00:47:49Z the row put BM-1's site fault on site 0, and at 00:48:06Z a customer-signed `SiteLost{0}` was answered 201 at index 10. `RestorePlacement{0}` while the intent was current was refused 422 `IntentCurrent`, as it should be. The executor started `ReplaceMember{lost 0, spare 3}`, and the coordinator started 3,118 group instances. By 00:53Z 3,093 were complete with 0 errored.
- **The fail.** 17 judged `Tokens` chains could not move on the deployed glue. For 14 of them, the row's member on site 1 held only the bootstrap entry, while site 2's voter held 4 entries and had voted for site 0. `group()` asked only the row's member to campaign, about 50 times, each answered `started`, and site 2 refused it the vote. For 2 chains (`tokens:726`, `tokens:2977`), the highest-generation row was a placement-try member outside the gen-1 config. `tokens:953`'s site 1 member answered 409 (never joined). Line 2 counts a single chain short as a fail.
- **Fix 1, `campaign-rotate`** (f085d4ba, main 93852794). A group asks the next live voter to campaign when the one it asked still names no leader. It passes over a row member that is not a voter, and starts with every row of its chain. Deployed at 01:19Z as Workflows version f7bc3b1e; every other script's version list was unchanged. 16 of the 17 restarted instances completed within 50 s, in 7 or 8 asks each, a "started" on the row's voter followed by one on the next voter. `tokens:976` read config index 7 with voters on sites 3, 1 and 2. The excluded `tokens:133`, `2125` and `2378` had the same shape and completed too; they stay excluded. This is recovery evidence for the fix, not a pass for m6aL.
- **`tokens:953` is a precreate miss.** The precreate's repair pass chose chains by row count, the 615 with rows on only two sites. `tokens:953` had rows on all three, but its site 1 member had never joined, so the pass never looked at it. `before` now checks that every row member of every included chain answers `/status` as joined, and excludes a chain that fails with its reason.
- **Fix 2, `move-unmoved`** (OPEN-QUESTIONS R62). 6 groups could never finish: `tokens:953`, `repo:0` (no group ever formed), and the first-create leftovers `repo:6201` to `repo:6204`. One errored group errors the coordinator and leaves the `ReplaceMember` current for good, which blocks the restore. Such a group now ends unmoved and pages, and the coordinator finishes with the list (`decisions/DECISIONS.md`, "`move-unmoved`").
- **Campaign steps.** The coordinator's `/campaign` steps are counted per row in the m6aL results from `groups.jsonl` (asked and elected). On the deployed glue, about 50 steps per stuck chain were asked and none elected. After `campaign-rotate`, each of the 16 (and 3) took 1 or 2 asked steps, and one elected.

### m6aL's verdict lines (2026-11-25, judged 03:24:48Z)

The `ReplaceMember` reached `IntentDone` at about 03:07Z, at Release index 6. The coordinator restarted under `move-unmoved` and completed with `{groups: 3118, unmoved: [repo:0, tokens:953]}`. Each unmoved entry reads "no leader after 720 asks: no live joined voter quorum without site 0". `repo:6201` to `repo:6204` moved once their groups had every row of the chain: 4 asks for 6201 and 18 for each of the others.

- **Line 2: FAIL.** 3,099 chains judged (19 excluded), 3,095 held on A, 3,093 moved, 2 short. Controls: 4 of 4 untouched.
  - Both short chains are pre-row data faults, not move faults. `tokens:953` is a precreate miss: its site 1 member never joined. `repo:6201` is a first-create leftover whose member names collide: `#1#f0` is a `Tri` voter on site 0 and a `Solo` try on site 1, so the snapshot read the `Tri` config before the loss and the `Solo` one after (OPEN-QUESTIONS R65). Its `Tri` group [1, 2, 3] holds no voter on A.
  - `before`'s new check excludes both. Replayed on m6aL's before-rows, it finds `tokens:953` ("`tokens:953#1#f1` on site 1: not joined", read from `/status`) and `repo:6201` to `repo:6204` ("`#1#f0` on sites 0 and 1"). m6aL2 runs on fresh chains.
- **Signers: PASS.** Release entries from SiteLost to completion:

| Index | Entry | Signer |
| --- | --- | --- |
| 2 | `SiteLost{0}` | customer |
| 3 | `SiteMode{0: Drain}` | drain key, graft-ops' drain after SiteLost, before the replace path |
| 4 | `ReplaceMember{lost 0, spare 3}` | executor |
| 5 | `IntentStep 0` | executor |
| 6 | `IntentDone` | executor |

  The drain key and the customer key signed nothing in the replace path (indexes 4 to 6).
- **Line 3: PASS.** 451 coordinator steps, registry rows read from sites 1 and 2 (64 shards each), and the single-copy refill before the full one.
- **Campaign.** 3,733 campaign steps over 3,114 chains, and 3,112 elected. Verdicts: started 3,353, resumed 14, bootstrapping 365, busy 1. That is 1.006 of the 3,095 chains held on A: nearly every idle chain whose leader was on A needed the coordinator's ask, as DESIGN's request-driven campaigns intend (OPEN-QUESTIONS R66).
- **Versions.** Every site script, the edge and graft-ops are unchanged. `graft-m6a-workflows` went from f5e20fc4 to f7bc3b1e (`campaign-rotate`) and then ed42f9a7 (`move-unmoved`), deployed mid-row as ruled.
- **The spare's warm shards** (OPEN-QUESTIONS R67). After the replace, 25 git shard containers stayed active on the spare with no load, about $2.5 an hour.

### m6aL's recovery half (2026-11-25): PASS

- **Undrain.** The site fault lifted at 03:25:49Z. graft-ops undrained A at 03:57Z (Release index 7, drain key), after `OPS_RECOVER_MS` (30 min, lowered under test-faults as recorded above).
- **restore2.** A customer-signed `RestorePlacement{0}` committed at 03:57:56Z (index 8). Its first coordinator started 3,118 groups, and none stepped in 8 minutes: the groups the replace had moved were idle with lapsed promises, and the glue asked for a campaign only on a dead replace.
  - The fix, `restore-campaign` (main d5ecbbe7, Workflows b1a6a928), asks for a campaign on every move kind when the voters name no leader.
  - The 3,118 restore groups and the coordinator were deleted (by 04:37Z), and graft-ops restarted the coordinator at 04:37:56Z. By 04:40Z, 3,117 of 3,118 were complete with 0 errored. `repo:0`, which never formed, ends unmoved at `ASKS_MAX`.
- **The unmoved chains after restore2.** `tokens:953` elected once A was back: sites 0 and 2 were followers at term 2, with commit moving from 4 to 6 at 04:41Z, and site 1 never joined. The row wrote nothing to it. `repo:0` holds three placement tries, all at commit 0 with no config (`tests/m6/raw/m6a/m6aL/unmoved-status.jsonl`).
- **The judge, raw and fixed.** `home`'s own lines (`judge.jsonl`, 04:53Z) read writes 0 of 37 and home 72 short, with the 4 controls changed. All three are harness causes, not row faults:
  - **writes:** the read went to a follower by read-index while each group idled, so it answered `notLeader`;
  - **home:** the config read ran while groups idled (60 unread, some stale), and the judge required home = A for the 8 register chains homed on B;
  - **controls:** the controls sat on [1, 2, 3] with the spare, so the restore moved them off it, as it should.
- **The fixed harness:**
  - `bf50-edge`'s `/read` campaigns the home and reads it as leader.
  - The judge merges a reread of unread configs and compares each home with its own before-loss home.
  - Controls exclude a chain on the site the move leaves.
  - `before` excludes unjoined members and names on two sites.
  - Run once over m6aL's saved data, it gives (`judge-fixed.jsonl`):

| Check | Result |
| --- | --- |
| line2 | 3,094 judged (24 excluded), 3,090 held on A, 3,090 moved, 0 short; controls 4 of 4 untouched: PASS |
| home | 3,090 of 3,090 back on [0, 1, 2], none on the spare, each with its own home: PASS |
| signers | the replace path signed by the executor only: PASS |
| line3 | 451 steps, rows from sites 1 and 2, single-copy refill first: PASS |
| writes | 37 of 37 acked values read back from the home leaders (`read-home-leader.jsonl`): PASS |

  The fixed line 2 passes because `before`'s exclusions now name `tokens:953` and `repo:6201` to `repo:6204`. m6aL's verdict stays FAIL on line 2 as ruled: those exclusions were written after the row ran.
- **Shards stopped.** Every config was checked first: none names the spare. The spare's 25 shards were stopped at 04:55Z, and site a's 25 and site b's 3 at 05:04Z, each through its own `/teardown` from a scratch Worker that was deleted after. The container list then read 0 active on every site. Containers cost $2.69 in 30 minutes before and $0.10 in the 6 minutes after. The day ended at $16.63 on acctcost (OPEN-QUESTIONS R67). `bf50-row.sh`'s `teardown` phase does this from m6aL2 on.
- **For m6aL2** (next UTC day, fresh chains):
  - controls that the restore cannot move: not possible as Tri chains, since a Tri group needs three voter sites and the only ones besides A are 1, 2 and the spare 3, while Solo needs an evaluation genesis. So the controls stay Tri on [1, 2, 3], and the judge counts them for line 2 only and leaves them out of `home`'s count, as it now does. A 4th site was ruled not worth a site day. **What the controls show:** that the replace leaves alone a chain with no voter on A. They say nothing about the restore, which moves every chain off the spare, them included;
  - one timed read by a real client through the product edge to a repository idle through the restore, for R66 (`bf50-probe.mjs`): `seed`, before the load, makes a user, its `Placement` (Tri, home A), its opened `Authz` chain and a PAT as the operator through the bench's `/op` (R14), then the repository `r66` through the edge's own `POST /api/v3/user/repos` under that PAT. Its chains (`repo:`, `authz:`, `tokens:`, two `names:` shards) are homed on A, so the loss and the restore move them like the rest, and no register put or `home` read touches them. `probe`, after `home`, reads each one's members' `/status`, waits 15 s so nothing ticks, then times `gh api repos/<owner>/r66` and `git ls-remote` from the client, cold and then again;
  - `teardown` after `probe`.
