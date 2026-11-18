# B0-34 m1b34: what made the validate and clone tails

Code-only follow-up to the combined graft-m1 run (`b034-m1.md`, a1 `mwlz5c1a`, row 10:28:13 to 10:43:13Z, kill at
300 s, second network hot at 420 s, hostile at 600 s). The operator asked (T-01a17bda) whether the validate p99 of
45.9 s, and the composed clone wait before the shard object answers (p99 7.6 s), come from (b)'s exclusive lock,
`index-pack` itself, a catch-up wait, or the queue behind the hot networks' stock misses on the same shard object, and
which of the 50 shard 503s were resets and which the abort. Cost $0: read-only Workers Logs and GraphQL pulls.

## Sources

- Every `GitShard` event of graft-m1-site-a, b and c from 10:22 to 10:47Z, 11,266 events, with the account and
  subdomain replaced by placeholders: `tests/fanout/raw/shardev-m1b34.jsonl.xz`.
- The run's `shard miss` lines (`raw/misslines-m1b34.jsonl`) and joined probes (`raw/misses-m1b34.txt`), and the
  container process samples (`raw/shardprocs-m1b34.jsonl`).
- Container vCPU and memory per minute from `instcpu.py` (`INSTALL=graft-m1`).
- `tests/fanout/attr.py` prints every per-object number below from the first two files:
  `python3 tests/fanout/attr.py tests/fanout/raw/shardev-m1b34.jsonl.xz tests/fanout/raw/misslines-m1b34.jsonl 1794997693575`.

The monorepo's first shard objects are `b824b984` on site a (the home) and `898a0f56` on site b. They took 6,188 of the
run's validates. The hot networks' validates ran on their own objects and containers (`git#<site>#net:<repo>`), never
on these. So the "hot networks' stock misses on the same shard object" in the question are the monorepo's own stock
misses: the clone probes whose want the tip moves had pushed over the diff cap or off a ref tip.

## Answer

**One queue explains the validate tail: the network's lock on the monorepo's container.**
- A stock upload-pack holds it shared for as long as its client reads.
- A catch-up's `/ingest` then waits for it alone.
- Every validate and upload-pack that arrives after that waiter queues behind it.

It is not (b)'s exclusive section, and not `index-pack`.

The composed clone wait is mostly a different thing: the wait for the running catch-up after a 409 "not a ref tip".
That catch-up is the same writer stuck in the same queue only in the second and hostile phases. So it is one fix for
the validate tail and the 503 tail, and the composed wait before the kill has a cause of its own.

## Item 1a: the validate tail

The validate p99 by phase on the shard object, against the edge's 45.9 s:

| Object | j9 | kill | second | hostile |
|---|---|---|---|---|
| site a `b824b984`, p50 / p99 | 2.3 / 9.7 s | 0.3 / 2.8 s (241 of 277 answered 503, the kill) | 1.1 / 38.0 s | 1.0 / 35.7 s |
| site b `898a0f56`, p50 / p99 | – | 1.3 / 11.8 s | 1.4 / 12.9 s | 3.0 / 54.4 s |

Of the 223 monorepo validates over 10 s, 204 were in the second and hostile phases.

**On site a, a queued writer, not a slow one.** Three catch-ups each waited 29 to 36 s before `indexIngest` even ran.
That is `/ingest`'s `adopt`, which takes the lock alone first: `stored.total_ms − index_ms` = 36.0, 35.1 and 29.4 s.
Their exclusive sections took 1, 28 and 30 ms (`lock_ms`), and their `index-pack` 0.5, 7.5 and 13.1 s, under the shared
lock as (b) intended.

| Catch-up on site a (UTC) | Waited for the lock alone | Lock alone | Validates started meanwhile | Their p99 |
|---|---|---|---|---|
| 10:37:44 to 10:38:33 | 36.0 s | 1 ms | 199 | 36.2 s |
| 10:38:46 to 10:39:56 | 35.1 s | 28 ms | 275 | 44.9 s |
| 10:39:59 to 10:41:24 | 29.4 s | 30 ms | 351 | 26.0 s |

- 90 of site a's 115 validates over 10 s in the second and hostile phases overlap one of these three waits.
- They end in batches: 55 of the 115 finish within 3 s of 3 or more others that started at least 2 s apart. That is
  the shape of a queue that drains at once when the writer gets the lock.
- The readers it waited behind were stock upload-packs. Site a's container had up to 15 stock `--shallow-file`
  upload-packs at once, about 2.6 GB RSS each and 40 GB together. Its stock misses' first byte came 20 to 99 s after
  they arrived (`misses-m1b34.txt`: container p50 19.6 s and p99 35.8 s over the diff cap).
- The container ran at 3.8 to 4.0 vCPU from 10:37 to 10:44 and at 12.0 GiB of 12 GiB memory from 10:39 to 10:41.

**On site b, (b)'s exclusive section was long twice: 35.1 s (10:38:50 to 10:39:35) and 35.3 s (10:42:22 to 10:43:07).**
- Neither was `index-pack`: the stored `index_ms` (35.3 and 36.9 s) runs to the end of the midx step, and the lock
  alone covers the move, the record, and `midxAfterIngest`.
- Both windows coincide with 14 to 21 stock upload-packs at 28 to 31 GB of RSS on that container, and the next sample
  shows `multi-pack-index write` running.
- So the 35 s is the midx write (every 50 ingests) starved for CPU and memory by the stock misses [inference: the
  shard reports no midx time on its own].
- Only 10 of site b's 46 validates over 10 s overlap those two windows. The rest coincide with stock upload-packs on
  the same container (p99 54.4 s with one in flight, 13.5 s without).

**What it is not:**
- **Not `index-pack` under the lock alone.** (b) held: 334 of the 359 monorepo catch-ups held the lock alone 1 s or
  less in all. The other 25 held it 4 to 7 s, and 35 s twice on site b. That is about one per 50 ingests, the midx
  write's cadence [inference: no `midx_ms` is reported].
- **Not a catch-up wait in `/validate`.** Validates go on the disk as it stands (`validate_on`), and only a missing
  base waits for a catch-up. 33 of site a's 503 validates were at or over 15 s (that wait running out), against 115
  slow 200s.
- **Not the container's validate admission.** Its 429s answered in about 2 s (423 in all: 387 on site a, 32 on site
  b), the agent's 2 s wait, and they are not in the slow tail.

## Item 1b: the composed clone tail

- 393 composed probes: edge→shard arrival (the clock offset taken out) p50 0.8 s, p90 2.3 s, p99 7.6 s. The shard
  side, arrival to first byte: p50 0.4 s, p99 16.8 s.
- 48 probes took over 5 s, all in j9 (15), kill (32) and second (1), and none in hostile, where every monorepo probe
  was stock.

| Cause of the 48 composed probes over 5 s | n |
|---|---|
| A 409 "not a ref tip or behind HEAD", then the wait for the running catch-up (`compose_then_catch_up`): p50 2.9 s, p99 13.2 s after arrival, over all 82 such | 22 |
| Edge→shard arrival 3 s or more, on site b (13) and site c (3) right after the kill moved the monorepo off site a, and in j9 on site a (5) | 21 |
| The wait for the chain (`Compose::ready`) on site c (13.5 s p50 after the kill) and b | 5 |

So the composed tail before the hostile phase is mostly the post-409 wait for a catch-up, and arrival delay on the
sites the kill moved clients to. It is the lock queue above only where those catch-ups themselves wait on it (second
and hostile). The p99 7.6 s of arrival is not the shard object's queue: arrivals with 50 or more requests started on
the object in the 5 s before had p99 2.4 s, below the 7.6 s of the rest. [inference: it is the edge's own retry to a
next site after the kill, which the probe's single timer includes.]

## Item 1c: the 50 shard 503s

The probe's 503 is the shard object's own answer, with no container reset in it. `outcome` was `ok` with status 503,
or the edge cancelled after it. Joined to their miss lines:

| What ran out | Site a | Site b | n |
|---|---|---|---|
| The 15 s catch-up wait right after the container's 409 (`stock: over the diff cap`, or `no chain` once): no catch-up ended within `wait` | 23 | 0 | 23 |
| The wait for the running catch-up (`idle`, unbounded), then a composed retry that missed, then the 15 s wait | 21 | 4 | 25 |
| The shard kill at 300 s: no chain on the restarted container, and one request that got no line | 2 | 0 | 2 |

- The 21 on site a waited 29 to 99 s in all (container column, p99 99.2 s).
- The "waited for the running catch-up" rows are this queue: they waited for the catch-ups in the table above, which
  were waiting for the lock behind stock readers, and then the 15 s ran out on the next one.
- Resets: only the 2 at the kill. The other 48 are timeouts of the catch-up wait.
- The abort is not among the 50. The driver's 120 s abort hit 21 monorepo probes and the hot networks' 41 in the
  hostile phase (`AbortError`, `b034-m1.md`). Those got no 503: they had no first byte by 120 s.
- One more hazard: `compose_then_catch_up`'s `idle` has no bound, so a miss can wait out a whole stuck catch-up before
  its 15 s even starts.

## Ranking, for item 3

1. **Stock misses over the diff cap after tip moves.** 30 probes, plus the 23 + 25 503s whose 409 was over the cap or
   not a ref tip. Their stock upload-packs, 2.6 GB each and up to 15 at once, held the lock shared for 20 to 99 s,
   which queued the catch-up writer and every validate behind it. Removing them removes the queue: one fix for the
   validate tail and the 503s.
2. **"Not a ref tip".** 19 probes went stock this way. 82 composed probes waited for a catch-up after the same 409,
   and 25 of the 503s were this path.
3. **Shard concurrency (k).** The monorepo stepped to k = 3, but pool shards take only forwarded validates. Clones and
   stock misses stay on the first shard, so more k does not touch this queue.

## Item 3: proposal outline (design first; it touches the composed path)

**(i) Compose a want over the diff cap from a newer base instead of serving stock.**
- When the want is a ref tip and its diff from the chain tip is over `composeDiffMax`, the chain's next step is due;
  the cap exists because a composed pack would carry most of two trees.
- Answer 409 `stock: over the diff cap` as now, but have the shard object start the chain step (`Compose::start`)
  and, after the running catch-up, compose again (`compose_then_catch_up` for this reason too), not stock.
- Stock only when the chain step fails.
- Test: a Go test that a want past a tip move over the cap composes after one step. A Rust test that
  `compose_then_catch_up` retries this reason.

**(ii) Bound and order the stock path.**
- At most N stock upload-packs per network container at once (N = 2 [guess]). The rest wait in the shard object, not
  on the lock.
- Bound `idle` by the caller's `wait`.
- A stock reader that holds the lock shared past T seconds stops taking new readers ahead of a waiting writer. Go's
  `RWMutex` already blocks new readers once a writer waits; the problem is the readers already in.
- Option: the stock path takes no network lock, as composed misses do since round 26, and serves from refs read once
  at its start. It needs an evict or fresh init to wait for in-flight stock reads, a reader count beside the lock.

**(iii)** A `midx_ms` beside `lock_ms` in the `/ingest` answer, to confirm the site b 35 s sections.

(i) and (ii) both touch the shard's protocol or the composed path, so I'll send them as a design before code, as
asked.
