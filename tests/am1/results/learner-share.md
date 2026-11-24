# Learner read-index: the share a call response could grant, and the harness clock (2026-11-18)

Code-only, $0. Item (b) of the orchestrator's review of `health-gate-lease`: before the learner read-index design is
built, measure the share of m51's learner reads that a call response could have granted, with the pre-change window
apart so the read gate's retries do not inflate it. If the share is small, stop. It is small, so the design stops here.
Also: (c) the confirm-rule target check is deferred as `decisions/OPEN-QUESTIONS.md` F92, and the harness now stamps
on Cloudflare's clock.

## (b) The share

The design (`tests/am1/results/health-gate-lease.md`, "4. The learner read-index design") would let the leader's
answer to any learner call carry its commit, read under its lease when the answer is built, and grant every read that
reached the learner before that call left. Its gain is the reads whose own read-index batch some other call could
have replaced.

**Method** (`tests/fanout/learnershare/share.py` on `raw/tsave-m51b33.jsonl.xz`, the four learners' trace rows). An
index read is a `ReadQuery` row with `ms` 1. Its batch is the first `IndexSent` to the leader at or after it. A read
counts as grantable if a learner call to the leader that is not a read-index batch and not a hedge resend (`CallSent`
at no `IndexSent` or `Hedge` instant: held-append acks and the like) left after the read arrived and no later than its
batch. A batch is avoidable only if every read in it is grantable. Reads reaching a share do not cut calls by themselves, so
the second table counts the batches a learner would send under a rule that rides. Windows are on Cloudflare's
clock: pre-change is the load before the leader change (the orb-stamped load start 11:16:37Z plus 7.4 s, to site a's
`StepDown` at 11:17:42Z, leader site 0); post-change is 11:18:30 to 11:21:30Z on site 1, the window the read gate's
retries inflated.

| m51, 4 learners | pre-change (58 s) | post-change (180 s) |
|---|---|---|
| index reads | 10,782 (186/s) | 290,058 (1,611/s) |
| read-index batches to the leader | 48.3/s | 38.4/s |
| other learner calls to the leader | 14.5/s | 16.8/s |
| reads a call that left anyway could grant | 11.9% | 13.7% |
| batches every read of which it could grant | 42 of 2,599 (1.6%) | 40 of 4,140 (1.0%) |
| reads another call reaches within 50 / 100 / 250 / 500 ms | 17.3 / 26.8 / 51.2 / 81.8% | 17.9 / 23.2 / 35.3 / 65.0% |

What a learner would send under each rule, read-index batches a second over the 4 learners: "ride" holds a read up to
D ms for another call to the leader and batches only the reads none reaches; "control" batches every D ms with no
riding (`LEARNER_READ_EVERY` raised to D). The control at 50 ms comes in under the measured rate by 3.4 a second in both windows (44.9 against 48.3, 35.0
against 38.4), so the model undercounts batches by about 7 to 9%; the ride-against-control savings below are differences
within the model and stand. Corrected 2026-11-24 (`tests/am1/results/trace-dedup.md`): the measured batch rates read 45.1 and 23.9 a second before the trace merge kept equal rows inside one save.

| batches/s | 50 ms | 100 ms | 250 ms | 500 ms |
|---|---|---|---|---|
| pre-change, control | 44.9 | 27.8 | 13.2 | 7.1 |
| pre-change, ride | 40.2 | 22.7 | 8.4 | 2.3 |
| post-change, control | 35.0 | 22.4 | 11.6 | 6.6 |
| post-change, ride | 31.3 | 18.4 | 7.8 | 3.0 |

[measured, `tests/fanout/raw/learnershare-m51b33.txt`]

**Reading.** Riding answers saves 3.6 to 5.1 batches a second over the 4 learners at every window, pre-change and
post-change alike, so the read gate's retries do not change it. At today's 50 ms cadence that is 48.3 to
about 43 a second: about 5 of the 4 learners' 64.5 `/raft` calls a second to the leader on m51, about 7%. Most of the
larger cuts at longer windows are coarser batching, which the control gets with no protocol change, at the cost of
that much added latency on every learner read (round p50 58 ms today). The other calls run at about 3.6 a second per
learner against 11 batches, and only 1.6% of today's batches have every read reached by a call that left anyway, so most
reads have no call to ride.

So the expected cut is about 7% of learner `/raft` calls, about 5 a second, for a `crates/raft` change to the append
answer's format and the read-grant path. **Stopped before any design-doc or `crates/raft` change**, as the review
asked. m51's halving of the learners already cut learner `/raft` by half and the stall share at matched load from 31 to
16% and 45 to 19% (`tests/am1/results/c-design.md`, m51). If learner calls must fall further, the measurement points at
fewer read-index rounds (a learner serving more reads from its applied state under DESIGN "Follower read"'s narrow-read
rule), at a longer learner batch window if a read latency budget allows it (the control row), or at fewer learners, not
at answers riding other calls.

## (c) The confirm-rule target check

Deferred until the learner work lowers leader load, per the review. Recorded as `decisions/OPEN-QUESTIONS.md` F92,
with the `peer_storage_slow` candidate and the m50 and m51 numbers.

## The harness clock

The orb's clock is unsynchronised (`timedatectl`: "System clock synchronized: no") and ran 7.0 to 7.9 s behind
Cloudflare's today: `node tests/fanout/srvclock.mjs` against `www.cloudflare.com` read +7,821 to +7,853 ms (±20 to
51 ms) at 14:00Z, and +7,780 ms (±103) against `graft-m2b-tools-edge`. `cdn-cgi/trace`'s `ts=` read +6,897 to
+7,809 ms in twelve 0.5 s samples.

- `tests/fanout/srvclock.mjs`: `measure()` bounds the offset from `Date` headers. Each answer's whole second D was read
  at a server time in [D, D + 1000) and an orb time in [sent, received], so the offset lies in (D − received,
  D + 1000 − sent). After the first sample, each request is timed to reach the server at a whole second by the current
  estimate, which halves the interval down to the round trip (6 samples, ±20 to 30 ms here). `serverClock()` keeps
  one offset, re-measured every 60 s, for a long-running tracer.
- `tests/fanout/terms.mjs` and `tests/nr3/tsave.mjs` stamp `at` on that clock, and log the offset as their first line.
  Every tsave reader skips lines without `rows`, and every terms reader skips lines without `members`, so the extra
  line changes no reader.
- `tests/fanout/rows.sh`, `costrow.sh` and `b034.sh` source `tests/fanout/srvclock.sh`, which measures once per driver
  (exported as `SRV_OFFSET_MS` to the drivers and tracers it starts) and stamps status lines with `stamp`. If the
  measurement fails, lines say "(orb clock)".
- The rule is in `tests/fanout/README.md`, "Clocks".

Test: `node --test tests/fanout/srvclock.test.mjs`, a simulated server ahead of or behind a fake orb clock by 130 ms to
7,463 ms, with uneven up and down legs; the answer must be within its stated error and tighter than the round trip.
Plants, each turning it red: `hi` widened by a second, `lo` taken from `sent` instead of `received`, the header's
second not floored, and the offset's sign flipped. Smoke: `terms.mjs repo:65536 6` and `tsave.mjs repo:65536`
against graft-m2b wrote `{"clock":{"offsetMs":7863,"errMs":28}}` and `{"offsetMs":7872,"errMs":29}`, with `at`
7.9 s ahead of the orb (read-only `/status` and `/trace` on a kept fixture).

## Checks

- `node --test tests/fanout/srvclock.test.mjs`: 2 of 2; 4 plants caught.
- `bash -n` on the three drivers; `node --check` on `terms.mjs` and `tsave.mjs`.
- `scripts/check_design.py`, `check_names.py`, `check_links.py`: OK.
- No Rust, site, edge or `crates/raft` change, so no cargo checks, sweep or Porcupine.

Hosted rows: none ($0). graft-m2b was B0-02's for B0-29 run 6 and is now free (B0-02's idle reading runs to about
14:18Z).
