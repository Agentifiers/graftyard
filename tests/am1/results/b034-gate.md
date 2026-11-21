# B0-34's hits line and the read gate's member read ($0 note, nothing built)

The orchestrator's ruling on mt14 (2026-11-21): the gate split is accepted, but hits p99 cannot reach 1 s while the
gate waits on a member read whose p99 is 1.35 to 2.17 s. This note answers (a) to (d) from the samples of mt12, mt13
and mt14 (`tests/am1/raw/a1-samples-b0-34-{mwq2e1ad,mwqah46u,mwqhzvfd}.jsonl.gz`, kept on the orb) and from the edge's
Workers Logs push lines. No hosted spend.

**Short answer.** About a third of hits read members in the gate. Those reads are one narrow `access` rows read on
the repository's followers by read index, which goes past `LEARNER_READ_EVERY` on a group with no learners. Hiding
the gate behind the hit's other work does not pass the line alone: `chain_open` is now as long as the gate on the slow
hits. Hiding the gate and capping `chain_open` near 300 ms together would put j9's hit p99 at 808–893 ms on all three
rows [inference, counterfactual from the stages].

## (a) How many hits read members at all

A hit's `gate` stage is 0 when the per-isolate decision cache (`workers/edge/src/lib.rs`, `mod gate`) held a decision
under `READ_CACHE` (10 s) for the (repository, credential) pair, and the read's wall time otherwise.

| Row | j9 hits | gate read members | hit p50 / p99, cached decision | hit p50 / p99, gate read |
|---|---|---|---|---|
| mt12 | 282 | 92 (33 %) | 368 / 824 ms | 726 / 2,241 ms |
| mt13 | 233 | 65 (28 %) | 404 / 859 ms | 721 / 1,693 ms |
| mt14 | 227 | 75 (33 %) | 536 / 961 ms | 994 / 2,866 ms |

Over all phases the share is 32, 29 and 36 %. **Every hit whose gate used a cached decision was under 1 s at p99 on
all three rows; the line fails on the third that read.**

- **By credential:** one. Load34's probes all send the `admin` PAT (`Basic x:<tok>`; `tests/am1/load34/load34.mjs`,
  `cloneTtfb`). There are no anonymous or `ghs_` hits in B0-34, so the split by credential cannot be made from these
  rows. The cache key is (repository, token hash), so all 100 clients share one key per isolate.
- **By isolate count:** the probes do not log the edge isolate. The edge's push lines do: 111 distinct isolates served
  mt14's pushes (78–88 a minute), and 119 mt13's (76–89 a minute), from the same 100 Load34 clients. Probes arrive at
  about 1 a second. If they spread evenly over ~80 isolates, an isolate would see the key again within 10 s about one
  time in eight and ~88 % of hits would read members; the measured 28–36 % means each key's probes land on about 8
  isolates in a 10 s window, so Cloudflare keeps a client's connections on few isolates [inference: share read ≈
  e^(−10 s × rate / N); 30 % gives N ≈ 8].

Pushes on the same rows: 50 % (mt14) and 51 % (mt13) of push lines show `gate;dur=0`.

## (b) Which reads, on which member, and how long

The gate runs two reads at once (`read_gate`): the repository's `access` rows (`gate_rows`, `api::core_rows`, through
`spread_read`) and the owner's `Authz` (`gate_authz`). A read-class request takes the owner's `Authz` from `rcache`
when one is at most `READ_CACHE` old. Placement is not read in the gate: the repository's record comes from `resolve`,
which is `front`.

Clone probes carry only `gate`'s total. Pushes carry both reads' walls (`server-timing` in each push line). The
nonzero ones, mt14 against mt13:

| Read | mt13 p50 / p90 / p99 | mt14 p50 / p90 / p99 |
|---|---|---|
| `gate` (the longer of the two) | 488 / 660 / 1,019 ms | 635 / 1,001 / 1,772 ms |
| `gate_rows` (`access` rows on the repository's group) | 482 / 657 / 1,019 ms | 634 / 1,001 / 1,772 ms |
| `gate_authz` (owner's `Authz`) | 273 / 352 / 552 ms | 273 / 346 / 552 ms |

The gate is `gate_rows`: `gate_authz` is shorter in nearly every push and did not move between rows. The clone
probes' nonzero `gate` (p50 / p99 461 / 1,142 ms on mt13, 638 / 1,730 ms on mt14) has the same shape, and the
API path's narrow read of the same group (`api:tier`, 180,000 samples a row) split into its member stages:

| `api:tier` | p50 | p90 | p99 |
|---|---|---|---|
| mt13 total | 499 | 671 | 1,479 |
| mt13 `tier_grant` (wait for the leader's read index to apply) | 246 | 410 | 645 |
| mt14 total | 656 | 1,044 | 1,891 |
| mt14 `tier_grant` | 346 | 625 | 917 |

So about half the read is the read-index grant on the member and half the round trip and answer.

**Which member kind.** B0-34's monorepo groups ran with no learners: memguard's `learners` field is empty in every
sample on mt13 and mt14, and `terms.mjs` shows config `1/0` (index 1, 0 learners). `timed_spread_order` therefore sends
each read to a follower (site 1 or 2, chosen at random; the leader is left out), up to `SPREAD_TRIES`, and to the
leader only when every follower declines. Every gate read in these rows is a follower read-index read.

**Push p99** (the ruling's item 3): `push:human:tip` is an `ls-refs` (`tipOf`), whose HEAD and refs pages take the
same follower-read path; its p50 moved from 432–554 to 618–826 ms with `api:tier`. It is the same member read, not
the gate. It stays its own line.

## (c) The learner read-index path and a 200 ms `LEARNER_READ_EVERY`

The gate's `access` read and the API tier reads use the same path (`graftyard_lookup::read_spread`: learners first,
then followers, by read index; `design/DESIGN.md`, "Follower read"). A learner batches its read-index requests at most
every `LEARNER_READ_EVERY` (50 ms), a follower every `FOLLOWER_READ_EVERY` (10 ms)
(`crates/raft/src/member/install.rs`).

- **On B0-34's groups as run, no effect.** They have no learners, so every gate read is a follower read, batched at
  10 ms. `learner-cadence`'s 200 ms floor changes nothing here.
- **On a group with learners (B0-32's and B0-33's shape, and production's per DESIGN),** the gate reads learners first.
  The note puts a 200 ms floor at about 100 ms more on average per read, up to 200 ms
  (`tests/am1/results/learner-cadence.md`, on `origin/r33-cadence-note`). Applied to the gate: a gate read's p50 would
  move from ~640 to ~740 ms and its p99 by up to 200 ms [inference]. That moves about a third of hits by 100 ms on
  average, and more of the reads past `FOLLOWER_WAIT` (1 s), where a learner answers `busy` and the edge tries the
  next member. It does not change the conclusion below, but it would make it harder to reach.

## (d) Taking the gate off the hit's critical path, within the revocation bound

The bound: reads honour a revocation within `READ_CACHE`, 10 s (`design/DESIGN.md`, Shape, Identity; "4. Reads with a
cached token may succeed for up to 10 s after the revocation"). Options, costs, and what each buys on these rows:

1. **Serve the hit on the cached decision and re-check behind it ("stale-while-revalidate").** A decision older than
   `READ_CACHE` still lets the hit stream, and a fresh gate runs beside it; a refusal cuts the stream. Breaks the
   bound: bytes leave before a decision under 10 s old. Not allowed as written.
2. **A longer-lived cached decision for some repositories.**
   - *Public repositories:* `authorize_read` allows any caller on a public repository without reading the token's
     grants (`crates/identity/src/access.rs`). The only thing that could change is the repository turning private, a
     repository setting that commits on its own group. A cached `Allow` on a public repository could live as long as
     the isolate's view of that setting does. Costs: the visibility change would bind within the cache life, not 10 s.
     That is a design change to "Identity" needing an operator decision. B0-34's monorepo is private, so this buys
     nothing on these rows.
   - *Private repositories:* the decision depends on the token row, the collaborator rows and the owner's `Authz`. Any
     life past 10 s breaks the bound for revocations. Not allowed.
3. **Share decisions across isolates.** The same (repository, credential) key is read about once every 10 s per
   isolate, and ~80 isolates serve each minute. A decision kept anywhere all isolates of a colo see would turn 28–36 %
   reads into about one read per key per colo per 10 s, within the bound. Cloudflare products that qualify:
   - *The Workers Cache API* (`caches.default`, per colo) holds a response for a TTL. A gate decision is a few bytes.
     Cost: a cache `match` on every hit, typically single-digit ms in the colo [doc, unverified on this install], and a
     `put` per read. It is not in `research/cloudflare.md`'s first table, so it needs a row and a "Stack" status
     before any build (AGENTS.md, "Conventions").
   - *A Durable Object per (repository) on the edge path* adds a hop and a failure domain on every hit. Rejected.
4. **Prefetch the decision before it expires.** An isolate that served a key in its last `READ_CACHE` re-reads it in
   the background at ~8 s, so a steady stream of hits always finds a fresh decision. Within the bound (each decision
   still at most 10 s old when used). Cost: one background member read per (isolate, key) per ~8 s while the key is
   hot, the same rate as now for a key seen every few seconds and more for a key seen once. On B0-34 it helps only
   isolates that see the key at least twice in ~18 s; with ~80 isolates per minute, most see it about once, so it
   would cut the 28–36 % share only partly [inference].
5. **Hide the gate behind the rest of the hit, as `hit-gate` already does for `chain_open`.** Already built: the
   gate and the manifest's chain checks run at once, and no byte leaves before `Allow`. What is left is the max of the
   two.

**What would pass the line.** On j9 with the row's first 1.5 s dropped (the cold-edge artifact), the measured p99 and
counterfactuals from the same samples:

| j9, first 1.5 s dropped | mt12 | mt13 | mt14 |
|---|---|---|---|
| measured | 1,118 ms (6 over 1 s) | 1,249 (8) | 1,573 (32) |
| gate hidden (0 ms), `chain_open` as measured | 1,118 (6) | 1,149 (7) | 1,161 (9) |
| gate as measured, `chain_open` ≤ 300 ms | 902 (0) | 1,081 (4) | 1,573 (30) |
| gate hidden, `chain_open` ≤ 300 ms | 813 (0) | 893 (2) | 808 (0) |
| gate hidden, `chain_open` ≤ 150 ms | 663 (0) | 817 (2) | 658 (0) |

Neither change passes the line alone on every row; both together pass it on all three [inference: the counterfactual
keeps every other stage as measured and takes the max of the gate and `chain_open` as `hit-gate` runs them]. On mt14
the slowest hits after the gate is hidden have `gate` 356–627 ms and `chain_open` 538–786 ms, so `chain_open` is the
next limit there.

**Recommendation.** Option 3 (a colo-shared gate decision under the same 10 s life) with the Cache API is the only
option inside the bound that removes most gate reads, and it needs a `research/cloudflare.md` row and an operator
decision on the product first. Option 4 is inside the bound with no new product but buys less. Either one only passes
the line together with the `chain_open` item already noted (isolate cache of checked pack headers,
`/tmp/chainopen-analysis.txt`). Nothing is built.
