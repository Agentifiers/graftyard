# B0-02's leader stall (2026-11-12)

The operator's decision on `carried-3.md`'s last miss: don't accept the stall as F35 and don't lower `NEAR_RTT`; find
what held the leader's AppendEntries send, fix it if it is ours, and show it with a bare-object control if it is the
platform's. Gate: B0-02 passes 8 of 8 at 300 and 8 of 8 at 330 offered. Branch `m0-carried-4`, from `main` with
`m0-carried-3` merged.

## What r330-6 showed

From `carried-3.md`'s `/stats` (`raw-carried-3/m0c3-r330-6.stats.json.gz`), aligned on the leader's commit of 110:

- The leader committed 113 at +510 ms, and that input's storage confirm took 248 ms (its other 374: p99 79 ms).
- The near follower's `/raft` calls came every 150 to 160 ms until +311 ms. The next came at +648 ms with 24 KB, and
  the one after at +812 ms with 142,835 bytes whose body took **347 ms** to arrive (2.4 ms/KB, against about
  0.5 ms/KB in the run's other calls). Three slow rounds followed.
- The far follower's bodies stayed at their usual 0.4 to 0.6 ms/KB in the same window, except one of 288 ms.

So the window has two parts: a send that left late after a slow leader confirm, and a large body that arrived slowly.
Both happen while the leader has a SQLite write unconfirmed. The leader's AppendEntries leave before its write in the
same input (`host.rs`, `step`: `send` issues the fetch in a microtask, the transaction runs one macrotask later), so
the send itself is not behind its own write. What was behind the later one is the subject of the control below.

## The bare-object control

`tests/m0carried/gateprobe/probe.ts`, deployed as Worker `graft-m4-gateprobe` on the same account: two Durable
Objects with no Raft code, `Leader` hinted enam and `Peer` hinted weur. Each round the leader sends a body of N bytes
to the peer and, depending on the mode, writes a 50 KB row in one `transactionSync`. The peer records when the request
reached its handler and how long `req.arrayBuffer()` took. Modes are interleaved round by round on one pair of
objects, so placement and the hour are shared. Bodies are runs of `x` (`fill=x`), as B0-02's values are.

- `nogate`: send, then write one macrotask later. The member's order.
- `nowrite`: send only.
- `gate`: write, then send.
- `stream`: the body through a `FixedLengthStream`, the write after the writer's `close()` resolved.
- `streamnowait`: the body through a `FixedLengthStream`, the write one macrotask after the send, as `nogate`.
- `nogatekv`: as `nogate`, the write through KV `put` with `allowUnconfirmed`.
- `late`, `latekv`, `resp`: a send issued `lateMs` after an earlier write, and a response sent while a write is
  unconfirmed.

### Small bodies: the output gate

| Finding | Evidence |
| --- | --- |
| A send issued after a SQL write waits for its confirm | `gate` call time = `nowrite` + about 30 ms, the write's `sync()` |
| A send issued before the write is not held | `nogate` = `nowrite` at 50 KB: p50 131 ms each, p99 151 / 149 ms over 10,102 and 5,033 rounds (`gplong-*`) |
| A send 5 ms after an unconfirmed SQL write is held for what is left of the confirm | `late` |
| A KV `put` with `allowUnconfirmed` holds nothing | `latekv` |
| A response leaving while a write is unconfirmed is not delayed | `resp` |

So the member's order is right for the gate: its AppendEntries leave before the input's write. workerd's source
agrees: `fetchImpl` waits for the output locks once, before the request starts, and the body is pumped by a
`waitUntil` task with no gate check (workerd `22a428de`, `src/workerd/api/http.c++` `fetchImpl` and
`fetchImplNoOutputLockAttempt`; `OutputGate::wait` "will not be affected by any future call to `lockWhile()`").

### Large bodies: the stall

With bodies of 150 to 400 KB, the size of the entries B0-02 sends at 330 commands/s when a round runs long, a write
that starts while the body is still in flight stalls the body in a small share of calls. Body arrival over 150 ms,
and calls over 400 ms, by mode (all `gpbig*`, `gpstream-*`, `gpsnw-*` runs, 11:00 to 11:00Z, every 200 ms):

| Body | Mode | Rounds | Body over 150 ms | Call over 400 ms |
| --- | --- | --- | --- | --- |
| 150 KB | `nogate` | 2,210 | 1 | 6 |
| 150 KB | `nowrite` | 2,196 | 0 | 0 |
| 150 KB | `stream` | 1,944 | 0 | 0 |
| 250 KB | `nogate` | 1,795 | 25 | 6 |
| 250 KB | `nowrite` | 1,775 | 3 | 5 |
| 250 KB | `stream` | 1,784 | 3 | 3 |
| 250 KB | `streamnowait` | 1,792 | 0 | 1 |
| 400 KB | `nogate` | 3,987 | 53 | 36 |
| 400 KB | `nowrite` | 3,952 | 9 | 9 |
| 400 KB | `stream` | 3,707 | 3 | 2 |
| 400 KB | `streamnowait` | 1,777 | 5 | 5 |
| 150 and 400 KB | `nogatekv` | 297 | 0 | 0 |
| 150 and 400 KB | `gate` | 508 | 0 | 2 |

A second set at 11:12 to 11:20Z (`gpmid-*`) found no effect at smaller bodies: at 60 KB no mode stalled a body
in 2,392 to 2,399 rounds each, and at 150 KB `nogate`, `nowrite` and `streamnowait` stalled 1, 0 and 1 of about 2,390.

At 250 to 400 KB, `nogate` stalled the body in 78 of 5,782 calls (1.3%), `nowrite` in 12 of 5,727 (0.2%), and the
streamed body in 11 of 9,060 (0.12%), the same as no write. The stalled bodies took 150 to 1,079 ms, the same
2 to 3 ms per KB that r330-6's 139 KB body took. The writes themselves were fast: every stalled `nogate` round's
`sync()` was 16 to 62 ms, so the stall is not a slow confirm. Nothing in the isolate was busy: the leader's 20 ms
ticker never lagged.

**Cause: the platform's, below the API [inference].** The output gate does not hold the body (workerd source above),
yet an array body in flight when a SQLite write starts is the one that stalls, and a body handed over through a
stream, whose pump the runtime drives from the stream's writer, does not. The control has no code of ours in it, so
the stall is not in the member. What in the runtime or the transport causes it is not visible from the API.

## The design change

A member-to-member call now sends its encoded envelope through a `FixedLengthStream` of its length
(`workers/site/src/host.rs`, `call_peer`): the call writes one chunk to the stream's writer and closes it, and the
fetch takes the stream's readable side. The wire bytes and the callee are unchanged. `design/DESIGN.md` (Scale,
"Commit on `Tri`") and `decisions/DECISIONS.md` ("`m0-carried-4`: a member call streams its request body") carry it.
`crates/raft/src` is unchanged, so neither the long sweep nor Porcupine was rerun.

Rejected: moving the log and state rows to KV with `allowUnconfirmed` plus an explicit `sync()`, which also stalled
nothing in the control but rewrites the storage layer; holding the write until the body has left, for which the
platform gives no signal (the fetch resolves on the response, and the pump is private).

The leader trace that found this stays in the site Worker, test installs only: `Trace` kinds `AppendQueued` (12),
`AppendIssued` (13), `AppendEnded` (14), `Stepped` (15), and `Flushed` (16), appended, and the ticker logs lag from
20 ms. `tests/m0carried/stall.py RUN.stats.json` prints the leader's trace and the follower's `/raft` arrivals around
each slow call to the near follower.

## The near bound no longer fails a create

The operator's addition to this branch. On `graft-m2b`, LM-1's create of `repo:131277` (home enam) ran all 8
`PLACE_TRIES` against its weur follower at 114, 110, 108, 110, 110, 110, 111 and 111 ms, read 123 ms on the final
check, and failed; a create a minute later passed. That install's enam to weur pair sits over 100 ms, so the same
code would refuse creates in production.

The bound is now a goal, not a gate (`design/DESIGN.md`, "Placement is checked by latency"; the amendment to
`decisions/DECISIONS.md`, "A group's home must have a voter within `NEAR_RTT`"):

- `/place` still re-places the nearer follower up to `PLACE_TRIES` times and takes the first try within the bound.
- A try refused only by the bound is kept as a candidate. When none passed, the leader puts in the one with the
  lowest near round trip (`graftyard_types::nearest_try`), if it is nearer than the member it started with;
  otherwise it keeps that member. The other candidates are cleared, as refused tries were.
- Either way the create succeeds. The answer carries `nearOver` (the kept try's ms) or `kept` with every try's round
  trips, the kept round trip is the leader's `Rtt` row, and `/stats` counts the group in `placedOverNear`.
- The daily check takes the same path, so it keeps trying and moves a member only to a nearer one.
- Not built: a bound relative to the install's measured baseline for the pair. With the nearest kept it would only
  save tries on an install like `graft-m2b`, where a create now spends all 8 (about 30 to 40 s), and no component
  keeps a per-install pair baseline yet. Recorded in the DECISIONS amendment.

Test: `crates/types/src/placement.rs`, `nearest_try_keeps_the_best_when_every_try_is_over_the_bound` (LM-1's eight
tries keep the 108 ms one, a tie keeps the current member, a tie between tries keeps the earlier). For the hosted
check, test installs take `{"near_ms": MS}` on `/place` (one bound for every region), and the test edge's create and
`run.ts --near-ms` pass it.

## B0-02 on the change

Pending the `graft-m3c` site day.
