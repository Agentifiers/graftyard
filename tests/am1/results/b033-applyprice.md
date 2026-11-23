# B0-33: what the repository leader's apply turn pays for, priced stage by stage ($0)

The lead asked for the leader-only parts of hosted apply to be priced separately: the reply render (`out_json`), the
resolutions, the feed ring in `stepped`, and the sends. It also asked why hosted apply costs 1.25 to 3.17 ms an event
when the bed costs 0.11 ms a command. Everything here is local or from raws already on main (blB, qlC, pgA, pfA). No
hosted spend.

**Short answer.** Hosted apply is bound by the number of SQLite calls it makes, not by Rust work, the render, or the
sends. The leader makes about 6.3 fold reads and about 3 write statements per event. Each hosted call costs 0.13 to
0.58 ms in a read-only apply slice, against at most 0.064 ms in local workerd, where that figure includes the Rust
work around the call. On the same leader, 64% of the reads in an entry's turn read a row it already read in that turn.
The cut that follows is a fold-read memo scoped to one transaction (DECISIONS, `read-memo`, proposed).

## Method

- **Bed** (`tests/leaderprof`): wasm32 under V8 in local workerd, with real SQLite, the member's own `tier.rs` and
  `txn.rs`, and B0-33's mix at 60 and 11 commands an entry. This head adds the leader's real done-reply path, priced
  stage by stage: `waiter_clone`, `render_parse`, `render_fill`, `render_ok` (by kind), `reply_build`, `reply_send`,
  `reply_reparse`, and `feed_ring`. It also adds `sink=URL`, which sends to an outside HTTP sink so that workerd's
  process CPU holds only the caller's side of each send.
- **Read counts**: `tests/leaderprof/readprobe.patch` (scratch, not product code) counts every `TierSql` query by
  stage. It also counts distinct (query, args) pairs per entry, and adds a `memo=1` switch that memoizes `sql_get` and
  `sql_prefix` until `flushed`. To reproduce, run `git apply tests/leaderprof/readprobe.patch`, then the bed, then
  `git apply -R`.
- **Hosted**: heavyturns' join (`slices`) on each run's leader trace and Workers Logs, with apply slices split by what
  else ran in the same slice.

## Stage prices (bed, N = 60, 131 KB entries, 6 peers) [measured, local]

| Stage | Unit | ms per entry | ms per command |
|---|---|---|---|
| `render_ok` (`out_json`; acquire 0.83, pr 0.14, task 0.13, label 0.08, comment and renew 0.07, push 0.002 ms each) | command | 7.3 | 0.122 |
| `render_parse` / `render_fill` / `reply_build` / `reply_send` / `reply_reparse` | command | 0.1 / 0.4 / 0.5 / 1.1 / 0.8 | 0.002 / 0.007 / 0.008 / 0.018 / 0.013 |
| `waiter_clone` | command | 0.0 | 0.000 |
| `feed_ring` (runs only once a poller read the owner, so 0 on B0-33) | entry | 0.8 | 0.013 |
| `fold_apply` | entry | 7.1 | 0.118 |
| `sql_tx` (184 statements) | entry | 3.3 | 0.055 |
| `peer_encode` + `peer_send_js` | peer | 4.5 | 0.075 |

- The whole done-reply path is about 0.17 ms a command, about 4% of the hosted 4.5 ms. **The reply render is not the
  main cost** [inference from measured].
- Every kind renders validly: 200 and 201 statuses, and a push renders no JSON.

## Sends [measured, local; whether hosted bills this to the isolate is unverified]

| Case | Process CPU, 6 peers | Process CPU, 0 peers | Per peer send |
|---|---|---|---|
| Mixed, 131 KB entries, DO sink | 110–117 ms/entry | 59 | — |
| Mixed, outside HTTP sink | 136–146 | 71–72 | ≈11–12 ms |
| Issue-only, 4 KB entries | 71 | 38 | ≈5.6 ms |

In-isolate encode and send cost about 4.5 ms an entry. The rest of the send cost is outside the isolate's clock.

Hosted, resend-only slices cost 0.7 to 2.4 ms each (blB 0.7, qlC 1.1, pgA 2.4, pfA 1.4; n 119 to 1,398, carrying 25 to
97 KB). A send is cheap in hosted billing. Of the apply slices that also carried an append, all the appends were new
entries, built in the same turn as the apply (see the split below).

## Where hosted apply's time goes [measured, hosted raws]

Apply slices, split by the Work rows in the same slice: A is apply only, AS is apply whose own step appended the next
entry, and N is a new entry built in a step without an apply.

| Run | A: ms, events, stmts | AS: ms, events | N (decide + append, no apply): ms |
|---|---|---|---|
| qlC | 11.6, 12, 46 | 20.4, 14 | 15.1 |
| blB | 53.6, 61, 206 | 117.9, 72 | 79.7 |
| pgA | 66.6, 44, 136 | 180.1, 78 | 81.8 |
| pfA | 155.5, 43, 147 | 110.6, 54 | 113.6 |

- The "send factor" is the next entry's decide and Append, built in the apply's own turn (`advance_commit` → `pump` →
  `decide`). The sends themselves are not the cost. The next entry's decide costs about as much as an apply.
- A deferred transaction costs 0.13 to 0.98 ms a statement on hosted, against 0.018 ms in the bed: 7 to 50 times more.
  A one-statement flush costs about 1.7 (qlC), 7 (blB, pgA), and 18 (pfA) ms. That is a fixed cost of 3 to 5% of
  joined CPU.
- A read-only apply slice has no flush and no append. Dividing its CPU by its fold reads (events × 6.3, the bed's
  reads per event below) gives 0.13 to 0.15 ms a read on blB and qlC, 0.24 on pgA, and 0.58 on pfA. The bed's upper
  bound is 0.019 to 0.106 ms a read, and that bound includes all the Rust work.
- Followers pay 0.6 to 1.5 ms an event against the leader's 3.8 to 5.9. Followers do no decide and no render, and
  render alone is 250 of the 471 reads an entry. **Inference:** the leader-only part is mostly SQL reads.

## Reads per entry, by stage (bed, `readprobe.patch`) [measured, local]

| Mix (N = 60) | Reads in apply / render / consumers / feed | All reads | Distinct (query, args) |
|---|---|---|---|
| B0-33 mix | 139 / 250 / 45 / 35 | 471 | 142 (30%) |
| task only | 66 / 240 / 0 / 3 | 311 | 16 |
| push only | 185 / 0 / 180 / 3 | 370 | 252 |
| renew only | 68 / 120 / 0 / 3 | 193 | 74 |
| pr only | 428 / 180 / 0 / 3 | 613 | 434 |
| acquire only | 187 / 7,500 / 0 / 123 | 7,812 | 194 |

With `memo=1` (one memo per transaction, cleared in `flushed`), on the B0-33 mix at N = 60:
- render reads fell from 250 to 23 and apply reads from 139 to 112;
- render CPU fell from 6.3–8.1 to 2.4 ms an entry, and fold apply from 6.7–8.8 to 5.9–6.2;
- every reply status was the same with and without the memo.

At N = 11, render reads fell from 35 to 11 and apply reads from 41 to 29.

Acquire renders read 125 rows per command. Each renders the holder's live leases by scanning the lease index. The
probe reads these as the same repeated rows.

## What it means for the cut [inference]

- Hosted apply cost per event ≈ (reads per event + statements per event) × the host's cost per SQL call. The cost per
  call is what varies between hosts (A41, pfA, pgA). The call count is what the code decides.
- The memo removes about 64% of an entry's reads on the B0-33 mix, with the reply bytes unchanged. Here is what that
  is worth on hosted. At 6.3 reads and about 3 statements an event, cutting reads to about 2.5 an event removes about
  40% of a leader apply event's SQL calls. On pgA's pooled 0.16 ms a call that is about 0.6 of 2.75 ms an event (22%),
  and on pfA's 0.34 ms a call about 1.3 of 3.17 ms (40%).
- B0-32 needs about 29% off its burst commit stage. That falls in this range but is not guaranteed.
- Statements are the next lever (blB: 206 an entry for 61 events). They are not in this cut.

## Limits

- The bed's per-read figure is an upper bound with the Rust work in it. The hosted per-read figure is a division by
  the bed's reads per event, not a hosted count. A hosted count needs the probe's counter on a test install, as a
  `test-faults` stat, on a site day.
- The slices cover only the turns after a `/raft` call. That they stand for the leader's other turns is an
  assumption, as in `b033-pga.md`.

## Teardown after an a1 that exited 4 [verified in code, not observed live]

`rows.sh` runs `teardown.py $rid $rid` whatever a1's exit code, and `rows.sh` and `costrow.sh` both end with
`teardown.py --sweep`. teardown.py tears down shard containers only. Learner and voter Durable Objects stay joined, as
they do on every row.

## After `read-memo` (branch `r33-readmemo`) [measured, local]

`tests/leaderprof/memo.sh` runs the member's own `tier.rs` and `txn.rs` with the memo in local workerd.
- All seven edge cases hold: `scratch`, `buffered`, `flushed`, `snapFlushed`, `clear`, `prefixKey`, `refused`.
- It compares a memo-on leader and follower against a memo-off member:

  | Mix | Replies byte-identical | Leader core-tier SQLite reads, memo off → on | Follower |
  |---|---|---|---|
  | B0-33, N = 60, 300 entries | 36,000 of 36,000 | 139,899 → 23,366 | 19,879 |
  | B0-33, N = 11, 300 entries | 6,600 of 6,600 | 22,238 → 7,018 | 5,212 |
  | push only, N = 60, 100 entries | 12,000 of 12,000 | 12,220 → 12,119 | 12,119 |

- Fold digests are equal after every entry, and there were 0 wasm traps.
- Plant 97 (`tests/m0carried/plant97.py`) catches 6 of 6 mutants. The sixth was missed until the `prefixKey` case
  was added.
