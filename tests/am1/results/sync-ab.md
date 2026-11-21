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

**Full checks** on this branch: fmt 0; workspace clippy 0; wasm32 clippy `-D warnings` site 0 and 0 with
`test-faults`, edge 0 and 0, ops 0 (no `test-faults` feature, 101 with it as before); `cargo test --workspace`
1,337 passed, 0 failed; `go test` 0; `check_design`, `check_names`, `check_links` 0. `crates/raft` is untouched.

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
