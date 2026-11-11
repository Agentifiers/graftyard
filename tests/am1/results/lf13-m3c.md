# LF-13 on `graft-m3c` (2026-11-11)

LM-1's row LF-13, both modes, 3 runs each, on its own install `graft-m3c` (`tests/am1/results/install-m3c.md`), so it no longer queues behind the 500-agent rows on `graft-m2b`. Branch `lf13-m3c`, from `origin/main` `422bc02`, merged with `c1db5ac` (a site-only page). `graft-m1-*` and `graft-m2b-*` were not touched.

Gate line (`build/BUILD-PLAN.md`, AM1-WP6, LF-13): "Every held poll returns within 1 s with its cursor and no events, new polls are refused until it catches up, and no event is lost". Fail line: "Any poll held or accepted while it is behind, or any event skipped".

## Summary

**LF-13 passes, both modes, 3 of 3 runs each, on the final build** (site day 4, `9d59283`; batch 4, 04:18Z to 04:28Z), against the gate line under either reading of A26:

- **Gap: pass.** A real gap formed in 3 of 3 runs: F's feed stopped with `why: gap` after the new leader's first round. Every held poll (999 of 999 in each run) came back with its cursor and no events, 0 before the lift, within 1 s of the lift (all at 99, 306 and 799 ms after it) and within 1 s of F's stop (0 ms). New polls were not refused while behind: F answered `lagging` to 1 of 50 probes per run, because a gap ends at F's next accepted round, 1 ms after it starts; 0 to 2 probes were admitted after the lift but before F's stop. No event was lost: F and the reference listed the same 5, 4 and 2 lease events from the pre-fault cursor.
- **Install: pass.** `InstallSnapshot` ran in 3 of 3 runs, with truncation past F at 9.7, 8.1 and 7.0 s into the sever. Every held poll (999 of 999 each) came back with its cursor and no events, 0 before the lift, within 1 s of the lift (at 788, 83 and 437 ms) and within 1 s of F's stop (0 ms). New polls were refused while F caught up: 48, 50 and 49 of 50 `lagging`, and 2, 0 and 1 probes were admitted in the window after the lift but before F's stop. No event was lost: after the install F answered the pre-fault cursor `gone` (the ring rule), and from the oldest cursor it serves, F and the reference listed the same 15, 14 and 13 lease events.

The "any poll held or accepted while it is behind" half of the fail line still has one opening: in the window between the lift and F's stop (83 to 799 ms here), F does not yet know it is behind and admits new polls. That held 0 to 2 of 50 probes per run. None of those polls returned an event past the gap or hole, because F's stop answers them too.

Earlier batches on this install failed narrowly and each led to a fix below (`tests/am1/results/lf13-m3c.md`, "Earlier batches").

## The fixes

1. **A stop between two waits still answers a held poll** (`3597c6f`, `workers/site/src/feed.rs`). A held poll is woken by every fed entry and then registers a new waiter. A gap or an `InstallSnapshot` that began between two of its waits drained no waiter of it, so the poll waited out its 25 s. That is why F stopped only 193 and 0 of 999 on `graft-m2b`. The feed now counts stops (`Feed::stops`), and a poll admitted before a stop answers at once with its cursor and no events (`Feed::wait`). Test: `feed::tests::a_stop_between_two_waits_of_a_held_poll_still_answers_it`.
2. **A lease acquired and released in one catch-up batch reports both** (`429eede`, `crates/git/src/feed.rs`, `Ring::feed`). A follower's catch-up applies many entries in one input and feeds each with the tiers as the whole input left them, so such a lease never entered the ring's live set and its `Released` was lost. This explains fanout r3 gap 2's lease event missing on F. Test: `a_lease_acquired_and_released_in_one_catch_up_batch_reports_both` (`crates/git/tests/lease_gates.rs`).
3. **The ring restarts at an install's base at once** (`7972504`, then `606b403` and `9d59283`). The input that installs a snapshot applies through it and feeds the ring no entry, and the member follows again in that same input. `dfa5410`'s hole check fires only when the next entry arrives, so until then a poll from the pre-fault cursor answered `hold` and skipped what the snapshot covered (batch 1: 19 and 11 events missing on F). `Ring::skip` restarts the ring at `through`, with the live lease set the installed tier holds, so a cursor from before gets `gone` and a lease the next entry releases still reports `Released`. `606b403` seeded the live set from the installed tier. `9d59283` moved the call from `Repo::stepped`, which runs before the install's table swap is flushed and read the old tables (batch 3: 1, 0 and 2 `Released` missing), to `crate::load::finish_install`, which reads the loaded tables. Test: `a_ring_skipped_past_an_install_refuses_older_cursors_before_the_next_entry`.
4. **`/sever`, the gap fault** (`3597c6f`, test-faults only, `workers/site/src/host.rs`, `member.rs`). `POST /sever {"for_ms": MS}` on one member fails every call to and from it, except the observation routes, until MS ms pass; `{"for_ms": 0}` lifts it. It is set and lifted in that object at once, unlike the site fault's 1 s plan cache. The site fault could not stop F before the transfer, so O was level with F in 8 of 9 runs. With the sever, gap mode waits until F's `last` stands still while O's is past it, then transfers to O and commits on O. O's first round then starts past F's last entry. A gap formed in 12 of 12 runs here. The route answers `{now, until}` on the member's clock, so the driver times the lift on F's clock.

Planted bugs (`tests/am1/lf13/plant.py`, on `tests/am1/rest/plant.py`'s runner): 7 of 7 caught. Two plants hit the stop count, two the catch-up batch, and three `Ring::skip`: no restart, events kept, live leases forgotten. `finish_install`'s call has no unit test, because it needs a Durable Object's storage. The hosted install runs check it.

Checks before each fix commit: `cargo fmt`, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and for ops (ops has no `test-faults` feature), `cargo test --workspace` (670 passed), `go test` in `containers/git/server`, and the three `scripts/check_*.py`. `crates/raft/src` did not change, so the long sweep was not run. No wire enum changed.

## Driver changes (`tests/am1/rest/lf13.mjs`)

- Both modes fault F with `/sever` (`cc353f3`).
- Held polls ask for `lf13-held/**`, which no write of the driver touches, so only F's stop, or their own 25 s, returns them. The driver's lease commits are on `lf13/**`. The feed API (`design/AGENT-LAYER.md`, activity feed, `paths=`) allows this, so no server change was needed.
- A26: every answer carries `answeredAt` on F's clock (`3597c6f`). The driver reads the lift from the `/sever` answer's `now` and F's stop from its feed's `lastStop.at`, and reports `fromLift` and `fromStop` for every held poll.
- Each run starts with the home (site 0) leading and every member following. A leader off home hands off to home 10 s after it starts (`maybe_hand_off`). After the previous run's transfer, that landed mid-run and made F the leader, so every held poll answered `notServing`.
- Install: the archive cap is 1,024 B, and two lease loops run during the fault, so truncation passes F in 6 to 11 s, well inside the polls' 25 s. The cap is lifted once truncation has passed F. Under the cap, the leader archived about once a second with no commits, its install source moved under F's load, and every finished load was refused with "install refused: the loaded fold is not at through". F stayed `installing` for minutes (`tests/am1/raw/m3c-try2`).
- When F answers `gone` after an install, F and the reference are compared from the oldest cursor F serves. Both lists are cut at the lower of the two members' cursors (`8c6048f`). An entry one member had applied and the other not yet had counted as an extra event on F.
- Leases are `m3-member`'s Shared leases under its PAT, so the run needs no installation token and no `graft-ops`.

## Runs

`LF13_ENV=tests/am1/lf13/env-m3c.sh bash tests/fanout/lf13.sh` as the orb service `lf13m3c`, `HELD=999`. Logs: `tests/am1/raw/m3c-*/` (git-ignored); the recorded batch is `tests/am1/raw/m3c-batch4`. Times are on F's clock. "From lift" is a held poll's `answeredAt` minus the `/sever` answer's `now`. "From stop" is its `answeredAt` minus F's `lastStop.at`. "Before stop" counts the probes F admitted (`hold`) after the lift but before it learned it was behind.

### Batch 4 (recorded): site build `9d59283`, 04:18Z to 04:28Z (`tests/am1/raw/m3c-batch4`)

| Run | Fault reached | Stopped / why | Before lift | With events | From lift: ≤ 1 s (all at) | From stop: ≤ 1 s (max) | `lagging` of 50 | Held before stop | Events F / ref / missing / extra |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gap 1 | gap: F stood at 251 while O held 252 (1.6 s), transferred, O led; sever 10.6 s | 999 / gap | 0 | 0 | 999 (99 ms) | 999 (0 ms) | 1 | 0 | 5 / 5 / 0 / 0 (`hold`) |
| gap 2 | gap: 263 vs 264 (2.1 s); sever 10.0 s | 999 / gap | 0 | 0 | 999 (306 ms) | 999 (0 ms) | 1 | 1 | 4 / 4 / 0 / 0 (`hold`) |
| gap 3 | gap: 275 vs 276 (1.6 s); sever 14.7 s | 999 / gap | 0 | 0 | 999 (799 ms) | 999 (0 ms) | 1 | 2 | 2 / 2 / 0 / 0 (`hold`) |
| install 1 | truncated to 2,436, F at 2,428, at 9.7 s; sever 11.0 s | 999 / install | 0 | 0 | 999 (788 ms) | 999 (0 ms) | 48 | 2 | 15 / 15 / 0 / 0 (F `gone`, then from its oldest cursor) |
| install 2 | truncated to 2,548, F at 2,542, at 8.1 s; sever 10.4 s | 999 / install | 0 | 0 | 999 (83 ms) | 999 (0 ms) | 50 | 0 | 14 / 14 / 0 / 0 (same) |
| install 3 | truncated to 2,659, F at 2,657, at 7.0 s; sever 8.4 s | 999 / install | 0 | 0 | 999 (437 ms) | 999 (0 ms) | 49 | 1 | 13 / 13 / 0 / 0 (same) |

Every held poll in every run answered `hold` with a cursor (999 of 999). F served the pre-fault cursor again 22.6 to 24.1 s after the lift in gap mode, and answered it `gone` 48.6 to 51.9 s after the lift in install mode. Driver-side timing, lift to the poll's arrival at the driver, was at most 609, 831, 1,302, 1,371, 707 and 1,012 ms; this includes the tools edge round trip from the orb.

### Earlier batches (diagnostic; each led to a fix above)

| Batch (site build) | Gap: gap formed / stopped / from lift ≤ 1 s / from stop ≤ 1 s / missing on F | Install: install began / stopped / from lift ≤ 1 s / from stop ≤ 1 s / `lagging` / missing on F |
| --- | --- | --- |
| 1 (`3597c6f` + `429eede`) | 3/3; 999 each; 0, 999, 999 (max 1,099, 981, 810 ms); 999 each (0 ms); 0, 0, 0 | 3/3; 999 each; 999 each (max 582, 350, 230 ms); 999 each; 49, 49, 49; **19, 11, 1** (F answered `hold` from the pre-fault cursor in 2 runs) |
| 2 (`7972504`) | 3/3; 999 each; 0, 999, 999 (max 1,177, 927, 827 ms); 999 each; 0, 0, 0 | 3/3; 999 each; 999 each (max 358, 226, 975 ms); 999 each; 49, 49, 48; **2, 1, 1** (`gone`, then a `Released` missing from the oldest cursor) |
| 3 (`606b403`) | 3/3; 999 each; 999 each (max 350, 638, 860 ms); 999 each; 0, 0, 0 | 3/3; 999 each; 999 each (max 813, 856, 246 ms); 999 each; 48, 48, 49; **1, 0, 2** |

Before batch 1, the driver needed the changes above (`tests/am1/raw/m3c-predeploy`, `m3c-handoff`, `m3c-try1` to `m3c-try4`):
- The first gap run with `/sever` formed a gap, and F stopped all 999 polls (`why: gap`). Two runs then failed to start with "need a leader and two followers", because the group was asleep.
- In 3 gap runs, F led during the hold (home hand-off): 999, 999 and 665 polls answered `notServing`.
- In 3 install runs, the 4,096 B cap never truncated past F within 17.5 s.
- In 2 install runs, F stayed `installing`, because the leader's install source moved under its load.
- Three gap runs in `m3c-try1` formed a gap in 3 of 3.
- In `m3c-try3` (site day 1), 3 install runs reached the install, but F answered `gone` and the driver did not yet compare from the oldest cursor, so 15 and 16 read as missing. `m3c-try4` was 2 gap runs on that build.

## A26: "within 1 s" of what

Both readings are reported. In every gap and install run on this install that reached its fault, F answered all 999 held polls in the same millisecond as its stop (from stop: max 0 ms). From the lift, the reading depends on when the new leader's or installing leader's next round reaches F, up to one `CALL_DEADLINE` (1 s) after the lift. The leader resends to an idle follower every `CALL_DEADLINE`, and to one marked `cannot_follow` every `CANNOT_FOLLOW_RETRY` (5 s). So from the lift the row can fail on timing alone: batches 1 and 2 had 0 of 999 within 1 s in one gap run each (1,099 and 1,177 ms), while every poll still came back at F's stop. A26 stays open for a lead decision; this round only measured both. On the recorded batch both readings pass, so LF-13's verdict here does not depend on it. `build/BUILD-PLAN.md` and `decisions/OPEN-QUESTIONS.md` are left for the lead thread to update.

## Cost

`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-11T01:11:14Z 2026-11-11T04:29:24Z`, at `research/prices.json`'s overage prices with included allowances ignored, for `graft-m3c-*` alone: **$2.55**. The window covers the build, four site days and 33 LF-13 runs: 49,147 Workers requests and 185,659 ms of CPU; 830,253 Durable Object requests, 1,557,033 rows written and 189,990 s of active time; and containers at $0.55 (411 CPU s, 204,838 GiB·s, 409,676 GB·s). The recorded batch alone (04:18Z to 04:28Z) cost $0.23. R2 and Queues are not counted; their use is small. The $15 cap was not approached.

## Design contradictions (quoted; no design file edited)

- `design/AGENT-LAYER.md`, activity feed: "A member holds polls only while following: its last AppendEntries left no gap, no `InstallSnapshot` catch-up, and it applied an entry within FEED_BATCH of the leader's `at`". A member is "following" again in the very input that installs a snapshot. Its ring had no entries there until `7972504`, so "no event is lost" depended on the ring's hole check, which ran only at the next entry.
- `build/BUILD-PLAN.md`, LF-13: "Every held poll returns within 1 s". From the lift, this is bounded below by the leader's resend interval to a recovered follower (`CALL_DEADLINE`, 1 s), not by the feed. A poll the member answers at its stop can still read as 1,177 ms after the lift (A26).
- `build/BUILD-PLAN.md`, LF-13 also says "then its log truncated past the follower for an `InstallSnapshot` catch-up". The test-faults archive cap that makes this happen in seconds also moves the leader's install source about once a second. A follower's load then never finishes against the source it began with. In production, `LOG_MAX_BYTES` and `SUM_EVERY_MS` (60 s) make that rare, but nothing in the design ties how fast the source moves to how long an install takes [inference].

## Commits on `lf13-m3c` (pushed, not merged)

- `429eede` Feed ring keeps a lease acquired and released in one catch-up batch.
- `3597c6f` Site feed: a stop between two waits still answers a held poll; LF-13 sever fault.
- `cd27f7f` Merge `origin/main` (`c1db5ac`, a site page only).
- `cc353f3` LF-13 driver for graft-m3c: member sever, both A26 readings, no installation token.
- `7972504` Feed ring restarts at an install's base at once, so an older cursor is gone.
- `606b403` `Ring::skip` seeds the live leases from the installed tier.
- `8c6048f` LF-13 driver: compare F and the reference up to the lower of their cursors.
- `9d59283` Site: restart the feed ring from the loaded tables when an install finishes.
- This document and `install-m3c.md`.
