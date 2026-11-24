# voter-shed: a voter answers edge requests at once past an open-request bound (design, not built)

Asked by the orchestrator, 2026-11-18: option (b) from "Why the leader's AppendEntries stall 1 to 6 s under 500 agents"
(`lm1-memory.md`). This page is the design only. No code exists yet. Evidence comes from `tests/fanout/votershed/`,
replayed on m47's trace (`raw/tsave-m47b33.jsonl.xz`, B0-33, 01:00:18 to 01:05:18Z) and B0-02's m2bb29d
(`raw/tsave-m2bb29d.jsonl.xz`, B0-29, 02:17:10 to 02:27:10Z).

## What it is

A voter (a leader or a follower, never a learner) counts the requests open in its handler. When an edge request
arrives and that count is already at or past `EDGE_OPEN_MAX`, the voter answers it at the top of `Routes::fetch`,
before reading its body. It never steps it into `raft::Member` and never waits on the output gate. So a request that
would have queued ahead of `/raft` and the leader's own confirm leaves the isolate in microseconds.

**What it never sheds:**
- `/raft`;
- every member-to-member call (any request without `x-graft-client`);
- the harness's observation routes (`/status`, `/stats`, `/trace` and the rest of the `observe` list);
- the leader's own `sync()`, confirm and AppendEntries path. These are not requests, so the counter never sees them.

Only a request that carries `graftyard_lookup::CLIENT_CALL` (`x-graft-client`) is a candidate. Only an edge sets that
header, never a site script, cron, sweep, fold audit or peer lookup. That makes it the edge-originated test the
orchestrator asked for, and it is already on every edge call. `x-graft-from` is a test-faults header, so it cannot
serve.

## The rules

| Route | Under the bound | At or past `EDGE_OPEN_MAX` |
|---|---|---|
| `/raft`, member calls, observation routes | unchanged | unchanged, never shed |
| `/read` (edge) | unchanged | `200` `overloaded_reply(OVERLOAD_RETRY_MS)`, which the lookup already retries |
| `/outcome_by_request` (edge) | unchanged | `200` `{"busy": true}` |
| `/commit`, `/commit_many`, `/outcome` (edge) | unchanged | unchanged (see "Writes" below) |
| `/suspect` (edge), repeat of a cached `promised` | answered from the cache, no step | answered from the cache, no step |
| `/suspect` (edge), first report of its key | stepped as today | stepped as today, never shed |
| `/suspect` (edge), any other report | stepped as today | `200` `{"verdict": "busy"}` |

**`/read`.** The answer is the existing `overloaded` reply with `retryAfterMs`. `call_opt` retries it on the same member
after that wait, up to `OVERLOAD_TRIES`, and never reports it with `suspect` (`classify`, `Answer::Overloaded`).
`spread_once` takes it as `Spread::Shed`, which by the "Follower read" rule never moves the read to the leader. A
read-class request then answers 503 with `Retry-After`. So a shed read cannot fall through to the leader.

**`/outcome_by_request`.** `{"busy": true}` is already a skip with no report in `classify`. In `receipt`, the ask gets a
non-200-shaped answer and `busy()` decides. `busy()` is true when this isolate heard the member within
`COMMIT_RECEIPT` (2 s). In that case the send stays open with no report and no re-send, which is the right outcome for
a leader that is alive and overloaded. When the isolate has not heard the member, the missed receipt is reported as it
is today. A shed does not report more than a slow answer does.

**`/suspect`: liveness.** A voter needs one report per colo inside SUSPECT's window, not hundreds:
- The voter keeps the last verdict it stepped per (colo, about, term), for `SUSPECT_REPEAT` (1 s [guess]), and only
  when that verdict was `promised`.
- A repeat inside that time is answered with the cached `promised` at once, at any open count, with no step. While
  this member's promise runs, a repeat from the same colo adds nothing: `reports` keys by colo and keeps the newest
  time, and the window is 10 s.
- Every other verdict (`started`, `busy`, `leader`, `mayNotLead`, ...) is never cached, so the next report steps.
- The cache entry is dropped when the member's term changes or its promise ends (`promise_until` passes), so a report
  that would start a campaign once the promise lapses always steps. That is the case that elects.
- Past the bound, a first report of a (colo, about, term) key still steps. So the first report from a new colo is
  never shed.
- Past the bound, only a report whose key stepped within `SUSPECT_REPEAT` with a non-`promised` verdict is shed, as
  `busy`. A colo whose report was cached or stepped in the last second loses nothing, because the step it would make
  repeats one already counted.

The edge's damper (`SUSPECT_GAP`, 1 s, per isolate) stays. This cache is the member-side counterpart across the edge's
hundreds of isolates.

**Writes.** `/commit`, `/commit_many` and `/outcome` are not shed. A commit already has `Admit`'s budget and its
`retryAfterS`. A commit shed at the handler top would come back as a missed receipt, which the edge re-sends to the
same member (by design, "Commit and read paths"), adding load instead of removing it.

## The bound, `EDGE_OPEN_MAX` = 64 [guess]

It counts what sits ahead of `/raft`: the isolate's open requests, the `Entered` row's `open` field. That field is
counted by `Inbound` today, which only test-faults builds compile. The shed needs a plain counter in every build. It
is one `Rc<Cell<usize>>` guard taken at the top of `fetch`.

From m47, leader-to-follower out legs corrected for pair skew, by the leader's open requests at send
(`votershed/a7.py`):

| leader open at send | 0–25 | 25–50 | 50–75 | 75–100 | 100–125 | 200–250 | 300–400 | 500–750 | 1,000+ |
|---|---|---|---|---|---|---|---|---|---|
| out leg p50 (ms) | 38 | 237 | 734 | 869 | 1,023 | 1,881 | 2,849 | 3,799 | 4,956 |

The out leg reaches CALL_DEADLINE (1 s) at about 100 open, and is already 0.7 s at 50 to 75. A bound of 64 keeps it
under about 0.7 s on this trace. That is lower than the 300 to 1,000 band the append-stall section suggested, because
the leader's own open count, not the follower's, sets the out leg (`lm1-memory.md`, "voter-shed design, unfinished").
It stays [guess] until a run measures the out leg with the shed on.

On B0-29 (m2bb29d) the leader's open count was p50 9, p99 34, max 145, and 6 of 6,374 edge `/read`s entered at or
past 64 (0.1%). So the bound sheds nothing on a row that passes.

## What it would remove, replayed on m47 (`votershed/removed.py`, B = 64)

| member | inbound | `/read` + `/outcome_by_request` shed | `/suspect` entered | `/suspect` stepped with the cache (at most) | removed |
|---|---|---|---|---|---|
| 0 | 520/s | 171/s | 119/s | 40.3/s | 250/s (48%) |
| 1 | 339/s | 97/s | 45/s | 28.1/s | 113/s (33%) |
| 2 | 532/s | 93/s | 194/s | 54.5/s | 233/s (44%) |

Corrected 2026-11-24 (`tests/am1/results/trace-dedup.md`): the old trace merge dropped equal rows inside one save, so it read stepped 8.4 / 6.1 / 18.3 a second and removed 282 (54%) / 135 (40%) / 269 (51%).

- The replay holds each member's arrivals as traced. A shed read's retry after `retryAfterMs` comes back, so on a
  member that stays past the bound the arrival rate does not fall by the whole share. What falls is the work and the
  queue time each one holds ahead of `/raft`.
- The `/suspect` cache figure is an upper bound on steps: the trace has no `promise_until`, so the replay never drops
  a cache entry at promise end.
- The cache alone, with no bound, answers 37 to 72% of stepped reports on m47 at `SUSPECT_REPEAT` 1 s
  (`votershed/damp2.py`; it read 63 to 73% before the 2026-11-24 correction).

## The test that fails on main

`workers/site` has no handler-level test harness today, and `Routes::fetch` takes a `worker::Request`. The plan keeps
the rule pure and tests it there, as `gate` is tested:
- `fn shed(route, client, open, suspect_cache_hit) -> Shed` in `member.rs`.
- One test drives a voter's request mix: 200 edge `/read`s open, then a `/raft` from a peer, then a member's own
  `/read` (no client header), then an edge `/read` and an edge `/suspect` first report. It asserts that the `/raft`
  and the member's call enter (`Shed::Serve`), the edge `/read` is `Overloaded`, and the first `/suspect` is `Serve`.
- On main there is no shed, so the edge read is served and the test fails.
- A second test drives `SuspectCache` through a promise that ends: a repeat inside 1 s with the promise running is
  answered, and the same repeat after the promise ends steps.

Planted bugs, each one a single-line change:
1. Shed `/raft` past the bound.
2. Shed a request with no `x-graft-client` header.
3. Shed the first report of a new colo.
4. Cache a `started` verdict.
5. Keep a cache entry after the promise ends.
6. Use `>` for `>=` at the bound.
7. Answer a shed `/read` with a 503 (which `classify` reports as missed).
8. Shed `/commit`.

**The sim needs the same rule, since `/suspect` handling must keep liveness.** `crates/raft/tests/common/sim.rs` gets
a member-side `suspect_cache_ms` and an open-request bound on its edge reads. The documented long sweep (harsh, gray,
live at 4,000 seeds) then runs with both on, and Porcupine on all three histories. The gray target: every group moved
within 20 s ("Gray region"). No `crates/raft/src` change is planned. The cache lives in the site host, keyed on the
verdict the member already returns. If the sweep shows that a cached `promised` can mask a lapse that `promise_until`
does not cover, the cache moves into `on_suspect` instead, and that design comes back to you first.

## Design and decision text

- **DESIGN, "Follower read".** One sentence: a voter whose handler holds `EDGE_OPEN_MAX` (64 [guess]) open requests
  answers an edge `/read` `overloaded` and an edge `/outcome_by_request` `busy` at once, before its body, and never
  `/raft` or a member's call.
- **DESIGN, "Gray region".** One sentence: a voter answers a repeat `suspect` report from a colo whose last report it
  stepped as `promised` in the last second from that verdict, until its term changes or its promise ends.
- **DECISIONS, `voter-shed`.**
  - Beat: (a) per-voter report caps at the edge, held as the follow-up, and (c) AppendEntries off the output gate,
    not started.
  - Flip: a run where the out leg still passes 1 s with the leader under the bound, which would point back at (c),
    or a gray sweep seed past 20 s with the cache on.
- **OPEN-QUESTIONS.** Whether the leader's open requests are mostly output-gate waiters that a shed does not reach.
  This is the main risk below.

## Risks and what would show them

1. **The shed may not shorten the out leg.**
   - Most of the leader's open requests may be answered reads and commits waiting in `ready_or` on `settled()`. A shed
     at the top stops new ones, but not those already waiting.
   - The trace cannot tell a waiter from a request still being stepped. A run with the shed on, compared at the same
     load on the same tsave, is the measurement.
   - If the out leg still climbs with the open count at 64, the cause is the gate's waiters, and (c) is the next
     design.
2. **It may not bound the queue at all.** B0-02 warned that `CallSent` stamps raw `Date.now()`. If the out leg is the
   sender's clock reading early, the shed changes the stamp, not the transit. The run should stamp `CallSent` with the
   member clock as well (test-faults only, one more field).
3. **API errors.** Shed reads become 503s with `Retry-After` that the client sees, on rows that already fail.
   - On m47, 61 to 63% of the leader's edge reads entered past 64.
   - Learners take the shed reads that `read_spread` sends them, but the leader's `packed:lead` and `core:lead`
     (143/s and 64/s on member 0) are leader-only reads.
   - B0-33 gates commit p99 and agent-class ok/s, not API p99, so the trade fits that row. B0-29's API p99 gate is
     untouched, since nothing sheds there.

## Ownership

- The member side (`workers/site/src/member.rs`, `host.rs`) is this thread's.
- The edge needs no change: `overloaded` and `busy` are handled today.
- T-01a1d805 owns `spread_once` and `spread_verdict`. A shed `/read` is a `Spread::Shed` answer it already handles,
  and this design adds no new answer shape.

## Point 2: what the leader's open requests are (m47, before any code)

The orchestrator approved the design on condition that the trace first shows whether the leader's open requests that
track the out leg sit before the handler body (sheddable) or wait past it (not sheddable). They wait past it, so the
shed as designed does not move the out leg. Scripts: `votershed/p2*.py` on `/tmp/shed/rows.pkl` (`prep.py`), load
window 01:00:18 to 01:05:18Z, each member only while it led (terms-m47b33.jsonl).

**Entries vs open.** With the leader at 64 or more open, its entries were `/read` 44%, `/outcome_by_request` 18%,
`/commit` 17%, `/suspect` 9%, `/raft` 8%, `/outcome` 3% (104,698 entries, all three members). Entries are not the open
count, which weighs each request by how long it stays:

| leader member | `/raft` share of open at ≥ 64 (exact, Entered → CallLeft) | read residence, low-commit bins (open ÷ read rate) | edge reads' share of open at ≥ 64, upper bound |
|---|---|---|---|
| 0 | 16% | p50 37 ms (1 bin only) | not bounded this way; NNLS fit 0% |
| 1 | 19% | p50 8 ms, p90 20 ms (20 bins) | ≤ 3.5% |
| 2 | 7% | p50 7 ms, p90 131 ms (10 bins) | ≤ 8.7% |

A non-negative fit of the open count to 6 s trailing arrival rates by class (`p2o2.py`, R² 0.45 to 0.69) puts reads'
residence at 0.00 s on all three leaders, and the commit family (`/commit`, `/outcome`, `/outcome_by_request`) at 1.0
to 3.3 s, 80 to 100% of the non-`/raft` open. The fit cannot separate `/outcome_by_request` from `/commit` (their
rates correlate 0.64 to 0.83), so the 1.0 to 3.3 s is the family's [measured, as a fit].

**The open count follows commits, not reads** (`p2i.py`, 1 s bins while leading):

| commit-family arrivals in the prior 3 s | leader median open (members 0 / 1 / 2) | edge reads/s p50 (max) |
|---|---|---|
| 30 to 149 | 3 / 1 / 1 | 195 to 264 (456 to 1,336) |
| 150 or more | 376 / 3 / 700 | 199 to 358 (1,581 to 2,182) |

Up to 1,336 edge reads a second passed through a leader with a median of one request open. The reads are answered
in milliseconds (lease reads); what stays open is `/commit` held up to `COMMIT_HOLD` (1 s) and `/outcome` up to its
`wait_ms` (10 s), each then held by `ready_or`'s `settled()`. Point 1 forbids shedding `/commit`, and `/outcome` is
the same admitted ticket's wait.

**The out leg does not depend on the read rate** (`p2e.py`, gate-open sends only, 956): Spearman of the out leg with
the leader's open 0.80, with commit-family arrivals 0.66, with loop lag 0.67, with edge reads/s 0.14, with `/suspect`/s
−0.05. Within an open band the read rate does not move it: at 400+ open, p50 4,110 ms at 0 to 50 reads/s and 3,565 ms
at 200+.

**The round trip, skew-free** (`p2p.py`: CallSent → AnswerArrived on the leader's clock, less the follower's
Entered → CallLeft; 1,748 answered calls). Its excess over 300 ms splits:

| leader open at send | n | p50 / p90 | output gate still closed at send | then loop lag at send | rest: wire, and the answer waiting for the leader's turn |
|---|---|---|---|---|---|
| < 64 | 220 | 285 / 2,791 ms | 14% | 0% | 86% |
| 64 to 299 | 567 | 390 / 4,378 ms | 32% | 3% | 65% |
| 300+ | 961 | 990 / 6,586 ms | 37% | 20% | 43% |

The follower spends 0 ms at p50 on these calls. The leader's output gate and turn queue, filled by commit waiters
being released, hold the rest [inference: the trace does not stamp when a held send or a returned answer gets its
turn].

**Verdict.**
- The handler-top shed of edge `/read` and `/outcome_by_request` would take at most about 4 to 9% of the leader's open
  count where it is measurable, and the out leg does not move with the read rate. It would not shorten the out leg.
- What holds the leader is its commit waiters and its output gate. That is (c)'s territory: AppendEntries that do not
  wait behind the gate and the waiters' release, or fewer commit waiters held open at once.
- The shed still has value as the smaller safety bound the orchestrator named: it caps a read flood (1,794 to 2,203
  reads/s in some leader seconds), and its follower side and `/suspect` cache are unaffected by this finding.
- Not built. Waiting on the orchestrator's choice.
