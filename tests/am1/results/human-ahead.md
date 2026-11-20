# human-ahead (2026-11-20, code only): the authz-source counter, and the read-ahead cap held for a decision

The orchestrator (T-01a17bda) asked for one branch with two parts:
- the authz-source counter on `commit_issue_rid`;
- a cap on read-ahead: when the queue holds a Human or MergeQueue command, cut so that at most one frozen batch is
  ahead of it. Its expected effect on m63r33 was 2.5 s to about 1.7 s, and 1.7 s to about 1.0 s.

The counter is built. The cap is not: on m63r33's trace, the rule as written moves 24 of 340 Human cuts, and its
expected effect needs a stricter rule. That needs the orchestrator's decision.

## The authz-source counter (built)

`workers/edge/src/api.rs`, test-faults only: after an issue command's commit, the edge logs one Workers Logs line
`{"authzSource": "dep" | "render" | "fallback", "rid": ...}`.
- `dep`: the member filled the owner's `Authz` from the leader's dependency read.
- `render`: the edge's own render carried it (a later send, or an `outcome_by_request`).
- `fallback`: a member that names no source; the edge then asks `outcome_by_request` with the whole render.
- A refusal or a merge logs nothing.

The pure rule is `authz_source`. Its test is `a_committed_issue_reply_names_its_authz_source`. Plants:
`tests/m0carried/plant69.py`, 3 of 3 caught:

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | a member that names no source is not counted as the fallback | CAUGHT |
| 1 | every named source counts as dep | CAUGHT |
| 2 | a refusal or merge counts as the fallback | CAUGHT |

The row's verdict line reads the counts with a Workers Logs query on `authzSource` per value.

## The read-ahead cap (not built)

Replay: `tests/fanout/aheadreplay.py` on `tests/fanout/raw/tsave-m63r33.jsonl.xz`. For each cut that took a Human or
MergeQueue command, it matches the cut to its entry (the last appended index, plus the frozen batches held before the
cut, plus the entry in flight). It then moves the command K batches nearer, as if those batches had not been cut
ahead of it, and reads that entry's apply time.

| Frozen batches ahead, at most | Human/MQ cuts moved | cut to apply p50 | p90 | p99 |
| --- | --- | --- | --- | --- |
| main (up to READ_AHEAD, 3) | 0 of 340 | 1,391 ms | 3,398 | 4,573 |
| 1 (the rule asked for) | 24 | 1,355 | 3,165 | 4,322 |
| 0 | 231 | 1,000 | 2,548 | 3,997 |

Why the rule asked for barely moves:
- At most one frozen batch was ahead of all but 26 of m63r33's 343 Human/MQ cuts (site a: 61 with one ahead, none
  with two; site b: 146 with one, 26 with two). Only those 26 are capped, and 24 of them match an entry in the trace.
- The "1.7 s to about 1.0 s" step is a cut with one frozen batch ahead becoming a cut with none. "At most one" leaves
  it where it is.
- The 1.0 s floor is the entry in flight plus the command's own round: with no frozen batch ahead, cut to own apply
  was p50 1,000 ms on both sites.

What "none ahead" would mean in code: while a Human or MergeQueue command is queued, `cut_early` cuts no batch ahead
of the entry in flight. The next batch is then cut at apply, not early, so its dependency reads (m63r33 `/stats`:
`deps:all` p50 471, p99 2,323 ms against round p50 477) no longer overlap the round. That trades away B0-31's
read-ahead (`read-ahead`, `decisions/DECISIONS.md`) whenever a Human command waits. On m63r33 a Human command was
queued at most cuts (Human taken in 343 of 402 cuts), so under that load it is close to switching read-ahead off.
Each entry would then take a round plus a read: about 470 ms more a cycle at p50, which is about the cut rate agent
throughput rides on. The orchestrator's 10% agent ok/s flip would likely fire [inference, not replayed].

A narrower form keeps read-ahead for agent batches. If a Human or MergeQueue command arrives while a frozen batch
holds no Human command, the cut puts the Human command into a batch frozen ahead. That is B0-29's late join (option
(b)), which the orchestrator rejected.

### Options for the orchestrator

1. **Build "at most one" as asked.** Expected effect on m63r33: Human cut to apply p50 1,391 to 1,355 ms, p99 4,573
   to 4,322. That is within the noise of one row, so a row cannot show it worked.
2. **Build "none ahead while a Human or MergeQueue command waits."** Expected p50 1,391 to 1,000 ms, p99 4,573 to
   3,997, but at the price of read-ahead under mixed load. Before a row, it needs an in-memory load test of agent
   throughput against main.
3. **Build neither,** and take Human p99 as two loaded rounds plus a median round. That is the re-based B0-33 line
   FRAME names.

## Files

- `workers/edge/src/api.rs`: `authz_source`, the test-faults log line, and its test.
- `tests/m0carried/plant69.py`: the counter's plants.
- `tests/fanout/aheadreplay.py`: the replay above.

## Checks

Each by its exit code, on the branch from main `b5d9c23e`:
- `cargo fmt --all -- --check` 0; `cargo clippy --workspace --all-targets -- -D warnings` 0;
- wasm32 clippy `-D warnings`: site 0 and 0 with `test-faults`; edge 0 and 0; ops 0, and 101 with `test-faults`, which
  ops does not have (expected);
- `cargo test --workspace --no-fail-fast` 0: 1,328 passed, 0 failed;
- `go test ./...` in `containers/git/server` 0;
- `scripts/check_design.py` 0, `check_names.py` 0, `check_links.py` 0 (with `.worktrees` moved aside).

No `crates/raft` change, so no long sweep or Porcupine. No DESIGN or DECISIONS edit: the counter is test-faults
tracing, and the cap is not built.
