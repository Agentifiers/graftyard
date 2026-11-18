# dep-hedge-adaptive: each chain's dependency read is hedged at its own p90 (2026-11-18)

Branch `dep-hedge-adaptive` from main `dcef0f59`. Code-only; no hosted spend. The operator (T-01a17bda) approved
`merge-tail`'s DEP_HEDGE change in direction but not as a fixed 150 ms, because B0-31's apac leader reads enam's
identity groups with `deps:authz` p99 672 to 725 ms, so a fixed 150 ms would hedge most of that leader's reads. Asked:
replay the rule on B0-31 m3fr12 beside B0-29 run 5, report the hedged share and the extra reads a second on the
identity leaders at 150 and 200 ms, and if the far rate passes about 20%, make the delay follow the chain's read time.

## 1. The replay [inference, from recorded percentiles]

No run kept per-read times for B0-31: m3fr12 left no a1 jsonl or trace in the repository, and its `deps:*` split is
in `tests/am1/results/lm1-rows-m3f.md` ("read-ahead's flip", "Rows m3fr16", "B0-31's buckets-up commit tail"). So each
kind is a lognormal fitted to its recorded p50 and p99, over the read counts recorded for the half
(`decisions/DECISIONS.md`, "`dep-hedge`", Cost). m3fr12's `deps:authz` p50 is not recorded; 293 ms is m3fr11's, the
same leader, row and read [inference]. Run 5's counts and percentiles are `a1-b0-29-mwlu6gge.jsonl`'s `leader.split`;
m3fr17's are `a1-b0-32-mwljfpch.jsonl`'s. Script: `tests/m0/cuttrace/hedgerate.py`.

Hedged share of dependency reads, and the extra reads a second the identity leaders serve:

| run | reads/s | fixed 150 ms | fixed 200 ms | per-chain p90 (built) |
|---|---|---|---|---|
| B0-29 run 5 (enam leader) | 19.0 | 5%, +0.9/s | 2%, +0.3/s | about 10%, +1.9/s |
| **B0-31 m3fr12 up half (apac leader)** | 14.4 | **79%, +11.5/s** (authz 96%, +7.0/s) | **61%, +8.8/s** (authz 84%, +6.1/s) | about 11%, +1.6/s |
| B0-31 m3fr16 up half (apac leader) | 14.1 | 83%, +11.6/s | 66%, +9.3/s | about 11%, +1.5/s |
| B0-32 m3fr17 (enam leader) | 11.5 | 16%, +1.8/s | 9%, +1.1/s | about 11%, +1.3/s |

- **The far rate is far past 20%**, so a constant delay is out: at 150 ms the apac leader would hedge almost every
  `Authz` read and double the load it puts on `authz:7100001`, the furthest group it reads.
- The per-chain p90 hedges about one read in ten on every chain by construction. Its delays come out about 120 ms
  (run 5 authz and tokens), 100 ms (run 5 names, at the floor), 480 ms (m3fr12 authz), 320 ms (m3fr12 tokens) and
  240 ms (m3fr17 authz).
- What it buys, per kind (read p99 before and after, same fit): run 5 authz 236 → 182, tokens 252 → 179; m3fr12 authz
  736 → 707, tokens 533 → 476; m3fr17 authz 612 → 365. The far leader gains little because its p99 is distance, not a
  held request; the hedge is for F83's held requests and for near leaders' tails.

**Run 5's commit p99 under the rule** (`freezesim.py`'s replay of run 5's leader, `hedgerate.py` with the trace; the
at-apply read-plus-decide samples, n 499, p50 126, p90 189, p99 354; the hedge's second read drawn from the same
samples, as in `merge-tail.md`):

| delay | hedged | read p99 | commit p50 | commit p99 |
|---|---|---|---|---|
| main (1 s) | 0% | 354 | 381 | 646 |
| fixed 150 ms | 13.0% | 279 | 379 | 589 |
| per-chain p90 (built) | 10.5% | 287 | 379 | 593 |

The p90 rule keeps nearly all of the fixed 150 ms gain on run 5 (593 against 589) and costs the apac leader about one
hedged read in ten instead of eight in ten. Caveat as before: the replay assumes a hedge's two reads are independent;
a slow identity leader makes them correlated and the gain smaller [inference].

## 2. What was built [built, `crates/lookup/src/lib.rs`]

- `dep_hedge_after(chain)`: the p90 of the chain's last 64 dependency read times in this isolate (`DEP_MS`, a
  plain-data `thread_local!` map from `ChainId` to the times), clamped to `DEP_HEDGE_FLOOR` (100 ms) and `DEP_HEDGE`
  (1 s), and `DEP_HEDGE` until the chain has 16.
- `dep_read` takes the chain and a clock, asks the delay, and records each read that answered, hedged or not (the time
  the caller waited, so a won hedge records the shorter time). A failed read records nothing.
- `call_hedged`'s signature and `read_deps` in `workers/site/src/host.rs` are unchanged; nothing in `crates/raft`.
- Window and floor are [guess]: 64 reads is about 10 s of one chain at run 5's rate, so a leader that moves is
  re-learned within seconds; the floor sits above run 5's `deps:names` p90 (about 76 ms), under which a second request
  mostly races a first already on its way back.

**Test**: `a_dependency_reads_hedge_waits_its_chains_own_p90` drives `dep_read` on a fake clock. Untimed, a 400 ms read
sends nothing; after 64 near reads (40 to 166 ms, p90 154) a held 300 ms read is hedged at 154 and the second wins at
214 (main: 300); a far chain timed at 250 to 691 ms keeps a 649 ms delay, so its 600 ms read sends nothing, and the
near chain's delay is unmoved by it; reads at 4 s cap the delay at 1 s; a 20 ms chain waits the 100 ms floor. It fails
on main (`tests/m0carried/plant42.py`, mutant 0, 9 of 9 caught). `plant35.py`'s anchors moved to the new line; 5 of 5
still caught.

## 3. What would change it

- A hosted run where `depHedges.asked` is well past 10% of `depReads` × chains (the window lags a leader that slowed),
  or where `won` is near zero at the far leader (its tail is distance, and the hedge there only adds load): then
  raise the quantile, or exempt chains whose p90 is past a round.
