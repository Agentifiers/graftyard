# `colo-gate`: a data-center share of the read gate's decisions and of checked chain headers (design and probe, nothing built)

The orchestrator's ruling on `b034-gate` (2026-11-21) approved option 3 for design and a probe, not a build: share the
read gate's `Allow` decisions between the edge isolates of one data center through the Workers Cache API. It also asked
whether `chain_open`'s checked pack headers can use the same store. This file holds the probe, the replay, and the
answer. Nothing in `workers/` or `crates/` changed. The design text is in `design/DESIGN.md` ("Stack", "Follower read",
Shape's git caches paragraph, "Failure modes", "Deployment modes"), and the decision is `decisions/DECISIONS.md`,
"`colo-gate`".

**Short answer.**
- The Cache API works on the hostname kind the test edge uses (`*.workers.dev`). In LAX a `match` took p50 6 / p99
  12 ms and a `put` p50 7 / p99 17 ms, against `gate_rows`' p50 of 634 ms on mt14. An entry that one isolate put matched
  from 9 others.
- A shared entry alone does not pass the hits line. Replayed over mt12 to mt14 with `chain_open` capped at 300 ms, mt14
  still has 16 hits over 1 s at p99 1,451 ms. Most of the hits left waiting come more than 10 s after the last read.
  With a re-read ahead at 5 s, the replay gives p99 813 / 1,021 / 864 ms on mt12 / mt13 / mt14. mt13 still misses by
  21 ms, and its gate is not the cause.
- A checked chain header is **not** immutable, so it does not hold the way the ruling hoped. Chain files are named by
  commits, not by their bytes, and a fold deletes them. A header set keyed by the bucket's `head` etag does hold,
  because every put of `head` changes that etag. With that key, one product row (the Cache API) serves both caches.

## The probe (2026-11-21, `tests/colo/probe.sh`)

The probe used scratch Workers on the account's `workers.dev`, with no site day and no change to graft-m1:
`graft-m1-coloprobe` (`tests/colo/probe.js`), a second copy `graft-m1-coloprobe2`, `graft-m1-coloprobe-front`
(`front.js`, which reaches the probe over a service binding, as `graft-front` reaches the edge), and `graft-m1-r2probe`
(`r2probe.js`, read-only on `graft-m1-packs-a`). All four are deleted. Every request ran from the orb and landed in
LAX.

| Check | Result |
|---|---|
| 1,000 rounds of put then match, 200-byte entries, one isolate | 1,000 of 1,000 matched. `put` p50 7 / p90 9 / p99 17 ms (max 89). `match` p50 6 / p90 9 / p99 12 ms. A miss p50 5 / p99 13 ms |
| The same through the service binding (500 rounds) | 500 of 500 matched. `put` p50 5 / p99 14 ms. `match` p50 5 / p99 11 ms |
| One put, then 300 matches at 25 at once | 300 of 300 hit, from 9 distinct isolates. `match` p50 6 / p99 12 ms (max 18) |
| `max-age=3` | Hit at 0.1 and 1.7 s, missed from about 3 s on |
| `delete`, then 20 matches from 9 isolates | `delete` answered `true`, and 0 of 20 matched |
| Key built on the request's own URL: put direct, matched through the binding | **Missed.** Through a binding the request URL carries the caller's hostname (`graft-m1-coloprobe-front...`), so the two keys differ |
| Key on a fixed origin (`https://gate.graftyard.invalid/...`) | Matched through the binding and from a second script, on `caches.default` and on `caches.open("graft-gate")` |

The docs say the contents "do not replicate outside of the originating data center", that `delete` "only purges
content of the cache in the data center that the Worker was invoked", that `put` resolves "regardless of whether the
cache successfully stored the response", and that the API is "not currently available" for Workers fronted by Access
(https://developers.cloudflare.com/workers/runtime-apis/cache/). They also say calls count toward the 1,000 per
request and the six connections waiting for headers (https://developers.cloudflare.com/workers/platform/limits/).
These are copied into `research/cloudflare.md`, "Cache API", and its first table (GA [inference]: the page has no beta
label).

**Not measured:** other colos, a colo under a production edge's load, and eviction under many tenants' keys.

## Replay: what the shared entry does to j9's hits (`tests/colo/replay.py`)

The replay walks each row's j9 hits in start order through one shared entry. There is one key, because Load34 sends
one PAT to one repository, and one colo, because every client runs on the orb.
- A hit whose gate stage was 0 used its isolate's decision and stays at 0.
- A hit whose gate read members matches the entry if one was put (at its read's start plus the read's wall) and its
  read started under `READ_CACHE` ago. It then pays 12 ms, the probe's p99.
- Otherwise the hit reads as measured, and its read becomes the entry.
- "Re-read ahead" means a hit that matches, or a hit with a cached isolate decision, whose entry is at least 5 s old
  starts one background read (600 ms, about `gate_rows`' p50). At most one such read is in flight.
- `chain_open` is capped where named, and the gate and `chain_open` overlap as `hit-gate` runs them. The row's first
  1.5 s are dropped, as in `b034-gate.md`.

| j9, first 1.5 s dropped | mt12 | mt13 | mt14 |
|---|---|---|---|
| measured | 1,118 (6 over 1 s) | 1,249 (8) | 1,573 (32) |
| colo entry, `chain_open` as measured | 1,118 (6) | 1,249 (8) | 1,451 (24) |
| colo entry, `chain_open` ≤ 300 ms | 881 (0) | 1,021 (3) | 1,451 (16) |
| colo + re-read ahead at 5 s, `chain_open` as measured | 1,118 (6) | 1,149 (8) | 1,161 (9) |
| colo + re-read ahead at 5 s, `chain_open` ≤ 300 ms | **813 (0)** | **1,021 (3)** | **864 (0)** |
| colo + re-read ahead at 5 s, `chain_open` ≤ 150 ms | 663 (0) | 1,016 (3) | 714 (0) |
| gate hidden entirely (`b034-gate`'s bound), `chain_open` ≤ 300 ms | 813 (0) | 893 (2) | 808 (0) |

Hits that waited on members, and member reads per row:

| | mt12 | mt13 | mt14 |
|---|---|---|---|
| measured: hits whose gate read members | 92 | 65 | 75 |
| colo entry: hits that waited / member reads | 28 / 28 | 31 / 31 | 35 / 35 |
| colo + re-read ahead: hits that waited / member reads | 4 / 52 | 8 / 52 | 7 / 52 |

**Why sharing alone falls short.** On mt14, with sharing alone, the hits over 1 s that still read members came 10.2 to
15.4 s after the previous read. Load34's probes arrive at about one a second, but the hits that miss their isolate's
cache are sparse, so the shared entry has often just lapsed. Re-reading ahead keeps one entry fresh for about one read
per 5 s per colo, roughly the member reads that sharing saves. This is why the design adds `COLO_AHEAD`.

**mt13's 1,021 ms is not the gate.** Its three slow hits with the gate at 12 ms total 1,149, 1,313 and 1,021 ms. Their
other stages (`front` 227 to 693 ms, and the rest after the gate) set them, so no gate change passes mt13. With the
gate hidden entirely, mt13 is still at 893 ms with 2 hits over 1 s, and those two are the same `front` hits.

These figures are a replay [inference]. Each hit's other stages are held as measured, and the background reads' load
on the members is not modelled. At 52 reads a row against 65 to 92 measured, the members see fewer reads, not more.

## Can checked chain headers use the same store?

The ruling's premise was that a checked pack header is content-addressed and immutable. **It is not, in this code.**
- Chain files are `compose/<net>/base-<tip>.pack` and `part-<from>-<to>.pack` (`crates/git/src/key.rs`, `Compose`).
  The name is the commits the pack covers, not a hash of its bytes.
- The builder deletes the old base's files after a fold (`crates/git/src/host.rs`, `ready_chain`).
- A rebuilt step for the same tips writes the same key again, from a different `pack-objects` run, with another etag.
- A header set kept by key alone could therefore name a pack that has since been deleted or rewritten. Today
  `open_chain` declines such a manifest to the shard before any byte leaves. With a key-only cache, the body's
  `etagMatches` get would fail mid-answer and cut the clone instead.

What holds is a set keyed by the bucket's `head` object etag:
- Every put of `head` changes its etag. R2 is strongly consistent for read-after-write and delete [doc,
  https://developers.cloudflare.com/r2/reference/consistency/].
- The builder puts every file a head names before it puts that head, and deletes an old base's files only after the
  head that replaces it (`ready_chain`). So every file of the head an edge read was there when the edge read it.
- On a hit, the edge reads `head` beside the manifest. A pack the head names takes its count, etag and length from the
  set. Any other pack is checked as today.
- Bodies still stream on their etags, so a fold after the head read cuts the answer exactly as it does today. Nothing
  new is cut.

Measured on `graft-m1-packs-a` from LAX (`r2probe.js`, 20 rounds):
- `compose/1048603/`, 13 packs: the checks took p50 374 / p99 1,399 ms, and one `list` took p50 90 / p99 230 ms.
- `compose/1048591/`, 1 pack: the checks took 83 / 293 ms.
- The listed etags matched every ranged get's etag.

The cost is one `head` get (about one R2 class-B read, like one check) and one cache match, in place of up to 20
checks. A set is rebuilt once per data center per new head. On mt14, 1048607's head was put 31 times in bucket a and
33 in bucket c during the 15-minute load, about two a minute per bucket.

So yes: one Cache API row serves both, as long as the header set's key is the head etag and not the pack key. Without
the colo share, an isolate cache under the same key also works, but it keeps the stickiness that limits isolate caches
(about 8 isolates per key in 10 s, `b034-gate.md`).

## Cost

- Hosted: four scratch Workers and about 3,700 requests on graft-m1's account. No site day and no container. Under
  $0.01 [inference: Workers requests are inside the plan's included 10 million].
- The Cache API has no price line on the Workers pricing page. Its calls count against per-request limits, not as
  billed requests [inference].

## What a build would be (not started)

1. In the edge, `mod gate` (`workers/edge/src/lib.rs`): `colo_key(repo, who, record)`, the SHA-256 path on a fixed
   origin plus the edge's key version; `colo_get` bounded by `COLO_MATCH_WITHIN`, serving only `Allow` with a stamp
   under `READ_CACHE` less `COLO_SKEW`; `colo_put` after `reopened`'s `Allow`; and the `waitUntil` re-read at
   `COLO_AHEAD`.
2. In the chain path, `crates/git/src/host.rs` `manifest_check`: a `head` read beside the manifest and a header set
   keyed by the head etag, in the isolate and the Cache API.
3. Tests that fail on plausible wrong builds: a key that drops the credential, repository, or record; a put of
   `Hidden`; a stamp taken from the put time instead of the fold read; a match served past `READ_CACHE` less
   `COLO_SKEW`; a header set keyed by pack key; and a set put after a failed check.
4. It ships on an edge day, not a site day.
