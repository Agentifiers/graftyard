# sync-ab: does the leader's input load make its storage confirms slow? (B0-33, graft-m2b)

LM-1, 2026-11-21. Follows `tests/am1/results/stall-line.md`: on B0-33 m64r33, site 0's leader confirmed its per-entry
transaction at p50/p90/p99 50/337/1,798 ms, while each follower confirmed the same transaction in 12 to 16 ms. The
lead's question is whether that is the leader's input load or the platform's (`decisions/OPEN-QUESTIONS.md`, F106).
Tags follow `design/SHARED.md`, "Read this first".

## The switch (option b, approved by the lead)

The leader's inputs on m64r33, from the trace's `Entered` rows (`tests/fanout/syncab.py`), in requests a second:

| Route | site 0 leading, 00:12:00–00:14:10 | site 1 leading, 00:14:15–00:16:46 |
|---|---|---|
| `/commit` (edge) | 98.2 | 143.8 |
| `/outcome` (edge ticket waits past COMMIT_HOLD) | 52.3 | 105.3 |
| `/raft` (43.9 and 63.6), of which the 4 learners | 34.3 | 60.0 |
| `/outcome_by_request` (edge receipt asks) | 38.9 | 1.1 |
| `/read` (edge) | 7.5 | 1.5 |
| Total | 248.6 | 319.9 |

One test-faults switch, **lean**, removes the edge's `/outcome` and `/outcome_by_request` traffic from the leader while
`/commit` still goes to it. The row's arm C and arm B remove the learners' `/raft` with the existing `LEARNERS` knob.

- **Edge** (`workers/edge/src/lib.rs`, `lean_asked`): a request with `x-graft-lean` and the bed token commits lean.
  One edge build serves every arm; a product edge never reads the header.
- **Lookup** (`crates/lookup/src/lib.rs`, `Hooks::lean`), for repository commits (pushes, REST and GraphQL writes,
  merges):
  - each `/commit` send carries `"hold_ms": LEAN_HOLD_MS` (6 s), spliced in first, so a push's inline pack is not
    parsed again (`held_body`);
  - while the send is open no `/outcome_by_request` ask goes out (`send_only`). The send's own 200 is the reply; an
    error status is reported; no answer by RECEIPT_OPEN (8 s) is sent again, reported only when the member is not
    `busy`. Those are `receipt`'s rules for a send's end, with the asks left out.
- **Site** (`workers/site/src/member.rs`, `CommitReq::hold_ms`, `commit_hold`): `/commit` holds the command that long
  for its outcome instead of COMMIT_HOLD (1 s). Past it, it answers the ticket as before, and `commit_hedged` then waits
  on `/outcome` as before.
- **Driver**: `a1.mjs --lean` puts the header on every git and API request; `b033.sh` takes `LEARNERS=` (set and
  empty) for no learners, and `A1_EXTRA=--lean` for arm B.

The edge's leader `/read`s are not moved. They are 7.5/s and 1.5/s (3% and 0.5% of the inputs), and the read the
edge sends to a leader is the one `read_spread` already sent there after every follower declined.

**What a client sees does not change, only latency** (the lead's condition):
- A held `/commit` answers the same `done`, or the same `admitted` ticket at the deadline, that it answers at
  COMMIT_HOLD. The edge's handling of each answer is the code it runs today.
- RECEIPT_OPEN still bounds every send, and a commit still ends in an outcome or in `commit_hedged`'s explicit
  "no outcome" error (503 with `Retry-After`).
- Skipping the asks changes nothing about acknowledgment. An ask's answer was only ever taken when it was the leader's
  own `done`, `NotLeader` or `pending` (`receipt`); `Acked` is still built only inside `outcome`.

**The product build is unchanged.** Everything new is under `#[cfg(feature = "test-faults")]` or
`#[cfg(any(test, feature = "test-faults"))]`. Product builds (`worker-build --release`, no features) of main
`d9a8e8811` and of this branch:
- the wasm `code` section is byte-identical for both the edge (3,904,865 B) and the site (11,460,984 B), and so are
  every other section but `data`;
- the `data` sections differ only in panic-location line numbers (175 u32 words in the edge, 234 in the site), each
  one the line field of a `core::panic::Location` in `workers/edge/src/{lib,api,ident}.rs`,
  `workers/site/src/member.rs` or `crates/lookup/src/lib.rs`, because lines moved. No string differs.

## Tests and plants

- `crates/lookup`: `a_lean_send_holds_its_commit_and_asks_nothing`:
  - the held body keeps every field and adds `hold_ms` first (also for `{}`);
  - LEAN_HOLD_MS stays under RECEIPT_OPEN;
  - `send_only` takes a held 200 (`done` or `admitted`) as the reply, reports an error status, and sends a silent send
    again, reported only when not busy;
  - with the flag off (`Hooks::default()`), `receipt` asks twice on a send that answers at 5 s.
- `workers/site`: `a_commit_names_its_hold_only_on_a_lean_send`: a body as `held_body` writes it is held 6 s; a body
  without the field, every product edge's, is held COMMIT_HOLD.
- `workers/edge`: `ls_refs_go_to_the_shard_only_when_a_bed_request_asks` now also covers `lean_asked`: off by default,
  and on only with the header and the right token.
- `tests/m0carried/plant75.py`: 9 plants, **9 of 9 caught**:
  - no hold on a lean send;
  - a trailing comma on an empty body;
  - a hold past RECEIPT_OPEN;
  - a silent lean send always reported;
  - an error status taken as a reply;
  - `receipt` asking once (flag off);
  - the header without the token;
  - the site ignoring the hold;
  - a product body held longer.

**The plant left in (fixed, 2026-11-21).** The first push, `2f4014d9`, carried plant75's mutant 4: `send_only`
took any status as the reply (`Some((_, v)) => Reply(v)`), so in arm B a sick leader's 503 would have been the commit's
reply. Its own test fails on it (the lead's catch). The checks I reported had run on the clean tree. Then the orb
service that ran them restarted its script, plant75 ran again, and my commit step stopped the service while mutant 4
was planted. SIGTERM skips Python's `finally`, so the file was never restored, and the commit took it. The fix
restores `Some((200, v)) => Reply`, `Some(_) => Missed { report: true }`. plant75 now turns SIGTERM into `SystemExit`,
so a stopped run restores its file, and the check service ends in `sleep` so it never reruns. Mutant 4 is that exact
plant, and it is caught on the fixed tree.

**Full checks** on this branch: fmt 0; workspace clippy 0; wasm32 clippy `-D warnings` site 0 and 0 with
`test-faults`, edge 0 and 0, ops 0 (no `test-faults` feature, 101 with it as before); `cargo test --workspace`
1,338 passed, 0 failed, with `a_lean_send_holds_its_commit_and_asks_nothing` and
`a_commit_names_its_hold_only_on_a_lean_send` among them (on main 5108240b, after the fix); `go test` 0; `check_design`, `check_names`, `check_links` 0. `crates/raft` is untouched.

## Row plan (after the merge, standing queue)

1. One graft-m2b site day from merged main with test-faults.
2. `bash tests/fanout/reretire.sh tests/fanout/reretire/graft-m2b.txt`, which must print RERETIRE OK.
3. Three B0-33 rows back to back on fresh placements, as m64's: arm A (flag off, LEARNERS as today), arm C (flag off,
   `LEARNERS=`), arm B (`A1_EXTRA=--lean`, `LEARNERS=`). $10 for all three; if three do not fit, A and B only.

**Flip, written before the row (the lead's):** if the leader's confirm p90 does not fall at least 3× in arm B
against arm A, input load is not the cause, and it is recorded under F106 as the platform's, and stopped. C against A
attributes the learners alone; B against C, the edge-side cut.

Per-arm verdict lines come from:
- `python3 tests/fanout/syncab.py TSAVE SITE@FROM@TO`: inputs a second by route, confirm wall p50/p90/p99, ticker
  stalls of 500 ms or more, and held-input batches;
- commit p99 from the a1 row (`.leader.commitMs`, and the human commit stage from the push log);
- agent commands a second from `tests/m0/cuttrace/cuts.py`.

On m64r33, `syncab.py` reproduces `stall-line.md`'s site 0 figures: confirm p50/p90/p99 50/337/1,798 ms, 24 stalls,
and 142 batches holding 25,064 of 32,313 requests.

## The row (2026-11-21, graft-m2b, standing queue)

- **Site day `sab`** from main `c4d5a1333` with test-faults, from 10:46:24Z. A new shard image was built,
  `sha256:3aec7324…`, because `containers/git` changed in `a7db96dc2`. Sites a, b and c exited 0, and edge `e7997554`
  serves at 100% (`tests/fanout/raw/siteday-sab.status`, `tests/am1/raw/runs/fanout-sab-{siteday,edge,image}.log`).
- **Reretire:** `bash tests/fanout/reretire.sh tests/fanout/reretire/graft-m2b.txt` printed **RERETIRE OK graft-m2b**.
  All 266 of 266 were not joined and had the retired row (`tests/fanout/raw/reretire-graft-m2b.log`).
- **Three B0-33 rows**, back to back on fresh placements. Each had 5,000 agents, 300 s and k 4, with m64's template. In
  all three, site 0 led at term 2: 0 leader changes, 0 `TransferStarted`, 0 `TransferAbandoned`.

| Arm | Tag, repo, run | Switches | Load from |
|---|---|---|---|
| A | sabA, 131379, `mwqbz2c4` | flag off, learners `enam:2000,weur:1000,apac:1000` (m64's) | 11:37:06Z |
| C | sabC, 131380, `mwqcdvtq` | flag off, `LEARNERS=` | 11:48:41Z |
| B | sabB2, 131381, `mwqd4xt9` | `A1_EXTRA=--lean`, `LEARNERS=` | 12:09:54Z |

The first arm-B start, tag sabB, refused to run because `rows.sh` needs a clean tree and `syncab.py` was edited.
Nothing ran under it.

## Verdict lines

Site 0, leader, over each arm's 300 s load window (`tests/fanout/syncab.py`; commit stages from the push log; agent
commands from `tests/m0/cuttrace/cuts.py`) [measured]:

| Line | A | C | B |
|---|---|---|---|
| Leader inputs/s | 314.2 | 244.0 | 167.4 |
| `/commit` | 146.0 | 125.1 | 123.0 |
| `/outcome` | 75.7 | 71.1 | 0 |
| `/raft` (of which from learners) | 58.0 (55.8) | 36.9 (0) | 34.5 (0) |
| `/outcome_by_request` | 12.6 | 0.2 | 0.0 |
| `/read` | 3.7 | 2.7 | 2.2 |
| Confirm wall p50 / p90 / p99 ms | 52 / 379 / 2,076 | 52 / 136 / 997 | 58 / **101** / 204 |
| Ticker stalls ≥ 500 ms | 10 (9,122 ms) | 0 | 0 |
| Held-input batches (requests held) | 464 (61,315 of 94,258) | 251 (19,522 of 73,211) | 60 (4,034 of 50,224) |
| Human commit stage p99 ms | 3,589 | 2,029 | 2,290 |
| Merge commit stage p99 ms | 2,728 | 2,358 | 2,129 |
| Leader `commitMs` p99 ms (`/stats`) | 6,387 | 3,648 | 3,776 |
| `queue:slot` p99 ms | 5,798 | 2,983 | 3,150 |
| Agent commands/s (cut) | 103.2 | 94.3 | 88.8 |
| StepDown, all held | 5 (3 LoopLag, 2 CommitErrors) | 7 (2 LoopLag, 5 CommitErrors) | 6 (CommitErrors) |
| Edge 503s | 525 | 5,434 | 7,315 |
| Agent pushes `fetch failed` | 3 (`UND_ERR_INFO`) | 1 (`UND_ERR_INFO`) | 206 (203 `ERR_HTTP2_STREAM_ERROR`) |

Every arm had 0 edge 500s, 0 exceptions, and 0 closure or other-object lines. The followers confirmed at p90 13 to 108
ms in every arm.

## The flip and the ruling

**The flip is positive.** Arm B's confirm p90 fell 3.75× against arm A's (379 to 101 ms), past the 3× line written
before the row. So F106 is not recorded as the platform's. The leader's confirm wall tracks its input count.

**Attribution.** On a log scale, C alone accounts for 77% of the fall in p90 (379 to 136 ms), and B against C for the
rest (136 to 101 ms). In inputs, C removed 70.2 of the 146.8 inputs/s B removed, and the learners' `/raft` (55.8/s)
is most of that. The lead's ruling puts the learners' share of the log-fall at 87%, measured from the learner `/raft`
rows alone. Either way, the learners are the larger lever and outcome polling the smaller.

The lead's ruling (2026-11-21):
- **Next:** a $0 design note on learner AppendEntries cadence (fewer, larger learner `/raft` calls, same learner
  count), and a breakdown of `queue:slot`'s tail. Nothing is built until the lead has read it.
- **Not pursued:** taking outcome polling off the leader. It is the smaller lever (136 to 101 ms), and the 6 s lean
  hold is the likely cause of arm B's 203 HTTP/2 `fetch failed` pushes (`decisions/OPEN-QUESTIONS.md`, A38). The lean
  switch stays test-only.
- No further hosted row on graft-m2b until the note is approved.

**Agent commands/s are confounded.** They fell from 103.2 (A) to 94.3 (C) and 88.8 (B), but C and B also had 10 to
14 times A's edge 503s. Almost all were "tier rows did not answer… every member asked shed the read", on `graphql`,
`rest-repo-write` and `task`. Without learners, the edge's tier reads all land on the two followers, which then took
125 to 137 `/read`/s each, against 1.4 to 1.6 in arm A. The agents that got those 503s pushed less. So the fall in
commands/s measures the agents' reads shed, not a throughput cost of a lighter leader [inference].

**Arm B's `fetch failed` pushes.** 203 of 206 ended in `ERR_HTTP2_STREAM_ERROR`, 111 of them in one second at T+159 s.
They were held p50 5.9 s (p10 4.1, p90 8.0) and 192 had started within T+150 to 170 s. Arms A and C had 3 and 1, all
`UND_ERR_INFO` within 21 ms. They are the shape of a lean `/commit` held up to `LEAN_HOLD_MS` (6 s) on a shared HTTP/2
client connection that the orb's side reset under many long-open streams [inference, A38].

## Close-out and cost

- **Teardown.** Each arm ended with `teardown.py --sweep`, and each sweep exited 0. Arm C's sweep tore down
  `git#2#net:131380` at 11:59:17Z, and arm B's tore down `git#2#net:131381` at 12:18:36Z.
- **A shard that came back after arm B's sweep.** The lead's watchdog found site c's `git#2#net:131381` running at
  about 13:05Z, after my verdict lines had said no shard containers were running. From Workers Logs and the container usage:
  - The sweep's kill reached the edge at 12:18:22Z, and the shard's `/teardown` ran 12:18:22 to 12:19:49Z (wall
    87,849 ms). Three invocations of the object were open then, by their end times and walls: a keep-warm
    `/catchup?warm=1` from the leader begun 12:15:12Z, a second begun 12:16:39Z, and the object's alarm begun
    12:15:33Z, which waits on the catch-up its `warming` key restarts. The kill ended the running catch-up's piece with
    "The container has not been started" at 12:19:19Z, and the first keep-warm call and the alarm ended then too.
  - `/teardown` deletes the alarm but not the `warming` key. A new alarm began at 12:19:49Z, as the teardown returned,
    and restarted the catch-up from that key, which started the container again: site c's instance billed 50 and 34
    CPU-s at 12:22 and 12:23Z after 0.4 to 0.9 a minute. The second keep-warm call ended at 12:23:22Z after 403 s with
    "Too many API requests by single Worker invocation", and the container stayed up, idle at 720 MiB.
  - **So no sweep missed a running container.** Costrow's sweep tore 131381's shards down and its recheck found none
    running at 12:18:36Z. The shard's own alarm started site c's container again about three minutes later, from the
    `warming` key the teardown left [measured, Workers Logs and `containersUsageAdaptiveGroups` by minute; the
    mechanism is inferred from `workers/site/src/shard.rs`, `/teardown`, `Jobs::run` and `alarm`].
  - **Torn down** by `teardown.py --sweep` at 12:37:53Z, after the retire and before the purge. The sweep's recheck
    found no shard container running. At 13:06:48Z the containers API gave site c's instance as `failed`, exit
    137, updated 12:37:52Z (the teardown's kill), and site a's and b's as stopped at 12:16:32 and 12:16:34Z. The
    install had 0 running containers. The watchdog's message reached this thread before 12:37Z, so its "about 13:05Z"
    is approximate. The sweep that answered it is the one that tore the container down.
  - **Cost.** 131381's three shard instances billed 99 CPU-s, 13,332 GiB-s and 22,220 GB-s from 12:18:36Z,
    **$0.04** (`containers.py`, `NETS=131381`). Almost all of it is site c's instance at 720 MiB a minute from 12:23
    to 12:37Z.
  - Not fixed here. The fix would be `/teardown` deleting the `warming` key, a site-script change for a site day. It
    goes to the lead with this file, not into this branch.
- **Retire and purge** (`tests/fanout/raw/retire-sab.log`). All 13 names of 131379 to 131381 were not joined, with
  the retired row, at 12:40Z. The purge took 118,910 objects to 0, and its second pass found 0.
- **Spend:** graft-m2b **$4.77** by `PREFIX` from 10:46:24Z to 12:54Z (`acctcost.py`), and $6.67 for the UTC day. The
  arms were $2.25 (A), $1.14 (C) and $1.06 (B) by their run meters, under the $10 stop. The account's spend over the
  same span was $8.24, which includes other installs.

## Files

- `tests/fanout/raw/{costrow,rows}-sab{A,C,B2}.status`, `costrow-sabB.status` (the refused start),
  `memguard-sab{A,C,B2}.jsonl.xz`, `terms-sab{A,C,B2}.jsonl`, `tsave-sab{A,C,B2}.jsonl.xz`,
  `arrivals-sab{A,C,B2}.jsonl`, `shardgate-sab{A,C,B2}-b0-33-{0,1,2}.jsonl`, `siteday-sab.status`,
  `reretire-graft-m2b.log`, `retire-sab.log`.
- `tests/am1/raw/runs/fanout-sab-{siteday,edge,image}.log` and `a1-b0-33-fsab{A,C,B2}-b033.log`, masked.
- The a1 row and push logs stay local, as m64's did: `tests/am1/raw/a1-b0-33-{mwqbz2c4,mwqcdvtq,mwqd4xt9}.jsonl` and
  `a1-pushes-b0-33-*.jsonl`.
- `tests/fanout/syncab.py` now prints the learners' share of `/raft` (peer 192 and up).
