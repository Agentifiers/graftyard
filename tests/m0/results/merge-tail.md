# merge-tail: B0-29 run 5's merge p99, the commit tail's freeze, and three small fixes (2026-11-18)

Code only, no hosted run. The evidence is B0-29 run 5 (`tests/am1/results/b029-m2b.md`, "Run 5"; a1 `mwlu6gge`,
`tests/am1/raw/a1-b0-29-mwlu6gge.jsonl`, repository 131357, load 08:20:53–08:30:53Z, trace
`tests/fanout/raw/tsave-m2bb29e.jsonl.xz` from 1794990053053 ms) and Workers Logs for `graft-m2b-edge` and
`graft-m2b-site-{a,b,c}` over the load. m3fr17's B0-32 records are `tests/am1/results/lm1-rows-m3f-data/m3fr17-raw/`.
Branch `merge-tail` from main `459d09e7`. The operator's asks (T-01a17bda), in order: which stage of `merge:only` takes
the tail and the causes of the 7 × 409 and the 503; one design for `cut_early`'s freeze and its effect on the 47%; and
three small fixes.

## 1. Merge p99 12,516 ms

### The p99 is the one 503 [measured]

59 `merge:only`: 51 × 200, 7 × 409, 1 × 503. Nearest rank puts p99 on the slowest, the 503 at 12,516 ms. Without it
the slowest is 6,946 ms (a 200) and the line's 2 s would still fail: 16 of 59 merges took over 2.2 s, p50 1,638 ms.
So the 503 sets the number, and the stages below set the line.

### The 503: the shard's answer was lost on the way back [measured]

`PUT …/pulls/549/merge`, trace `9992baa9…`, edge 08:29:27.4–08:29:39.934Z. `server-timing`: auth 67, resolve 226,
then nothing until total 12,496, with no `merge_plan` mark. So the wait was inside `merge_plan`
(`workers/edge/src/api.rs`), between the shard call and its body. The body was `the git cache: Error: Network
connection lost.`

- The shard (site a's `GitShard`, object `3a03a226…`) ran that `/merge` at 08:29:27.29Z and logged it ok, 86 ms,
  status 200. The edge got "Network connection lost" 12.3 s later.
- That object was not reset: in 08:29:27–08:29:41Z it ended 1,340 `/ask`, about 350 `upload-pack` ok, and the next
  merge (`ns=550`, 08:29:28.36Z, 79 ms) reached its edge, which answered 200 at 08:29:29.6Z. No `exceededMemory`,
  `reset` or abort line in that window.
- The same object threw, all load long: 1,869 `upload-pack` invocations ended `exception`, each about 20 s wall, with
  "names:708: no member answered", and 910 more `canceled` (08:20:50–08:31:30Z, `$workers.outcome` counts). 31 edge
  errors "merge ask https://shard/ask?repo=131357: Network connection lost" fell in the same minutes: 11 of the 32 lost
  answers (the 31 plus the 503) came within 25 ms of a shard exception against 13% of random moments, 4 within 5 ms
  against about 3%. So the losses go with the shard's exceptions, not with load in general.
- Not settled: how one invocation's uncaught exception loses another invocation's answer that the shard had already
  logged ok. The Durable Objects docs say an uncaught exception may terminate the instance [doc,
  developers.cloudflare.com, "Rules of Durable Objects"], but the object served on. [unverified] Removing the
  exceptions (below) removes the candidate; if a lost answer recurs without them, it is a platform question.

### Why the shard threw: `names:708`, read by every upload-pack that carries no record [measured]

`names:708` is `account_shard(owner)` for m3org (account 7100001), the shard of its `Placement` row. The shard's
handler reads the owner's `RepoOwner` and `Placement` rows (`member::repo_owner`, `owner_record`) when the request
names no `record`. `/validate`, `/merge`, `/demand` and `/catchup` carry it; `shard_read`, which every
`/upload-pack` and `/info-refs` from the edge uses, did not. So 19,129 edge upload-packs each started a `Names` read on
the shard, and 1,869 of them waited LOOKUP_FOR (20 s) and threw.

The edge's `names:708` reads are slow for a second reason, and the merges share it (below).

### The 200s' tail: resolve and an untimed gap, the same as pushes [measured]

Over the 15 slowest non-503 merges (over 2.2 s), by `server-timing`: resolve 54% of their wall, `merge_plan` leg
(edge to shard and back, less the shard's own work) 27%, commit 14%, shard compute 2%, auth 2%. Resolve was the
largest part in 10 of them. The shard computes a merge in p50 68, p99 194 ms; `shard:waited` and `shard:catchup` are 0.

The traces show where resolve's time goes: in those merges, no Durable Object invocation runs for 76% of the wall
(42.4 s of 55.4 s), against 54% in the 15 fastest. In merge 510 (4,463 ms), for one, the edge read four rows at 0–26
ms, then nothing reached any member for 2,096 ms, then `/merge` ran 78 ms. Edge CPU per merge was 11–31 ms.

It is the leader lookup's mark (`graftyard_lookup::lookup`, `lookup-poll`), and this is the cause [measured]:

- `cached_leader` returned a leader only if the isolate's config named it, and an isolate learns a config only from a
  `NotLeader`. A system chain whose leader always answers, such as `names:708` and `names:152`, never sends one. So
  every edge isolate kept no cached leader for them, and every read of them took the lookup path: a post to the
  record's bootstrap names under the chain's mark.
- Only one request per isolate holds a chain's mark. The others poll the cache every 50 ms for a leader that is never
  cached, then take the mark when it frees. So each isolate's reads of the chain ran one at a time.
- In a 6 s sample of site a, 2,718 of 4,203 member `/read`s were walk posts (`SentBy::Walk`), 350 cached-leader
  posts and 1,135 spread. One isolate's walk posts to `names:708`'s leader (`f2d040…`): 120 in 4.9 s, never two at
  once by the object's clock, the next starting p50 30 ms after the previous ended. Another isolate's: 42 in 4.9 s,
  gap p50 106 ms. Each read takes 3–20 ms at the member; the wait is the queue in front of the mark.
- The edge resolves every write fresh (`rcache::through`, write class), so every push and merge reads `names:152`
  (login) and `names:708` (placement) and takes its place in that queue. Push resolve p99 6,039 ms and merge resolve
  p99 5,731 ms are this wait [inference from the traces above; `server-timing` does not split resolve].

The fix (built): `lookup` keeps the member that answered as leader (`Known::answered`), and `cached_leader` falls back
to it when the config names no leader on that site. A later reply naming another site drops it. The next read posts it
as the cached leader with no mark, as reads of a repository chain do after their first `NotLeader`. Test
`a_leader_found_by_its_bootstrap_name_is_cached`; plants 0–2 below.

### B0-32 (m3fr17) shares the pre-commit stages [measured]

m3fr17's pushes: resolve p50/p99 500 / 1,743, auth 54 / 1,837, context 112 / 1,874, gate 0 / 1,869, validate 242 /
1,634, commit 743 / 3,654, total 2,294 / 7,376. Run 5's: resolve 583 / 6,039, auth 190 / 2,858, gate 186 / 2,250,
context 0 / 1,558, validate 475 / 5,143, commit 501 / 822, total 2,583 / 8,994. m3fr17's merges: resolve p99 209,
`merge_plan` p99 13,147 with `shard:waited` p99 13,103 (its one `warming` 503 waited on a catch-up), so its merge
tail is the shard's catch-up, not resolve. Both rows' push p99s sit in the stages before the leader, and auth, gate
and context read the same system chains (tokens, installation, `Authz`) through the same lookup. So this fix
should move resolve, auth and gate on both rows [inference]. T-01a1d805 confirmed it stopped its pre-admission trace,
and this thread owns it for B0-29 and B0-32.

### The 409s: the base moved under the merge, as the row's own traffic makes it [measured]

All 7 are `MergeError::BaseModified` ("merge command … refused: BaseModified" in the edge log), refused by
`Merge::decide`: the plan's `base_old` was no longer the base ref's value at decide. In 6 of 7, another merge's 200
ended inside the 409's window (`502` inside `503`, `506` in `507`, `532` in `533`, `535` in `534` and `536`, `541`
in `542`). The seventh (511) overlapped merge 510's commit (510 ended 5 ms after 511). a1 opens and merges 10 PRs a
minute against `main`, each in its own task, so merges overlap whenever one takes over about 6 s of another's
start-to-commit, which the resolve wait made common: the 409s had resolve p50 1,053 ms against 414 for the 200s. This
is GHES's documented answer for a moved base (`design/SHARED.md`, "Same-repository merge"), and `decide` must keep
refusing it (a 409 is never retried into success). Shorter merges make fewer overlaps; nothing else should change.

## 2. Commit p99 669 ms: the freeze

### What the 47% is [measured]

Replaying each `/commit` arrival to the cut that took it and the append that carried it, from the trace's kind 29
rows (`tests/m0/cuttrace/tailcut.py`): replayed commit p50 346, p99 643 ms. Over the 112 commands at or above
p99: arrival to cut 53%, cut to append 27%, append to apply 20%. 82 of the 112 rode an early cut. Over all commands
the three are 29 / 29 / 43%. The tail's arrival-to-cut wait is p50 410 ms, max 1,102: a command waits for the freeze
of the entry after the one in flight.

### The freeze is not where the tail is decided [measured, by replay]

A replay of the leader (`tests/m0/cuttrace/freezesim.py`: measured arrivals, measured rounds in order, the
read-plus-decide time of each cut taken at an apply, p50 126, p99 354 ms; three seeds) under each freeze rule:

| freeze rule | commit p50 | commit p99 |
|---|---|---|
| main: append + mean round − mean read | 383 | 675 |
| append + 0.6 or 0.8 × mean round − mean read | 391 | 669–672 |
| append + mean round − 1.5 or 2 × mean read, or at the append | 391 | 669 |
| a read asked every 25, 50 or 75 ms of the flight, the latest answered one used | 371–383 | 661 |
| oracle: apply − this entry's own read (unreachable) | 362 | 646 |

The replay reproduces the measured 669 (main's rule, 675). No reachable freeze rule takes more than 14 ms off p99, and
even the oracle, which knows each read's length in advance, takes 29. The reason is in the cut rows: 2,553 of 3,057
cuts were early and their entries carried the next batch, and the 504 cut at an apply had queued commands in 478 cases
with rounds p50 121 ms, under main's wait of a mean round less a mean read: the entry committed before the freeze
came, and no rule that waits for a read can freeze sooner than a read before an apply it cannot predict.

So the 47% "waiting for the freeze" is the shape of one entry in flight, not the freeze's timing. A command that
arrives just after a freeze waits out the entry in flight and the next one's read; moving the freeze trades one end for
the other.

### What does move it: the read [measured, by replay]

| dependency read | commit p50 | commit p99 |
|---|---|---|
| as measured (p99 354) | 383 | 675 |
| hedged at 300 ms (min of the read and 300 + a second read) | 382 | 643 |
| hedged at 200 ms | 380 | 602 |
| hedged at 150 ms | 380 | 589 |
| hedged at 100 ms | 378 | 565 |
| half as long | 310 | 531 |

The read is the batch's `DepsWanted`: one `fresh` read per chain (`authz:7100001` p99 230, `tokens:N` p99 246,
`names` p99 165, all p99 324; `/stats` `split.deps`). `dep-hedge` sends a second read only after DEP_HEDGE (1,000 ms),
and run 5 asked one hedge in 3,117 reads (`depHedges`). A hedge at 150–200 ms, in the replay, brings p99 to 589–602,
under 600 and the 660 line [inference: the replay draws the hedge's second read from the same distribution as the
first, so it assumes the slow reads are independent; a slow member would make them correlated and the gain smaller].

### The design sent before any change in `crates/raft` [design, not built]

No change to `cut_early`. The proposed change is outside `crates/raft`: `DEP_HEDGE` in `crates/lookup` from 1,000 ms
to a measured value near the read's p90, about 150 ms on run 5 (`split.deps:all` p90; the hedge already exists, built
by `dep-hedge`). It costs at most one extra member `/read` for the reads past it, about 10% of 3,117 in 600 s, half a
read a second on a leader that serves 600. What it does to the 47%: the replay says the tail's arrival-to-cut wait
stays, and the cut-to-append part (27% of the tail, the read) shrinks; commit p99 675 → 589.

The post-apply dependency read: it cannot start earlier without breaking an invariant. `decide_held` re-reads only
the chains the held batch gained after its early read; the read of a batch cut at an apply starts at that apply,
because the batch did not exist before it. The cuts at an apply are the entries whose round ended before the freeze,
and an earlier freeze does not help them (the table above).

## 3. The three small fixes [built]

- **Cut rows read exactly.** `cut_row` (`workers/site/src/host.rs`) moves the agent tokens from bits 48+ of `batch` to
  bits 42+ of `ms`, with the bytes at 21 bits each, and narrows each class's `left` to 13 bits. Every column is now
  under 2^53, so the JS number the trace passes through holds it. `tests/m0/cuttrace/cuts.py` reads both layouts (old
  rows have bits above 48 in `batch`) and prints byte-identical output on run 5's trace. The host test now packs a cut
  with one Human command, ENTRY_MAX soft bytes and saturated bytes and tokens, and checks every column survives an
  f64 and reads back.
- **`retirenets.mjs` leaves no orphaned purge.** purge.py starts in its own process group (`spawn`, `detached`), and
  retirenets kills that group on SIGTERM, SIGINT or SIGHUP, then exits. Reproduced locally with a fake `uv` that sleeps
  30 s: node killed with TERM after 1.5 s, the old `execFileSync` child ran on and wrote its file; the new one did not.
- **The 131357 buckets.** A listing of every one of 131357's 22 prefixes in `graft-m2b-site-{a,b,c}-packs` (the a,
  b and c buckets) at the time of this record found 0 keys in each.
- **The edge's shard reads carry the owner's record.** `shard_read` (`workers/edge/src/lib.rs`) now sends
  `record=<owner's placement record>` as `/validate` and `/merge` do, so upload-pack and info-refs make no `Names`
  read on the shard. Not hosted yet.

## Checks and plants

- Plants: `python3 tests/m0carried/plant41.py`, 6 of 6 caught: cached leader only from the config's voters (main's
  rule), the answering member never kept, an answered member kept for any site, tokens back above bit 48, `left` at
  16 bits, all bytes unclamped.
- Full checks on this branch: fmt, workspace clippy, wasm32 clippy for site, edge and ops (site and edge with and
  without `test-faults`), `cargo test --workspace` 1,165 passed and 0 failed, `go test` in `containers/git/server` ok,
  and the three `scripts/check_*.py` ok.
- No change in `crates/raft`, so no long sweep or Porcupine run is owed.

## What would show it on a run

The next B0-29 site day with this branch should show, against run 5: walk posts (`SentBy::Walk`) a small share of
member `/read`s, push and merge resolve p99 near their p50s, no `upload-pack` exception with "names:708", and no
"Network connection lost". The commit p99 is not expected to move from this branch alone; the DEP_HEDGE change is
the one that should (design above).
