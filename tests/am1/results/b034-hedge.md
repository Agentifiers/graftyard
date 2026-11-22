# B0-34: a human push's validate hedges to the next shard (`validate-hedge`), code only

Branch `validate-fallback`, from main `5a9883ab`. Nothing hosted ran. The design is `design/DESIGN.md`, Shape, "Git
cache shards (derived)", and journey 1, step 2. The decision, the arithmetic and the flips are in
`decisions/DECISIONS.md`, "`validate-hedge`".

## What the ruling asked, and what the data changed

1. **"A streamed human validate."** The straddling push was not streamed.
   - a1.mjs and Load34 push 20 KB human packs, under `INLINE_MAX` (1 MiB). They go through `place_teed` and
     `validate`, whose body was already replayable on every try.
   - So no cap or re-read is needed: every buffered pack can be resent.
   - Streamed packs, over 1 MiB, keep the probe-first rule (`streamed_site`), and the design says so. None of B0-34's
     human pushes stream. Only journey 9's 5% of agent packs at 1 MiB can, when the commands push them past
     `INLINE_MAX`.
2. **"A deadline from the human validate p99 outside the kill, plus margin."** That deadline would miss the line.
   - Human validates outside the kill, B0-34 mt13 to mt16 (each row's −5 s to +60 s around the kill dropped), 2,105
     pushes: p50 295, p95 1,122, p99 1,725, p99.9 2,763 ms.
   - A deadline of p99 plus margin is about 1.8 s. The straddler's client total with a deadline D is about
     1,617 + D ms (front 331, a second validate of about 250, commit 642, client 394). At 1.8 s it answers at about
     3.4 s, past 3 s.
   - So the build is a **hedge** at 1 s instead. A try unanswered for 1 s gets the next site's shard asked beside it,
     and the first deciding answer wins. A slow but healthy home that answers at 1.1 s still decides; a deadline would
     have dropped it and paid a second validate.
   - Predicted straddler: validate about 1,250 ms, client about 2.6 s [inference].
   - The cost: about 7% of human pushes send a second validate (7.0% passed 1 s outside kills).
3. **Agents' 2 s rule.** Agent validates get no hedge (mt16 p50 1,500, p99 5,129 ms).
4. **G30.** The last site is never hedged past. A rehydrating last-site shard still holds the push for its whole wait,
   so `chain-wait` holds.

## The code

| What | Where |
|---|---|
| `hedged`: n tries, a hedge timer on every try but the last, the first 200, 422 or 429 decides, a failure moves on at once unless a hedge already did | `workers/edge/src/lib.rs` |
| `VALIDATE_HEDGE` = 1 s; `validate_hedge(agent)` is `None` for agents | same |
| `validate` runs its tries through `hedged`, marks a passed shard cold, and puts `validate_site`, `validate_tries` and `validate_hedge` in `server-timing` | same |
| Straddling pushes: `STRADDLE_MS` (default `1000,500,0`) human pushes, each on a new branch, started that long before the kill call, line `push:human:straddle` | `tests/am1/rest/a1.mjs` |
| Hostile load: `HOSTILE_AT`, `HOSTILE_CLONE_PER_S`, `HOSTILE_CAP`, and `HOSTILE_K`, which asks each hostile leader for k through `/bench_seed` from `hostileAt` for 3 minutes and records the grants | `tests/fanout/b034.sh`, `tests/am1/rest/a1.mjs`, `tests/am1/load34/` |
| The verdict lines from a row's raws | `tests/fanout/hedgerow.py` |

`/bench_seed {"fanout": k}` goes through both warm sites' pool ledgers (`crate::fanout::force` →
`Grant::ask` → `/pool/grant` → `PoolLedger::ask` → `pool_grants` with `owner_share`). So a forced k exercises the
owner's budget the same way a stepped k does, and its answer carries the grants.

`hedgerow.py` on mt16's raws (`.worktrees/mt16/tests/am1/raw/`, `mwrlshin`, on this orb) finds the three pushes that
straddled the kill, at −1,056, −93 and −65 ms, with validates of 2,087, 1,236 and 873 ms. mt16's edge carried no
`validate_site`.

## Tests and plants

- `a_human_validate_asks_the_next_shard_beside_a_slow_one_and_the_last_try_keeps_its_wait` drives `hedged` on a fake
  clock. Its cases:
  - mt16's straddler: hedged, it decides on site b at 1,250 ms; unhedged, on the home at 2,087.
  - A slow home that still answers first decides.
  - A failure moves on with no hedge.
  - A passed try's failure starts nothing extra.
  - A hedge's 422 decides.
  - The last try waits 9 s unpassed.
  - Every failure is kept.
- `tests/m0carried/plant85.py`: 7 of 7 caught.
  - No hedge, which is the single-shard wait.
  - A hedged agent.
  - A hedged last try.
  - The passed shard not marked cold.
  - The slow try dropped at the hedge (a deadline, not a hedge).
  - A passed try's failure starting a second try.
  - A 422 that does not decide.

## The row this needs

The full B0-34 row on graft-m1, from merged main, under the standing queue and the $10 stop. Settings:
- `STRADDLE_MS=1000,500,0`.
- `HOSTILE_AT=480`, `HOSTILE_CLONE_PER_S=6`, `HOSTILE_CAP=16`, `HOSTILE_K=4`.

Lead the verdict with `hedgerow.py`'s lines:
- the kill-phase human push p99 against 3 s;
- each straddler with its `validate_site`, tries and hedge;
- the hostile owner's pool slots per warm site against `owner_share` 6.

Three hostile networks each asking k 4 want 9 pool slots, so the owner's share of 6 must bind.
