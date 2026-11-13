# `fresh-repo`: LM-1's two fresh-repository gaps (2026-11-13)

Branch `fresh-repo`, from main `739f6e3`. LM-1's m23 run on `graft-m2b` (`tests/am1/results/lm1-memory.md` on branch `lm1-memory` `96e0df7`, rounds 10 and 11) reported two gaps when it made fresh repositories, neither traced:

1. One repository create answered 503 "0 of 3 joined", and each member said "RepoId(DbId(131316)) has no RepoOwner row".
2. On 131317 and 131320, `main` was missing from ls-refs for seconds after the 201, and `a1.mjs` stopped with "the repository has no main". LM-1's `a1.mjs` now waits up to 120 s for `main`.

## Summary

- **Both gaps come from one cause: a create held open for 1 to 2.5 minutes by the new groups' near placement** [measured]. On `graft-m2b` and `graft-m3c` the enam–weur pair measures 101 to 136 ms, over enam's 100 ms `NEAR_RTT`. A new group's leader re-places its weur follower up to `PLACE_TRIES` (8) times, and each try's `/join` measures 20 round trips to each peer, about 7 to 12 s. A create can make two new groups: its `Names` shard and the repository's own. `graftyard_lookup::create` waited on that `/place` before it returned. One create on main took 141 s, and 76 s plus 28 s of that were `/place`.
- **Gap 1 was not a create that returned success.** It was LM-1's driver's `GET /repos` sent while the create was still placing. `RepoOwner` had not committed, so the GET's group create (`ensure_group`) asked every member to join, and each one refused because it checks that row first. The GET existed because `mkrepos.mjs`'s `api()` re-sent the create's POST after about 60 s, got 422 "name already exists", and fell back to a GET.
- **Gap 2 was not an acknowledged push missing from ls-refs.** `mkrepos.mjs` took 422 plus GET 200 as a finished create, so `a1.mjs` started while the first create was still placing (131317). In 131320's case the client had abandoned the create at 180 s, so `push_init` never ran. Every ls-refs that LM-1's logs hold came after a push the server had not yet acknowledged.
- **Fix.** `create` asks the leader for `/place` with `{"background": true}`. The leader answers at once and runs the same placement under `wait_until`, holding the daily check's marker so two placements of a group never overlap. `mkrepos.mjs` sends a create once, with a 400 s timeout, and no longer counts 422 plus GET as a create.
- **Hosted, `graft-m3c`, 50 creates back to back, each followed by a push.**
  - 0 create failures and 0 creates over 60 s. Create wall p50 31.0 s, p90 35.0 s, max 55.2 s. The 5 before the fix ranged 30.5 to 141 s.
  - 0 reads after a 201 that were not 200.
  - 0 missing `main` after the create, and 0 pushes not read back on the next ls-refs.
  - 50 of 50 groups ended `Normal` with 3 voters, 24 of them re-placed after the create returned.
- **Checks.** fmt, workspace clippy, wasm32 clippy (site and edge, with and without test-faults), `cargo test --workspace`, and the three design checks pass.
  - No crates/raft change, so no sweep, Porcupine or plants run.
  - No cargo test can reach the change. It is the order of two Durable Object calls, and only workerd runs the members. The kept check is `tests/am1/fresh/fresh.mjs`, which fails on main's sites (one create of 5 at 141 s, over its 60 s bound) and passes on this branch's.
- **Cost** $0.91 for 20:20Z to 22:56Z on `graft-m3c` (`tests/fanout/m2bcost.py`), which includes the diagnosis. The cap was $10. Shards of repositories 65587 to 65700 were torn down.

## The create path, and where it went wrong

`POST /orgs/{org}/repos` on the edge (`workers/edge/src/repos.rs`, `create`):
1. `CreateRepo` on the owner's `Authz`.
2. `RepoOwner` on `names:<name_shard(Repo(id))>` through `commit_system`. That calls `graftyard_lookup::create` first, which makes the shard's group when the shard has never been used.
3. `placement`.
4. `push_init` (the README commit), which makes the repository's group through `ensure_group` → `graftyard_lookup::create`.
5. `InitRepo` on the new group, whose leader renders the 201.

`graftyard_lookup::create` joins the record's bootstrap members, campaigns until a member leads, and then (`place`) waited for the leader's `/place`. Each repository member's `/join` checks the `RepoOwner` row before it stores anything (`workers/site/src/member.rs`, `check_source`).

So until step 2's group was placed, `RepoOwner` did not exist. Any read of the repository meanwhile (`tier_rows`, `refs_page` → `ensure_group`) tried to make the group and was refused by every member: "0 of 3 joined". Until step 4's group was placed, `main` did not exist.

## Evidence

Each line comes from Workers Logs, through the observability telemetry API, over the scripts' windows [measured].

### LM-1's m23 creates on `graft-m2b` (2026-11-13)

| Repository | What the logs show |
| --- | --- |
| 131316 (`fm23new500`) | Edge POST ran 17:11:46 to 17:13:52 and returned 201. `names:994` was new: registry 404 at 17:11:55, its placement done at 17:13:32 after all 8 tries ("no try within NEAR_RTT; kept the nearest"). mkrepos re-sent at about 17:12:47 and got 422, then sent a GET at 17:12:57, which answered 503 "0 of 3 joined", each member reporting "has no RepoOwner row". The original create later succeeded |
| 131317 | mkrepos took 422 plus GET 200 as a create. The repository group's placement finished at 17:14:58 and `push_init` validated at 17:15:01, but `a1.mjs`'s first ls-refs had already reached the shard at 17:14:21, at index 4 with 0 refs |
| 131320 | The original POST started 17:43:50 and was cancelled at the client's 180 s abort, so `push_init` and `InitRepo` never ran. Placement finished at 17:45:28. The upload-pack at 17:45:57 found index 6 and 0 refs |
| 131321 | Create took 35 s. Its 1,704 ms wait for `main` is not explained: the edge logged one upload-pack (17:52:37, 200) [unverified] |

Create walls on `graft-m2b-edge` that day were 125.8, 125.7, 68.0, 112.6 and 35.5 s, and one more was cancelled at 179.9 s.

Why the client's first fetch threw after about 60 s, which started the re-send, is not known [unverified]. A direct fetch from this orb to `graft-m3c` survived 99.7 s.

### Before the fix, `graft-m3c` at main's sites (`8399807e` / `92d7411d` / `d4da9406`, edge `fe34113c`)

The first was the probe `m3org/fr-probe1` = 65587 at 20:27Z: 99.7 s, 201. `names:146` was new and took about 8 joins on site b.

Then `fresh.mjs frb 5` (21:47 to 21:53Z, `fresh-repo-data/before.jsonl`):

| Create | Wall | `/place` on the leader | Reads during the create |
| --- | --- | --- | --- |
| frb-0 (65588) | 141.4 s | 28.1 s (`names`) and 76.1 s (`repo:65588`, 8 tries, kept the nearest at 105 ms) | 5 × 503 "0 of 3 joined", then 18 × 200 |
| frb-1 | 31.9 s | 1.5 s | 1 × 503 |
| frb-2 | 30.5 s | under 0.2 s | none |
| frb-3 | 38.8 s | 11.0 s | 2 × 503 |
| frb-4 | 39.0 s | 8.1 s | 2 × 503 |

Every read and push after each 201 passed, because the driver sends each create once and waits for it. That matches gap 2: the missing `main` needed a driver that started before the 201.

### After the fix (site day 21:57 to 22:05Z, sites `e6f4e36f` / `2562e7c3` / `9dc135f2`, edge `73114b50`, `tests/am1/results/install-m3c.md`)

`fresh.mjs frc 50`, 22:12:39 to 22:52:54Z, one create at a time, each followed by a clone, a push to `main`, and two ls-refs reads (`fresh-repo-data/after-run50.jsonl`):

```
{"creates":50,"fails":{"create":0,"slow":0,"readAfter":0,"mainAfterCreate":0,"push":0,"mainAfterPush":0,"voters":0},
 "readsDuringCreate":{"200":93,"404":1,"503":35},"createP50":31037,"createP90":34993,"createMax":55182}
groups with 3 voters: 50 of 50; re-placed: 24
```

`/place` now answers in 17 ms to 1.5 s (p50 109 ms, 100 calls). The placements ran after the 201s, and the leaders logged 97 of them (`fresh-repo-data/after-placement-logs.txt`):
- 49 passed with no re-placement.
- 30 re-placed the follower in 1 to 7 tries, taking 8 to 63 s.
- 15 kept the nearest of 8 tries, taking 73 to 87 s.
- 3 kept the member they started with.
- None failed.

On main, each of those walls would have been part of a create.

A 3-create smoke before the run (`fresh-repo-data/after-smoke.jsonl`) passed every check except its own voter check. That check read the leader's config, and an idle group's lease lapses after its last request, so no member led. `fresh.mjs` now reads the committed config with the highest index instead.

## What is left

- **The create still takes about 30 s** [measured]: two bootstrap creates whose joins each measure 20 round trips to each peer (5 to 12 s per join set, `member/join` walls in the logs), plus `push_init` and `InitRepo`. Cutting it further means fewer join measurements, which is not this change.
- **A `GET /repos/{o}/{r}` sent while its create runs answers 503 "0 of 3 joined"** in the window before `RepoOwner` commits: 35 of 129 such reads [measured]. No read after a 201 did. The repository does not exist yet in that window, so GitHub would answer 404. Answering 404 when every member refuses for a missing `RepoOwner` row is a possible follow-up, recorded under "Beat" in `decisions/DECISIONS.md`, "`fresh-repo`".
- **LM-1's `a1.mjs` stopgap**, which waits up to 120 s for `main`, is not needed once mkrepos sends a create once, but it does no harm. It is on `lm1-memory` and was not changed here.

## Files

- `crates/lookup/src/lib.rs`: `place` sends `{"background": true}`. `PLACE_FOR` stays 240 s for members built before this change.
- `workers/site/src/member.rs`: `/place` with `background` runs `place_background`, which starts `place_inner` under `wait_until` behind the `near_check` marker.
- `design/DESIGN.md`: the "create fails" sentence on placement was stale since `m0-carried-4`. Invariants, "Placement is checked by latency", now has the create path.
- `decisions/DECISIONS.md`: "`fresh-repo`".
- `tests/am1/runs/mkrepos.mjs`: sends a create once.
- `tests/am1/fresh/fresh.mjs`: the hosted check.
- `tests/am1/fresh/siteday.sh`: this branch's site and edge day on `graft-m3c`.
- `tests/am1/results/fresh-repo-data/`: the rows above.
