# The commit path's follow-up requests, F92's third run, and the orb DNS workaround (2026-11-18)

Code-only, $0, and no change to `crates/raft` or any request path. The orchestrator asked for three things after B0-33
m52 (`tests/am1/results/b033-m52.md`):
1. What one late `/commit` costs in follow-up requests at today's timings, and whether a slower receipt cadence once
   the leader is past its confirm alarm would remove most of them without hurting a healthy leader. This is a design
   proposal only.
2. m52's numbers in F92, and a test of whether the leader's own load makes its storage slow, against storage luck, on
   m50, m51 and m52.
3. The DNS and proxy workaround in `tests/fanout/README.md`.

## 1. What a late `/commit` costs

### The path, as the code runs it

`commit_hedged` and `receipt` in `crates/lookup/src/lib.rs`, and the member routes in `workers/site/src/member.rs`:
- **The send.** The member's `/commit` steps the command, then holds the request up to `COMMIT_HOLD` (1 s) for its
  outcome. If the outcome came, it answers `done`. Otherwise it answers `admitted` with the ticket.
- **The `/outcome`.** For an `admitted` answer, the edge sends one `/outcome` with `wait_ms` 10,000 (an 11 s call).
  The member holds it until the ticket resolves or 10 s pass. This happens once per command that was not done within
  the hold.
- **The ask.** If the send has no reply at `COMMIT_RECEIPT` (2 s), the edge sends `/outcome_by_request` with the
  request id and no pack, with `CALL_DEADLINE` (1 s). A `pending` answer ends the send as `admitted`, and the edge then
  sends the `/outcome` above. An `unknown` answer, or any other 200, keeps the send open, and the edge asks again
  after the next `COMMIT_RECEIPT`.
- **The cap.** A send still open at `RECEIPT_OPEN` (8 s) is a miss: it is sent again, and the member is reported only
  if it is not `busy`.

So a command not done within the hold costs **one `/outcome`, held until its entry commits**. A send whose handler
has not answered after 2 s adds **one ask for each 2 s it stays open**.

### Measured on m52 against m51

The leader's handler wall time by route, from Workers Logs (sampled; `tests/fanout/routewall.py`, raw in
`raw/commitasks-m52b33.txt`):

| leader | `/commit` p50 / p90 / p99 | sends ≥ 2 s / ≥ 4 s | `/outcome` p50 / p90 | `/outcome_by_request` p50 / p90 |
|---|---|---|---|---|
| m51 site a (healthy, 11:16:44–11:17:42Z) | 883 / 1,198 / 2,624 ms | 2.3% / 0.0% | 137 / 403 ms | 110 / 328 ms |
| m51 site b (after the change) | 1,009 / 1,951 / 3,803 ms | 9.6% / 0.4% | 506 / 2,111 ms | 219 / 1,053 ms |
| m52 site a (14:44:21–14:48:08Z) | 1,197 / 3,599 / 4,994 ms | **32.6% / 5.4%** | **4,685 / 9,767 ms** | 251 / 774 ms |
| m52 site b | 1,124 / 3,231 / 4,799 ms | 30.2% / 4.5% | 3,557 / 6,377 ms | 216 / 797 ms |

Follow-up rates per leader-second (`raw/streams-m5{1,2}b33.txt`), with the open requests each holds (Little's law:
rate × mean handler time from the same sample):

| per leader-second | m51 | m52 | open on m52's site a |
|---|---|---|---|
| `/commit` sends | 92.9 | 133.0 | 133 × 1.56 s ≈ 207 |
| `/outcome` (once per command not done in the hold) | 38.5 (0.41 per send) | 84.7 (0.64 per send) | 85 × 5.19 s ≈ **440** |
| `/outcome_by_request` (asks, plus 4 to 5/s walk and leader lookups) | 12.4 (0.13 per send) | 61.5 (0.46 per send) | 62 × 0.35 s ≈ 21 |
| follow-ups per send | 0.55 | **1.10** | |

The ask rate fits the receipt model: 32.6% of sends at 2 s or more plus 5.4% at 4 s or more gives 0.38 asks a send,
against 0.44 measured (58.7 asks/s at `SentBy::Ask` against 133.6 sends/s in the first 50 s, `b033-m52.md`, "The
leader's inbound by stream"). The rest is sampling and sends over 6 s.

**So, on m52, one late command costs 1 `/outcome` held about 4.7 s (p50), plus about 0.7 asks of about 250 ms each.**
The `/outcome` waiters hold about 440 of the leader's open requests, two thirds of its p50 of 661. The asks hold about
21, about 3%. By count, asks are 15% of the leader's inbound and `/outcome` 21%.

### Would a slower receipt cadence after the confirm alarm remove most of them?

**No, only the asks, which are the smaller and cheaper part.** The receipt cadence sets only the asks. The `/outcome`
waiters follow from `COMMIT_HOLD` expiring with the entry not yet committed (`queue:slot` p50 5.4 s on m52), and the
edge sends one per command whatever the cadence.

On m52's site a, the 13 confirm alarms and the 10 s after each covered 128 of the 227 s it led. In those seconds the
leader took 63.7 asks/s, against 47.5 in the rest. A first ask at 4 s instead of 2 s while the leader is past its
alarm would leave only the sends still open at 4 s: 5.4% of sends against 32.6%. That is about 45 fewer asks a second
in the alarm windows, about 11% of the leader's inbound then. It would free about 15 open requests, about 2% of the
open count, and leave every `/outcome` waiter in place. [inference, from the handler-time shares above]

**On a healthy leader** it changes nothing, since m51's healthy site a had 2.3% of sends at 2 s or more. The rule
would apply only after an alarm.

**What it would cost.** Both earlier decisions turned down a longer `COMMIT_RECEIPT` because it delays the report on a
leader that is really gone (`decisions/DECISIONS.md`, `receipt-rule` "Beat" and `burst-receipts` "What it beat").
Making it conditional does not avoid that:
- **The edge cannot see the alarm.** The confirm alarm is the leader's own `Health` state in `crates/raft`. The edge
  would need it in the member's replies, a "hot flag in leader replies", which `receipt-rule` also turned down because
  it needs a raft change.
- **An edge-only stand-in has already been turned down.** "Slow down after an ask answered 200" is the "treating any
  200 in the term as `busy` with a longer window" that `burst-receipts` beat: it spares a member that answered once
  and then died.
- **A leader past its alarm is the gray case.** It is the one most likely to stop answering, so the delayed report
  lands where the report matters most. Its own `STEP_DOWN` transfer covers a leader that is sick but alive, not one
  that hangs.

**Recommendation: do not build it.** It would reopen two decisions to remove about 11% of the leader's inbound and
about 2% of its open requests, and none of the follow-up that holds the queue. If the commit path is the next lever,
the numbers point at the `/outcome` waiters, about 440 open on m52, and so at the leader's admission queue that keeps
entries from committing within the hold. The operator closed (c1b), which capped those waiters, so that needs new
evidence or a new idea before any design. B0-02's F91 A/B shows first whether the queue was refs pages.

## 2. F92: the slow syncs follow the leadership, and why

m52 added to F92 (`decisions/OPEN-QUESTIONS.md`):
- Site a's leader had 36 of 320 syncs at 1 s or more (max 4,144 ms) and handed off on its 14th confirm alarm.
- Site b then had 12 of 71 at 1 s or more (max 3,806 ms), starting 7 s after the transfer.
- The other members stayed under 150 ms.

### The test

`tests/fanout/syncload.py` works per member and second over each run's 300 s load window (`raw/syncload-m5{0,1,2}b33.txt`).
For each second it takes the inbound requests and the time a `sync()` was in progress. It splits them by role, from the
terms file, with the old runs shifted onto Cloudflare's clock (m50 +7,310 ms, m51 +7,440 ms). It also pairs each sync
with the deferred transaction it confirmed (`Flushed` `index`, the statement count).

| | m50 | m51 | m52 |
|---|---|---|---|
| syncs ≥ 1 s per minute, leaders | 12.8 (site a), 4.0 (site b) | 3.0 (site a), 4.4 (site b) | 9.4 (site a), 10.1 (site b) |
| syncs ≥ 1 s, followers (about 600 follower-seconds per run) | **0** | **0** | **0** |
| leader seconds at 0–25 inbound req/s: syncs ≥ 1 s per minute | 0.0 (28 s) | 6.3 (19 s) | 2.9 (41 s) |
| follower seconds at 25–100 req/s: syncs ≥ 1 s per minute | 0.0 (68 s) | 0.0 (74 s) | 0.0 (262 s) |
| Spearman, a second's sync time against its inbound, leaders only | 0.11 | 0.13 | −0.04 |
| the same, followers only | 0.26 | 0.19 | 0.10 |
| the same, all members pooled | 0.58 | 0.59 | 0.51 |
| statements per deferred transaction, leader p50 (followers: 2) | 98 to 233 | 1 to 182 | 205 to 311 |
| share of a leader's syncs ≥ 1 s, under 250 statements against 1,000+ | 1% → 19% (site b) | 1% → 12% at 250+ (site b) | 0% → 60% (site b), 5% → 11% (site a) |

[measured, `tests/fanout/syncload.py`; one per run, `raw/syncload-m5{0,1,2}b33.txt`]

**Reading.**
- **Not storage luck.** Luck would show slow syncs on followers at some rate. Across three runs and about 1,800
  follower-seconds there were none at 1 s or more, while every leader had 3 to 13 a minute. Each slow streak began
  within 3 to 7 s of the member becoming leader, and ended when it stopped leading.
- **Not inbound request rate, as such.** Within a role, a second's sync time hardly tracks its inbound: −0.04 to 0.13
  for leaders. Leaders had slow syncs even in seconds with under 25 inbound requests. The pooled 0.51 to 0.59 is the
  difference between the roles, not load within a member.
- **The leader's writes.** A leader's deferred transaction carries p50 98 to 311 statements, a follower's 2. A
  leader's share of slow syncs rises with the statements a sync confirms, most clearly on m52's site b (0% under 250
  statements, 60% at 1,000 or more) and m50's site b (1%, then 19%). [inference] The leadership's own write volume per
  transaction is the best-supported cause.
- **What is not shown.** Followers write too little to tell, from these runs, whether storage would slow under the
  same writes on any member. That needs a follower given leader-sized transactions. It is a sim or bench question,
  not one these traces answer.

**What this means for F92's candidate.** The candidate judged a handoff target by its own confirms. That does not
help: a target's confirms are fast because it is a follower, so every target passes, and its syncs slow once it
leads. F92 now records that a confirm trip under write load should hold, as a trip with no qualifying target already
does, rather than transfer. That needs its own `crates/raft` design, the sweep and Porcupine. It stays deferred, as
the orchestrator decided, and B0-33's zero-change line fails on it until then.

## 3. The DNS and proxy workaround

`tests/fanout/README.md`, "When the orb cannot reach a workers.dev host":
- the symptom (SERVFAIL, `CONNECT` 502, `fetch failed` / `ENETUNREACH`, "no repo") and how to check for it;
- `NO_PROXY` with `.workers.dev`;
- the DoH pin in the orb's own `/etc/hosts`, with a backup and a restore;
- the rule never to commit the pin or the address, nor put either in a driver.

## Tools added

- `tests/fanout/routewall.py`: a member script's handler wall time by route, from Workers Logs.
- `tests/fanout/syncload.py`: per member and second, sync time against inbound, by role, with the sync-to-transaction
  pairing.

## Checks

- `scripts/check_design.py`, `check_names.py` and `check_links.py`: OK.
- `python3 -m py_compile` on both new tools.
- No Rust, site, edge or `crates/raft` change.
