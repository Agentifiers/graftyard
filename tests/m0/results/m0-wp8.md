# M0-WP8: restore window (2026-10-30)

`src/wp8/` is a copy of the M0-WP2 Raft prototype (`src/raft/`, not edited) whose state machine is a git-ref fold. It runs as its own scripts, `graft-wp8-a` (enam hint), `graft-wp8-b` (weur), and `graft-wp8-c` (apac), each binding the other two sites' `Wp8Member` class with `script_name`, and `graft-wp8-edge`, the only public entry, behind the harness bearer token. Three buckets, `graft-wp8-a`, `-b`, and `-c`, were created with the same location hints. Nothing binds `graft-m0-tools`, `graft-raft-*`, or `graft-wp4-*`. `run/wp8.ts deploy` deploys; `run/wp8-replay.ts b028|neg|install|window` runs the tests.

## What was built

**Kept from the copy.** Terms and votes, the log with chained digests, AppendEntries with `verify` (prev digest, entry digest, `at` never decreasing, and each ref event's old value against the fold), majority commit, one entry in flight, and request-id dedupe. Leases, pre-vote, `suspect`, and transfer were dropped because B0-28 does not measure them. Elections run when the test asks (`/campaign`).

**The fold** (design/DESIGN.md, Shape, "Packs"; design/SHARED.md, "Refs and pushes" and "Repository repair, tombstones, and retention"):

- `refs`, and `ref_log`, with one row per ref value: the index and `at` that set it, and the index and `at` that replaced it. A row is pruned once it was replaced more than `RESTORE_WINDOW` (35 days) before the applying entry's `at`. A per-ref watermark records the newest pruned index, so `decide` can tell "this ref was absent at `to`" from "`ref_log` no longer knows".
- Packs. A push of at most `INLINE_MAX` (scaled to 8 KiB) is inline in the entry. A larger one is placed by the edge as one stored pack: create-only puts (`If-None-Match: *`, R2-checked SHA-256) to all three buckets, durable at two. After the commit, the leader writes a create-only `Committed` marker per stored pack.
- Archiving (leader). The range is committed entries older than 30 days, or over `LOG_MAX_BYTES` (scaled to 160 KiB). The leader first places the range's inline objects as one stored pack and commits `Promoted`. It then puts the archive object under `archive/<chain>/<from>-<to>-<sha256>` in its own site's bucket and the next one. It HEADs both and commits `Archived { key, from, to, digest, fromAt, toAt, sites }` only if both are present with that digest. `decide` refuses an `Archived` that does not extend the archived prefix, that names fewer than two buckets, or whose range still has unpromoted inline packs.
- Snapshots (one follower, by `to / SNAPSHOT_EVERY` over the followers). It snapshots every 40 entries (`SNAPSHOT_EVERY` scaled) and at least once a calendar day. Every fold table goes to `snapshot/<chain>/core/0-<to>-<sha256>` in two buckets, and the leader HEADs both and commits `Archived` with `from = 0`.
- Truncation (every member, the leader too). A member walks the committed archive records in order. It drops a range only if the range is applied, older than 30 days (or the log is over the cap), and present with its digest in both buckets by its own HEAD. A failed HEAD stops the walk. A member that holds the range's entries rebuilds the same bytes, checks the digest, and re-puts the missing copy create-only, unless the test turns repair off.
- `InstallSnapshot`. A member that needs an entry at or below the leader's base gets the newest committed snapshot and the archive records from there to the base. It HEADs every key in two buckets, applies the snapshot rows, replays and verifies the archived digest chain, and then follows.
- GC (leader, once per calendar day). Roots are `refs` plus every `ref_log` value. It reads every live pack from R2 (checksums verified) and walks the closure over real git links. It writes one geometric pack of live objects and one cruft pack of unreachable ones, each object carrying the time it left the root set (git's cruft mtimes). It commits `Compacted { consumed, produced, readAt }`, and only then deletes the consumed packs and markers, at most `GC_RATE` (20) per run. Cruft expires 14 days after it left the root set. `decide` refuses a `Compacted` if any ref changed after `readAt`.
- The orphan sweeper deletes a pack with no marker, older than 24 h, not live, whose request id never committed. When the request committed, it writes the missing marker instead.
- `RestoredTo`. `/indexAt` finds the last index with `at <= t`. When that entry is truncated, it reads the archive object whose committed `fromAt` covers t. The leader then reads `ref_log` at `to`, walks the targets' closure over live and cruft packs from R2, and checks every reached object's SHA-1. The command carries what it read (`RestoreCheck`, like `deps` read once through `fresh`). `decide` refuses if `to` is outside the window, if `ref_log` was pruned past `to` for a named ref, if a `Compacted` committed after the walk, if the targets differ from the fold's `ref_log`, or if anything is missing or unreadable.

**Packs are a faithful object model, not git packs.** The runner makes every object with real `git` (commits, trees, and blobs with real SHA-1s). The edge checks each SHA-1, parses each object's links from its raw body, and stores packs as R2 objects that list (type, raw body) per object. The member never runs `git`, so no container is involved. Why this is enough for B0-28: the row asks whether refs return to their targets and whether every object they reach is still readable after GC and archiving. That depends on root sets, cruft expiry, deletion order, and R2 presence. It does not depend on the pack encoding, whose rehydration B0-22 already measured in a container. A delta chain inside one pack file cannot lose an object on its own. The model omits `index-pack --strict` and delta bases across packs (see "Not tested").

**The clock.** Every member's calendar time is real time plus an offset the test sets through `/clock`, plus a fixed per-site skew (b +2 min, c -2 min). An entry's `at` is `max(leader calendar, previous at)`, so it never decreases across the day-12 leader change from a to b, whose clock runs 2 minutes ahead. Raft's own timers stay on the real clock. Constants scaled for the replay are marked [scaled] in `src/wp8/types.ts`.

**The audit** (`/audit` on the edge) checks the fail condition from outside the members. Each member records every truncation and install (base before and after, and the keys it relied on). The audit requires each moved range to be covered, with no gap, by archive records committed in the leader's fold, or by a committed snapshot plus archives for an install. It also HEADs every committed key in its two buckets with its digest.

## Runs

**B0-28 (`wp8-b028-r3`).** Days -40 to 0 build history: `main` gets a push every day, `feature` every third day, and `release` every fourth day. Every fifth push is a stored pack and the rest are inline. One maintenance pass (archive, GC, sweep, snapshot, truncate) runs after each day's pushes. On day 0 the targets are recorded: `feature` 78f4a566 and `release` 3aae8759, whose closure is 129 objects. On day 1, `feature` is deleted and `release` is force-pushed to an unrelated root commit. Days 1 to 30 continue daily pushes and passes. Leadership moves from a to b on day 12. Follower c is offline from day 16 to day 25 and catches up by AppendEntries, because the leader's base had not passed c's log. On day 30 the runner asks `/indexAt` for day 0 at 04:00 and restores both refs.

| Measure | Value |
| --- | --- |
| Simulated span, wall time | 71 days (day -40 to day 30) in 550 s |
| Entries committed | 278 |
| `Archived` committed, archives and snapshots | 50 archives covering 1 to 150, 65 snapshots, each in two buckets |
| Truncations recorded (a, b, c) | 42, 42, 33; every member's base at 150 on day 30 |
| Day-0 index for the restore | 150, already truncated on all three members; its `at` was read from archive 145-150 in bucket b |
| `Compacted` committed by GC | 29, deleting 3 packs each time (cap 20) |
| Objects reachable after the restore | 397 (96 commits, 192 trees, 109 blobs), 0 missing, 0 bad SHA-1, read from 2 live packs in R2 |
| Fold digests after the restore | equal on a, b, and c at index 278 |
| Committed keys missing a copy | 0 of 115 |

The deleted `feature` objects and the old `release` objects stayed in the root set through `ref_log` for the whole run. GC therefore never moved them to cruft (0 cruft in every GC line), and the restore found them in the live geometric pack.

**Control without `ref_log` roots (`wp8-noreflog-r1`).** The same replay on a group whose GC roots are `refs` only. GC moved the old `feature` and `release` objects to cruft on day 1 (88 objects) and deleted them on day 15, 14 days later (88 expired). On day 30, `decide` refused the restore: "unreachable: 2 objects missing" (both targets). So the B0-28 pass depends on `ref_log` being GC roots, and the reachability check catches the loss rather than restoring refs to missing objects.

**Negative test (`wp8-neg-r5`).** Three ranges were aged past 30 days in turn by moving the calendar.

| Step | Injection | What happened |
| --- | --- | --- |
| N1 | Leader a's second put fails | a HEADed `{a: present, b: absent}` and did not commit `Archived`. No member truncated (bases 0, 0, 0) |
| N3 | Second put fails and a commits `Archived` without HEADing (a defective leader); repair off on every member | `Archived` 1-10 committed with one copy. a, b, and c each HEADed it, refused to truncate past it, and kept their logs (bases 0, 0, 0; `refused` 1 each). The audit listed the key as present in a only |
| N4 | Repair on for b and c | b re-put the missing copy from its own entries (same digest), HEADed both, and truncated to 10. c then truncated to 10. The audit found 0 keys short of two copies and 0 bad truncations |
| N2 | Leader a deposed after both puts, before `Archived` (`abandonAfterPut`) | Both copies of 11-18 existed, but no record committed and nobody truncated (bases 10, 10, 10). The new leader b archived the same range: same content-addressed key, put refused as existing with the same digest, then HEAD, commit, and truncate to 18 on all three |
| N5 | Audit control: c truncates by age alone (`blindTruncate`) while b's archive puts lose their second copy | The audit reported "c blind 18->25 at 2026-11-15 has no committed Archived" |

**Follower offline past the archive boundary (`wp8-inst-r1`).** c was offline from day -36 to day 0 while a and b archived and truncated to 24 and snapshotted at 102. On rejoining, c's next entry was below the leader's base. It loaded snapshot 0-102 after HEADing it in two buckets, needed no archives, applied 103 and 104, and caught up in 5.4 s. Fold digests were equal on all three at 104.

**Which time `ref_log` prunes by (`wp8-win-r1`).** Tag `v1` was set on day -40 and deleted on day 1, and the test restored it to day 0 on day 30. Pruning by replacement time (the default) restored it on all three members. Pruning by write time (`bywrite-`) dropped the value on day -5, 35 days after it was written and while it was still the tag's live value. `decide` refused: "ref_log no longer holds refs/tags/v1 at 84". Without the per-ref watermark, the same `ref_log` read would have returned "absent", and the restore would have deleted the tag [inference from the code path].

## Verdict

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-28 | Day 30 restore of a branch deleted and a branch force-pushed on day 1, with GC (29 compactions) and archiving (50 archives, 65 snapshots, every member truncated past day 0) running. Both refs returned to their day-0 targets on all three members. 397 reachable objects read from R2 with 0 missing and 0 bad SHA-1. All 115 committed `Archived` keys present in two buckets. 0 truncations without a committed `Archived`. Negative test: with a failed second put, a deposed leader's put, or a defective leader's unchecked `Archived`, no member truncated past the range | Both refs return, every reached object readable, archives named by committed `Archived` records in two buckets; fail if the restore is refused, an object is missing, or a follower truncates past a range with no committed `Archived` | **Pass**, on the object model and the scaled constants below |

## What limits the result

- **Packs are modeled.** Objects and their links are real git, but pack files are JSON lists of objects, not git packs, and nothing runs `index-pack` or `git fsck`. A pack whose deltas point at a base in another pack, a thin pack, or a geometric repack that drops a delta base would need a real-pack rerun with the B0-22 container.
- **Scale.** 278 entries and about 400 objects. `INLINE_MAX`, `LOG_MAX_BYTES`, and `SNAPSHOT_EVERY` are scaled (8 KiB, 160 KiB, 40) so every mechanism fires within a 71-day replay. A first negative run with `LOG_MAX_BYTES` at 24 KiB archived almost the whole log, because `Archived` and `Promoted` entries add log bytes. That is a scaling artifact at a cap near the log's size, not at 2 GB.
- **The clock is injected.** Thirty days are a clock offset. Nothing measures what runs, or fails to run, over a real month: Cron Triggers, alarms, R2 lifecycle rules, or object eviction.
- **Maintenance is driven by the runner** (`/tick` once a simulated day), not by Cron Triggers or alarms, and GC runs at most once a simulated day.
- **Snapshots read the fold in one synchronous step**, so `snap_undo` and batched multipart uploads were not built. At this size a snapshot is one put.
- **One chain is the network.** No forks, and no `Compacted` across member chains.
- **Bucket locations.** The buckets were created with `--location` enam, weur, and apac, and the R2 API lists their location as null. Which region each bucket is in was not checked.

## Not tested

- Real git packs, `index-pack --strict`, and rehydration of the restored state into a git cache shard.
- The orphan sweeper deleting an orphan: it ran every simulated day and found none to delete, because no placement failed.
- A whole-tier `ResetTo`, the metadata tier, child groups, and the vault's 35 days past GC.
- A restore while pushes run (the `Compacted`/`RestoredTo` race guards exist in `decide` but were not raced on purpose).
- Leadership changing in the middle of an archive step, other than the abandoned-put case (N2).

## Proposed design edits

1. **design/DESIGN.md, Shape, "Packs", the `RESTORE_WINDOW` comment.** Change "GC roots are each member chain's fold refs plus its `ref_log` fold table, which keeps every ref value of the last RESTORE_WINDOW." to "GC roots are each member chain's fold refs plus its `ref_log` fold table, which keeps each ref value until RESTORE_WINDOW after the entry that replaced it, and records per ref the newest index it has pruned, so `RestoredTo` refuses rather than read a pruned value as an absent ref." Evidence: `wp8-win-r1`. Pruning by write time lost a tag that was still live 5 days before the restore point, and only the watermark turned a silent deletion into a refusal. design/SHARED.md, "Refs and pushes" ("which keeps every ref value of the last 35 days") and its retention bullet need the same wording.
2. **design/DESIGN.md, Shape, "Packs", `pub struct Archived`.** Change `pub struct Archived { pub key: R2Key, pub from: Index, pub to: Index, pub digest: Digest }` to `pub struct Archived { pub key: R2Key, pub from: Index, pub to: Index, pub from_at: Timestamp, pub to_at: Timestamp, pub digest: Digest }`, with the comment "`from_at` and `to_at` let `RepoRestore` find the last index with `at <= t` inside an archived range from the fold alone." Evidence: in `wp8-b028-r3`, the day-0 index (150) was truncated on every member by day 30. `/indexAt` could find it only through the committed records' times and one archive GET (archive 145-150 from bucket b). The design's `to is the last index with at <= t` (Release group, `RepoRestore`) has no index from time to archive.
3. **design/SHARED.md, "Repository repair, tombstones, and retention", first bullet.** Change "Its `decide` reads the targets from the chain's `ref_log` and checks that each is reachable from live or cruft packs." to "The leader reads the targets from the chain's `ref_log` and walks their closure over the live and cruft packs before `commit`, as it reads `deps`. The command carries the targets and the pack-set index it walked, and `decide` refuses if the fold's `ref_log` disagrees, anything was missing, or a `Compacted` committed after that index." Evidence: `decide` is pure (design/DESIGN.md, the `Command` trait) and cannot read R2, but the walk needs every live and cruft pack. In `wp8-noreflog-r1`, this check is what refused a restore to two deleted objects.
4. **design/SHARED.md, "Refs and pushes", "GC is the only deleter of objects".** After "It deletes an object only 14 days after the object leaves that set, at a capped rate.", add "Its `Compacted` names the index its roots were read at, and `decide` refuses it if a ref changed after that index, so a push or `RestoredTo` that names an object GC just expired makes GC rerun rather than lose it." Evidence: the restore and GC each read state and then commit, and the prototype needed a guard on each side (`readAt` in `Compacted` and in `RestoredTo`). Not raced in this run [inference].
5. **design/DESIGN.md, Shape, "Packs", the `Archived` comment.** After "followers truncate only through a committed range whose key they can HEAD in two buckets", add "A member that HEADs a committed key in fewer than two buckets and holds the range's entries re-puts the same bytes create-only, checks the digest, and HEADs again, and an alarm fires while any committed key has one copy." Evidence: `wp8-neg-r5` N3. A committed `Archived` with one copy stopped every member's truncation indefinitely (the log grows toward `LOG_MAX_BYTES` with no exit). N4: repair from a member's own entries restored the second copy and truncation resumed. The design names the refusal and no way out of it.
6. **design/DESIGN.md, Shape, "Packs", the `Promoted` comment.** Change "It commits `Archived` for a range only after `Promoted` covers it." to "It commits `Archived` for a range only after `Promoted` covers every inline pack the range names; a range that names none needs no `Promoted`." Evidence: `Promoted.pack` is "always Stored", so a range with no inline pack has nothing to promote. In `wp8-b028-r3`, 33 of the 50 archived ranges named inline packs and 17 named none (counted from the archive objects in R2), and the prototype archived those 17 with no `Promoted`.

## G6: GC raced against pushes and `RestoredTo` (2026-10-30)

Open question G6 (decisions/OPEN-QUESTIONS.md) asks whether GC's `Compacted` needs a read index. The question is whether a push or `RestoredTo` that names an object GC is expiring can commit between GC's root read and its `Compacted`. Same bed (`graft-wp8-*`), redeployed with the race hooks below. `run/wp8-race.ts` runs the race, `run/wp8-race-summary.ts` counts it, and the raw rows are in `results/raw/wp8-g6-*.json` and `results/raw/wp8-noreflog-g6-*.json`.

**What changed in the prototype.**

- GC for the race is `gcRace` in `src/wp8/member.ts`. It reads roots and the pack set at index `readAt` without holding the leader's command queue. It places its geometric and cruft packs, waits a random hold, then submits `Compacted { consumed, produced, readAt }`. In the full design GC takes seconds to minutes to read and repack, and pushes keep committing meanwhile. The old `gc()` ran inside the queue, so nothing could interleave with it.
- `decide` refuses `Compacted` if a ref changed after `readAt`. On a refusal, GC first tries a cheap rerun: it walks the current roots over the objects it already loaded plus any new inline objects. If none of the objects it would expire is now reachable and the stored pack set is unchanged, it resubmits the same packs with the new `readAt`. Otherwise it runs a full rerun: re-read, replan, re-place. A full rerun leaves the first attempt's packs unnamed, for the orphan sweeper.
- A push that sends no objects (pointing a ref at a commit already stored) checks connectivity over the live packs at index `readAt`, and carries it. `decide` refuses it if a `Compacted` committed after that index (`pushExisting`). `RestoredTo` already carried its walk's index the same way.
- Group-name tokens turn each guard off, so all three members agree: `-nocg-` (the `Compacted` guard), `-nopg-` (the push guard), and `-norg-` (the `RestoredTo` guard).

**One trial**, on the simulated calendar, 40 trials per group:

1. Push branches `p<n>` and `r<n>`, each one commit with a 12 KB blob, so each is a stored pack. Delete both a day later.
2. One day after the deleted values stop being roots, GC moves their objects to a cruft pack. With `ref_log` roots that is 35 days after the delete; in the `noreflog-` groups it is at the delete.
3. On the expiry day, 14 days and one minute after that, three operations start together, each after a random 0 to 4 s delay:
   - GC, with a random 0 to 2.5 s hold before it submits `Compacted`.
   - (a) A push that points `p<n>` back at its deleted commit, sending no objects, with a random 0 to 0.8 s hold between its connectivity read and its submit.
   - (b) A `RestoredTo` of `r<n>`, with a random 0 to 0.8 s hold between its walk and its submit. With `ref_log` roots, the target is a value set the day before and deleted, which the design keeps. In the `noreflog-` groups, the target is the seeded value, whose objects this GC is expiring.

   Each command first takes the plan's own time (about 2 s of pack reads and placements), so commit orders fall both ways.
4. A refused push or restore is retried as a client would: re-read, then re-send with a fresh request id, up to 10 times. A refusal for missing objects is final.
5. Check: every object reachable from `refs`, and from `ref_log` in the design groups, is in a live pack in R2. A push or restore that committed must have its target present. One that was refused must have left its ref unset.

Each lost object is counted once, in the trial where it first went missing.

| Group | Roots | Guards off | Trials | Writes that committed inside GC's window | Commit orders seen | Trials that lost an object | Objects lost |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `g6-on-r1` | refs + `ref_log` | none | 40 | 22 | GRp 18, PRG 9, RGp 5, PGR 4, RPG 4 | **0** | **0** |
| `g6-on-r2` | refs + `ref_log` | none | 25 (stopped by a platform reset, below) | 22 | | **0** | **0** |
| `noreflog-g6-on-r1` | refs only | none | 40 | 22 | Gpr 18, PRG 9, PGr 7, RPG 5, RGp 1 | **0** | **0** |
| `noreflog-g6-on-r2` | refs only | none | 40 | 23 | | **0** | **0** |
| `g6-nocg-r1` | refs + `ref_log` | `Compacted` | 40 | 27 | GRp 13, RPG 9, PRG 7, PGR 7, RGp 4 | 23 | 23 |
| `noreflog-g6-nocg-r1` | refs only | `Compacted` | 40 | 25 | Gpr 15, RGp 11, PGr 7, RPG 5, PRG 2 | 25 | 32 |
| `g6-nopg-r1` | refs + `ref_log` | push | 40 | 32 | PRG 15, RPG 9, GRp 5, others 11 | 5 | 5 |
| `noreflog-g6-nopg-r1` | refs only | push | 40 | 29 | RPG 9, PRG 6, PGr 6, Gpr 6, others 13 | 8 | 8 |
| `noreflog-g6-norg-r1` | refs only | `RestoredTo` | 40 | 28 | RPG 12, Gpr 10, PRG 9, others 9 | 3 | 3 |

In the commit-order column, G is `Compacted`, P is the push, and R is the restore, in entry order. A lower-case letter means the command was refused in the end.

**Where the controls lost objects.** Every loss is the commit object that a committed push or restore pointed a ref at, which leaves the ref dangling. Classified by entry index:

- `Compacted` guard off: all 23 losses in `g6-nocg-r1`, and all 32 in `noreflog-g6-nocg-r1` (14 by the push, 18 by the restore), were a write that committed after GC read its roots and before its `Compacted` committed. GC expired the object while the write made it reachable again.
- Push guard off: all 5 losses in `g6-nopg-r1` and all 8 in `noreflog-g6-nopg-r1` were a push whose connectivity read came before a `Compacted` and whose entry came after it.
- `RestoredTo` guard off: all 3 losses in `noreflog-g6-norg-r1` were the same pattern for the restore.

So the race has two sides, and each guard closes one. In the `Compacted` window, the write commits first and GC's stale plan expires its object. In the write's window, GC commits first and the write's stale read finds the object. With all guards on, neither order lost anything in 145 trials, 89 of which had a write commit inside GC's window.

**Retries.**

- Every refused command settled (0 unsettled). Pushes and restores refused by a guard were re-sent by the client after a fresh read: 52 guard refusals of pushes and restores across the four guarded runs. They then either committed or were refused for missing objects.
- No refusal for missing objects was false: the object was absent from every live pack each time. In a design group, a push of `p<n>` ends refused when GC expired its commit first. That is correct, because the branch's value left `ref_log` 50 days earlier.
- GC committed in every trial: 145 of 145 guarded, within at most 3 attempts.

**What the guard costs.**

| Runs | Trials | GC reruns (trials) | Cheap reruns | Full reruns | Attempts per GC | GC time, median and max | Packs left for the sweeper |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Design roots, guards on (`g6-on-r1`, `-r2`) | 65 | 44 (68%) | 25 | 30 | 1.85 | 4.3 to 4.7 s, 8.9 s | 30 |
| Refs-only roots, guards on (`noreflog-g6-on-r1`, `-r2`) | 80 | 45 (56%) | 0 | 57 | 1.71 | 4.0 to 4.4 s, 7.6 s | 57 |
| `Compacted` guard off (both controls) | 80 | 0 | 0 | 0 | 1.00 | 3.3 to 4.5 s, 7.5 s | 0 |

These rates are for this test's load, not a forecast: every trial puts two writes inside a window of a few seconds on purpose. A GC rerun happens only when a ref changes during GC's own run. The expected count is GC's duration times the repository's ref-update rate [inference].

- **A cheap rerun** rechecks the objects GC would expire against the new roots and resubmits the same packs. It costs one more fold walk and one more entry.
- **A full rerun** re-reads every live pack and places new packs. It was needed whenever the write made an expiring object reachable again (in design groups, a push of an expiring commit), or a new stored pack arrived. That is the rerun that saved objects: 424 objects that a first plan would have expired were kept by reruns across the four guarded runs (the same object counts again in each trial that saved it). It also leaves the first attempt's placed packs unnamed (87 in all), which the orphan sweeper deletes 24 h later.
- GC's median time per run was 4.0 to 4.7 s guarded against 3.3 to 4.5 s unguarded. A rerun adds one entry round, or one more read and placement, a second or two at this size. Run-to-run spread is about as large as the difference.

**Platform note.** `g6-on-r2` stopped at trial 25 when the leader's Durable Object instance was reset during a `gcRace` call ("Connection closed: this Durable Object instance is no longer active"). The runner treats a failed call as fatal, so the run ended there. The 25 completed trials stand.

**Not tested here.**

- A member reset between GC's placement and its `Compacted`. The placed packs would be orphans, as in a full rerun.
- GC racing an `Archived` or `Promoted` entry. Neither changes refs, so the guard ignores them. `Promoted` adds a stored pack, which changes `consumed`. `decide` already refuses a `Compacted` that consumes a pack no longer live, but nothing refuses one that misses a newly promoted pack. That pack stays live, so nothing is lost [inference].
- More than one GC at once. One leader runs GC.

| Question | Result | Verdict |
| --- | --- | --- |
| G6: does `Compacted` need a read index? | Guards on: 145 trials, 0 objects lost, every refusal retried to a correct end. `Compacted` guard off: 48 of 80 trials lost 55 objects, every one a write committed between GC's root read and its `Compacted`. Push guard off: 13 of 80 lost. `RestoredTo` guard off: 3 of 40 lost. GC reran in 89 of 145 guarded trials (61%) under this deliberately dense race, 1.8 attempts per GC | **Yes**, on both sides: `Compacted` carries the index its roots were read at, and a push that names existing objects carries the index its connectivity check read, like `RestoredTo` |

**Design sentence to add.** design/SHARED.md, "Refs and pushes", after "It deletes an object only 14 days after the object leaves that set, at a capped rate." add: "GC reads its roots at an index and its `Compacted` names that index, and `decide` refuses it if any ref changed after it. GC then rechecks only whether an object it would delete became reachable: if none did, it resubmits the same packs at the new index, and otherwise it replans. A push that names objects it does not send, and `RestoredTo`, name the pack-set index their connectivity walk read, and `decide` refuses either if a `Compacted` committed after it, so the client re-reads and retries [measured, tests/m0/results/m0-wp8.md, G6]." The open question G6 can close with this sentence as its answer.

**Teardown.** `graft-wp8-a`, `-b`, `-c`, and `graft-wp8-edge` were deleted after this run, along with the three `graft-wp8-*` buckets. No later Milestone 0 row needs them: B0-30 (hot-group storage) is M0-WP9's, on its own bed. The code in `src/wp8/` and `run/wp8*.ts` redeploys the bed with `node run/wp8.ts deploy`.
