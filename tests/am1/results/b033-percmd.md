# b033-percmd: what the B0-33 leader's cycle is made of, the ref-lane share, and the leader's clock ($0)

LM-1, 2026-11-22. These are the three $0 measurements in FRAME, "Operator decision, 2026-11-22, B0-33 measured before a
design flip, and a $50 research budget". They read the baseline set's traces (`tests/am1/results/b033-baseline.md`:
blA, blB, blC2) and the leader and near-voter objects' Workers Logs, which are kept 7 days. Tags follow
`design/SHARED.md`, "Read this first".

**Short answer.**
- **The cycle.** On every run, a cycle splits into four parts: the wire round trip (13 to 18%), the near voter's
  handling (1 to 4%), the leader on CPU (39 to 53%) and the leader off CPU (25 to 47%).
- **CPU per command.** The leader burns 4.5 to 5.5 ms of CPU per command cut, about the same on EWR and on IAD. The
  slow hosts lose time off CPU: 247 to 345 ms a cycle against EWR's 128 [measured].
- **The ref-lane share.** Ref lanes would move 16.3% of the offered commands and 6.4 to 25.7% of the committed ones,
  because only an agent push's ref update moves [measured].
- **The leader's clock.** Under load the leader's clock reads 1.2 to 2.4 s behind at p50, and its Workers Logs
  timestamps come from the same late clock. A41's stall counts and sync-ab's confirm p90s are therefore readings of a
  late loop, not durations [measured; the reading is inference].

## 1. The cycle and the 6 ms

### Which timestamp Workers Logs records

- **On the near voter,** Workers Logs' `timestamp − wallTimeMs` falls within a few ms of the trace's `Entered`, which
  is stamped at the top of the handler: p50 −2 to −10 ms, p10 −34 to −66 ms, p90 +4 to +22 ms. So `timestamp` is the
  invocation's end and `timestamp − wallTimeMs` is its start. The start sits at or a few ms before handler entry, so it
  includes the input wait. On a voter, whose isolate is idle, that wait is a few ms [measured].
  - The voter's `wallTimeMs` is p50 97 to 116 ms against a handler of 0 to 5 ms and an idle wire round trip of 94 to
    122 ms. Its wall therefore runs past the moment the answer reached the leader, so it does not measure the response
    alone [measured]. What keeps the invocation open is not traced [unverified].
- **On the leader under load,** Workers Logs' start for a voter's `/raft` call is stamped before the voter sent it:
  start − `x-graft-sent` is p50 −913 / −1,025 / −1,190 ms on blB / blA / blC2, against +62 to +89 ms idle [measured]. So
  the leader's events are stamped on its own late isolate clock, as `tests/am1/results/lm1-rows-m3f.md` found on
  m3fgb1.
- **Consequence.** No timestamp from the leader, in the trace or in Workers Logs, gives real arrival or handler start
  under this load. The leader's queue wait cannot be read from timestamps. What can be read is its CPU
  (`cpuTimeMs`, which needs no clock) and the cycle on the near voter's clock.

### The split

Per cycle, on the near voter's clock for 90 to 300 s of load (`tests/fanout/b033probes/host/cycsplit.py`):
- the cycle runs from the near voter's handler entry for entry k to its entry for k+1;
- the leader excess is the cycle less the idle wire round trip and the voter's handling, as in `second.py`;
- the leader's CPU per cycle is Workers Logs' `cpuTimeMs`, summed over every leader invocation in the window and
  divided by the entries sent to the near voter. One isolate runs one turn at a time, so every input's CPU is in series
  with the entry path, whatever route carried it [inference from the runtime's single-threaded isolate].

[measured]

| Run (leader colo) | Entries/s | Commands an entry | Cycle mean | Wire (idle RT) | Voter handling | Leader on CPU | Leader off CPU | CPU per command cut | Leader CPU, share of wall |
|---|---|---|---|---|---|---|---|---|---|
| blB (EWR) | 1.95 | 61 | 514 | 94 (18%) | 19 (4%) | **273 (53%)** | **128 (25%)** | 4.50 ms | 53% |
| blA (IAD) | 1.36 | 62 | 738 | 97 (13%) | 8 (1%) | **288 (39%)** | **345 (47%)** | 4.65 ms | 39% |
| blC2 (IAD) | 1.43 | 58 | 695 | 122 (18%) | 11 (2%) | **315 (45%)** | **247 (36%)** | 5.45 ms | 45% |

- **The 6 ms is CPU.** On every run the leader burns 4.5 to 5.5 ms of CPU per command cut, or 273 to 315 ms per entry
  of about 60 commands. CPU alone caps one group at 1000 / 4.5 to 5.5, which is 180 to 220 commands a second, before
  any wire or wait [inference]. Fable's sabA arithmetic gave 5.1 to 6.2 ms.
- **The slow host is off CPU, not more CPU.** The IAD leaders spent the same CPU per command and per entry as EWR's,
  but 247 to 345 ms of each cycle off CPU, against 128 ms. That time is the turn waiting to run, or the leader waiting
  on its own storage, and nothing here splits the two [inference]. It is the host gap A41 asks about, now measured
  without the leader's clock.
- **Where the CPU sits.**
  - By route, `/commit` invocations carry 46 to 53% of the leader's CPU and learner `/raft` 14 to 25%. The rest is
    `/entries`, `/outcome_by_request`, `/read` and `/outcome`.
  - 80% of it is in a few invocations of 20 ms or more CPU: 2.8 to 6.1 a second, about two to three per entry. These
    are the turns that cut, decide, encode and persist an entry, and they run inside whichever input woke them
    [measured; the attribution to turns is inference].
  - What fills those turns is not split here. Fable's local profile puts the Rust path under 1 ms per command
    [unverified on the host].

**What this sizes.** If the 4.5 to 5.5 ms per command fell to 1 ms, an EWR leader's cycle would lose about 210 ms of
its 514. At the same 61 commands per entry that is about 3.3 entries/s, or 200 commands/s [inference, linear in
commands]. On the IAD placements, off-CPU time would still hold the cycle near 450 to 590 ms.

## 2. The share ref lanes would move

DECISIONS, "One group per repository, one entry in flight, and group commit", Beat: lanes "would move under a quarter
of a 500-agent fleet's commands, because pull request records stay on the base branch's group". AGENT-LAYER,
"Alternatives considered", says lanes move only the agent ref updates.

Counted from each run's own committed commands by the group they would land in (a1's per-kind results, 2xx or `ok`
only). A lane takes an agent push's ref update (`refs/heads/claude-*`). Everything else stays on the repository's group:
draft PRs, comments, task edits, stand-ins, leases and renewals, Human pushes, merges and the fast user's REST writes
[measured]:

| Run | Committed (a1) | Leader `/stats` commands | Agent pushes (lane) | Lane share of committed | Lane share of offered |
|---|---|---|---|---|---|
| blA | 20,600 | 20,852 | 1,359 | **6.6%** | 16.3% (41.67 of 256.3/s) |
| blB | 29,344 | 29,638 | 7,545 | **25.7%** | 16.3% |
| blC2 | 22,170 | 22,700 | 1,414 | **6.4%** | 16.3% |
| sabA | 30,184 | 30,352 | 5,477 | **18.1%** | 16.3% |

- **Offered,** B0-33's mix is 16.3% agent pushes, under DECISIONS' quarter.
- **Committed,** the share follows how many pushes the leader admitted. The well-placed run, blB, is the only one at a
  quarter (25.7%). On the stalled runs admission refused most pushes, so lanes would move 6 to 7% of what committed.
- **Not counted:** whether a push costs the leader more CPU than the commands that stay, since a push carries a pack
  inline (about 90 KB mean). Workers Logs gives CPU per invocation, not per command kind, so a lane's CPU share is
  unmeasured.
- **What lanes would leave.** Even moving every agent push, the base group keeps 74 to 94% of its committed commands,
  and on the stalled runs the leader was already refusing pushes. So lanes would not take a stalled leader under its
  limit [inference].

## 3. The leader's clock

Fable 5.2's second opinion (thread T-01a1f697-3c0e-7728-b532-d291447f507b). These probes are now in
`tests/fanout/b033probes/host/` and were rerun here on the committed traces [measured]:
- `drift.py`: `back` is the leader's `AnswerArrived` minus the voter's `CallLeft`, per 30 s bucket.
  - Idle it is +32 to +111 ms.
  - Under load it is −541 to −1,426 on sabA, −386 to −1,056 on blB, −1,088 to −2,196 on blA, and −679 to −3,126 on
    blC2.
  - So the leader stamps a voter's answer arriving 0.4 to 3.1 s before the voter stamped it leaving.
- `stale.py`: net of idle, the leader's clock reads behind at p10/p50/p90 by 803/1,450/2,797 ms (sabA), 559/1,169/1,812
  (blB), 1,036/1,888/3,692 (blA) and 950/2,449/3,759 (blC2). On sync-ab's arm B2, at 168 inputs/s, it reads
  −567/−18/743.

The reading [inference]:
- A busy isolate's `Date.now()` reads its last I/O (`tests/am1/results/lease-clock.md`).
- So A41's ticker stalls and the confirm p90s rest on a late clock. They measure the loop running late, not a storage
  wait or a host pause. That includes sync-ab's flip, p90 379 → 136 → 101 ms (`decisions/DECISIONS.md`, "`sync-ab`").
- The host spread itself survives on the voter's clock, at about 1.2 to 1.9× in leader excess rather than the stall
  counts' 10×. Section 1 then puts that spread in the leader's off-CPU time.
- Recorded in `decisions/OPEN-QUESTIONS.md`, A41, and in the `sync-ab` entry's Status. A41 is an edit, so the
  OPEN-QUESTIONS Total stays 234 (recounted).

## Tools and raws

- `tests/fanout/wlobj.py` (new) pulls one object's Workers Logs invocations: timestamp, wall, CPU, path and `x-graft-*`
  headers, with no account fields. On 10 s of blB's leader its CPU sum matched the exact `calculations` count to the
  millisecond.
- `tests/fanout/b033probes/host/`: Fable's 20 probes and README as delivered, plus `cycsplit.py` (new), the split in
  section 1.
- `tests/fanout/raw/wl-bl/{blA,blB,blC2}-{f0,f1}.json.xz`: the leader's (f0) and near voter's (f1) invocations over each
  load window, 78,210 / 87,119 / 82,914 on the leaders.
