# LM-3 on graft-m4a: "a bad controller at 1% swaps back"

`build/BUILD-PLAN.md`, AM3-WP1: LM-3's clause "a bad controller at 1% swaps back". The plan was approved by the lead
(2026-11-25) with these lines:
- **Pass:** with `GATE` unset and the agents deploy token invalid, a test-bad c2 at 1% breaches `GATE_ERRORS` on
  `graft-agents-front`'s counts, and the agents `graft-ops` commits the pre-signed `ControllerPointer` to c1 itself.
- **Fail:** no breach, or a webhook reaching c2 after the pin.
  The lead's ruling after r2 (2026-11-25): a webhook counts against this line only if it reached c2 from an isolate that
  had already pinned c1, or more than `POINTER_TTL` (5 s) after the pin committed; the propagation inside `POINTER_TTL`
  that the pass line allows does not. By that line r2 passes. A57 does not gate the clause.
- **Also:** the two refusal probes once hosted, and the flush peak per 30 s.

Stops: $3 for the row, and graft-m4a's UTC day at most $35. **Stand-in:** the runner account stands in as the agents
account (`FRAME.md`, "Operator decision, 2026-11-24, AM1 closed and the runner account stands in as the agents
account"); every resource there was named `graft-agents-m4a-`.

Run r1: the edge is LM-1's BF-69 edge day `d3d76118`, built test-faults from main `281683f6`, which carries `lm3-gate`
(`709a1d2a`) and `lm3-peak` (`58cb488d`). The agents scripts were built from `lm3-row` `3d5ada33` (main `271fedba`
plus the driver). Driver `ROW=lm3 bash tests/am3/hosted/lf7-day.sh r1 <step>`, logs in `tests/am3/raw/lm3-r1/`.
Start state and sweep 06:06:07Z; **done line 06:47:26Z** (the end sweep).

## Verdict

**The gate and the pointer pass; the row fails on its fail line.** Two webhooks reached c2 after the pointer, from
an isolate that held a stale pin from before the roll. The gate worked as designed. What failed is the front's
pinning: a pin left over from an older `ControllerPointer` sends traffic to the split while the view is behind
(OPEN-QUESTIONS A56).

| Line | Result |
| --- | --- |
| The gate breaches on the front's counts, `GATE` unset | `GATE_ERRORS: N+1 16 5xx of 16 answers (0 thrown), N 0 of 3097` at 06:31:26.840Z, 1 min 49 s after the roll committed; the agents `graft-ops` read it from `GET release/errors` summed over 3 of 3 sites [measured, the agents `graft-ops`' log] |
| The pointer commits with the API blocked | `ControllerPointer` to c1 committed at entry 529, 06:31:26.933Z, 93 ms after the breach line, signed by the agents executor; the deploy token had been invalid since 06:30:50Z (probe 400). No hand-posted pointer [measured, `final-view.json`, `block.log`] |
| No webhook reaches c2 after the pin | **Fail: 2 of 50,890.** At 22.98 s and 23.13 s after the pointer, two webhooks answered 500 from c2 `ceaad4a1`. Both carried the pin `d8b368a1`, LF-7 r2's c1 of 2026-11-24, which no longer exists on the script. Of the other 50,888 after the pointer, 50,887 answered 202 from c1 `6abab3e9` and 1 was a client-side error with no answer [measured, `hooks.jsonl.gz`] |
| Pinned to N within `POINTER_TTL` | the first webhook pinned to c1 `6abab3e9` came 22 ms after the 201. Isolates on the stale pin kept sending until 746 s after it: 402 webhooks in bursts, each alarmed (`pinned to d8b368a1 answered from …`) [measured, `hooks.jsonl.gz`, front logs] |
| Refusal: a version no upload committed | `POST release/errors` 422, "version 00000000-…-00000000005a is no graft-controller version of a release this install's Release group uploaded"; the read after it equal to the one before [measured, `probe.json`] |
| Refusal: over `FLUSH_CALLS_MAX` | `POST release/errors` 422, "a flush counts at most 10000 calls, this one 10001", naming N+1 `ceaad4a1`; the read after it equal to the one before [measured, `probe.json`] |
| The return | after `unblock` (06:34:44Z) the agents `graft-ops` deployed c1 100 / c2 0, cleared `CONTROLLER_DEFAULT` (front `74fc621b`), and committed `IntentStep` and `IntentDone` at 06:35:29.98Z; count 532, no intent current, `controller_pin` c1, `rolled.Controller` c1 [measured, `final-view.json`] |
| Flush peak | not read hosted. `peakFlushCalls` is per isolate, and the status request after the row landed on isolates that had sent none, as did the three reads after the roll: 0. Upper bound from the sampler: at most 2,658 webhooks in any 30 s across all isolates, so no flush could exceed that. That is under half the cap (5,000), and the refusal probe was the only `TooMany` [measured, `hooks.jsonl.gz`]. The edge's 3,114 stored answers over 2 min 49 s agree [measured, `errors.log`] |

**Clock correction (added after run r2, 2026-11-25).** The sampler's times are the orb's clock, which read 10.2 s
behind Cloudflare's when r2 measured it (08:47Z, from the API's `Date` ticks). r1's lines above compare the sampler's
clock with Cloudflare's commit time uncorrected. On Cloudflare's clock, the two stale-pin webhooks came 33.2 s and
33.3 s after the pointer, not 23 s; the first webhook pinned to c1 came 107 ms after it; and one more webhook reached c2,
unpinned, 3.18 s after the pointer, inside `POINTER_TTL` (5 s), from an isolate that had not refreshed yet. After the
pointer, c2 answered 3 of 51,434. The verdict and A56's cause are unchanged [measured, `hooks.jsonl.gz`, corrected].

## Why two webhooks reached c2: the stale pin

- **What the isolate held.** `d8b368a1` is the `ControllerPointer` pin that LF-7 r2 committed at entry 486 on
  2026-11-24. The group cleared it at entry 520's roll (an intent that rolls a pinned script clears its pin,
  `crates/release/src/group.rs`), and this row's pointer set it to `6abab3e9` at 529.
- **How it was still held.** An isolate that starts cold reads the whole history: 6 pages of 100 entries here. When
  a refresh runs out of time, the front keeps the pages it verified (`Reader::take`). Replaying the first 5 pages
  gives `controller_pin` `d8b368a1` at count 500 [measured, relview on the row's pages]. A cold isolate that read 5 of
  6 pages within `ABANDON_MS` (4 s) therefore pinned an entry it would have cleared on the sixth page.
- **Why that reached c2.** A pin overrides the split with a version override. `d8b368a1` is no longer a version of
  `graft-agents-m4a-controller`, which is a new script with 4 versions (c0, c0's first upload, c1 and c2). The
  override names a version the script lacks, so the request falls through to the deployment's split, and 1% of that
  is c2. Before the roll, the same isolates' 196 stale-pin webhooks all landed on c1 at 100%. After the pointer, the
  split stayed 1/99 until `unblock` (06:35:25Z), and 2 of the stale-pin webhooks in that window landed on c2 [inference from the counts; Cloudflare does not
  document what an override to a missing version does; the counts here show it falls through to the split]. After c1 went to 100% at 06:35:25Z, the 317 later stale-pin webhooks all answered from c1.
- **Not this row's change.** The gate, the counts and the route behaved as designed. The cold-read pin predates
  them. LF-7 r2 had an older pointer in history too (r1's, entry 475), but the roll that cleared it (481) was on the
  same page, so no partial read could stop between them. Here the old pin (486) is on page 5 and the roll that
  cleared it (524) on page 6.

## Setup

- **No edge day:** LM-1's `d3d76118` serves the route (`GET` and `POST release/errors` answered 401 without a JWT
  before the row). R57's diff was LM-1's: FORMATS_ENABLED +21.6 and GRAFTYARD_CACHE_LIMIT added, nothing else.
- **`agents init`** drew a new executor, `643a76ef…`; re-registering Apps 3801088 and 3866624 committed all 8
  commands; `Keys` generation 7 at entry 521.
- **Scripts:** controller c0 `cbd39b5d`, front `ead53e3b`, agents `graft-ops` `2c256bf5` (`GATE` unset). Its cron
  fired at 06:22:24Z with no reset, 8 min 27 s after the deploy (R52's wait).
- **Releases:** c1 `fcd70122…` and c2 `416db57e…` (`--features test-bad`) at 522 and 523. N is c1: rolled (524) with
  the token blocked, aborted to itself (525), then c1 `6abab3e9` at 100% and `IntentDone` (527).
- **The row:** sampler from 06:25:58Z (2 clients, 69,635 signed `issues` deliveries to 06:45:12Z). `release roll c2`
  committed at 528, 06:29:37.8Z. The agents `graft-ops` set `CONTROLLER_DEFAULT` to c1 (front `3034b4c5`), then
  deployed c2 1 / c1 99 at 06:30:29Z. Block at 06:30:50Z; c2 answered 28 webhooks in all, 26 of them before the
  pointer.
- **A hung read:** the `roll` step's first API read of the controller's deployment hung for 2.5 min with no answer;
  killing that curl let the step go on. Nothing was posted twice.

## Teardown, state left, cost

- **The runner account:** the agents cron is off; `graft-agents-m4a-ops`, `-front`, `-controller` and `-probe`, the
  five store secrets, the deploy token, and the bucket with its 12 objects are deleted, and the row's 0600 files are
  shredded. Only `graft-runners` is left.
- **graft-m4a:** edge `d3d76118` and `graft-ops` `ac4cd9a5` (schedules `[]`) as LM-1 left them; no site day. The
  `Release` group is at count 532, no intent current, `Keys` generation 7, whose `agents_executor` seed is deleted
  (the next `ControllerRoll` needs a new `Keys`). The fault plan is clear.
- **End sweep:** no shard container running.
- **Cost:** the row $0.06 on graft-m4a (06:00Z to 06:47Z) and $0.00 on the runner account; graft-m4a's UTC day is
  $1.58 [measured, `acctcost.py`].

## Run r2: the gate and the pointer pass; 2 webhooks reach c2 inside `POINTER_TTL`

Run r2 from main `09494b06`, which carries A56's fix (`ace1aa13`, DECISIONS "`controller-default`", "Caught up
first"). Same driver (`ROW=lm3 ... r2`), logs in `tests/am3/raw/lm3-r2/`. The edge is still `d3d76118` (it answered 401
to `release/errors` without a JWT before the row); the sites are LM-1's site day 3 (`eb5c77f2`, `c6b12562`, `3a1ea21b`).
Start state and sweep 08:12Z; **done line 08:44:50Z** (the end sweep). Times are Cloudflare's; the sampler's are shifted
by the orb clock's measured 10.2 s.

**Verdict: the stale pin is gone, and the gate, the pointer, the refusals and the return pass.** By the fail line as
written, "a webhook reaching c2 after the pin", r2 still fails: 2 of 15,923 webhooks after the pointer reached c2, at
1.13 s and 2.40 s. Both were unpinned answers from isolates whose view was under `POINTER_TTL` old and not yet
refreshed. That is the propagation the design allows ("pins N within `POINTER_TTL` of its next request"), not a view
that isn't the group's state. Whether the line counts the `POINTER_TTL` window is the lead's call.

| Line | Result |
| --- | --- |
| The gate breaches on the front's counts, `GATE` unset | `GATE_ERRORS: N+1 170 5xx of 170 answers (0 thrown), N 0 of 18692` at 08:38:44.796Z, 6 min 32 s after the roll committed (r1: 1 min 49 s); see "The gate's late read" [measured, the agents `graft-ops`' log] |
| The pointer commits with the API blocked | `ControllerPointer` to c1 at entry 540, 08:38:44.860Z, 64 ms after the breach line, signed by the agents executor; the deploy token had been invalid since 08:33:21Z (probe 400) [measured, `final-view.json`] |
| No webhook reaches c2 after the pin | **2 of 15,923**, at 1.13 s and 2.40 s after the pointer, both unpinned, inside `POINTER_TTL`. 0 webhooks carried a pin other than c1's, and 0 reached c2 later than 2.40 s [measured, `hooks.jsonl.gz`] |
| Pinned to N within `POINTER_TTL` | the first webhook pinned to c1 `5bc20f05` came 0.38 s after the commit; the last unpinned one came 3.96 s after it. From then until `unblock`, every webhook was pinned to c1 [measured, `hooks.jsonl.gz`] |
| No stale pin (A56) | 0 webhooks carried any pin but c1's, and the front logged no pin-mismatch alarm (r1: 631) [measured] |
| Refusal: a version no upload committed | 422, the read after it equal to the one before [measured, `probe.json`] |
| Refusal: over `FLUSH_CALLS_MAX` | 422, "a flush counts at most 10000 calls, this one 10001", naming N+1 `e737db38`; the read after it equal [measured, `probe.json`] |
| The return | after `unblock` (08:41:37Z) the agents `graft-ops` deployed c1 100 / c2 0, cleared `CONTROLLER_DEFAULT` (front `0a355d41`), and committed `IntentStep` and `IntentDone` (541, 542) at 08:41:55.99Z; count 543, no intent current, `controller_pin` c1 [measured, `final-view.json`]. The 863 unpinned webhooks from 08:41:51Z are the front after the return, which pins nothing once no roll is current and the default is gone |
| Flush peak | again not read from the front: every status read landed on an isolate with `peakFlushCalls` 0. Upper bound from the sampler: at most 2,552 webhooks in any 30 s across all isolates, under half the cap [measured] |

**The gate's late read.** From the roll to the breach, five runs of the agents `graft-ops` logged no `gate
controller` line: one `gate pending` before the block and four `no deployment of Controller read` after it. Its gate
returns `Pending` without a log line when either `GET release/errors` read misses `VIEW_CALL_MS` (1 s). The edge
answers that read by asking the three sites' `errors` objects one after another, each within `VIEW_CALL_MS`. The probe
Worker in the same account timed the same read at 0.79, 1.22, 0.97, 1.14 and 1.55 s [measured], so most runs' reads
missed the deadline and the gate waited [inference: the gate's miss is not logged]. The breach still committed with
the API blocked, but 6.5 minutes after c2 started failing instead of about 2. OPEN-QUESTIONS A57.

**Setup:** `agents init` drew executor `e08c6206…`, the Apps re-registered (8 of 8), `Keys` generation 8 at 532.
Scripts: controller c0 `9fe0868d`, front `d2dea896`, agents `graft-ops` `e284c442` (`GATE` unset), whose cron fired
08:21:56Z with no reset. Releases c1 `dc9b1966…` and c2 `16bd167b…` at 533 and 534; N is c1 (roll 535 with the token
blocked, `Abort` 536, c1 `5bc20f05` at 100%, `IntentDone` 538). Sampler from 08:31:24Z (2 clients, 43,685 deliveries).
`release roll c2` at 539, 08:32:13Z; c2 at 1% from 08:32:55Z; block at 08:33:21Z. c2 answered 224 webhooks in all.

**Teardown, state left, cost.** The runner account: the agents cron off, the four `graft-agents-m4a-` scripts, the
five store secrets, the deploy token and the bucket (12 objects) deleted, the 0600 files shredded; only
`graft-runners` is left. graft-m4a: sites `eb5c77f2`, `c6b12562`, `3a1ea21b` (crons `[]`), edge `d3d76118`, `graft-ops`
`ac4cd9a5` (schedules `[]`), all as found; `Release` count 543, no intent current, `Keys` generation 8 (its
`agents_executor` seed deleted); the fault plan clear. End sweep: no shard container running. Cost: the row $0.06 on
graft-m4a (08:10Z to 08:45Z), $0.00 on the runner account; graft-m4a's UTC day $2.18 [measured, `acctcost.py`].
