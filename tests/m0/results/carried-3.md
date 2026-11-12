# B0-02 on groups placed within `NEAR_RTT` (2026-11-12)

The operator's decision on F68 (`carried-2.md`): (a) bound the home's near round trip at placement, and (b) find
what the about 1 ms per KB of an append is. Reading B0-02 per placement was rejected. Branch `m0-carried-3`, from
`main` at `4f61b75`. Same load and edge as `carried.md` and `carried-2.md`: `tests/m1/wp2/run.ts bench`, 1 KB
commands through `graft-m3c-wp2-edge`, one new `Tri` register group per run led from site 0 (enam). Every group was
created through the new placement: the test edge's `create` calls the leader's `POST /place`, as
`graftyard_lookup::create` now does. Raw rows are in `raw-carried-3/` (each run's `.bench.json` and `/place` answer
in `.create.json`; `/stats` of three runs).

Site day on `graft-m3c` at 08:12Z from `m0c3-deploy` `32fc31d` (`m0-carried-3` `5741a9a` merged with
`origin/hot-shards` `e206fb0`, `test-faults`, hot-shards' shard image), versions a/b/c `a67119f5`/`2785d5ff`/
`535a8800`; runs 08:21 to 08:44Z.

## The design change

`Placement::new` refuses a voter set whose home's nearest other voter measured over `NEAR_RTT` for the home's
region (`PlacementError::NoNearFollower`, appended last): 100 ms for enam and weur homes, B0-12's median enam to weur
pair, and 198 ms for apac homes, its median enam to apac pair, the same quantile (`b0-12.json`). A new group's leader
runs `/place` once it leads. If the home is over its bound, the leader re-places the nearer follower with new
generations through `/replace`'s loop, up to `PLACE_TRIES` (8). At the cap it restores the old member and fails the
create with every try's round trips. The leader measures each candidate itself, so its `Rtt` row is the new member's.
The home leader's alarm runs the daily check: rows a day old are measured again, and a home over the bound on two
checks a day apart re-places the same way. `design/DESIGN.md` (the location paragraph, Shape `NEAR_RTT`,
`PLACE_TRIES` and `Placement`, "Placement is checked by latency", "Placement failures") and
`decisions/DECISIONS.md` ("`m0-carried-3`") carry it.

## B0-02: 16 runs

| Run | Offered | Carried /s | `/place` | Near RTT (ms) | Far RTT (ms) | Commands / entry | Round p50 / p99 | Leader confirm p99 | Commit p99 | Failed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 300 | 280 | kept (100 ms) | 100 | 254 | 58.3 | 186 / 287 | 49 | **540** | 0 |
| r300-2 | 300 | 280 | kept (81 ms) | 81 | 164 | 33.8 | 113 / 143 | 51 | **249** | 0 |
| r300-3 | 300 | 280 | kept (82 ms) | 82 | 241 | 36.6 | 123 / 174 | 51 | **290** | 0 |
| r300-4 | 300 | 280 | kept (85 ms) | 85 | 243 | 35.6 | 119 / 195 | 45 | **320** | 0 |
| r300-5 | 300 | 280 | kept (97 ms) | 97 | 155 | 41 | 138 / 157 | 31 | **292** | 0 |
| r300-6 | 300 | 280 | kept (87 ms) | 87 | 259 | 37.3 | 124 / 194 | 39 | **316** | 0 |
| r300-7 | 300 | 280 | kept (81 ms) | 81 | 236 | 36.4 | 121 / 190 | 35 | **324** | 0 |
| r300-8 | 300 | 276 | kept (95 ms) | 95 | 186 | 41.9 | 138 / 227 | 48 | **368** | 0 |
| r330-1 | 330 | 307 | re-placed: 126 ms, 3 tries | 93 | 181 | 47.7 | 144 / 220 | 27 | **370** | 0 |
| r330-2 | 330 | 308 | kept (76 ms) | 76 | 227 | 38.2 | 116 / 169 | 46 | **271** | 0 |
| r330-3 | 330 | 308 | re-placed: 112 ms, 4 tries | 100 | 219 | 62.7 | 182 / 301 | 45 | **538** | 0 |
| r330-4 | 330 | 308 | kept (75 ms) | 75 | 208 | 37.5 | 113 / 185 | 44 | **318** | 0 |
| r330-5 | 330 | 308 | re-placed: 114 ms, 1 try | 100 | 205 | 51.8 | 157 / 242 | 55 | **443** | 0 |
| r330-6 | 330 | 308 | re-placed: 115 ms, 4 tries | 97 | 223 | 52.8 | 155 / 269 | 79 | 614 | 0 |
| r330-7 | 330 | 308 | kept (95 ms) | 95 | 157 | 48.4 | 145 / 234 | 42 | **425** | 0 |
| r330-8 | 330 | 308 | re-placed: 106 ms, 2 tries | 89 | 210 | 45 | 136 / 197 | 31 | **323** | 0 |

**Verdict: 15 of 16 pass (8 of 8 at 300 offered, 7 of 8 at 330); r330-6 is between pass and fail at 614 ms.
0 failed commands in all 16.** Against `carried-2.md`'s 12 of 16 the bound removed every miss its runs traced to a
near follower over 120 ms: every group here has its near follower at 75 to 100 ms. 5 of 16 groups were re-placed,
in 1 to 4 tries (14 tries in all), and no create hit the cap. Commit p99 correlates 0.74 with the near RTT
over these 16 runs; the three at 100 ms reached 443 to 540 ms.

**r330-6's miss is one stall about 20 s in, not the placement.** All 208 of its commits over 600 ms fall in one window
of 500 commands (`m0c3-r330-6.stats.json.gz`). At 19.72 s the leader's storage confirm took 248 ms (its other 374:
p99 79 ms). From 19.78 s, no AppendEntries reached the near follower for 337 ms (its calls had been about 155 ms
apart). The next one carried what queued meanwhile, 139 KB, and its body took 347 ms to arrive (2.5 ms per KB,
against about 0.5 ms per KB in the run's other calls). Three rounds of 339, 464, and 389 ms followed, and rounds were
back at about 155 ms by 21.3 s. The follower's own confirms stayed at 13 to 16 ms, no call to it failed, and neither
member's ticker lagged. Without that window the run's p99 is 485 ms. The leader's AppendEntries do not wait for its
confirm, so what held the send is not shown. A stall of the leader's object that delayed both its `sync()` and its
outbound call, as F35's storage stalls do (`m0-wp4.md`), fits, but that is a hypothesis [unverified]. The near bound
does not remove it, and no change in this branch does. A tighter bound would leave more headroom: the 9 runs at 89 ms
or less all stayed at or under 324 ms. It would cost about 1.6 times the tries per create: 26% of the 27 measured
near pairs were at 90 ms or less, against 41% at 100 ms or less.

## The per-KB append cost (b)

Followers now record each `/raft` call of 1 KB or more as `[at, bytes, body ms, work ms]` (`/stats` `raftIn`): the
request body's arrival (`req.bytes()`), then decode, step, and the output gate together, since the isolate's clock
moves only across I/O. Matched to the leader's AppendEntries call times (send to answer) over 34 run-peers (16 runs
and the pilot, both followers):

| Part | Median ms per KB |
| --- | --- |
| Whole call (leader) | 0.76 |
| Body arrival (follower) | 0.54 |
| Decode, step, output gate (follower) | 0 in every call |
| The rest (network both ways, the leader's send, the reply) | 0.14 |

So the about 1 ms per KB is the binding's transfer of the request body, not postcard encoding, the SQLite write, or
the follower's `verify` (which ran within one clock tick in every call). The code cannot make it cheaper short of
sending fewer bytes. Compressing entries would flatter B0-02's repeated `x` values and not real pack bytes, and is a
wire change, so it is not done (`decisions/DECISIONS.md`, "The per-KB append cost is the request body's transfer").

## Checks and cost

`crates/raft/src` is unchanged, so the long sweep and Porcupine were not rerun. Full checks on `5741a9a` plus the
docs: `cargo fmt --check`, workspace clippy, wasm32 clippy for site, edge, and ops (with `test-faults` for site and
edge; ops has no such feature), `cargo test --workspace` (705 passed, 0 failed), `go test ./...` in
`containers/git/server`, and the three `scripts/check_*.py`: all pass. New test:
`crates/types/src/placement.rs`, `new_refuses_a_home_with_no_voter_within_near_rtt` (both sides of each bound, an
apac home, either side's row, the error's fields, and no bound without `near`).

graft-m3c usage in the window (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-12T08:10:00Z
2026-11-12T08:46:00Z`): $4.17, which includes hot-shards' idle `repo:65551`.
