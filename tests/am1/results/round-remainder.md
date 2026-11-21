# round-remainder: where B0-33 m64r33's loaded round went outside the followers ($0, 2026-11-21)

LM-1, analysis only, nothing built. It follows `tests/am1/results/leader-cycle.md` ("What the round is made of under
load"), which left the loaded round's excess outside the followers' handlers unattributed. The lead asked for two
checks: `tests/fanout/applegs.py` on that remainder, and `tests/fanout/docpusec.py` (B0-02, main `3bd655dd`) on
m64r33's leaders, to see whether the leader runs near 1 vCPU, as B0-32's leader did in its burst
(`tests/am1/results/lm1-rows-m3f.md`, "Loss 1's CPU check").

Row: B0-33 m64r33 on `graft-m2b` (`tests/am1/results/b033-m64.md`), repo 131378.
- Site 0 (`repo:131378#1#f0`) led 00:12:00 to 00:14:10Z.
- Site 1 (`repo:131378#2#f1`) led after the 00:14:13 transfer; the window used is 00:14:15 to 00:16:46Z.
- Idle control: site 0 → 1, 00:09:00 to 00:11:40Z.
- `tests/fanout/isolates.py` finds no shared isolate (7 members, 8 isolates). So the learners' CPU on site a (`#3#l0`,
  `#3#l3`, 106 CPU-s each over 00:11 to 00:17) is not the leader's.

## 1. Leader CPU: busy, but not pinned at 1 vCPU

`docpusec.py`, CPU-seconds per second (upper bound, scaled to the minute's periodic `cpuTime`) [measured, estimate]:

| Leader, window | Requests/s p50 | CPU-s/s p50 / p90 / max | Mean | Seconds ≥ 0.8 | Best 5 s |
|---|---|---|---|---|---|
| site 0, 00:12:00–00:14:10 (130 s) | 234 | 0.38 / 0.63 / 1.38 | 0.40 | 5 | 0.73 (00:13:30) |
| site 1, 00:14:15–00:16:46 (151 s) | 316 | 0.51 / 0.83 / 1.02 | 0.54 | 23 | 0.83 (00:14:42) |

Per-minute (`docpumin.py`): site 0 had 22.2 and 24.8 CPU-s in 00:12 and 00:13, so 0.37 and 0.41 vCPU. Site 1 had 31.6 and
37.0 CPU-s in 00:14 and 00:15, so 0.53 and 0.62 vCPU. Invocation CPU p99.9 on the leader was 119 to 168 ms (site 0) and
135 to 250 ms (site 1), against 27 to 31 ms idle (`durableObjectsInvocationsAdaptiveGroups`).

- **Not B0-32's shape in the mean.** B0-32's burst seconds ran at 0.79 to 1.05. Here the leader averaged 0.4 to 0.54
  and was at or over 0.8 in 4% (site 0) and 15% (site 1) of its seconds.
- **Long turns are there.** The p99.9 invocation took 4 to 8 times its idle CPU.
- **Site 0's ticker stalls are not CPU-bound seconds.** Its 100 ms ticker logged 55 stalls of 20 ms or more in 130 s,
  summing 32.3 s: 39 over 200 ms, max 3,301 ms at 00:13:34. Site 1 logged one of 104 ms. Over site 0's 24 stalls of
  500 ms or more (25.7 s), its CPU in those seconds summed 29.3 CPU-s, mostly 0.3 to 0.6 a second. A loop held 1 to 2 s
  at 0.4 CPU-s per second was waiting on something, not computing. Possibilities are a synchronous storage call, which
  SQLite DO calls are, or a turn analytics books to a later second. This trace cannot tell which [unverified].

## 2. The append's time outside the follower: not bytes

`tests/fanout/legsplit.py` (new) joins as `applegs.py join` does. Per call, **outside** = the leader's round
(`CallSent` to `AnswerArrived`, its isolate clock) − the follower's invocation (Cloudflare's start to `CallLeft`).
That is wire both ways plus the leader's late send and its late read of the answer. Each difference is on one clock,
so no skew estimate enters it. Workers Logs extracts are in `tests/fanout/raw/applegs-m64/` (no account fields).

| Leader → follower | Calls | rtt p50 | Outside p50 / p90 | Follower qin p50 | Follower handler p50 |
|---|---|---|---|---|---|
| 0 → 1, idle | 181 | 94 | **94** / 106 | 0 | 0 |
| 0 → 1, loaded | 265 | 233 | **198** / 847 | 4 | 17 |
| 0 → 2, loaded | 270 | 329 | **261** / 947 | 3 | 27 |
| 1 → 0, loaded | 300 | 339 | **276** / 523 | 31 | 10 |
| 1 → 2, loaded | 344 | 402 | **320** / 534 | 15 | 42 |

Outside p50 by the AppendEntries body the call carried (the leader's last `AppendQueued` to that follower within 300 ms):

| Leader → follower | Empty | 1 to 40 KB | 40 to 64 KiB | 64 KiB to 150 KB | Over 150 KB |
|---|---|---|---|---|---|
| 0 → 1 | 129 (9) | 224 (97) | 147 (18) | 205 (48) | 190 (93) |
| 0 → 2 | 593 (7) | 273 (95) | 233 (19) | 278 (52) | 246 (97) |
| 1 → 0 | 1,204 (3) | 267 (122) | 256 (35) | 227 (30) | 312 (110) |
| 1 → 2 | 1,171 (3) | 327 (142) | 281 (37) | 292 (37) | 335 (125) |

- **The remainder is not the wire's bytes** [measured]. In every pair the outside p50 for bodies over 150 KB is
  within 45 ms of the 1 to 40 KB band, and lower in two pairs. The 120 ms that `leader-cycle.md` saw between small and
  large full entries is in the first-ack round. Here it is in neither the wire nor the follower.
- **The idle leg is the wire.** 94 ms, with the follower's invocation 0 ms of it.
- So the loaded excess, 100 to 230 ms per call at p50, is on the leader's side. This is also the evidence behind
  parking `raw-inline` (`decisions/DECISIONS.md`): raw bytes would cut nothing here.

## 3. What the remainder follows

| Leader → follower | Outside p50, by the leader's CPU-s in the send's second: < 0.3 / 0.3–0.6 / ≥ 0.6 | By its clock staleness at send: < 200 / 200–1,000 / ≥ 1,000 ms |
|---|---|---|
| 0 → 1 | 144 (73) / 211 (162) / **580** (30) | 118 (24) / 192 (154) / **531** (87) |
| 0 → 2 | 238 (82) / 278 (160) / **817** (28) | 212 (24) / 244 (159) / **649** (87) |
| 1 → 0 | 339 (41) / 244 (145) / 285 (114) | 171 (5) / 244 (166) / 340 (129) |
| 1 → 2 | 346 (41) / 324 (162) / 298 (141) | 198 (5) / 300 (183) / 363 (156) |

Staleness is the leader's member clock (raised by peer stamps) minus its isolate's own `Date.now()` at send:
p50 626 to 634 ms on site 0 and 944 to 966 ms on site 1, against 0 idle. The leader's clock ran behind its callers on
both leaders, as on B0-32.

- **Site 0: the remainder is the leader's lateness** [measured]. In seconds where its CPU was under 0.3 and its clock
  close to its callers', outside was 118 to 238 ms, near the idle wire plus a little. Calls in its ≥ 0.6 CPU-s seconds
  or with a clock ≥ 1 s stale took 531 to 817 ms. These are the same seconds as its ticker stalls.
- **Site 1: the remainder follows its input queue, not its CPU** [measured]. Outside does not rise with CPU. It rises
  with requests open in the leader's handler when the call left: under 100 open, 176 ms on 1 → 0 (37 calls); 100 or
  more, 287 to 289 ms (263 calls). On 1 → 2 it was 313 to 335 ms at every depth. Site 1 took 320 requests/s as
  leader: `/commit` 144, `/outcome_by_request` 105, `/read` 64. The p50 open at entry was 264.
- **Inference:** on both leaders the round's excess is time the leader's isolate takes to issue a send and to take its
  answer while it is servicing about 250 to 320 edge requests a second. On site 0 this showed as stalled turns, and on
  site 1 as a deep input queue with a clock about 1 s behind. That is B0-02's B0-32 shape, "calls queuing on the
  leader's input side", without the leader being pinned at 1 vCPU in the mean.

## Answer to the lead

- **Is the leader near 1 vCPU?** Partly. Site 0 had a mean of 0.40, with 5 of 130 s at or over 0.8. Site 1 had a mean
  of 0.54, with 23 of 151 s at or over 0.8. Both had long turns (invocation CPU p99.9 120 to 250 ms) and a clock 0.6 to
  1 s behind their callers.
- **Is the round remainder leader-side?** Yes [measured]. It is neither bytes on the wire (flat across body sizes) nor
  the follower (qin p50 3 to 31 ms, handler 10 to 42 ms). It grows with the leader's lateness (site 0) and with its
  input depth (site 1).
- **Is it CPU alone?** Not shown. Site 0's 1 to 3 s stalls sat in seconds of 0.3 to 0.6 CPU-s. A local profile of the
  commit path can say what fills a turn. The hosted analytics cannot: Workers Logs bills spawned work to whichever event
  is current.
- **One fix for B0-29, B0-32 and B0-33?** Plausible, by the shape [inference]. All three rows' leaders took the edge's
  `/commit`, `/outcome_by_request` and `/read` traffic directly while running the group. Coordinated with B0-02
  (T-01a1ccdd) rather than building anything here.

## Files

- `tests/fanout/legsplit.py`: the outside-the-follower split by body, CPU and staleness.
- `tests/fanout/raw/applegs-m64/`: the five Workers Logs extracts (`applegs.py fetch`) and `docpusec.py`'s output for
  `#1#f0`, `#2#f1`, `#1#f2`, `#3#l0` and `#3#l3`.
- Trace: `tests/fanout/raw/tsave-m64r33.jsonl.xz` (already on main).
