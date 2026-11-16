# fanout-dups: B0-33 m39's inbound trace, and duplicate forced appends in one turn

Branch `fanout-dups`, built off `main` adc86c42. It reads the first hosted run of `inbound-trace`
(`tests/m0/results/inbound-trace.md`): LM-1's B0-33 on site day m39 (main a0bca583, with promote-warm,
registry-restore, push-reads, feed-ahead, receipt-rule and the test-faults inbound trace), repo:131343, a1 run
mwjghnzg, 16:06:40Z to 16:17:18Z, k 4, 8 learners, 5,000 agents. Terms 2 to 9, leader commit p50 5.8 s and p99 29.7 s.
The trace is `tests/fanout/raw/tsave-m39b33.jsonl.xz` (tsave `EVERY=10000`, `LEARNERS=1`).

```
xz -dc tests/fanout/raw/tsave-m39b33.jsonl.xz > /tmp/m39.jsonl
python3 tests/f74/inbound.py /tmp/m39.jsonl --from 2026-11-16T16:06:40Z --to 2026-11-16T16:17:18Z
python3 tests/f74/fanout.py /tmp/m39.jsonl --from 2026-11-16T16:06:40Z --to 2026-11-16T16:17:18Z
```

## 1. Client load ahead at the callee: refuted

Completeness: voters 0 and 2 are one host each, missing 1,955 of 194,231 and 938 of 153,745 arrival numbers (about
1%). Voter 1 reset once and its two hosts overlap in time (26,211 rows run backwards across the overlap). Learners
are near complete.

`inbound.py` joined 8,657 voter-to-voter `/raft` calls to the callee's `Entered` row:

| out leg | calls | requests ahead p50 / p90 | open at entry p50 / p90 | ahead, p50 share |
|---|---|---|---|---|
| under 200 ms | 1,310 | 96 / 2,182 | 30 / 678 | `/read` 0.67 |
| 200 ms to 1 s | 1,370 | 115 / 1,496 | 3 / 45 | `/read` 0.44, `/suspect` 0.18, `/raft` 0.14 |
| 1 to 3 s | 2,693 | 760 / 2,170 | 3 / 49 | `/read` 0.57, `/suspect` 0.27 |
| 3 s or more | 3,284 | 2,030 / 4,726 | 3 / 55 | `/read` 0.58, `/suspect` 0.29 |

A delayed call entered a callee with 3 handlers open (p50), and arrivals during its leg ran 333 to 374 a second, the
callee's ordinary rate. So the requests "ahead" are the arrival rate times the leg: the callee was not backed up, and
prompt calls entered with more open (30 at p50) than delayed ones [measured].

## 2. The legs are held on the caller, in its fan-out

`fanout.py`, legs = total − work (each on one clock, so the pair's skew cancels), by the call's place among the
calls its caller stamped in one turn:

| place in the turn | calls | legs p50 | legs p90 |
|---|---|---|---|
| turns of six or fewer | 3,093 | 842 ms | 4,357 ms |
| calls 1–6 of a larger turn | 1,745 | 1,000 ms | 4,097 ms |
| calls 7–12 | 1,348 | 1,434 ms | 5,190 ms |
| calls 13+ | 2,305 | 2,693 ms | 7,948 ms |

- Calls past a turn's sixth answered, less the callee's work, more than 300 ms before any of the turn's first six:
  46 of 3,418. That fits a Workers invocation's six fetches waiting for headers, the rest queued
  [doc, https://developers.cloudflare.com/workers/platform/limits/#simultaneous-open-connections].
- Delayed calls (out ≥ 1 s) were sent with 93 of the caller's fetches awaiting (p50, p90 256), against 22 for prompt
  ones. 2,934 of 6,254 delayed calls were overtaken by a later-sent call from the same caller to the same callee, so
  there is no one first-in, first-out queue per caller: the pools are per invocation [inference].

## 3. What fills the fan-out: duplicate forced appends in one turn

Turns with 10 or more calls to one voter carried 3,834 of site 0's 7,921 voter-to-voter calls, 595 of site 1's 1,005
and 401 of site 2's 1,515. In those turns, 1,146 of site 0's 1,245 `AppendQueued` rows (12) repeated a last index
already queued to that voter in the same turn; 164 of 174 on site 1; 101 of 106 on site 2 [measured].

So a leader still sends tens of rounds to one voter in one turn after `read-round-gap`. Which send path makes them,
and why the gap does not hold them, is not in the trace: `AppendQueued` carries no reason and the raw clock, not the
member clock. The candidate is that `READ_ROUND_GAP` is measured on the member clock, which lease-clock floors at
each newer peer stamp, so a floor jump of 20 ms or more inside a turn frees the slot [inference]; an answered round
frees it too, and heartbeats, retries and catch-up can repeat a queued last index as well. The simulator check below
names the source.

## Records

- `decisions/OPEN-QUESTIONS.md`, F74: client load ahead at the callee refuted, and the caller's six-slot fan-out per
  invocation holds the legs [measured]; the clock-floor cause of the duplicates [inference].
