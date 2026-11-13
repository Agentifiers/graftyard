# `pool-grants`: the per-owner pool budget on every path (2026-11-13)

Branch `pool-grants`, from main `a2687fe`. LM-1 found that `design/DESIGN.md` (Shape, `HOT_POOL`) said "`pool_grants` holds each owner to its share", but `pool_grants` ran only in tests. `shards_for` gave every network the same first k - 1 pool shards by rendezvous, and the leader's `Fanout` commit raised k with no grant. B0-34's lines "the hostile owner held to its per-owner pool budget" and "the monorepo's k reduced by another owner" would have measured missing code. This branch wires the budget on every path. The full B0-34 row stays with LM-1.

## Summary

- **Each site's first pool shard keeps a pool ledger, and the leader asks it before every `Fanout` step.** `git#<site>#pool:1` keeps a `PoolLedger` in its SQLite. The network leader asks both warm sites' ledgers at once (`/pool/grant`). Each ledger runs `pool_grants` over its live leases, asker last. The leader then commits 1 plus the fewer granted.
- **A hostile owner is held to its share, and another owner's step up never reduces a network.** The leader steps down only as `fanout` asks, and an unreachable ledger keeps k. Leases lapse unless renewed within LOAD_STALE.
- **`shards_for` serves each network its own granted slots**, so no pool shard serves two networks.
- **Hosted two-owner smoke on `graft-m3c`: every check passed**, 13 of 13 in run r3 and 15 of 15 in run r4. Run r3 also showed that one site could lease a network slots the committed k did not use. The leader now gives those back (`99a36c2`), and r4 checks for it.
- **Checks.** Full checks pass. **13 of 13 plants caught.** Hosted cost: $0.38, against the $15 cap.

## Change

| Piece | What it does |
| --- | --- |
| `PoolLedger` (`crates/git/src/shard.rs`) | One lease per network: its pool slots (`0..HOT_POOL`), its owner, and its last renewal. `ask(network, owner, want, now, share)` first drops leases older than LOAD_STALE. It then keeps every other lease whole and runs `pool_grants` with the asker last, so the asker keeps what it holds and grows only within its owner's share and the free slots. New slots are the free ones in the network's rendezvous order. A network with no lease asks from zero, so one whose lease lapsed never takes slots back. `renew` renews a lease without growing it. A gap of more than `LEDGER_GAP` (90 s [guess]) with no call counts against no lease, so a ledger outage frees nothing. |
| `pool_ask`, `granted_k` | The leader asks for the k it wants (`fanout`'s step, or the committed k, which renews), and never asks while k is 1 and stays 1. `granted_k` steps up to 1 plus the fewer grant, never past the ask and never below k. It steps down only to `fanout`'s k. A ledger that did not answer keeps k. |
| `shards_for(network, site, count, pool: &[u16])` | The rendezvous first shard, then shard `count + p` for each granted slot `p`. It replaces the rendezvous pool order. `push_validator` takes the same slots. |
| `/pool/grant`, `/pool/renew`, `/pool` on `GitShard` (`workers/site/src/shard.rs`) | Served only by the object named `git#<site>#pool:1` (checked against `id_from_name`), from storage, with no container. Test installs add `/pool/down {ms}`, which makes the ledger answer 503. |
| The leader (`workers/site/src/fanout.rs`, `Grant`) | After each `/fanout` report, `pool_ask`. If it asks, both warm ledgers are called at once with a 3 s deadline [guess], `granted_k` runs, and `RepoEvent::Fanout { k }` is committed when k changed. A ledger that granted more than the committed k - 1 is asked again for k - 1. Test installs' `/bench_seed {"fanout": k, "pool_down": ms}` runs the same path. |
| The first shard (`workers/site/src/shard.rs`) | Once a minute, on its own validates, it renews its network's lease (`/pool/renew`) and keeps the slots answered, plus its site's grant from the leader's `/fanout` reply. It forwards pushes among its first shard and those slots, at most k - 1. `/status` shows `pool`. |

`decide` is unchanged. k enters the log only as the leader's own `Fanout` entry, and no group's `decide` reads a ledger. The ledger lives in an existing shard object, so there is no new failure domain. The `git caches` instance and noisy-tenant rows in `design/DESIGN.md`, "Failure modes", now name it. No request path calls the Cloudflare API: the grants are calls between the install's own Durable Objects.

## Design and decision edits

- `design/DESIGN.md`, Shape, `shards_for` and `HOT_POOL`: the ledger, the grant before a step, the lease, the outage rule, and why pool shards are no longer shared by rendezvous. The agent block's "A step takes idle warm pool shards both warm sites' ledgers granted" is made in both `design/DESIGN.md` and `design/AGENT-LAYER.md`. Failure modes: the `git caches` instance and noisy-tenant rows.
- `decisions/DECISIONS.md`, "The per-owner pool budget is a lease ledger in each site's first pool shard". It records three places where the code differs from the operator's default design, with the reason for each:
  - The leader also asks at its held k, which renews the lease.
  - A short grant never steps k down.
  - A ledger's own silence is not counted against leases.
- `build/BUILD-PLAN.md`, AM1-WP1: a "Pool grants" paragraph.

## Hosted smoke

The raw logs named below are under `tests/am1/raw/pool/` on this branch's orb and are not committed (`tests/am1/.gitignore`); every figure here is copied from them.

`tests/am1/pool/smoke.mjs` on `graft-m3c`, after hot-shards-3 freed it at 07:04Z.
- **Owners.** Owner A is org `m3org` (7100001), with one network, `pg-mono`. Owner B is user `m3-admin` (7100003), with three networks, `pg-h1` to `pg-h3`.
- **How k is stepped.** Each step is the leader's `/bench_seed {"fanout": k}`, which runs `Grant::ask`, the same code a load report runs. The committed k is read back from the fold (`ranges`).
- **Install.** Shards were `standard-4` with test-faults.

| Deploy | Sites a / b / c | Edge | Image |
| --- | --- | --- | --- |
| `c1b0ef8`, 08:02 to 08:14Z | `1269f296` / `62d95783` / `a07cac12` | `893f8450` (unchanged edge code, same formats and `--ops`) | `graft-m1-site-gitshard-git@sha256:304cb7d2…`, built from this branch's `containers/git`, which is main's |
| `99a36c2`, 08:44 to 08:52Z | `df3164a6` / `17c2613a` / `722c52f9` | unchanged | unchanged |

### Run r3 (`c1b0ef8`, 08:31 to 08:42Z, `tests/am1/raw/pool/smoke-r3.log`): 13 of 13 checks pass

| Phase | What happened | Check |
| --- | --- | --- |
| 1 | A asked 7: both warm sites (0 and 2) granted 6, slots `[5,0,2,6,3,1]`, so k = 7 committed. Two pushes then went through; site 0's first shard `/status` showed k 7 and `pool` `[5,0,2,6,3,1]`, the slots granted | Pass |
| 2 | B asked 8 on each network. h1 was granted slot 4 on both sites, so k = 2. h2 (warm sites 0 and 1) got 0 on site 0 and 6 on site 1, so k stayed 1. h3 got 0 and 0. A asked 7 again: k 7 and the same slots | Pass: A not reduced; B 1 pool shard; no slot on either site held by A and B |
| 3 | Both of A's ledgers were set down for 120 s. A asked 3: grants `null`, `null`, and k stayed 7 (no step down). h1 asked 8 with its ledgers down: k stayed 2 (no step up) | Pass |
| 4 | A asked 1 (released). B asked 8 again: h1 was granted 6 on both sites, so k = 7; h2 and h3 got nothing on site 0, which h1 had filled. B's total was 6, its share | Pass |
| 5 | B stopped asking. A renewed at k = 2 every 55 s for 200 s, then asked 8: granted `[1,5,0,2,6,3]` on both sites, so k = 7. B's leases had lapsed and freed their slots | Pass |

**Found in r3.** h2's step in phase 2 was granted 6 slots on site 1 and none on site 0, so the leader kept k = 1, but site 1's ledger went on leasing h2 the 6 slots until they lapsed. That is harmless to the budget (h2's owner was still within its share), but it held pool shards no network served from. `99a36c2` makes the leader give back what a ledger granted past the committed k - 1, with a kept test (`an_unreachable_ledger_keeps_k`, the two-ledger case) and two smoke checks.

### Run r4 (`99a36c2`, `tests/am1/raw/pool/smoke-r4.log`)

15 of 15 checks pass. The same phases as r3, with two more checks: after phases 2 and 4, no ledger leases a network more than its committed k - 1.

| Phase | What happened |
| --- | --- |
| 1 | A asked 7: granted `[3,0,5,2,4,1]` on sites 0 and 2, so k = 7. Two pushes went through, and site 0's first shard showed `pool` `[3,0,5,2,4,1]` |
| 2 | h1 was granted slot 6 on both sites, so k = 2. h2 (sites 0 and 1) and h3 (sites 0 and 2) were granted nothing; h2's site 1 ledger, which granted 6 in r3, now holds nothing for it. A still had k 7 with the same slots |
| 3 | Ledgers down: A asked 3 and kept 7; h1 asked 8 and kept 2 |
| 4 | A released. h1 was granted `[6,2,5,3,1,4]`, so k = 7; h2 and h3 got nothing. B's total was 6 |
| 5 | B stopped. A renewed at k = 2 for 4 minutes, then asked 8: granted `[0,3,5,2,4,1]`, so k = 7 |

Teardown after r4: 6 containers (`git#0#pool:4`, the first shards of the four networks, and `git#1#net:65585`), all 200.

### Not shown hosted

- **k stepped from real load.** The smoke drives k through `/bench_seed`, which calls the same `Grant::ask` a load report reaches after `LoadReports` and `pool_ask`. A load-driven step, a site kill, and the clone and push gates are B0-34's, which LM-1 scripts.
- **A ledger object's restart.** The phase 3 outage is a test fault inside the object, not a reset.

## Tests and plants

Kept tests (`crates/git/tests/agent_load.rs`, `crates/git/tests/shard.rs`):

| Test | What it holds |
| --- | --- |
| `b0_34_k_is_set_in_band_and_the_hostile_owner_is_held_to_its_pool_share` | Rewritten to run the leader's hosted path: `LoadReports` from two warm sites, `pool_ask`, two `PoolLedger`s, and `granted_k`. Each minute on each site it checks each owner within its share and no pool slot held by two networks. It also checks that the monorepo reaches 7, is never reduced, and that the hostile owner's three networks hold at most 2. |
| `a_hostile_owner_with_three_hot_networks_is_held_to_its_share` | Three asks of 7 from one owner give 2, the next owner gets the other 5, no slot is shared, the result holds every minute, and with `owner_share` no owner takes the whole pool. |
| `another_owners_step_up_never_reduces_a_networks_held_shards` | Ten minutes of another owner asking 7: the network keeps the same 4 slots, and after a step down it gets only free ones. |
| `an_unrenewed_lease_expires_and_frees_its_shards` | A lease lapses after LOAD_STALE and its slots go to the next ask. A first shard's renewal keeps a lease without growing it. An hour of ledger silence frees nothing. |
| `an_unreachable_ledger_keeps_k` | `granted_k` with either grant missing keeps k, up or down. The fewer grant bounds a step, and a short grant never steps down. `pool_ask`'s cases are covered. Two disagreeing ledgers: the trim gives back the extra slots. |
| `first_shard_is_in_count_and_the_rest_are_the_granted_pool_slots`, `raising_k_moves_no_network_and_keeps_the_earlier_shards`, `pushes_spread_over_the_k_shards_by_push_id` | `shards_for` and `push_validator` on granted slots. |

Plants (`tests/am1/pool/plant.py`, log `tests/am1/raw/pool/plants.log`): **13 of 13 caught**, none missed, all compiled:
- The ledger ignoring the share.
- The asker listed first (other leases not kept first).
- New slots taken from another network.
- Leases that never expire.
- An ask or a renewal that does not renew.
- The ledger's own silence counted.
- An expired network re-asking from what its leader holds.
- The larger grant bounding a step.
- An unreachable ledger read as a grant of nothing.
- A short grant stepping k down.
- A held k never renewed.
- `shards_for` back on rendezvous.

Full checks (`tests/am1/runs/checks.sh`, log `tests/am1/raw/pool/checks.log`): `cargo fmt`, workspace clippy, wasm32 clippy for the site and the edge with and without test-faults and for ops, `cargo test --workspace`, the three design checks, and the shard's Go tests: all pass on `99a36c2`. `check_links.py` failed inside that run only because it scanned the run's own build output under `.worktrees/target-rows`, and passes (3,141 citations) once that directory is removed.

## Cost

$0.38 in total for `graft-m3c`'s own usage from 07:56Z (the image build) to the end of r4, by `tests/fanout/m2bcost.py` with `INSTALL=graft-m3c`. That includes $0.28 of containers: the first shards of the smoke's pushes, and one pool shard. Every shard was torn down after each run.
