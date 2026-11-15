# f74-trace: a per-call trace of member calls, for F74

Branch `f74-trace`, built off `main` 6a9857f2. It is the instrument F74 asks for (`decisions/OPEN-QUESTIONS.md`): on
the same `/raft` call, the caller's send stamp, the callee's handler entry and end, the caller's answer, and each side's
event-loop lag. It runs under `test-faults` only. A product build sends no new header, records no new row, and keeps
its ticker unchanged. Nothing in `crates/raft` changed.

## What a call records

Each `/raft` call gets an id from its caller's host. The caller sends the id in `x-graft-call`, beside the existing
`x-graft-sent`, `x-graft-from` and `x-graft-hop` headers. Ids start at the host's build time in ms times 1,000, so they
stay unique across resets and below 2^53, which keeps them exact in the JSON trace. Rows are
`[at, kind, peer, batch, index, ms]` in `Stats::trace`, which `/trace?since=` serves:

| Kind | Where | `at` | `batch` | `index` | `ms` |
|---|---|---|---|---|---|
| `CallSent` 23 (new) | caller, just before the fetch | the `x-graft-sent` stamp | call id | caller's `/raft` fetches waiting for headers, this one included | caller's loop lag |
| `CallArrived` 20 | callee, after the body is read | handler time | call id (was 0) | `x-graft-sent` | body read ms |
| `CallLeft` 24 (new) | callee, as the handler returns | the `x-graft-left` stamp | call id | handler entry, at the top of `Routes::fetch` before any injected fault awaits | callee's loop lag at entry |
| `AnswerArrived` 21 | caller, at response headers | arrival | call id (was 0) | `x-graft-left` | 0 |

Kind 22 is left free for `AppendAnswered` on the held far-leg merge (`m0-adaptive-append`). Trace kinds are append-only,
so 20 and 21 keep their columns, and only `batch` now carries the id. `tests/nr3/legs.py` ignores `batch` on those two
kinds, so its wire split still reads them.

Loop lag (`host.rs`, `loop_lag`) comes from the member's 100 ms ticker. It is the larger of two values: how late the
last tick woke, and how overdue the pending tick is now. A stopped ticker reads 0. Unit test:
`loop_lag_reads_the_pending_ticks_overdue_time_or_the_last_wakes`.

## The clock caveat

Workers' `Date.now()` advances only on I/O. It returns the time of the isolate's last I/O and does not move while code
runs [doc, https://developers.cloudflare.com/workers/reference/security-model/; quoted in `tests/m1/results/m1-wp2.md`].
For this trace that means:

- No I/O runs between the `CallSent` stamp and the fetch, so the stamp is the clock the fetch was issued at. If the
  caller's turn ran JavaScript before the stamp, the stamp reads that turn's start. That time then counts in the leg
  before the stamp, the leader's own queue, and never in `out`. The caller's lag at send is there to show such a turn.
- The callee's entry stamp reads the turn that delivered the request. So `out`, from `sent` to `entered`, is the wire
  plus the platform's queue in front of the object, plus any of the callee isolate's turns ahead of it. The callee's
  lag at entry tells the last apart.
- `work`, from `entered` to `left`, is on one clock. It undercounts a handler whose last turn ran JavaScript after its
  last I/O.
- A lag reads 0 when no tick was due, so 0 means "no stall that held a tick", not "no CPU time"
  (`tests/m0/results/m0-wp12.md`, "A limit of the lag measure"). A stall that ends before the next tick shows only in
  the next tick's lateness.
- `out` and `back` cross two isolates' clocks. `tests/f74/calls.py` estimates each caller–callee pair's skew as
  (min `out` − min `back`) / 2, which assumes the fastest call took the same time each way [inference], and corrects
  both legs by it. Their sum, `total − work`, needs no skew estimate.

`awaiting` is there because one Workers invocation has at most six connections waiting for response headers, and a
seventh is queued until one of the six gets its headers [doc, https://developers.cloudflare.com/workers/platform/limits/,
"Simultaneous open connections"]. A `/raft` call waits for headers until the callee's handler returns. A leader with
ten peers and pipelined appends can therefore have more than six fetches open from one invocation. Whether the
platform counts a Durable Object's spawned calls against one invocation's six is not documented [unverified]. Grouping
`out` by `awaiting` in the trace answers it for this host.

## How to take it

On a test-faults install from a tree with this branch, `tests/nr3/tsave.mjs` saves every voter's rows and, with
`LEARNERS=1`, every learner's, before an isolate reset loses them:

```
LEARNERS=1 EVERY=10000 DONE_FILE=/tmp/f74.done node tests/nr3/tsave.mjs repo:ID > tsave-TAG.jsonl
python3 tests/f74/calls.py tsave-TAG.jsonl --from 2026-11-15T04:33:00Z --to 2026-11-15T04:34:30Z --rows calls-TAG.tsv
```

`EVERY=10000` is suggested because `TRACE_MAX` keeps 60,000 rows (2.9 MB of `[u64; 6]`), and a loaded leader adds
three to five rows per call. For each caller–callee pair, `calls.py` prints `total`, `out`, `work` and `back`. For the
calls past `--deadline` (1,000 ms), it says which leg held the time, or that the call never reached the callee in the
trace. It also groups `out` and the misses by the caller's waiting fetches, the caller's lag at send and the callee's
lag at entry. `--rows` writes one line per call.

What the trace decides, for F74's open fork (`tests/m0/results/admitted-bytes.md`, "The churn follow-up"):

- A miss with a large `out` and low lag on both sides, while `awaiting` ≤ 6, is the platform's queue or the wire.
- A large `out` only when `awaiting` > 6 is the per-invocation connection queue.
- A large caller lag at send means the leader's own loop.
- A large callee lag at entry, or a large `work`, means the callee.
- A call with `CallSent` and no `CallLeft` never reached the callee's handler, or the callee's rows were lost to a
  reset that `tsave` didn't catch.

## Checks

Before the hosted run, on this branch:

- `calls.py` on a synthetic trace: a two-clock trace with skews of +50 ms and −30 ms, one call queued 3 s before the
  callee, and one call that never reached its callee. It recovered `out` 3,040 ms with `work` 10 ms for the queued
  call, put that call in the `awaiting` [6, 12) row, and listed the other call as never reaching the callee.
- fmt, workspace clippy, and wasm32 clippy for site, edge and ops (site and edge with and without `test-faults`):
  clean. `cargo test --workspace`, `go test` in `containers/git/server`, and the three `scripts/check_*.py`: see the
  report.

No hosted run was made here. LM-1 takes the trace on its B0-33 site day from main once this merges.
