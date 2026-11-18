# What a leader's deferred transaction writes (B0-33 m50, m51, m52)

Code-only, $0. The orchestrator asked, after commit-asks (main b3c8474c), for: the statements a leader's deferred
transaction carries per confirm, by table and kind, at p50 and p99 for m52's leader; which of them a follower does not
write and which are test-faults only; and the top one or two cuts, ranked by statements removed per unit of risk, as a
design before any code.

**Answer.** The largest share is product code: `ExtendLeases` rewriting every live lease, about 1,365 statements each
time it fires, 46% of m52's leader statements. Test-faults rows are second: the `dup` check is 13 to 19% of the
statements in the transaction, and BF-14's `arrival` row adds one more write of its own per `/commit`, 31 to 44 per
leader confirm on m52. **Correction to F92:** a follower applies the same entries and writes the same fold statements,
lease rewrites included, and confirms them in p50 24 ms (max 85 ms), where the leader took p50 545 ms (max 2.8 s) on the
same indexes. So the statement count alone does not make a leader's sync slow. It multiplies something only the leader
has. commit-asks' "a follower's 2 statements" was an artifact: a follower flushes at once, and only deferred flushes are
traced (`Flushed`).

## Method

`tests/fanout/txmix.py` (new) reads two sources:
- **`bigTx`** in each row's leader `/stats`: the statement mix (verb, table, count, argument bytes) of the leader's
  newest 20 transactions of 100 KB or more (`host::stmt_mix`, kept in `Host::flush`);
- **the trace**: each deferred transaction's statement count (`Flushed`, `index`), and each member's `Applied` and
  `Confirm` rows.

It splits the `bigTx` mixes into transactions that carry an `ExtendLeases` (100 or more `INSERT c_lease`) and the rest.
It then scales each mix by the matching size class's total from the trace (1,000 or more statements, or fewer), and
pairs each leader sync with the followers' first sync after they applied the same index.

Windows (srvclock for m52; m50 +7,310 ms and m51 +7,440 ms):
- m52: 14:44:21–14:49:21Z;
- m51: 11:16:45–11:21:45Z;
- m50: 10:16:40–10:21:40Z.

Outputs: `tests/fanout/raw/txmix-m5{0,1,2}b33.txt`.

Caveat: `bigTx` keeps only transactions of 100 KB or more, so the "without `ExtendLeases`" mix is sampled from the
larger ordinary transactions (22 to 512 statements). The estimated shares apply it to every small transaction.

## 1. Per confirm, m52's leader

A leader's deferred transaction is one confirm (`flush`, then one `sync()`).

| | site a (leader 214 s) | site b (leader 68 s) |
|---|---|---|
| deferred transactions | 222 (1.04/s) | 38 (0.56/s) |
| statements per transaction, p50 / p99 / max | **205 / 1,378** / 1,448 | **311 / 1,786** / 1,786 |
| transactions of 1,000+ statements, and their share of the statements | 9, 21% | 16, **92%** |

What one transaction holds, per transaction (`bigTx`, m52):

| Group (tables) | Ordinary transaction (≈ p50 class; 22–421 statements) | With `ExtendLeases` (≈ p99 class; 1,471–1,786) | Follower writes it too? | test-faults only? |
|---|---|---|---|---|
| lease rows (`c_lease`, `c_lease_expiry` delete and insert, `by_holder`/`session`/`app`) | 41.6 | **1,365.2** | yes, through the same tier apply (`on_extend`, then the commands' events) | no |
| `request` rows (one per command) | 50.6 | 75.8 | yes (`Write::Apply`) | no |
| **`dup`** (BF-12's double-apply check, `INSERT … WHERE EXISTS`) | 50.6 | 75.8 | yes | **yes** |
| comment rows (`m_comment`, `by_issue`, `fts` delete and insert) | 48.5 | 69.5 | yes | no |
| agent task rows (`c_agent_task`, `m_agent_task_meta`) | 37.8 | 56.2 | yes | no |
| issue and PR rows (with `fts`) | 9.2 | 10.9 | yes | no |
| tier bookkeeping (`_fold`, `_digest`, `ids`, `counter`) | 5.6 | 5.8 | yes | no |
| ref rows (`c_ref`) | 3.4 | 6.4 | yes | no |
| `m_path_set` | 2.8 | 3.4 | yes | no |
| `log` and `raft_state` | 2.0 | 1.8 | yes | no |
| `request` retention `DELETE` | 1.0 | 1.0 | yes | no |

Undo and `_digest` appear only as the per-tier `_digest`/`_fold` rows above: one or two statements a transaction.

The writes that sit outside the deferred transaction:

| Write | Where | Rate on m52's leader | Follower? | test-faults only? |
|---|---|---|---|---|
| **`arrival`** (BF-14), a bare `exec` after every `/commit` reply, its own implicit transaction (`Routes::receipt_fault`) | every member that takes a `/commit` | 47.3/s at site a, 40.1/s at site b: **31 and 44 per leader confirm** | yes, 14 to 21/s | **yes**; default `arrival=sql`, and no B0-33 harness sets `mem` |
| `deps_log`, a bare `exec` per entry that used dependency reads (`Host::release`) | leader | about 1 per entry, ≤ 1/s | no | **yes** |
| trace rows (`Host::trace_at`) | memory only (`Stats::trace`) | — | — | yes; they write no storage |

## 2. Shares across runs

Estimated shares of all the leader's deferred statements, from `txmix.py`'s section 3:

| Group | m50 | m51 | m52 |
|---|---|---|---|
| lease rows | 25.9% | 12.2% | **45.5%** |
| `request` rows | 18.7% | 17.5% | 13.1% |
| `dup` (test-faults) | 18.7% | 17.5% | 13.1% |
| comment rows | 12.9% | 15.7% | 12.4% |
| agent task rows | 13.6% | 10.0% | 9.7% |
| everything else | 10.2% | 27.1% | 6.2% |
| `ExtendLeases` transactions among the leader's newest 20 of 100 KB or more | 10 | 2 | 8 |

**Test-faults share.** It is not the largest share of the deferred statements: `dup` is 13 to 19%. Count `arrival` as
statements too, and the test-faults writes on m52's leader are about 80 of about 300 a second at site a (27%) and about
90 of 440 at site b (21%). On m50 and m51 the leaders took 5 to 8 `/commit`s a second, so `arrival` added 1 to 9 writes
per confirm. The hosted rows do partly measure their own instruments, but the leading share is product code.

## 3. Why `ExtendLeases` fires so often

The size of the transaction comes from code; how often it fires is [inference].
- **Size.** `lease::extend` (`crates/git/src/lease.rs`) calls `write_renew` on every live lease with `expires > since`.
  That is three statements per lease: delete and insert the `lease_expiry` index row, and rewrite the `lease` row. m52
  held 461 to 468 live leases, so each `extend` is about 1,400 statements.
- **Frequency.** A leader marks `stalled_since` when its own entry is unacknowledged for CALL_DEADLINE, 1 s
  (`tick_leader`). A follower marks on a re-send of that age, and on every `/suspect`. The next entry carries
  `ExtendLeases`. m52's commit round was p99 2,079 ms and confirm p99 2,803 ms, so a slow round marks a stall. The
  next entry then rewrites every lease, which makes that transaction about seven times larger. On m52's site b the
  1,000+ transactions came 2.0 to 8.0 s apart. On m51, whose rounds were faster, there were none in the window.
- **Feedback.** On m52 the leader's sync after a transaction of 1,000 or more statements took p50 545 ms (10 of 24 at
  1 s or more), against p50 135 ms (19 of 233) after a smaller one. So a slow round can mark a stall that makes the
  next confirm slower, which can mark the next stall.

The `by` of these extensions is not traced. Gaps of 2 to 8 s suggest each `by` is a few seconds, against LEASE_TTL of
600 s.

## 4. The follower control: statements alone do not explain the leader's slow sync

| | leader sync after the deferred transaction | followers' first sync after applying the same index |
|---|---|---|
| m52, 1,000+ statements | n 24, p50 545, p90 2,183, max 2,825 ms; 10 at 1 s or more | n 48, p50 **24**, p90 50, max **85** ms; 0 |
| m52, fewer | n 233, p50 135, p90 930, max 2,784; 19 | n 466, p50 18, p90 30, max 138; 0 |
| m50, 1,000+ | n 10, p50 127, max 1,561; 1 | n 20, p50 26, max 63; 0 |
| m51, all (none 1,000+) | n 631, p50 50, p90 144, max 3,454; 13 | n 1,262, p50 18, p90 29, max 526; 0 |

A follower's transaction for that index carries the same fold statements as the leader's: the lease rewrites, request
rows, `dup`, and tier rows. Each member applies the committed entry through the same tier code and the same `SqlStore`.
A follower adds its own log append. Its flush runs at once, not deferred (`Host::step`: `defer` needs early messages,
which only a leader sends), so `Flushed` never traced it. That is where commit-asks' "followers: 2 statements" came
from, and the claim is withdrawn.

What only the leader has during its sync:
- **More `arrival` writes:** 40 to 47/s, against 14 to 21/s on followers.
- **Event-loop lag:** on m52, 138 lag rows on the leaders, 72 of them at 500 ms or more; 0 on followers.
- **Read load:** refs pages, packed reads and dependency reads.
- **Inbound:** 300 to 410 req/s, against 30 to 56 on followers.

None of these alone tracks the sync within the leader role:
- sync time against `/commit` in the second before: Spearman −0.08 to 0.03 on m52's leaders;
- sync time against the largest loop lag during it: 0.46 and 0.50 on m52, 0.16 to 0.33 on m50 and m51.

So F92's reading changes. Not storage luck still holds, since no follower had a sync of 1 s or more. But "the leader's
own write volume" is now "the leader's writes plus something else only the leader has, most likely its concurrent
activity" [inference]. Cutting statements should shorten the leader's large confirms; m52's p50 gap is 545 against
135 ms. It will not by itself bring them to a follower's 24 ms.

## 5. Ranked cuts, for the design (no code yet)

Ranked by statements removed per unit of risk.

**(1) `arrival=mem` and no `dup` row on load rows.** Test-faults only; harness, plus one site-script change.
- `arrival`: the harness sets `/stats?arrival=mem` on every voter at row start. That route already exists in
  test-faults builds, and B0-02 used it (`tests/m0/results/carried-6.md`). No site day, no code. It removes 31 to 44
  implicit transactions per m52 leader confirm. carried-6 measured that the row about doubles slow confirms: 123
  against 55 of about 2,800 at 150 ms or more.
- `dup`: BF-12's check stops writing in the transaction. `RequestStore::put` already returns false for a request id
  that has a row (`crates/raft/src/fold.rs`, "a double apply"), and `RequestSql::put` sees the pending rows too. A
  test-faults build counts that false in memory and serves it as `/audit` `dupRows`, with no `dup` statement. This
  removes 13 to 19% of the deferred statements. It needs a site day, a test that plants a double apply and expects `dupRows`, and a
  plant that drops the count.
- Risk: none to product. BF-12's evidence moves from a table to memory, so an object reset loses it. `/audit`'s
  `logTwice` still scans the log.
- Expected: about 13 to 19% fewer statements per confirm, and no per-`/commit` storage writes on any member.

**(2) An `ExtendLeases` floor.** Product; `crates/git` only.
- Rule: an `extend` whose `by` is under EXTEND_MIN moves no lease. EXTEND_MIN is 5 s [guess; 1/120 of LEASE_TTL, ten
  DRIFTs].
- Where: in `lease::extend`. `by` is in the entry, so the leader's scratch and every member's tier apply decide the
  same way, and `crates/raft` stays unchanged. The leader still stamps `extend` and clears the marker.
- Effect: a stall under 5 s leaves leases where they were; a stall of 5 s or more moves them by the whole stall, as
  now.
- Removes: about 1,365 statements from each firing, 26 to 46% of the leader's deferred statements on m50 and m52, and
  12% on m51.
- Risk: lease semantics. A lease that expires inside a stall under 5 s lapses up to 5 s earlier than now. A holder that
  renews on time is unaffected (TTL 600 s). The case is two holders on Exclusive scope after the first holder missed a
  whole TTL. That is the same advisory slack DESIGN already accepts for DRIFT (500 ms), ten times wider.
- Design edits: `design/DESIGN.md`, Shape (the `ExtendLeases` comment), and `design/AGENT-LAYER.md`, "Leases" (the
  stall sentence). A DECISIONS entry with a flip: a measured lease handoff that a sub-floor stall made wrong.
- Tests: a lease expiring 2 s into a 3 s stall lapses, and one in a 6 s stall moves. This fails on main for the 3 s
  case. Plants: the floor compared with `>` instead of `>=`, and the floor applied in scratch but not in apply, which
  the leader-follower fold digest catches.
- Rejected alternative: an O(1) extension, a cumulative stall credit that each lease reads at its grant. It removes the
  rewrite at any stall length but changes the lease rows, the expiry index order and the snapshot format. More risk
  for the same B0-33 effect.

**Not proposed:**
- `request` rows: `outcome_by_request` and idempotence need them.
- Comment, `fts` and agent task rows: product events.
- The `fts` delete before a new comment's insert: about 3%; a later tidy-up.

**What would confirm it.** After (1) and (2), a B0-33 row should show:
- no `ExtendLeases` transactions of about 1,400 statements;
- leader statements per confirm p99 under about 500;
- a smaller gap to the follower control.

If the leader's confirm p99 stays near 2.8 s with p99 transactions under 500 statements, write volume is ruled out as
the lever, and F92's remaining cause is the leader's concurrent activity. The order to test there is `arrival`, then
event-loop lag, then reads.

## Reproduce

```
python3 tests/fanout/txmix.py tests/fanout/raw/a1-b0-33-m52.jsonl tests/fanout/raw/tsave-m52b33.jsonl.xz \
  2026-11-18T14:44:21Z 2026-11-18T14:49:21Z tests/fanout/raw/terms-m52b33.jsonl
python3 tests/fanout/txmix.py tests/fanout/raw/a1-b0-33-m51.jsonl tests/fanout/raw/tsave-m51b33.jsonl.xz \
  2026-11-18T11:16:45Z 2026-11-18T11:21:45Z tests/fanout/raw/terms-m51b33.jsonl 7440
python3 tests/fanout/txmix.py tests/fanout/raw/a1-b0-33-m50.jsonl tests/fanout/raw/tsave-m50b33.jsonl.xz \
  2026-11-18T10:16:40Z 2026-11-18T10:21:40Z tests/fanout/raw/terms-m50b33.jsonl 7310
```
