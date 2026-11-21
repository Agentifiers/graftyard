# stall-line: what B0-33 m64r33's site 0 leader was doing in its 24 ticker stalls ($0, 2026-11-21)

LM-1, analysis only, nothing built. The lead's question (after main `c04215ef`): site 0's 24 ticker stalls of 500 ms or
more (25.7 s of its 130 s as leader, 00:12:00 to 00:14:10Z) sat in seconds of only 0.3 to 0.6 CPU-s
(`tests/am1/results/round-remainder.md`). This note lines each stall up with storage confirms, statements per turn,
alarms, catch-up reads and snapshots.

Sources:
- the row's trace (`tests/fanout/raw/tsave-m64r33.jsonl.xz`): kinds 8 `Lag`, 16 `Flushed`, 25 `Entered`, 30 `Work`, 31
  `Sync`;
- `/stats` `bigTx`, the leader's last 20 transactions of 100 KB or more, from `tests/am1/raw/a1-b0-33-mwpnhiva.jsonl`;
- Workers Logs for the leader object, 24,286 invocations with none truncated, in
  `tests/fanout/raw/stall-m64/wl-f0-invocations.json.gz`, with no account fields.

New tool: `tests/fanout/stallline.py`. Its output for site 0 is in `tests/fanout/raw/stall-m64/stallline-site0.txt`.

## Verdict

**The stalls line up with the leader's storage confirms, not with CPU.**
- 23 of the 24 stalls had a `sync()` pending, and pending confirms cover 41% of the stall time [measured].
- The isolate's inputs were held while a confirm was pending and released together when it resolved. That release is
  the other part of the stall [measured, with an inference on the mechanism].
- The statements are the leader's per-entry transaction. The same transaction confirms in 12 to 16 ms on each
  follower, so the statements are not what is slow; the leader's confirm of them is.

## 1. The confirms

`tests/fanout/syncs.py`, wall time of each storage confirm from the transaction to `sync()`'s resolve:

| Member, window | Confirms | Wall p50 / p90 / p99 / max | Confirms of ≥ 150 statements: n, wall p50 / p90 |
|---|---|---|---|
| **site 0 leading**, 00:12:00–00:14:10 | 343 | **50 / 337 / 1,798 / 2,729** | 87, **111 / 992** |
| site 1 following, same window | 269 | 12 / 14 / 29 / 44 | 86, 13 / 16 |
| site 2 following, same window | 303 | 12 / 16 / 30 / 58 | 85, 13 / 24 |
| site 0 idle, 00:09:00–00:11:30 | 190 | 28 / – / 86 / 97 | – |
| **site 1 leading**, 00:14:15–00:16:46 | 478 | 31 / 106 / **742** / 1,113 | 264, 23 / 100 |
| site 0 following, same window | 418 | 32 / 82 / 139 / 161 | 263, 34 / 82 |

- **A leader's confirm is slow; a follower's confirm of the same transaction is not** [measured]. The voters write the
  same entry with the same fold rows. Site 0's ≥ 150-statement confirms took p90 992 ms leading and 82 ms following.
  Site 1's took p99 742 ms leading and 29 ms following.
- **Wall time hardly follows the statements** [measured]. Over site 0's 343 confirms, wall against statements gives r 0.32,
  and against bytes r −0.12. By statement count the p50 was 50, 51, 107 and 202 ms for 0–50, 50–150, 150–250 and
  250–400 statements, with p90 94, 177, 592 and 1,305 ms.
- **What syncs.py says the isolate was doing.**
  - Of the 37 site 0 confirms of 300 ms or more, most are "storage": ticker wakes and requests ran inside them, with lag
    0 at both ends. An example is the 2,340 ms confirm at 00:14:09, with 22 wakes, 319 requests and lag 0 → 0.
  - The rest are "isolate late": the lag was already high at the start, for example 1,092 → 1,233 ms.
  - So in many of them the isolate was free and the write itself was slow.

## 2. The statement path

The transaction is the leader's flush (`Host::flush`, `commit_tx`): every statement queued since the last flush, run
synchronously in one transaction, then `storage().sync()`.

**Its mix**, from `bigTx` on the transactions of 100 KB or more:

| Statement | Per transaction | Bytes | Count across the 20 transactions shown |
|---|---|---|---|
| `INSERT log` | 1 | 110 to 260 KB (the entry, its inline packs as base64) | 20 |
| `INSERT request` | 20 to 31 | about 0.5 KB each | 413 to 615 |
| `INSERT m_path_set` | 3.6 to 10.7 | about 1 KB each | 72 to 213 |
| `INSERT c_ref` | about 4 to 11 | – | – |
| `INSERT c_agent_task` | about 7 to 8 | – | – |
| `INSERT m_agent_task_meta` | about 7 to 8 | – | – |
| `INSERT c_lease`, `c_lease_expiry`, `DELETE c_lease_expiry` | about 3.5 each | – | – |
| `INSERT m_comment`, `m_comment_by_issue` | about 4 each | – | – |

`Work` rows give the leader's statements per step as p99 265 and max 398.

- **How often:** 226 flushes and 343 confirms in 130 s, 2.6 confirms a second. That is about one transaction of 150 to
  280 statements per entry, plus small ones (1 or 2 statements: the raft state, or a log row alone).
- **Not the cost:** the statements' synchronous execution. `Stepped`'s synchronous step plus persist read 0 ms in all
  344 rows, and the followers run the same statements. Why the leader's `sync()` takes 4 to 60 times a follower's is
  [unverified]. A leader differs in two ways:
  - it answers 250 to 320 requests a second, mostly `/commit`, `/outcome_by_request` and `/read`, each a SQLite read or a
    held output;
  - it shares site a with the two busy learners `#3#l0` and `#3#l3`, which are other objects in other isolates.

## 3. The inputs held for the confirm

An isolate's `Date.now()` advances only at I/O, so requests that run back to back after one event share its timestamp.
For the requests that entered while a confirm was pending:

| Member | Confirms 0–100 ms | 100–300 ms | ≥ 300 ms | Resolve times as a share of all ms (baseline) |
|---|---|---|---|---|
| site 0 leading | 64% of 3,153 at the resolve time | 56% of 5,074 | 42% of 5,754 | 0.25% |
| site 1 leading | 56% of 10,989 | 45% of 905 | 65% of 188 | 0.29% |
| site 0 idle | 14% of 14 | – | – | 0.12% |

- **Under load, about half the requests that arrived during a confirm entered the handler at the very ms it resolved**,
  against a 0.25 to 0.29% baseline [measured]. That means they were held until it resolved. The Durable Object's input
  gate during `sync()` is the likely holder [inference; the runtime does not document whether `sync()` closes it].
- Site 0's 142 one-ms batches of 50 or more requests held 25,064 of its 32,313 requests. The largest were 779, 756, 679
  and 609 requests at one ms.
- 21 of the 24 stalls held a batch of 50 or more, and every stall had a confirm pending, a batch, or both.
- Inside the stalls, requests entered at 95 distinct times over 25.7 s, 3.7 a second, against 26.7 a second outside
  them. So the isolate ran in a few large bursts, not steadily.

**What a stall is, then** [inference from sections 1 to 3]:
1. A slow leader confirm holds the inputs.
2. The confirm resolves, and hundreds of held requests run back to back.
3. The 100 ms ticker, a timer queued behind them, fires late.

The 41% of stall time covered by a pending confirm is step 1; much of the rest is step 2. The CPU fits:
- stall seconds averaged 0.48 CPU-s and other seconds 0.37 (`docpusec.py`);
- p50 invocation CPU is 0.5 ms, so a 700-request burst is a few hundred ms of CPU spread over a second or more of
  queued turns.

## 4. What else ran in the stalls (no line-up)

| Candidate | Count in the window | Invocation CPU | Overlap with stalls |
|---|---|---|---|
| Alarms | 218 | 1.06 CPU-s in total, max 266 ms; one held its wall 2,558 ms with 0 ms CPU, which is waiting, not work | spans touch 20 stalls |
| Catch-up `/entries` pages (`bytes=4000000`) | 500 | 2.15 CPU-s, p50 2 ms, max 120 ms | 14 stalls |
| `/snapshot_commit` | 33 | 8 ms in total; walls up to 3,318 ms, waiting on the leader's own commit | 12 stalls |
| A large `Work` step (≥ 1.4 MB of appends) | – | – | 0 of 24 stalls; appends in the 300 ms before a stall p50 44 KB |

- Alarms and catch-up reads ran across the whole window, as they did outside the stalls, and their CPU is small next to
  the 31.9 CPU-s of invocations in the window (`/commit` 12.7, `/raft` 9.8, `/outcome_by_request` 2.3).
- `/snapshot_commit`'s long walls are a consequence: it waits for its own entry to commit.
- None of these lines up with the stalls as the confirms do.

## For B0-02's profile

A local, native profile of the commit path will not reproduce this. The stall time is a durability wait on the leader
object, plus the queue of inputs it releases, not CPU in decide, apply or encode. Statements per transaction and their
bytes would matter only through the confirm, and the followers' 12 to 16 ms for the same transaction says they are not
slow in themselves. Sent to T-01a1ccdd.

## Files

- `tests/fanout/stallline.py`: per-stall confirm overlap, and the held-input test.
- `tests/fanout/raw/stall-m64/`: the leader's Workers Logs invocations (`ts`, wall, CPU, the URL's last 30 characters,
  event type, outcome), and the outputs of `stallline.py` and `syncs.py` for site 0.
