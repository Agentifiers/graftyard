# lm1-floor-dump: floor-keyed learner dumps, the shard gate, and the catch-up overlap trace

Branch `lm1-floor-dump`, from main `3e5219318`. This branch is code only, with no hosted run. The orchestrator asked
for it from B0-33 m61's analysis (`tests/am1/results/b033-m61-why.md`). Tags follow `design/SHARED.md`, "Read this
first".

## 1. `dump-floor`: a cached dump answers any read whose floor it covers

- **What changed.** `family::dump_bytes` and `dump_wait` take the read's grant floor. A cached dump records the state
  it was built at (`DumpAt`: the core tier's `applied`, whether the metadata tier was loaded, and whether the fold is
  a metadata child). The dump serves a read when it was built at or above the floor from the same kind of fold. On
  main it served only while both tiers stood exactly where it was built.
- **Why it is linearizable.** The reason is the same as `narrow_hit`'s: every applied entry is committed, and the fold
  is a function of the committed log, so a dump at index i ≥ floor holds every entry acknowledged before the read
  arrived.
  - A leader's own reads are unchanged in effect, because their floor is its applied index.
  - A loading member never takes a loaded member's dump, and a metadata child never takes its parent's.
- **Design first.** `design/DESIGN.md`, "Follower read", now states the rule. `decisions/DECISIONS.md`, "`dump-floor`",
  records it. The flips are any read answered below its floor, or the next B0-33 still showing `rebuild` sheds near
  m61's 17,264.
- **Test.** `fanout::tests::a_learner_dump_read_is_served_at_or_above_its_floor` covers these cases:
  - A waiter at floor 2 whose rebuild landed at 3 gets the dump at 3, though entry 4 applied before it was asked
    again. On main it sheds, so the test fails there.
  - A read at floor 4 waits and is then built at 4.
  - A loading member and a metadata child each build their own dump.
- **Plants.** `tests/m0carried/plant55.py` catches 4 of 4:
  - main's equality rule;
  - a dump one entry below the floor;
  - a loading member taking a loaded member's dump;
  - a child taking its parent's dump.
- The existing dump tests now pass the floor (`dump_rebuilds_are_spaced_but_cache_hits_are_not`,
  `a_dump_is_built_once_per_state_and_shared_by_every_read`) and pass unchanged otherwise.
- **Scope.** This change does not touch `host.rs`. Its only file is `workers/site/src/family.rs`, plus the tests in
  `fanout.rs`.

## 2. The shard gate before load (harness)

- **What changed.** `tests/fanout/rows.sh` now runs `tests/fanout/shardwarm.mjs` for sites 0, 1 and 2 before every
  load row:
  - Each run drives the bed's `/_bed/catchup?shard=N`, a client call that starts a stopped container.
  - It counts a site as warm only on a 200 whose `/_bed/shard_status` reports `rehydrating: false`.
  - A row whose shards are not all warm within `SHARD_GATE_MIN` (10 min) is skipped and not counted, as with the
    learner gate. `SHARD_GATE=0` turns the gate off.
- **Why the `warm` keep never answered within 25 s.** This was not a bug in the keep's timing. It was two things:
  - **The keep never starts a stopped container (by design).** The leadership's `warm` (`prsync::warm`) calls
    `/catchup?warm=1`. `graftyard_git::serve` maps `Caller::Warm` on a stopped container to `Serve::Skip`, which
    answers a 200 `{"skipped": "stopped"}` (`idle-shards`: a warm that started containers kept every network's shards
    billing for a day). So before load, the keep "warmed" b's and c's stopped shards in 115 to 279 ms every 16 s for
    the whole setup. Workers Logs show `shard warm 131375 site 1 142ms` and `site 2 266ms` from −229 s to −5.6 s.
    Those 200s were skips, and the old log line could not tell a skip from a warm shard.
  - **Under load, the shards were busy catching up.** Once load started b's and c's containers (19:40:58 and
    19:41:02), the next keeps waited on a running rehydrate. `Jobs::wait` waits out the run in progress and then the
    next one. Its 25 s `FIRST_FOR` fell inside 122 to 130 s rehydrates, so the keep logged "no answer in 25 s" on all
    three sites from +65 s. Site a's catch-ups were themselves 25 to 73 s then.
  - **Fix: logging only.** The keep now logs `shard warm N site S skipped: stopped` when the shard skipped
    (`workers/site/src/prsync.rs`). The behaviour stays as `idle-shards` decided. The gate above is what wakes the
    shards before a row.

## 3. The catch-up overlap trace (logging only)

The "3.1× over-ingest" in `b033-m61-why.md` was wrong, and the trace's first job was to settle it. **Correction:**
- Site a's `shard catch-up stored` lines come from four shard objects:
  - the network shard `e55d03dc`;
  - three pool shards (`ac9ea63b`, `b21da616`, `27d07f6c`), which `k = 4` keeps caught up.
- Joined to request ids, the totals split as follows:

  | object | catch-ups | entries over the row |
  | --- | --- | --- |
  | each pool shard | 283 to 298 | 740 to 742 |
  | `e55d03dc` (the home shard) | 22 | 349 |

- The log held 762 entries. So no shard read its log much more than once [measured, Workers Logs joined on `rid`]. The
  2,348 was the sum over four shards.
- **The home container's 2.7 vCPU.** That container runs the network shard and, at k = 4, the pool slots on the same
  site's instance [inference: `instcpu.py` names the network container]. Its 22 catch-ups ran 3 to 189 entries each,
  and the last, at +322 s, ran 170.5 s. Its `index_ms` per pack was 106 ms against the pools' 18 to 21. So the home's
  slowness is index-pack CPU per pack on one container, not re-reading [inference].
- **The trace.** Each `shard catch-up stored` line now also carries:
  - `obj`, the shard object;
  - `from`, the cursor before the run, as the object's last run in this isolate left it (`null` after a reset);
  - `to`, the cursor after;
  - `refetched`, the entries read that `from` already held.

  `tests/fanout` can then split a site's catch-ups by shard and count overlap directly. There is no behaviour change.
- **Verdict line for the next row.** `refetched` summed per object should be near 0. Any page read twice shows there.

## Records

`decisions/OPEN-QUESTIONS.md` has two new rows, and Total is 203, equal to the row count:
- **G30.** A rehydrating last-site shard answering 503 at once. Not built: it contradicts `chain-wait`'s rule that the
  last site keeps the whole deadline.
- **F103.** `own_stall` not excusing calls sent during a shorter open confirm.

## Checks

All of these pass on the branch:
- `cargo fmt`;
- workspace clippy;
- wasm32 clippy for site, edge and ops, and for site and edge with `test-faults` (ops has no such feature);
- `cargo test --workspace` (site: 97 passed, 0 failed);
- `go test` in `containers/git/server`;
- the three `scripts/check_*.py` (`check_links` lists only build outputs under the untracked `.worktrees/`, as on
  main);
- `python3 tests/m0carried/plant55.py`: 4 of 4 caught.

`crates/raft/src` is untouched, so the long sweep and Porcupine do not apply.

## The row this enables

One m2b site day from this branch's merge, then one B0-33 row on m60's script, with a $10 stop. Verdict lines:
- tier sheds by gate (rebuild, readsMax, late) against m61's 17,264 / 10,873 / 30,578;
- warming 503s against 486;
- writes ending 503 against 34.8%;
- agent ok/s against 58.9;
- `refetched` per shard object.
