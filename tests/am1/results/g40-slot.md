# g40-slot: where h1A's agent validate time went, and the lever for it

G40's open half, 2026-11-24 ($0, raws only). B0-33 h1A (`graft-m2b`, run `mwub8kiw`) had a home site that never
answered 429, so `agent-spill` moved nothing on a full home gate. The lead asked for one lever: an agent validate
hedge near the home's p90, or a gate that answers 429 on queue age. This file prices both from the raws and records
the one built (`decisions/DECISIONS.md`, "`slot-bound`"). Tags follow `design/SHARED.md`, "Read this first".

The raws came from edge refusal's thread (T-01a20679-a80d-7288-a3a4-c13a1d909c97) and are kept on that orb in
`.amp/xfer/h1A/`:
- `a1-pushes-b0-33-mwub8kiw.jsonl`: 13,512 agent pushes, with each push's server-timing;
- `rows-h1A.status`: `pushfull.py` and `instcpu.py`;
- `shard-validate-nonok-h1A.jsonl`: Workers Logs, every `/validate` that did not answer 200 on sites a, b and c, from
  06:31:50 to 06:37:20Z.

## Where the agent validate time went [measured]

Agent validates by answering site and try count (`validate_site`, `validate_tries`; ms):

| Site, tries | n | validate p50 | p90 | p99 | Ended `full` |
|---|---|---|---|---|---|
| 0, 1 | 11,152 | 380 | 1,488 | 4,875 | 2,125 |
| 1, 2 | 709 | 4,562 | 6,634 | 7,138 | 680 |
| 2, 3 | 299 | 5,020 | 6,389 | 34,404 | 299 |
| none answered | 577 | 2,334 | 4,578 | 7,991 | 354 |

**The 1,010 move-ons were pool 429s, not a slow home.** Site a's network shard forwarded each agent validate to a pool
shard with `via=0&pool=1` (`workers/site/src/shard.rs`, the `push_validator` forward). The pool's container gate held
it the agent's whole 2 s slot wait (`containers/git/server/main.go`, `validateGate`), then answered 429 `validate queue
full`.
- **Site a's pool:** 1,007 such 429s, wall p50 2,007 ms and p90 2,017 ms, cpu p50 1 ms, all on one pool object. Each
  was paired with the first shard's call, which ended `canceled` at about 2,033 ms.
- **The forward passes a 429 through** (`200 | 422 | 429`), so the edge read it as `Busy` and moved the push to site b
  (`agent-spill`).
- **Site b's pool** did the same to 299, which went on to site c.

So the 1,010 pushes reached the next site with 2 s of their 5 s `AGENT_PUSH_BOUND` gone. Of them, 981 ended `full`,
most by `bound_at_send` before any send. 29 committed, with p50 790 ms of the bound left at their first send.

The other non-200s on site a did not move pushes:
- 459 first-shard 503s with no `via`, wall p50 95 ms, paired with 458 pool 409s, wall p50 126 ms. This is the
  pool-needs-catch-up path, which the edge sends to site a's shard again with `via`.
- 6 calls with status 0.

**The home's slow tail is the same wait.** Of the home's one-try validates:
- the quick ones (under 1.5 s, 10,037) had `v_walk` p50 65 and p90 173 ms, `v_index` p50 32 ms, and other time p50
  245 ms;
- 885 spent more than 1.5 s outside `v_walk` and `v_index`, and 728 of those ended `full`.

So the slow tail is the wait for a pool slot, not git work. Pool 5 ran at 4.03 vCPU peak on site a and 3.74 on site b
(instcpu).

## The levers priced

**An agent hedge at about 1.5 s (the home's p90): rejected.**
- About 1,115 one-try home validates (10%) pass 1.5 s. Each would start sites b and c beside it.
- The hedged tries reach the next site's network shard, which forwards to its own pool shard by the same trailer pick.
  Site b's pool 5 was at 3.74 vCPU, and h1A's 299 third tries show its gate full too.
- A hedge adds tries to gates that are already full. It does not make one of them refuse sooner. For a home that
  answers under 1.5 s it changes nothing.

**A gate that refuses on queue age, in its bound form: built.**
- The 2 s that cost the budget is the gate's own deadline, spent in a queue that does not drain in time.
- A 429 at once helps only if it comes back while the push still has bound to commit elsewhere. So the gate takes the
  push's own bound, and refuses at once when the agents queued ahead will not drain within it at its measured run
  time.
- Queue age alone (refuse when the oldest agent waiter is past N ms) refuses the same pushes later, and its N has no
  anchor. Depth over run time uses what the gate already measures.

**What it would have saved [inference].** The estimate replays each moved push's own stages: front, gate, context, and
validate less 2,007 ms per pool 429. That arithmetic reproduces `5000 − budget` on the ok pushes to 0 ms p50.
- With the pool's 429 at once, 244 of the 711 second-site pushes and 199 of the 299 third-site pushes would reach
  their first send with over 300 ms of bound left. That is 443 pushes that could commit, against 29 now.
- For scale, ok pushes had p1 343 ms and p5 809 ms left.
- It is not a hosted measurement. Refusing sooner also moves load to sites b and c sooner.

## Settle line

One B0-33 row with `slot-bound`, read against h1A:
- agent ok pushes;
- `validate_tries` ≥ 2 that end ok;
- the pool 429s' wall time in Workers Logs: none near 2 s on a push with under 3 s of bound left;
- per-shard vCPU by site.
