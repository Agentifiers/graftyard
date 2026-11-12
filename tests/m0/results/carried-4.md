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

Site day on `graft-m3c` at 15:45Z from `m0c4-deploy` `40d6ef8` (`origin/hot-shards` `fcc604f`, which holds `main`,
merged with `m0-carried-4` `2e4cd9f`: streamed member-call bodies and the placement change), `test-faults`,
hot-shards' image `sha256:7e0c4772…`; sites a/b/c `5181b35b`/`56885938`/`51f8c317`, test edge `graft-m3c-wp2-edge`
`d3d3fb9c`. Same load as `carried-3.md`: `tests/m1/wp2/run.ts bench`, 1 KB commands, one new `Tri` group per run
led from site 0 (enam), placed through `/place`. Raw rows in `raw-carried-4/` (`pass1-*` for this pass).

### The placement check, hosted

`m0c4-near40`: a create with `near_ms` 40, which no enam to weur pair meets. All 8 tries missed (93, 96, 108, 95, 110,
104, 99, 112 ms). The create succeeded with the 93 ms try (`nearOver: 93`), the leader's `Rtt` row read 93, `/stats`
counted `placedOverNear: 1`, and a 10 s bench on it carried 93/s at p99 393 ms. **Pass.**

### Pass 1: 16 runs

| Run | Carried /s | Near RTT | Commit p99 | Round p50 | Commands / entry | Leader confirm p99 |
| --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 280 | 103 | 716 | 157 | 54.1 | 132 |
| r300-2 | 280 | 108 | **494** | 167 | 51.1 | 54 |
| r300-3 | 279 | 99 | 738 | 157 | 50.8 | 300 |
| r300-4 | 280 | 101 | **514** | 165 | 51.4 | 92 |
| r300-5 | 280 | 97 | **385** | 145 | 43.9 | 81 |
| r300-6 | 280 | 109 | **574** | 180 | 56.3 | 100 |
| r300-7 | 280 | 103 | **588** | 180 | 57.1 | 133 |
| r300-8 | 280 | 99 | **342** | 141 | 42.7 | 64 |
| r330-1 | 307 | 100 | **481** | 155 | 52.7 | 43 |
| r330-2 | 307 | 76 | **426** | 118 | 39.4 | 87 |
| r330-3 | 307 | 89 | **469** | 150 | 51.4 | 79 |
| r330-4 | 308 | 83 | **462** | 146 | 49 | 90 |
| r330-5 | 308 | 84 | 679 | 128 | 43.6 | 141 |
| r330-6 | 307 | 100 | 614 | 178 | 61.5 | 155 |
| r330-7 | 307 | 86 | **583** | 142 | 48.4 | 103 |
| r330-8 | 307 | 100 | 692 | 168 | 61.9 | 300 |

**11 of 16 pass (6 of 8 at 300, 5 of 8 at 330); 5 between, at 614 to 738 ms; 0 failed commands.** Worse than
`carried-3.md`'s 15 of 16, on the same placements (near RTT 76 to 109 ms against 75 to 100).

**What moved is the leader's own storage confirm.** Its p99 was 27 to 79 ms in all 16 `carried-3` runs (08:22 to
08:43Z) and 43 to 300 ms here. Over 34 runs (`carried-3`'s 16, pass 1's 16, and pass 2's 2), commit p99 correlates
0.91 with the leader's confirm p99 and 0.38 with the near RTT. A slow confirm holds the round directly, because the
leader commits nothing above its own durable index (Shape, the write path): r300-3's trace (`tests/m0carried/stall.py`)
has leader confirms of 300, 328, 360 and two of 400 ms, against a p50 of 35. The AppendEntries sends are issued in the
same turn as the flush, not after its confirm, so the send is not what waits. The body stall this branch fixes is gone: a near
follower's body over 150 ms came 0 to 2 times per run, at 46 to 63 KB, except r300-1 (below).

**Not our write path.** A subagent diffed the `carried-3` deploy tree (`d85ae26` merged with `e206fb0`) against
`40d6ef8`: `family.rs`, `tier.rs`, `crates/raft`, and the repo fold are byte-identical, `SqlStore`, `persist`,
`commit_tx`, and `flush` are unchanged, and no added line in `workers` or `crates` writes storage. Per command the
leader wrote the same: about 1,090 bytes per event and 2.1 statements per command in both passes. The confirm also
barely depends on the transaction's size (correlation 0.21 between statements and confirm). The ticker never lagged
20 ms. The bare-object control's `sync()` stayed at p50 21 to 32 ms and p99 25 to 47 ms over 29,390 writing rounds
(10:20 to 11:20Z, `gateprobe-summary.json`), and p50 36 to 46 ms (max 67 to 108) at 16:50Z, while the members' confirms ran at p90 44 to 377 ms. Confirms
near 300 ms (295 to 305) came 5 times in pass 1 and never in `carried-3`. So the confirm rose on the platform between
08:43Z and 15:51Z, for these members and not the bare object [inference: the two differ in write shape, a member's
transaction carrying 80 to 400 statements against the control's one]; f33a516's stream, the one runtime change near
the write, cannot be ruled out by this data: every pass below runs it, and no same-window A/B without it was run.

**r300-1 is a follower stall, not the leader's.** 30 s in, the near follower's own confirm took 8.7 s (its others:
30 to 133 ms), it answered nothing for 8 s, the leader's calls to it missed 6 deadlines, and the leader caught it up
with entries of 0.3 to 1.5 MB (`failedCalls: 6` in the bench row). The far follower carried the quorum meanwhile at its
300 ms rounds (append p50 321 ms), which is what pushed the tail over 600 ms [inference from the trace].

### Pass 2: a regression, reverted

Pass 1's trace showed BF-14's `arrival` row, one bare `sql.exec` per `/commit` reply on test installs, as the one
write in a round outside the member's transaction. `42c846c` moved it into the member's queue. Site day 16:25Z
(`ed263cf1`.../`3a2cc321`, after the code change): r300-1 at **1,000 ms** and r330-1 at **1,608 ms**, leader confirm
p99 524 and 607 ms, entries of 73 and 100 commands. The subagent's reading of workerd's source explains it in part:
SQLite writes until the next event-loop turn share one implicit platform commit, and `transactionSync` is only a
savepoint inside it, so the bare `exec` ran in the same commit as the entry, while the queued rows were flushed by the
next input's synchronous `flush`, a separate commit mid-round. Why that made every confirm about 5 times slower is not
shown [inference]. `42c846c` is reverted (`eeb4bea`); the row stays a bare `exec`.

### Pass 3: back on pass 1's code

Site day 16:39Z from `73380fd` (pass 1's code), four runs: r300-1 **387**, r330-1 **506**, r330-2 **431**, and r300-2
at 664 ms between. In r300-2 the leader on site 0 stepped down 30 s in (`StepDown { Confirm { p99_ms: 749, slow: 6,
of: 27 } }`): its confirms ran 103 to 744 ms from the run's first second, `STEP_DOWN` moved the lead to site 1, and the
run carried 212/s. The new leader's confirm p99 was 399 ms. The other three runs' confirms were 35 to 89 ms.

## Verdict

**B0-02: between pass and fail, not passed.** 11 of 16 on pass 1, and 3 of 4 on pass 3, against the gate's 8 of 8 at
300 and 8 of 8 at 330. The leader stall `carried-3.md` traced to a stalled AppendEntries body is fixed: no run here
missed by a body stall on the near follower. Every miss here follows a slow storage confirm on a leader (or, in r300-1,
on the near follower), at write volume the same as `carried-3`, and a bare object on the same account did not see it.
The confirm is the platform's (F35's class) [inference], and on a slow-confirm afternoon the best placement on this
install does not meet 600 ms on every run: commit p99 is about round p50 plus the leader's confirm tail.

The design already answers a sustained slow confirm with `STEP_DOWN` (r300-2 above). What it does not answer is a
confirm tail of 150 to 400 ms that stays under `STEP_DOWN`'s trip: commit then waits for the leader's own durable
index on every round. Raft allows the commit to count the leader's copy only once durable, and this design must keep
that rule, because a reset leader resumes its term with no election (Shape, the write path; `advance_commit`). So no
code change in this branch can take the leader's confirm off the commit path, and the operator's call is needed:
measure again in a quieter window, accept B0-02 as between on slow-confirm days (F35), or change the commit rule
(counting a leader's copy before its confirm needs a different answer to the reset-leader case, and is a design change
under `crates/raft/src`).

## Checks and cost

Full checks on `2e4cd9f` (fmt, workspace clippy, wasm32 clippy for site, edge, and ops, with `test-faults` for site and
edge, `cargo test --workspace` 710 passed and 0 failed, `go test ./...` in `containers/git/server`, and the three
`scripts/check_*.py`): all pass. `42c846c` and its revert passed wasm32 clippy both ways. `crates/raft/src` is unchanged,
so the long sweep and Porcupine were not rerun. New test: `nearest_try_keeps_the_best_when_every_try_is_over_the_bound`.

graft-m3c usage (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py`): $5.42 for 15:40 to 17:13Z (three site days and
23 bench runs), $1.03 for 09:00 to 11:30Z, which includes other threads' use. The bare-object control is on the same
account and is in those figures.
