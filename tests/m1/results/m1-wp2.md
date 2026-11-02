# M1-WP2: Raft group core (pure crate)

Run 2026-10-31 on `crates/raft` (`graftyard-raft`), a sans-IO member state machine with no Workers dependency. It is
not yet hosted in a Durable Object, so BF-12 and BF-13 below ran in a simulator, not on the real stack; the gate rows
still need the hosted code.

## Simulator

`crates/raft/tests/sim.rs`. Three members, one seeded generator per run:

- 8% of messages dropped, 4% duplicated, each delayed 5 to 1,500 ms, so messages reorder.
- Member crashes at 0.3% per step, losing everything not persisted, with a restart from disk after 50 ms to 4 s.
- A crash in the middle of an input at 0.2% per input, losing that input's writes and outputs.
- One site cut off for 0.5 to 15.5 s at 0.3% per step.
- A fixed clock skew per member of up to ±200 ms, under `DRIFT`.
- Four clients over three keys doing get, put, and compare-and-set. They re-send with the same `RequestId`, poll
  `outcome_by_request`, and hedge only through `campaign` (the `suspect` path).

Checked after every message and step:

- At most one leader per term.
- No two members commit different entries at one index, compared by digest.
- Every committed index is on at least 2 disks.
- Every `Acked` write is in the log of every leader of a term at or after its ack.
- Every ticket result, ack or refusal, is at or below the resolving member's commit index.
- No `RequestId` is applied at two indexes.
- The digest chain is unbroken.

| Run | Seeds × steps | Result |
| --- | --- | --- |
| Default `cargo test` | 60 × 8,000 (484,512 steps) | Pass |
| `long_sweep`, release | 2,000 × 10,000 (20,148,290 steps) | Pass: 230,479 ops (86,173 acked, 36,424 refused, 90,914 reads, 16,968 with no reply, 1,852 answered by request id), 218,183 crashes (90,978 mid-input), 31,369 partitions, 1,541,014 messages (472,895 dropped, 55,710 duplicated), 2,868 leader terms |

The long sweep was run twice, by the builder and on review, with identical counts.

## Linearizability

- The long sweep's histories pass a Wing–Gong checker with Lowe's cache, per key, in the crate. That checker rejects
  five planted bad histories in its own tests.
- The same history exported for the Milestone 0 checker (`tests/m0/porcupine`) returned
  `{"operations":229188,"noReplyWrites":15677,"droppedFailedReads":1291,"result":"Ok"}`. One read changed to a value
  nobody wrote returned `Illegal`.

## The tests catch real bugs

Eight bugs were planted one at a time, and each failed at least one test:

- A quorum of one.
- Tickets resolved before commit.
- Request-id dedupe removed.
- The log up-to-date check removed from votes.
- A lease three times too long.
- An old-term entry committed by counting replicas (Raft figure 8).
- A promise ignored.
- A vote forgotten on restart.

Testing also found one real bug in the crate: a leader whose probe was refused re-sent at once without moving its next
index, so the leader and the follower looped forever. It now retries on `tick`.

## Gate scenarios, in the simulator

- **BF-12.** 300 seeded leader kills between quorum and reply, plus one detailed case. Every re-send got the original
  outcome, and every member ended with the value appended once. The gate is 10,000 kills on the hosted code.
- **BF-13.** A compare-and-set refused `Stale` only because of an earlier command in the same entry, with the leader
  lost before commit. The refusal was never returned, and the next leader decided the command fresh.

## Where the code departs from the design's Shape

- `decide` takes the fold at `applied` plus the entry's earlier commands, as the prose says, not `&Fresh`.
- `outcome_by_request` takes the command, because `replay` is a method on it.
- `MemberState` adds `promised_to` (whom the promise was given to) and `saved_at` (for the boot rule).
- A duplicate answered from its request row carries the leader's latest commit proof.
- A new leader whose `Noop` reaches no quorum within 3 s returns to follower and keeps `led_term`.
- `CallError::TooLarge` is the member's `COMMAND_MAX` refusal.

The Shape still states each of these the old way.

## On Durable Objects

Run 2026-11-01 in the Milestone 0 account. The crate is hosted in the site script's `Member` class
(`workers/site/src/host.rs`, `member.rs`, `wire.rs`, `register.rs`), deployed as `graft-m1-site-a/b/c` (enam, weur,
apac) by `workers/site/deploy.mjs` from `tests/m1/wp1/sites.json`. The test edge is `graft-m1-wp2-edge`
(`tests/m1/wp2/edge.ts`), and `tests/m1/wp2/run.ts` drives it. The raw rows are in `tests/m1/wp2/raw/`. The fold is a
string register per key with put, compare-and-set, and append (`register.rs`), the same as the crate's test fold.

### The host

- **Tables.** `raft_state` (the `MemberState` row), `log` (index, term, digest, entry), `reg` (the fold), `request`
  (the core-tier request table, rows older than `REQUEST_RETENTION` by entry `at` deleted on apply), `dup` (a request
  id applied at a second index), and `fault` (test kills). M1-WP1's `me`, `rtt`, and `Registry` are unchanged, and
  `graft-m1-driver`'s `/join` still returns its rows and the 409.
- **Persist and the output gate.** `Ready::persist` hands each `Write` to a store that turns it into SQL statements.
  The statements of one input then run in one `storage.transactionSync`, synchronously. The output gate holds every
  reply and subrequest issued after them until they are durable. Messages leave only from what `persist` returned. A
  storage error resets the object and releases nothing. `Write::Apply` writes the `reg` keys and the `request` rows the
  applied entries touch.
- **Restore.** The constructor reads the rows, rehashes each entry against its stored digest, and rebuilds the
  `GroupFold` by applying the log through `MemberState.applied`. It checks the result against `reg` and `request`, then
  calls `Member::restore` and persists its `Ready`.
- **Transport.** Each `Envelope` is one POST of a binary envelope to the peer's `/raft` through its site's binding,
  with `CALL_DEADLINE`. The peer steps it and returns its envelopes for the caller in the response body. A call that
  errors or misses the deadline goes to `call_failed`. The host never retries, because `tick` resends. `COMMIT_RETRIES`
  is the edge's budget: it re-sends with the same id.
- **Time.** `tick` runs every 100 ms while an input arrived in the last 3 s or a call is held. One alarm is allowed: a
  leader's, every 2 s while it served a request in the last 60 s. That is the drain-check alarm. It is set
  unconfirmed, and it only ticks. Campaigns start only from `/campaign` and `/suspect` requests.
- **Edge calls.** `/commit` holds up to `COMMIT_HOLD`, then answers `admitted` with the ticket. `/outcome`,
  `/outcome_by_request`, and `/read` (`fresh`) are the other calls. Every reply carries `meta` (`ReplyMeta`). A member
  that cannot lead answers `notLeader` with its committed config.
- **The edge.** It mints one 128-bit `RequestId` per client operation. It finds the group from its cached config, else
  from the bootstrap names `{chain}#{gen}#f{slot}` built from the record. It learns the config from `NotLeader.config`
  and the leader from `ReplyMeta`. On a missed receipt it sends `suspect {colo, about, term}` to the other voters and
  re-sends to the same member. It moves only when a member answers `leader`, `won`, or `resumed`, or when a `NotLeader`
  names a leader. A `NotLeader` that names no leader is reported to every voter. The command is never sent to a member
  that has not answered that it leads.

### BF-12: leader killed after quorum and before the reply

`run.ts bf12 --run bf12-full --kills 10000 --groups 10 --down-every 25`. Ten groups ran in parallel, with kills inside
each group in sequence. Each kill appends a unique token `i,` under a new request id, with a kill that fires when the
command's ticket resolves on the commit-advance path:

- `after_quorum`: the commit-advance writes (state and apply) are made durable, the resolution is dropped, a `fault`
  row with the original outcome is written, and the object calls `ctx.abort()`.
- `after_quorum_unsynced`: the same, with the commit-advance writes dropped too.
- Every 25th kill also holds the reset object down for 12 s, so another member must lead and answer the re-send.

The edge re-sends under the same id until it has an outcome. The audit (`raw/bf12-full.bf12-audit.json`) reads each
group's three members after one more write, so every follower learns the last commit.

| Mode | Kills that landed | Same outcome | Answered by another leader (at least; its quorum omits the killed member) | Re-send to outcome p50 / p99 / max |
| --- | --- | --- | --- | --- |
| `after_quorum` | 4,800 | 4,800 | 0 | 1,056 / 3,502 / 11,041 ms |
| `after_quorum` + down 12 s | 200 | 200 | 36 | 13,360 / 20,917 / 21,225 ms |
| `after_quorum_unsynced` | 4,800 | 4,800 | 0 | 1,016 / 2,933 / 9,934 ms |
| `after_quorum_unsynced` + down 12 s | 200 | 200 | 40 | 13,486 / 16,707 / 21,152 ms |
| **Total** | **10,000** | **10,000** | 76 | |

- 10,001 kills were sent and 10,000 landed. One `after_quorum_unsynced` kill (`repo:489009`, token 855) never fired:
  its first send missed `CALL_DEADLINE` before the leader admitted it, so the edge dropped the flag, as it does after
  any failed send. The re-send committed normally. One more kill made up the count.
- Every re-send returned the original outcome, with the same index and the same output. A changed outcome would have
  shown here, because the original is read back from the killed member's `fault` row.
- Zero double applies. On all 30 members, the register holds 1,000 or 1,001 distinct tokens in increasing order, each
  once, and every acknowledged token is present. The committed log holds no request id at two indexes. `dup` is empty.
  The `reg` row on disk equals the fold in memory. Request ids equal request rows on every member.
- Without the down window, the re-send reached the restarted object, which resumed `led_term` and answered from its
  `request` row (`replay`). With the down window, at least 76 re-sends were answered by another member that had
  become leader, and the rest by the old leader once it came back. Either way the answer is a re-send under the same
  id answered from the request row, never a second decide: no token appears twice anywhere.
- The run took about 35 minutes (02:09 to 02:44 UTC) on 10 groups, about 4.8 kills per second in total, so the full
  10,000 ran and the count was not cut.

### BF-13: leader lost before commit, Stale refused only by an earlier command in the same entry

`run.ts bf13 --run bf13-full --rounds 400 --groups 8`. In each round the leader gets one `/commit_many` call. It holds
`put` on `k:hold`, then admits `cas(k, unset -> "a")` and `cas(k, unset -> "b")` as separate `commit` inputs in one
turn. All three land in the next entry, so the second cas is decided `Stale` only because of the first. The `fault`
row records that the entry carried the first cas's events and only that request (`refusedInEntry` names the second
cas), so the setup is checked in every round, not assumed. The leader is then killed before that entry commits:

- `before_commit`: the entry's writes and every output are dropped, and the entry is lost.
- `before_commit_sent`: the entry is made durable and sent to both followers, then the leader dies before any answer,
  so the entry commits under the next term or the resumed one.
- Each kill also runs with a 12 s down window, so another member leads.

The edge re-sends `hold` and the second cas under their ids. The first cas's client gives up, so its effect is
unknown (no reply in the history). Then the edge reads `k`.

| Mode | Rounds | Stale in the killed entry | Killed entry committed later | Second cas returned | Read |
| --- | --- | --- | --- | --- | --- |
| `before_commit` | 104 | 104 | 0 | 104 ok | 104 `b` |
| `before_commit` + down | 96 | 96 | 0 | 96 ok | 96 `b` |
| `before_commit_sent` | 104 | 104 | 104 | 104 `Stale`, at a later committed entry | 104 `a` |
| `before_commit_sent` + down | 96 | 96 | 96 | 96 `Stale`, at a later committed entry | 96 `a` |

- Zero refusals were returned from uncommitted entries. The killed leader's `Stale` was never returned. All 200
  refusals that were returned name an index that a member holds as committed: 1 to 3 entries after the killed entry.
  In those rounds the killed entry committed under the next leader or the resumed term, the re-sent second cas was
  decided again on a fold that held the first cas, and it was refused at that later entry. When the killed entry was
  lost, the re-sent cas was decided again on a fold without the first cas, and it succeeded, at the lost entry's index
  (118 rounds) or the next one (82).
- Porcupine (`tests/m0/porcupine`, keys prefixed with the chain): `{"droppedFailedReads":0,"noReplyWrites":400,
  "operations":1600,"result":"Ok"}` on `raw/bf13-full.bf13-history.jsonl`. A mutant of that history, where the lost
  entry's `Stale` was returned for one `before_commit` round, returned `Illegal`.

### Commit latency, B0-01 style

`run.ts bench --run bN --seconds 60`: 20 callers × 5 workers × 1 put/s of 1 KB through the edge, as
`tests/m0/run/raft-bench.ts` did, on a new `Tri` group led from site A. Commit is measured on the leader, from a
command's admission to its ticket's resolution on the commit-advance path, as M0 measured it.

| Run | Commands | Failed | Per entry | Commit p50 | Commit p99 | Round p50 / p99 | Leader confirm p50 / p99 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| b1 | 6,000 | 0 | 21.8 | 338 ms | 735 ms | 210 / 562 ms | not recorded |
| b2 | 6,000 | 0 | 15.9 | 239 ms | 645 ms | 150 / 397 ms | not recorded |
| b3 | 6,000 | 0 | 16.2 | 238 ms | 763 ms | 140 / 542 ms | not recorded |
| b4 | 6,000 | 0 | 21.6 | 327 ms | 775 ms | 213 / 532 ms | 25 / 312 ms |
| b5 | 6,000 | 0 | 14.9 | 224 ms | 791 ms | 134 / 402 ms | 23 / 241 ms |
| b6 | 6,000 | 0 | 19.8 | 303 ms | 702 ms | 190 / 500 ms | 23 / 71 ms |
| Pilot (15 s) | 1,500 | 0 | 14.3 | 218 ms | 548 ms | 133 / 316 ms | not recorded |

M0's prototype, `tests/m0/results/b0-01.json` and `raw/b0-01-final*.json`: p99 433, 468, and 481 ms on the final
code, and 399 to 690 ms over 23 one-entry runs. Three controls ran on the deployed M0 prototype (`graft-raft-*`, group
`m1ctl-*`) in the same hour: p99 399, 653, and 1,240 ms (`tests/m1/wp2/raw/m0-control-*.json`).

- **Verdict.** The Rust host measures commit p99 702 to 791 ms in six one-minute runs at 100 commands/s. That is
  between pass (600 ms) and fail (1 s) in every run, and above M0's final 433 to 481 ms. M0's control in the same hour
  gave 399 to 1,240 ms, so the network varies widely, but the host's six runs cluster higher than M0's runs did.
- **Why it is slower** [inference from the counters above]. M0's leader sent AppendEntries before its own write and wrote on the next macrotask, so its
  confirm overlapped the round (`tests/m0/results/m0-wp2.md`, B0-01, point 1). This host lets the output gate hold
  each round's AppendEntries until the leader's own write is durable, which is the crate's persist-then-send rule.
  The leader's confirm is p50 23 to 25 ms, with a tail to 241 to 312 ms in two of three runs. That is added to every
  round, and the next entry waits for that round. The round is also about 20 to 40% longer than M0's (p50 134 to
  213 ms against 101 to 135 ms), and entries are larger (15 to 22 commands against 11 to 14). Both follow from the
  longer round under one entry in flight.
- What would close it is in "What the crate should expose" below.

### Fault found and fixed in the host

- `ctx.abort()` called from inside wasm hung the object instead of resetting it. On the first BF-12 kill, the killed
  member (`repo:100#1#f0`) answered no request for over 60 s, including `/ping`, while other objects of the class
  answered. The tail shows its calls `canceled`. The uncatchable abort unwound through the wasm module, and the
  workers-rs shim marked the instance terminated. The host now calls `abort` from a timer with no wasm frame on the
  stack, and every kill since reset the object and restored it. This applies to any Rust Durable Object that resets
  itself.

### What the crate should expose

- **Send before persist for the leader's own append.** The crate's rule is that nothing leaves before the input's
  writes are durable. For the leader's own `Append`, Raft only needs the leader's write durable before it counts
  itself in a quorum or reveals a commit, and M0 relied on that (`crates/raft/src/member/follower.rs` already notes
  the case of a restarted leader with a lost entry). A `Ready` that splits "messages safe before the writes" (the
  leader's AppendEntries) from "outputs after the writes" (resolutions, replies, acks), with `persist` taking a
  confirmation callback, would let a host send the round while its write confirms. The commit-advance path already
  waits for the leader's own ack.
- **Load a `GroupFold` from stored tables.** `GroupFold` has no constructor from a fold, request rows, config, and
  applied index, so the host replays the whole log through `applied` on every restore. That is fine for 1,000 entries
  and not for a real group. It needs `GroupFold::from_parts`, or `Stored` without a full log.
- **Decoders and serde.** The crate has `encode_entry` for the digest but no decoder, and none of its message or
  state types derive serde, so the host keeps its own codec for every `Message` variant, `LogEntry`, and
  `MemberState` (`wire.rs`, checked by two round-trip tests and by rehashing each entry on restore). Every crate field
  change has to be mirrored there by hand. The crate should own one encoding with a decoder.
- **Which commands an entry refused.** `Write::Append` names only the requests that produced events. A host learns
  which admitted commands were decided and refused only from ticket resolutions at commit. BF-13 had to infer it from
  the tickets still waiting.
- **Kills, not crate changes.** The faults are host test hooks (`kill` on `/commit`). The crate needs nothing for
  them.

### Design problems found

- "The leader proves its lease, appends at last + 1, sends to both followers at once" (`design/DESIGN.md`, Shape, the
  write path), together with "Each member's copy was confirmed by its own storage before its reply left"
  ("Invariants", acknowledgment point). These do not say whether the leader's AppendEntries may leave before the
  leader's own append is durable. M0 sent first and wrote after, and it passed B0-01 at 433 to 481 ms. The crate and
  this host write first, as `Ready::persist` requires, and B0-01 is 702 to 791 ms in six runs (table above). B0-01
  and B0-02 carry forward from M0 to "M1-WP2 on real code" (`build/BUILD-PLAN.md`, Milestone 0 status). On this
  evidence, real code that writes first sits between pass and fail. Either the design allows the leader to send
  before its own write is durable (Raft does, and the crate's follower code already handles the lost-entry case) and
  the crate changes as described above, or B0-01's margin is gone.
- "Every receipt and member-to-member call has a 1 s deadline and retries at most twice" ("Commit and read paths").
  The crate's `tick` resends AppendEntries and votes after `CALL_DEADLINE`, and a leader whose round reaches no quorum
  gives up after `CAMPAIGN_DEADLINE` (3 s, one try and two retries). So the host's transport makes one call per
  message and never retries it. The sentence can be read either way: as a transport retry or as this budget. No
  measurement here depends on which reading is meant.

## Second pass, 2026-11-01: early AppendEntries, one codec, shared ids

This pass makes the changes decided from the first pass's report, redeploys `graft-m1-site-a/b/c` and
`graft-m1-wp2-edge`, and reruns B0-01, a short BF-12, and BF-13. It ran from 07:20 to 08:15 UTC. The raw rows are in
`tests/m1/wp2/raw/`.

### What changed in the crate

- **The leader sends AppendEntries before its own append is durable.** `Ready::persist` still hands every write to
  storage first. Its `Out` now has `early`: the leader's AppendEntries, which the host may send before the writes are
  durable. Everything else waits for the writes, including votes, `AppendRes`, ticket and read resolutions, and the
  state row that holds promises. The leader counts its own copy toward a commit only up to `own_durable`. Only
  `Member::self_ack(now, SelfAck)` raises `own_durable`. `persist` hands out the input's `SelfAck` in `Out::self_ack`,
  and the host returns it after the writes confirm. A `SelfAck` is not `Clone` and has no public constructor. It names
  the leader's boot and a truncation epoch, so an ack from an earlier boot, or from before a truncation, is ignored.
  A host that never returns it loses liveness, not safety: those entries never commit. The leader commits nothing above
  `own_durable`, even when both followers hold the entry (Raft dissertation 10.2.1).
- **A resumed `led_term` writes two `Noop`s, commits nothing below the second, and sends them only once they are
  durable.** With early sends, a reset leader can lose an entry at index k that a follower holds in the same term. The
  resumed instance then writes a different entry at (term, k). Votes compare (term, index), so the two look equal. Had
  the resumed leader committed at k, the follower holding the lost entry could win the next term and overwrite a
  committed index. Committing only at k + 1 or above means every member of the committing quorum holds (term, k + 1),
  so it refuses that follower's vote as `LogBehind`. Sending the two `Noop`s only after they are durable keeps a second
  reset from leaving lost entries at both indexes.
- **`MemberState.lead_seq` and `follow_seq`, and `AppendReq.lead_seq`.** These were found by the 2,000-seed long sweep
  (HARSH seed 10385, "committed index 22 is on 1 disks"). A delayed AppendEntries from a reset leader's earlier
  instance reached a follower after the resumed instance had committed index 22 in the same term. That append carried
  an entry the earlier instance lost, so the follower truncated 22. A leader now increments `lead_seq` each time it
  leads, new or resumed, and stores it with `led_term` before any message leaves. Each AppendEntries carries it. A
  follower stores the highest `lead_seq` it accepted in the term, and refuses an older instance's append in the same
  term. `a_stale_instance_of_a_resumed_leader_cannot_truncate_its_successor` reproduces the case.
- **`codec`: one encoding with a decoder.** `codec::to_bytes` and `from_bytes` write a version byte (`CODEC_VERSION`
  = 1) and then postcard. They work for every type that derives serde in the crate: `LogEntry`, `MemberState`,
  `Envelope` and every `Message`, `GroupConfig`, `NotLeader`, `ReplyMeta`, and `Ticket`. A decoder refuses another
  version, truncation, and trailing bytes. `workers/site/src/wire.rs` is deleted. The digest's canonical encoding,
  `encode_entry`, is unchanged. `tests/codec.rs` pins two digests computed before this pass, with the old
  `RequestId(u128)`, and both are equal now. A `RequestId` is still hashed as its 128 bits.
- **Shared ids.** The crate re-exports `graftyard_types::{Index, Digest, RequestId, Ulid}` and drops its own. `Digest::ZERO`
  is now `graftyard_raft::PREV_ZERO`.
- **`GroupFold::from_parts(fold, applied, config, requests)` and `GroupFold::requests()`.** A host loads the fold from
  its tables. The retention queue is rebuilt from the rows' `at`. `apply` drops rows by `at`, so a loaded fold and a
  replayed one drop the same rows (`a_group_fold_loaded_from_its_tables_equals_the_replayed_one`).
- **`Out::refused: Vec<(Index, RequestId)>`.** It lists the admitted commands that `decide` refused, per entry, in the
  input that decided them. Their tickets still resolve `Refused` only at commit.

### What changed in the host

- **Early send, then the write one macrotask later.** An input that returns `early` messages issues their fetches at
  once. Its SQL transaction is queued and runs in a `setTimeout(0)` task, so the output gate does not hold the
  AppendEntries behind the leader's own write. Until that transaction runs, the host holds every other output of every
  input. Every edge or `/raft` response waits for the transaction (`Host::settled`), and the output gate then holds it
  until the transaction is durable. One `sync()` after the transaction reports each confirm and returns each `SelfAck`.
  Inputs with no early messages commit their transaction synchronously, as before.
- **Restore loads the fold from its tables.** `reg`, `request` (events now stored with the codec), and a new
  `group_config` row feed `GroupFold::from_parts`. With `RESTORE_CHECK` on, as on this install, the loaded fold is
  compared with the log replayed through `applied`, field for field. The log is still rehashed against its stored
  digests.
- **Request ids are ULIDs** in the edge (`rid()` in `edge.ts`) and on every host route, so one id has one spelling
  across raft, git, and the edge.
- **BF-13's refused set comes from `Out::refused`**, not from tickets still waiting.
- **A new kill mode, `before_commit_early`.** Only the entry's AppendEntries leave, before the write, and the object
  resets before the write is made.

### The simulator

- A write reaches the member's disk only when its storage confirm ends, 2 to 22 ms after the input, or 1 s on a
  gray site's slow confirm. The `SelfAck` then goes back to the member. A crash loses every write whose confirm has not
  ended, and every output still held behind the gate.
- The leader's `early` AppendEntries leave at once. Half of the mid-input crashes land after they left. HARSH adds
  `crash_unconfirmed`: per step, a 2% chance that a member with a confirm outstanding crashes.
- Counted per run: "leader writes lost after their AppendEntries left", and restarts after which another disk holds an
  entry of the restarted member's `led_term` that the restarted member lacks. HARSH's default run asserts at least one
  of the latter.

| Run | Result |
| --- | --- |
| `cargo test -p graftyard-raft` | codec 4, leadership 32, liveness 4 (1 ignored), scenarios 25, sim 5 (1 ignored): all pass |
| `long_sweep`, `RAFT_SEEDS=2000 RAFT_STEPS=10000`, release | Pass in 286 s. HARSH: 20,095,046 steps, 186,714 ops (64,557 acked, 25,740 refused, 71,626 reads, 24,791 no reply), 225,992 crashes (97,640 mid-input), 580 leader writes lost after their AppendEntries left, 252 restarts with a follower holding an entry the leader lost, 3,573 leader terms. GRAY: 20,082,141 steps, 2,294,903 ops, 24,524 write failures, 1,759 and 2,317 of the same two counts, 1,526 `STEP_DOWN` transfers, 0 forwarded commands |
| Porcupine (`tests/m0/porcupine`) on `RAFT_HISTORY` | HARSH `{"droppedFailedReads":2991,"noReplyWrites":21800,"operations":183723,"result":"Ok"}`; GRAY `{"droppedFailedReads":8,"noReplyWrites":133,"operations":2294895,"result":"Ok"}`; HARSH with one read changed to `nobody-wrote-this`: `"Illegal"` |

Planted bugs, one at a time, each run through every default test and a 300-seed long sweep:

| Planted bug | Caught by |
| --- | --- |
| A. The leader counts itself before its own write confirms (`advance_commit` from `last()`, not `own_durable`) | `the_leader_counts_itself_only_after_its_own_write_confirms`; `gray_network_keeps_every_invariant` (seed 5003, "committed index 222 is on 1 disks"); liveness `leaders_leave_slow_storage` (seed 200); long sweep (seed 10002) |
| B. A resumed `led_term` writes one `Noop` and may commit it | `a_resumed_leader_never_commits_beside_an_entry_it_lost`, `an_old_term_entry_is_not_committed_by_counting_replicas`, `a_stale_instance_…`; `gray_network_keeps_every_invariant` (seed 5004) |
| C. A follower takes an older instance's append in the same term | `a_stale_instance_of_a_resumed_leader_cannot_truncate_its_successor` (the long sweep at 300 seeds passed) |
| D. A resumed `led_term` sends its `Noop`s before they are durable | **Not caught.** All tests and the 300-seed sweep passed. A targeted double-reset scenario also passed on the plant, so it was not kept. The rule rests on the argument above [inference] |

### B0-01 on the new host

`run.ts bench --run dN --seconds 60`, the same load as the first pass. The M0 controls ran on `graft-raft-*` between
the runs, from a copy of `tests/m0/run/raft-bench.ts` in `/tmp` that writes to `/tmp`
(`raw/m0-control-p3-*.json`). Every run: 6,000 commands, 0 failed. Edge colo LAX, leader on site A (enam).

| Run (UTC) | Per entry | Commit p50 | Commit p99 | Round p50 / p99 | Leader confirm p50 / p99 | Append to B p50 / p99 |
| --- | --- | --- | --- | --- | --- | --- |
| d1 07:45 | 14.0 | 216 ms | 470 ms | 137 / 318 ms | 43 / 98 ms | 137 / 318 ms |
| d2 07:47 | 11.9 | 178 ms | 639 ms | 108 / 425 ms | 20 / 218 ms | 108 / 466 ms |
| M0 control 1, 07:48 | 13.4 | 200 ms | 697 ms | 116 / 422 ms | | |
| d3 07:49 | 16.0 | 242 ms | 675 ms | 153 / 479 ms | 40 / 168 ms | 153 / 521 ms |
| d4 07:50 | 12.6 | 183 ms | 682 ms | 112 / 487 ms | 25 / 487 ms | 111 / 339 ms |
| M0 control 2, 07:52 | 12.8 | 190 ms | 452 ms | 119 / 333 ms | | |
| d5 07:53 | 12.5 | 183 ms | 807 ms | 111 / 613 ms | 24 / 613 ms | 111 / 201 ms |
| d6 07:54 | 13.2 | 203 ms | 414 ms | 128 / 281 ms | 22 / 163 ms | 128 / 272 ms |
| M0 control 3, 07:55 | 15.2 | 228 ms | 593 ms | 143 / 437 ms | | |

- **Verdict: no clean pass.** Two of six runs pass 600 ms (d1 470, d6 414), and four fall between pass and fail
  (639 to 807 ms). None fails (1 s). The M0 controls in the same 11 minutes gave 697, 452, and 593 ms: one of three
  over the line. On this hour's network the Rust host and the prototype are level: median p99 657 ms against 593 ms,
  medians of commit p50 193 against 200 ms, and round p50 108 to 153 ms against 116 to 143 ms [measured].
- **Against the first pass.** p99 was 645 to 791 ms in all six runs, with round p50 134 to 213 ms. The round p50 is now
  108 to 153 ms, and commit p50 is 178 to 242 ms against 224 to 338 ms. The leader's own confirm (p50 20 to 43 ms) no
  longer sits in front of the round.
- **What the tail is** [inference from the columns]. In d4 and d5 the round p99 equals the leader's confirm p99 (487
  and 613 ms). A round whose quorum answered before the leader's own write confirmed still waits for that write: the
  commit needs `self_ack`. So a slow DO confirm still reaches commit latency, now as max(confirm, round) instead of
  confirm + round. Site C's append p50 is 203 to 349 ms, so in practice the quorum is A and B.
- Six earlier runs, and two controls, used a build from before the `lead_seq` fix. They are in `raw/pre-lead-seq/`
  (p99 951, 1,407, 602, 448, 624, 662 ms; controls 405 and 563 ms) and are not counted above.

### BF-12 and BF-13 regression

- **BF-12.** `run.ts bf12 --run bf12-p3 --kills 1000 --groups 10 --down-every 25` (`raw/bf12-p3.bf12-audit.json`):
  1,000 kills sent and 1,000 landed. 1,000 same outcome, 0 changed, 0 with no outcome. 0 double applies. On all 30
  members, `logTwice` 0, `dup` rows 0, 100 distinct tokens each in order, none missing, `reg` on disk equals the fold,
  and request ids equal request rows. 480 `after_quorum` (re-send p50 / p99 1,053 / 3,298 ms) and 480
  `after_quorum_unsynced` (999 / 3,481 ms). 20 and 20 with a 12 s down window, 9 and 3 answered by another leader.
- **BF-13.** `run.ts bf13 --run bf13-p3 --rounds 400 --groups 8` (`raw/bf13-p3.bf13-report.json`). 400 rounds, 400
  kills landed. In all 400, the crate's `Out::refused` named the second cas as refused in the killed entry. 0
  refusals were returned from uncommitted entries. 200 refusals were returned, all in the `before_commit_sent` rounds,
  where the killed entry committed later. In the 200 `before_commit` rounds the entry was lost, and the re-sent cas
  succeeded and read `b`. Porcupine on `raw/bf13-p3.bf13-history.jsonl`:
  `{"droppedFailedReads":0,"noReplyWrites":400,"operations":1600,"result":"Ok"}`. The same history with one
  `before_commit` round's successful cas2 changed to a refusal: `"Illegal"`.
- **BF-13 with early sends.** `run.ts bf13 --run bf13-p3e --rounds 120 --groups 6 --early` adds `before_commit_early`.
  The AppendEntries leave, the leader resets before its own write, and a follower may hold the entry the leader lost.
  120 rounds, 0 refusals from uncommitted entries, Porcupine
  `{"droppedFailedReads":0,"noReplyWrites":120,"operations":480,"result":"Ok"}`. With no down window (18 rounds), the
  resumed leader always overwrote the lost entry with its `Noop`s, and the re-sent cas2 succeeded at the index after
  them. With a 12 s down window, 13 of 18 lost entries were committed by the next leader, which held them as a follower,
  and cas2 was refused at a later index. The other 5 were overwritten.

### API changes for `crates/identity` and other `Command` crates

- **`Command`, `Fold`, `Scratch`, and `Record` are unchanged.** No method, bound, or associated type changed.
- **`RequestId` is `graftyard_types::RequestId(Ulid)`**, re-exported as `graftyard_raft::RequestId`, and so is
  `graftyard_raft::Ulid`. `RequestId(n)` with `n: u128` is now `RequestId(Ulid(n))`. `crates/identity`'s library
  builds unchanged. Its tests do not: `crates/identity/tests/common/mod.rs:226` writes `RequestId(id)` with `id: u128`.
  This pass does not own that file.
- **`Index` and `Digest` are `graftyard_types`'.** `Digest::ZERO` is gone. Use `graftyard_raft::PREV_ZERO`.
- **`Out` has three new fields.** A host or test harness must send `out.early` as well as `out.messages`. It must also
  return `out.self_ack` to `Member::self_ack(now, ack)` once the input's writes are durable. A harness that drains
  only `out.messages`, as `crates/identity/tests/common/mod.rs` does at lines 145 and 164, never sends the leader's
  AppendEntries, so its leader never commits. `out.refused` is optional to read.
- **Wire and state fields.** `AppendReq` gains `lead_seq`. `MemberState` gains `lead_seq` and `follow_seq`.
  `codec::to_bytes` and `codec::from_bytes` need `Serialize` and `DeserializeOwned` on the event type, but only where
  a host stores or sends entries. That bound is on the functions, not on `Fold::Event`.
- **New:** `codec`, `SelfAck`, `Member::self_ack`, `Member::own_durable`, `GroupFold::from_parts`,
  `GroupFold::requests`, `RequestRow` (already public), and `PREV_ZERO`.

### Design problems found

- "A member stores at most one entry per (term, index) … A restarted member that resumes `led_term` is the term's one
  leader, and a replaced instance cannot append" (`design/DESIGN.md`, Shape, "Invariants", one value per index).
  With the leader sending before its own append is durable, this is false as stated. A reset leader that resumes
  `led_term` can write a different entry at the same (term, index) as an entry a follower holds from its earlier
  instance. The HARSH long sweep counts 252 restarts in that state, and GRAY 2,317. The earlier instance's delayed
  messages can also still arrive after it is replaced. HARSH seed 10385 had one truncate a committed index. Safety
  needs the rules this pass adds: a resumed term's two durable `Noop`s, a commit floor at the second, and `lead_seq`
  on every append. Raft's own answer is a new term per leader instance, which `led_term` exists to avoid.
- "a follower confirms before it replies, and matches entries by digest, so a reset leader's lost entry is told
  apart" (`design/DESIGN.md`, Shape, the write path). Matching by digest covers AppendEntries. It does not cover votes,
  which compare (last term, last index) and cannot tell the two entries apart
  (`a_resumed_leader_never_commits_beside_an_entry_it_lost` fails without the commit floor). It also does not cover a
  delayed append from the earlier instance (`a_stale_instance_of_a_resumed_leader_cannot_truncate_its_successor`
  fails without `lead_seq`).
- "Waiting for its own write cost p99 645 to 791 ms (Scale, commit on `Tri`)" (same block). With the early send, p99
  is 414 to 807 ms over six runs and passes 600 ms in two. The leader's commit still waits for its own confirm, so a
  slow confirm sets the round's tail (d4 and d5 above). B0-01 on real code is level with the M0 prototype measured in
  the same hour, not under it.

### Checks

- `cargo test -p graftyard-raft`: all pass (codec 4, leadership 32, liveness 4 + 1 ignored, scenarios 25, sim 5 + 1
  ignored).
- Long sweep `RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=…`: `test result: ok. 1 passed … finished in 285.99s`.
- `cargo build -p graftyard-site --target wasm32-unknown-unknown --release` and `worker-build --release`: finished.
- `cargo clippy --workspace --all-targets --exclude graftyard-identity -- -D warnings`: clean.
  `cargo clippy -p graftyard-site --target wasm32-unknown-unknown --all-targets -- -D warnings`: clean.
  `cargo clippy -p graftyard-identity -- -D warnings` (library): clean. Its test targets do not compile, as noted
  above.
- `cargo fmt --check`: clean.
- `tsc -p tests/m1/wp2/tsconfig.json`: clean.

## The 24-hour linearizability rerun, started 2026-11-01

This is BM-1's "Linearizability rerun at 24 hours" (M1-WP8 owns BM-1; M1-WP2 carries it). It is also B0-11's carried note: "M1-WP2's 24-hour rerun with per-isolate fault counters and recorded client clock offsets". This section records the run's setup and start. The result goes in a later section once the finish service has written `tests/m1/lin/raw/lin24.report.json`.

**Run `lin24`: 2026-11-01T18:27:15Z to 2026-11-02T18:27:14Z.** Its chain is `repo:915635`, which hosts the test register. The finish service writes the report about 2 minutes after the end, plus the Porcupine time (the 9-minute smoke run's check took 2 s).

### Why the crate changed first: member memory

A member held its whole log and every request row in memory. Restore loaded the whole log from SQLite and replayed it. Natively that costs about 560 B per log entry and 643 B per request row. On Durable Objects, the `oom-probe` against `repo:778001` on graft-m1-site-* reset the leader object at about 387,000 request rows (about 780 entries of 1,000 puts each).

M0's B0-11 load is about 33 operations/s, 60% writes. Over 24 h that is about 1.7 M request rows under the 48 h `REQUEST_RETENTION`, with a log in the hundreds of MB. It cannot fit. Two changes fix this, and the run uses both:

- **The in-memory log is a window (crate).** The fixes:
  - `Member::set_log_window(n)` keeps `n` applied entries in memory. Once `2n` have applied above the base, it drops the older ones. `LogBase { index, term, digest }` names the committed entry below the window.
  - A leader whose follower needs an entry below its window does not send from memory. It puts a `Catchup { to, round, from, through }` in `Out::catchup`. The host reads those rows from its `log` table and calls `Member::catchup(now, c, entries)`, which sends them as that round, if the member still leads it.
  - A follower whose base is above an append's `prev_index` skips the entries at or below its base, which are committed and so match.
  - `Stored` gained `base`. `restore` takes the stored window above it.
  - The default keeps every entry, so `crates/git`'s stand-in group, `crates/identity`'s tests, and the scenario tests see no change. The site host sets `LOG_WINDOW` = 1,000, and its restore loads only the 1,000 entries below `applied` and the tail above it. The restore cross-check against a full replay runs only while `applied` ≤ 20,000.
  - `/log` and `/audit` now read the `log` table, not memory.
- **Request-row retention is a group setting (crate).** `Member::set_request_retention(d)` and `GroupFold::set_retention(d)` replace the fixed `REQUEST_RETENTION` in `apply`. Every member of a group must use the same value, because each drops rows as it applies. The site host reads it from `GRAFTYARD_REQUEST_RETENTION_S` only in a `test-faults` build. The lin install sets 900 s. The clients give up after 30 s, so no re-send outlives its row.

The simulator runs with a window of 6. Its `check` asserts these things after every step:

- no member's base passes `applied`;
- the base digest matches the member's disk;
- the committed-prefix checks read entries below the window from the disk.

Every default test passes. The 2,000-seed long sweep passed in 306 s:

- HARSH saw 28,334 catch-up reads from storage.
- GRAY saw 53,402.
- Porcupine on the export: HARSH Ok on 165,184 operations, GRAY Ok on 2,259,445.

### The install

- **Sites.** `graft-m1-lin-a`, `-b`, and `-c` (enam, weur, apac), from `tests/m1/lin/sites.json`. They have their own script names, so redeploys of graft-m1-site-* never touch them.
- **Build.** `node workers/site/deploy.mjs tests/m1/lin/sites.json --test-faults --vars tests/m1/lin/vars.json` builds the site script with `worker-build --release --features test-faults`. The deployed bytes are in `tests/m1/lin/raw/lin24-build/` (wasm sha256 `884113b7…3e9b0a`). Versions: a `351a3ac6`, b `98ce0e49`, c `66c00070`. They will not be redeployed during the run.
- **Load and chaos.** `graft-m1-lin-tools` (`tests/m1/lin/lin.ts`) binds only the lin sites' `Member` classes. It holds the load clients and the chaos loop as Durable Object alarms, so the run does not depend on the orb. Version `66ccc944`.

### The fault model

The model follows M0's B0-11 (`tests/m0/results/m0-wp2.md`). With `test-faults`, a site script reads `GRAFTYARD_FAULTS` once per isolate. It is set at deploy to `{"fail_call": 0.05, "drop_append_reply": 0.05}`:

- **Failed calls.** 5% of member-to-member calls fail before they leave. The host reports them to `call_failed` as errors.
- **Lost replies.** 5% of AppendEntries replies are lost after the callee ran the call. The leader sees a missed CALL_DEADLINE.
- **Resets and transfers.** The chaos alarm fires every 8 to 16 s:
  - 60% of the time it resets a member (`/reset`, `ctx.abort()`), half of them the leader;
  - otherwise it transfers from the leader to one of the other two sites, and 30% of those transfers are abandoned. An abandoned transfer's `TimeoutNow` is dropped, so the leader abandons it after the LEASE.
  - A target inside `RESET_QUARANTINE` refuses, as the protocol requires. Under this reset rate most transfers are refused (`Target`), as in the smoke run below. The chaos log records each one.

**Per-isolate fault counters.** Each isolate draws a random id when it first counts. `/isolate` answers the id, the plan, the number of rolls, and the counts by fault and target site (`fail_call>1`, `drop_append_reply>2`, `abandoned_timeout_now`). Before each step, the chaos alarm reads every member's `/isolate` and keeps the latest row for each isolate. It never deletes one. The driver copies them to `raw/lin24.isolates.json`.

### Load and clock offsets

**Clients.** There is one pinned load client each in ATL, LHR, and SIN. Each runs 4 logical clients at up to 5 operations/s, against 6 keys:

- 40% `get`; the rest `put` and `cas`, where `cas` expects the value the client last read;
- 20 s alarm chunks, with values unique across the run (the chunk start is in each value);
- the M1-WP2 edge rules: one RequestId per operation, kept on every re-send, `suspect` hedges, and config from `NotLeader`;
- a 30 s give-up, recorded as no reply.

The history rows carry each write's RequestId.

**Clock offsets.** At each chunk's start and end, a client reads `now` from every member's `/status`. It records `offset = now − (t0 + t1) / 2` together with the call's round trip. The finish step works out each pair of colos' offset from these:

- The offset is the difference of the two colos' offsets to the same member, in the same 60 s bucket. Per bucket, it uses the member with the smallest summed round trip.
- The difference is known to within (rttA + rttB) / 2.
- The bound is the largest |difference| + error over the run. Widening every interval by bound / 2 on each side covers it.

The finish step checks the history three ways: on the recorded clocks, at M0's 100 ms, and at the measured bound. Each check is per key with `tests/m0/porcupine -seg 2000`.

### Services (orb)

- `lin24-watch` runs `node tests/m1/lin/run.ts watch --run lin24` in a loop. It resumes from `raw/lin24.state.json` and never restarts a run. It copies chaos events to `raw/lin24.events.jsonl` and the counters to `raw/lin24.isolates.json` every 5 minutes. After the end it logs that the run is over and exits.
- `lin24-finish` runs `node tests/m1/lin/run.ts finish --run lin24` until `raw/lin24.report.json` exists. It sleeps until 90 s after the end. Then it stops the clients and the chaos loop, collects every history and clock row, and runs the three Porcupine checks. It writes the report with operation counts, latency, chaos counts, per-isolate fault totals, and clock statistics. It refuses to overwrite a report.

### The smoke run that proved the path (`lin-smoke1`, 9 minutes, same install)

| What | Count |
| --- | --- |
| Operations | 16,243, 0 with no reply |
| Latency | p50 297 ms, p99 1,508 ms, max 11.5 s |
| Resets | 27, 18 of them on the leader |
| Transfers attempted | 15, 3 abandoned. Most were refused as inside `RESET_QUARANTINE` |
| Isolates reporting counters | 6 |
| Faults injected, site 0 | 222 failed calls and 202 lost replies |
| Faults injected, site 1 | 227 failed calls and 256 lost replies |
| Faults injected, site 2 | 6 failed calls and 8 lost replies. Site 2 led least |
| Clock rows | 468 |

Per-colo offset to member clocks had a median of −15 to −6.5 ms. The cross-colo differences, median and range:

- ATL−LHR: −0.5 ms, from −23 to 53.5 ms;
- ATL−SIN: 1.5 ms, from −14.5 to 12 ms;
- LHR−SIN: 4 ms, from −9.5 to 17.5 ms.

The measured bound was 124 ms, so intervals were widened by 62 ms. Porcupine: all six keys Ok on the recorded clocks, at 100 ms, and at 62 ms. The raw files are `tests/m1/lin/raw/lin-smoke1.*`.

### The test-faults gate

Everything test-only in `workers/site` is compiled only with the `test-faults` feature:

- **Routes.** `/log`, `/audit`, `/faults`, `/deps_log`, `/isolate`, `/reset`, `/retire`, and `/probe_vote`.
- **Commit fields.** `kill`, `down_ms`, `hold_deps_ms`, and `hold_ms`.
- **Other fields.** `Join.no_claim`, `/replace`'s `fail_first`, and `/transfer`'s `abandon`.
- **Faults.** The BF-12 and BF-13 kill paths (`crate::faults::Kill`, `Host::fault_for` and `fire`, `die`), the `down_until` window, and the call-fault plan.
- **Tables.** `fault`, `dup`, and `deps_log`.
- **Variable.** The `GRAFTYARD_REQUEST_RETENTION_S` override.

`tests/m1/lin/grep-faults.sh` greps the built `index_bg.wasm` and `index.js` for 28 strings: route names, field names, kill-mode names, variable names, fault messages, and the gated functions' symbol names. Short routes such as `/isolate` compile to integer compares, so symbol names are needed. The release wasm keeps its name section.

- Product build (`worker-build --release`): `needles present: 0 of 28`.
- Test build (`--features test-faults`): `needles present: 28 of 28`.

## Integration pass, 2026-11-01: what changed around the running 24-hour run

`lin24` kept running through this pass, and nothing in it was redeployed. Its scripts `graft-m1-lin-a/b/c` and `graft-m1-lin-tools` still run the bytes in `tests/m1/lin/raw/lin24-build/`. At 23:47Z, 5 h 20 min in, the clients counted ATL 214,765, LHR 199,315, and SIN 188,759 operations, with 0 errors. The run ends at 2026-11-02T18:27:14Z, and `lin24-finish` writes `tests/m1/lin/raw/lin24.report.json` about 2 minutes later.

Changes in this pass that touch the gate and the run's code:

- **The gate covers more.**
  - The test register (`workers/site/src/register.rs`, `Kind::Register`) now compiles only with `test-faults`. A product member hosts no register. A `repo:` chain joined without an owner is a `Repo` member with no owner, and it skips the source check (`Source::Given`). The CLI's latency probe joins `repo:0` that way and never campaigns. The edge always joins with the owner.
  - `GRAFTYARD_GENESIS` (`SiteEnv::genesis_shortcut`) now compiles only with `test-faults`.
  - On the git edge, every `/_bed/` route, `BED_TOKEN`, the `x-graft-kill` and `x-graft-down-ms` headers, the `x-graft-outcome-index` and `x-graft-rid` response headers, `FORCE_FORMAT`, and `GRAFTYARD_GENESIS` compile only with `workers/edge`'s new `test-faults` feature.
- **The proof**, rerun on this pass's code:
  - `tests/m1/lin/grep-faults.sh` has two more needles, `GRAFTYARD_GENESIS` and `::register::`.
    - Product build (`worker-build --release`, `/tmp/site-prod`): `needles present: 0 of 30`.
    - Test build (`--features test-faults`, `/tmp/site-tf`): `30 of 30`.
    - A third candidate, `::genesis_shortcut`, is absent from both builds, because the release build inlines it. It is not a needle.
  - `tests/m1/integ/grep-edge-faults.sh` does the same for the git edge with 12 needles.
    - Product build: `0 of 12`.
    - The deployed test build: `12 of 12`.
  - A product install (`m1-wp7.md`, "End to end with the CLI") had no test variable in any script's bindings.
- **The commit hedge.** `graftyard_lookup::commit_hedged` now reports a `NotLeader` that names no leader with `suspect` to every voter, and keeps the member it found as the target (`m1-wp6.md`, "The commit hedge, fixed"). The site script does not call it, so the lin install's code and the M1-WP2 edges are unaffected.
- **`crates/raft`.** No source change. One simulator test helper, `serve_catchup` in `tests/common/sim.rs`, lost a boxed closure for clippy's `type_complexity`. The checks after it:
  - `cargo test -p graftyard-raft`: codec 5, leadership 33, liveness 4 (+1 ignored), scenarios 27, sim 5 (+1 ignored). All pass.
  - Long sweep `RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=/tmp/sweep-hist.jsonl`: `test result: ok. 1 passed … finished in 313.60s`.
  - Porcupine (`tests/m0/porcupine`): HARSH `{"droppedFailedReads":3422,"noReplyWrites":25525,"operations":165184,"result":"Ok"}`, GRAY `{"droppedFailedReads":6,"noReplyWrites":164,"operations":2259445,"result":"Ok"}`.
- **BF-12 on `Push`.** BF-12 ran again on repository groups with `Push` commands through the git edge: 1,000 kills, 1,000 original outcomes, 0 double applies. See `m1-wp6.md`, "BF-12 on `Push` commands".

## M1-WP8 pass, 2026-11-02: the 24-hour run is untouched

`lin24` keeps running on `graft-m1-lin-*` with the build it started on. Its services (`lin24-watch`, `lin24-finish`) were not touched. Its result is due after 2026-11-02T18:27Z, and `m1-wp8.md` marks BM-1's 24-hour half as pending until then.

Late in this pass `crates/raft` changed once: each AppendEntries is capped at `APPEND_BYTES_MAX` (1.5 MB past its first entry) as well as `CATCHUP_MAX` entries (`m1-wp8.md`, fix 4). The lin install runs the build from before it. Its 1 KB register entries never reach the cap, so the cap would change no call there. The pass also changed code the site script shares, and none of it is on the lin install:

- `crates/git`'s `RepoEvent::Refused`. Repo chains only; the lin chain hosts the test register.
- A `request` query on `/read`.
- The sweeper cron.
- The `SiteFault` test fault.
- `graftyard_lookup`'s `suspect` from `call_opt` and its unreadable-reply rule.

The checks after it:

- `cargo test --workspace`: all pass.
- The long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`): `ok`, 326.37 s before the raft change and 329.75 s after it.
- The new scenario test `a_lagging_follower_catches_up_in_calls_under_the_byte_cap`, which fails on the old code.
- Porcupine on its histories, the same counts before and after the raft change: HARSH `{"droppedFailedReads":3422,"noReplyWrites":25525,"operations":165184,"result":"Ok"}`, GRAY `{"droppedFailedReads":6,"noReplyWrites":164,"operations":2259445,"result":"Ok"}`.

## Gate-row pass, 2026-11-02: `Member::record`, and a leaderless lookup

This pass closed BF-14, BF-18 and BF-19 on the hosted test install (`m1-wp8.md`, `m1-wp5.md`, `m1-wp4.md`). It changed `crates/raft` once and `graftyard_lookup` twice. `lin24` on `graft-m1-lin-*` and its services were not touched, and the lin build has none of these changes.

### `crates/raft`: `Member::record`

BF-18 needs the repository leader to commit `Promoted` and `Archived`, which no client command decides. The new call is:

```rust
pub fn record(&mut self, now: Timestamp, id: RequestId, format: Format, events: Vec<Ev<C>>)
    -> Ready<C, Result<(), CallError>>;
// resolved later as
Resolution::Recorded { id: RequestId, result: Result<Index, NotLeader> }
```

- The events become their own `EntryBody::Events` entry, with `requests = [(id, 0..n)]`. The entry takes its turn as the one entry in flight, after id blocks and ahead of queued commands. Followers `verify` it like any entry, so the fold's `check` is the gate (`crates/git`'s `RepoFold` checks `Promoted` and `Archived`, `m1-wp5.md`).
- It is idempotent through the `request` row. An id that already has a row resolves at that row's index and appends nothing. An id already queued or in flight is not queued twice.
- It is refused with `TooLarge` past `ENTRY_MAX`, and with `Unsupported` for a format this release does not read.
- The `Command` trait is unchanged, so `crates/identity` and `crates/git` need no change.
- **Simulator.** The leader's host calls `record` with chance 2 per mille per step under register key `REC_KEY`. 1 in 200 of those calls re-sends an id already used. Each `Recorded(Ok(i))` must find the entry it recorded at `i`, committed. The existing no-double-apply check covers re-sends, and the harness test asserts more than `SEEDS` recorded entries committed.
- **Planted bug.** Skipping the request-row check in `record` was caught by the simulator's "request applied twice" check. The bug is reverted.

### `graftyard_lookup`

- **A pass that finds only leaderless voters now ends with `suspect` to every voter (`call_opt`).** After the site redeploy, `names:174`'s three members reset and none campaigned. The edge's only call to it was `/outcome_by_request`, a lookup that never reaches `commit_hedged`'s hedge. So every member answered `NotLeader` with no leader until the 20 s lookup gave up, on every request. Campaigns are request-driven, so a lookup is the one request such a group gets. After the fix, the first lookup elected a leader on `names:174`. `commit_hedged` already reported a leaderless `NotLeader` this way (`m1-wp6.md`).
- **`commit_hedged` takes its test hooks as one value.** Its signature is now `commit_hedged(b, chain, record, body, colo, for_ms, hooks: Hooks)`, with `Hooks { first: Option<&[u8]>, trace: Option<&RefCell<Vec<Value>>> }`. `first` is the first send's body (a test install's kill or receipt fault). `trace` is BF-14's per-step record of each send's target and the reason for it (`m1-wp8.md`). Product callers pass `Hooks::default()`. The release endpoint is the only one.

### Checks after the change

Run as the orb service `wp8c-checks` at 12:06 to 12:17Z. The fmt and clippy rows were rerun after `Hooks` (service `wp8c-grep`, 12:19Z).

| Check | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean. The first run failed on `commit_hedged`'s 8 arguments (`too_many_arguments`), and `Hooks` fixed it |
| `cargo test --workspace` | 235 passed, 0 failed, 2 ignored (27 test binaries) |
| Long sweep `RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=…`, release | `ok`, 333.25 s. HARSH: 160,066 ops, 226,083 crashes, 19,494 distinct ids recorded and 4,526 `Recorded(Ok)` resolutions. The simulator counts only `Ok` resolutions. HARSH crashes a member 113 times per seed, which ends most leaderships before their record commits, but the other outcomes were not counted. GRAY: 2,226,291 ops, 30,191 ids recorded and 35,079 `Recorded(Ok)` resolutions. A re-send resolves again, so resolutions can outnumber ids |
| Porcupine (`tests/m0/porcupine`), HARSH | `{"droppedFailedReads":3343,"noReplyWrites":27735,"operations":156723,"result":"Ok"}` |
| Porcupine, GRAY | `{"droppedFailedReads":11,"noReplyWrites":209,"operations":2226280,"result":"Ok"}` |

The operation counts differ from the previous pass's because `record` draws from the simulator's random stream, so every seed runs a different schedule. The outputs are in `raw/checks-20261102b/`.
