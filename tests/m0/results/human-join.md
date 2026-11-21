# `human-join`: Human and MergeQueue commands join the newest frozen batch on their own read (2026-11-21)

The lead's code-only request: un-defer option (b) of `decisions/DECISIONS.md`, "B0-29 late join deferred", narrowed to
Human and MergeQueue commands. Branch `human-join` from main `97d91ec0`. No hosted row; the lead routes rows after
review. The decision is `decisions/DECISIONS.md`, "`human-join`"; the design sentence is in `design/DESIGN.md`, "Shape",
the write path, and journey 1, step 3.

## What changed

`crates/raft/src/member/leader.rs`:
- `admit` hands a Human or MergeQueue command to `late_join` before its lane. It joins the newest frozen batch
  (`Deciding::joiners`) unless:
  - that batch was cut at a byte cap (`CutEnd::Soft`, `EntryMax`), or the command would take it past `entry_soft` or
    ENTRY_MAX (`AgentCmds` and `AgentTokens` cuts still take one);
  - an earlier Human or MergeQueue command still waits in a lane;
  - a transfer runs;
  - the leader would still freeze another batch behind it (`read_ahead`);
  - no entry is in flight, when the batch waits only on its own read.
- The batch's joiners share one `DepsWanted` at a time (`join_ask`). The first joiner with a chain asks it at
  admission; joiners admitted while it is out are covered by the next, asked when it answers (`join_read`).
- `decide_held` appends a joiner after the batch's commands only if a read asked no earlier than its own answered for
  every chain it names on the fold after the apply. Otherwise it goes back to the front of its lane with every joiner
  after it (`Lane::push_front`, `crates/raft/src/admit.rs`).
  The entry passes `decide` the newest read of each chain, and `DepsUsed.reads` (`crates/raft/src/member/mod.rs`)
  records which read that was.
- `drop_held_deps`, `end_leadership` and `admitted_bytes` count joiners.

The simulator's dependency check (`crates/raft/tests/common/sim.rs`, `deps_out`) now bounds each request that names a
chain by the read its entry's fold of that chain came from, not by the entry's first read, and counts late joins.

## Tests

`crates/raft/tests/deps.rs`, 17 of 17 pass:
- `a_human_admitted_after_the_freeze_rides_the_frozen_batch_on_its_own_read`: the row's shape. Three agent commands
  frozen behind entry k, with their read answered from before a revocation. A Human admitted after the freeze asks the
  joiners' read, answered revoked. It rides k+1 and is refused there, and the entry's one `Authz` fold is that read, so
  the agents are refused too. **On main it fails**: the Human rides k+2.
- `a_late_joiner_whose_read_is_late_rides_the_next_entry` (replaces
  `a_command_admitted_after_the_early_cut_waits_for_the_next_entry`).
- `an_agent_cmds_capped_batch_takes_a_human_joiner_and_a_soft_capped_one_does_not`: 70 agent commands, cut at
  AGENT_CMDS_ENTRY (64); the Human joins as the 65th and a later agent does not. Then a batch cut `Soft` takes no
  joiner, though the joiner would fit within `entry_soft` beside it.
- `a_joiner_that_gains_a_chain_at_apply_rides_the_next_entry`: entry k moves the dependency's owner. The joiner's read
  was of the old chain, so it rides k+2 on a read of the new one.
- `a_joiner_joins_the_newest_frozen_batch`: two batches frozen on a slow read (`read-ahead`); the Human joins k+2.
- `joiners_admitted_while_a_join_read_is_out_wait_for_the_next`: two Humans, one read out; the second asks nothing
  until it answers, then rides k+2 because the next read is still out at the apply.
- `a_batch_frozen_with_no_entry_in_flight_takes_no_joiner`.

`cargo test --release -p graftyard-raft -p graftyard-identity -p graftyard-git`: 787 passed, 0 failed.

### The BF-15 failure, and the design change it forced

The first build gave each joiner its own read, asked at admission. `cargo test --workspace` then failed
`bf15_revoked_token_with_1000_commands_queued` and `bf15_suspended_account_with_1000_commands_queued`
(`crates/identity/tests/raft.rs`): 1,000 Human commands queued behind one frozen batch read **2,002 chains for 7
entries**, against the test's bound of two chains per entry. Each joiner's own read was a read per command, the cost
BF-15 exists to rule out. The raft crate's own tests had no such bound.

The fix shares one joiner read per batch at a time. A joiner admitted while a read is out is not covered by it, since
that read was asked before its admission, so it waits for the next, asked when the one out answers. A burst then costs
one read per read time. BF-15 passes again, and read-after-admission holds: every joiner's covering read is asked at or
after its admission. The design text and the DECISIONS entry describe this rule, not the first one.

## Plants

`python3 tests/m0carried/plant78.py`: **13 of 13 caught** on the final code (0–11 in one run, 12 after its test was
added).

| # | Mutant | Result |
| --- | --- | --- |
| 0 | main's rule: no command joins a frozen batch | CAUGHT |
| 1 | a joiner asks no read: it rides on the batch's | CAUGHT |
| 2 | a joiner is decided on a read asked before its admission | CAUGHT |
| 3 | a joiner that gains a chain at apply is decided without a read of it | CAUGHT |
| 4 | the entry passes the oldest read of a chain, not the newest | CAUGHT |
| 5 | an AgentCmds-capped batch takes no joiner | CAUGHT |
| 6 | a Soft-capped batch takes a joiner | CAUGHT |
| 7 | agent commands join too | CAUGHT |
| 8 | a joiner joins the oldest frozen batch | CAUGHT |
| 9 | a joiner joins while the leader would still freeze another batch | CAUGHT |
| 10 | a joiner left out of its batch is dropped, not queued again | CAUGHT |
| 11 | every joiner asks its own read, even while one is out | CAUGHT |
| 12 | a joiner joins a batch frozen with no entry in flight | CAUGHT |

Misses along the way:
- On the first build, a mutant that dropped a redundant `waiting` test survived; that test is gone, and mutant 2 plants
  the real mistake.
- Mutant 6's test first cut `Soft` at 20 bytes, so no joiner fit anyway. It now cuts at 40 with a 100-byte second
  command, so a joiner fits by bytes and only `capped` keeps it out.
- Mutant 12 had no test until `a_batch_frozen_with_no_entry_in_flight_takes_no_joiner`.

## Long sweep and Porcupine

On the final code (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`, live half 4,000 seeds): **passed in 1,147 s**.

| Half | Seeds | Ops | Late joins | Longest healthy-quorum stall (seed) | Porcupine (`go run . -seg 2000`) |
| --- | --- | --- | --- | --- | --- |
| HARSH | 2,000 | 151,356 | 781 | 15,790 ms (11523) | Ok, 147,281 operations |
| GRAY | 2,000 | 2,358,169 | 43,045 | 24,763 ms (10789) | Ok, 2,358,169 operations |
| live | 4,000 | 6,379,871 | 10,553 | 24,481 ms (12672) | Ok, 5,428,302 operations |

- Every stall is under the 30 s bound. Recent sweeps on main had gray 27.2–28.5 s and live 28.3 s
  (`tests/am1/results/narrow-reads-3.md`, `tests/am1/results/round-time.md`).
- The simulator's read-after-admission check passed for every late join: each request naming a chain was admitted at
  or before the read its entry's fold of that chain came from.
- The first build's sweep, one read per joiner, also passed: 2,669 / 119,488 / 59,688 late joins, stalls 15.8 / 26.8
  / 28.5 s, and Porcupine Ok on all three. It was superseded by the BF-15 change below.

## Replay prediction

`tests/m0/cuttrace/humanjoin.py` replays each leader trace:
- From site 0's cuts, appends and applies, as `slotsplit.py` reads them.
- Each cut's Human and MergeQueue commands are spread evenly since the cut before [inference; the trace has no
  per-command class].
- A command that arrived while the cut before its own was frozen and unappended rides that batch if a joiners' read
  asked at or after its arrival lands by the append. Reads are one at a time, as in the code. Batches cut `Soft` or
  `EntryMax`, or cut with no entry in flight, take none, and once one joiner misses, the later ones do too.
- Appends are held fixed: the code never lets a joiner delay one.

At a 75 ms read [inference]:

| Row | Window | Human p50 | Human p99 | Within 1 s | Join share | All-command p99 |
| --- | --- | --- | --- | --- | --- | --- |
| B0-33 sabA | 11:37:06Z, 300 s | 863 → 617 | 3,863 → 3,278 | 61 → 81% | 53% | 3,799 → 3,761 |
| B0-33 sabC | 11:48:41Z, 300 s | 989 → 875 | 2,423 → 2,325 | 51 → 64% | 16% | 2,488 → 2,480 |
| B0-33 sabB2 | 12:09:54Z, 300 s | 1,007 → 900 | 2,352 → 2,240 | 49 → 60% | 17% | 2,265 → 2,245 |
| B0-29 m3fb29 | 00:24:05Z, 600 s | 448 → 404 | 1,128 → 778 | 98 → 100% | 27% | 1,130 → 1,066 |
| B0-29 m3fb2r2 | 01:55:15Z, 600 s | 352 → 344 | 653 → 593 | 100 → 100% | 5% | 714 → 688 |
| B0-29 m3fb2r3 | 02:16:32Z, 600 s | 458 → 417 | 1,131 → 958 | 99 → 99% | 23% | 1,289 → 1,261 |

With a read per joiner, the first build's rule, the same replay gave sabA / sabC / sabB2 Human p99 3,278 / 2,210 /
2,055 and B0-29 m3fb2r3 889. Sharing the read costs 0–185 ms at p99 and 5–15 ms at p50.

- **Against Fable 5.2's replay** of sync-ab (thread T-01a1f697): Fable used a read per joiner. Its p50 (596 / 863 /
  886) is 12–21 ms under these, and its p99 (3,207 / 2,084 / 2,134) is 71–241 ms under. Against this script's own
  read-per-joiner variant, p50 is within 6 ms.
- **B0-29 is a weaker lever.** Every B0-29 cut ended `Empty`, so frozen batches are short, and at 75 ms only 5–27% of
  Human commands join (at 0 ms, 31–79%).
- **B0-29's line is all-command `/stats` commit p99**, and about 84% of the row's commands are Agent, which do not
  join. The predicted move is 1,130 → 1,066, 714 → 688 and 1,289 → 1,261: about 25–65 ms. The median run would read
  about 1,040 against the 800 ms line, still over the 1 s fail line.
- **Human p99 on B0-29** falls 60–350 ms (1,128 → 778 on run 1).
- So `human-join` does not pass B0-29 alone [inference]. The B0-29 tail is the agent backlog behind frozen batches
  (`tests/am1/results/b029-m3f.md`, "The queue tail").

## Checks

On the final tree, all exit 0:
- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- wasm32 clippy for site, edge and ops, and for site and edge with `test-faults` (ops has no `test-faults` feature),
  plus front and runners;
- `cargo test --workspace`: 1,347 passed, 0 failed;
- `go test ./...` in `containers/git/server` and `tests/m0/porcupine`;
- `scripts/check_design.py`, `check_names.py`, `check_links.py`;
- `node --test tests/am1/rest/h1.test.mjs`.

The first full run had `cargo test --workspace` fail BF-15 (above). A second failure was environmental, the orb's disk
filling during the build, and was cleared before the rerun.
