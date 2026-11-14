# Admission prices what the log stores (2026-11-14)

The operator's item from LM-1's round 15 (m26c, main `2456b8f`): admission priced every agent command at about
10.8 KB while the leader's entries stored about 1.9 KB for 7.6 commands, so B0-33's capacity came to 30 to 47 agent
commands/s against its 140 line. Branch `admission-stored-bytes`, off main `2456b8f`.

## Verdict

**The admission gate passes** [measured, simulator]. B0-33's mix, with a 10.3 KB render on every side command as the
hosted edge sends it, admits 252.4 and 249.7 agent commands/s at round p50 225 and 315 ms, with none refused. On main's
pricing the same test admits 23.0/s at capacity 46/s and refuses 6,220, and fails. ENTRY_SOFT still bounds the bytes an
entry stores, and COMMAND_MAX and ENTRY_MAX still bound the whole encoded command in flight.

## The cause

`cut` and the moving mean `cmd_bytes` in `Admission` (`crates/raft/src/member/leader.rs`) priced `cmd.encoded_len()`.
A repository's `IssueCall` carries its `render`: the `HostedDir` its leader renders the reply from, which holds the
owner's whole AuthzFold as hex. `RepoCmd::encode` sends it and no event stores it. LM-1's round 15b measured with
the real codec over m3org's fold:

| Command | Priced (encoded) | Render | Rest (what reaches the log) |
|---|---|---|---|
| comment (gql) | 10,973 | 10,426 (authz hex 9,974) | 551 |
| label add | 10,806 | 10,272 | 538 |
| task edit | 10,888 | 10,277 | 615 |
| lease renew | 10,820 | 10,262 | 562 |
| lease acquire | 10,892 | 10,261 | 635 |
| agent push (no pack) | about 405 | none | about 405 |

So about 92% of an issue command's price was a reply render the log never keeps. `MergeCall` already leaves its
render out of `encode`.

## The change

- `Command::stored_len` (`crates/raft/src/fold.rs`), defaulting to `encoded_len`, so other folds don't change.
- Admission (`admit` in `leader.rs`) prices `stored_len()` less any inline agent pack, for both `cmd_bytes` (and so
  `capacity`) and the ENTRY_SOFT cut. COMMAND_MAX and ENTRY_MAX still count `encoded_len`.
- `RepoCmd::stored_len` (`crates/git/src/repo.rs`): an issue command is its encoded length less its render's JSON
  (plus the 4 bytes `null` would take). Every other command is priced whole.
- The agent block line in `design/DESIGN.md` and `design/AGENT-LAYER.md` now says entries are cut by stored bytes
  (no reply render). `decisions/DECISIONS.md` has the entry.

## Tests and plants

- `b0_33_with_hosted_render_bytes_admits_140_agent_commands_per_second` (`crates/raft/tests/admission.rs`). The test
  model's `KvCmd::Write` gains a `render` that `stored_len` leaves out; `load.rs` gains `Op.render`, `Load.slow_ms`
  (added to each one-way delay) and a per-entry round measure.

  | One-way + | Round p50 | Capacity | Agent cmds/s admitted | Refused | Human p99 | Largest entry (stored, packs included) |
  |---|---|---|---|---|---|---|
  | 50 ms | 225 ms | 889/s | 252.4 | 0 | 435 ms | 90,995 B |
  | 90 ms | 315 ms | 633/s | 249.7 | 0 | 620 ms | 118,819 B |
  | main's pricing | | 46/s | 23.0 | 6,220 | | |

  No entry stores more than ENTRY_SOFT plus one command past its agent packs.
- `an_issue_commands_render_is_not_priced` (`crates/git/tests/agent_bytes.rs`).
- `tests/m0carried/plant12.py`, 4 of 4 caught: admission prices the encoded command; the cut ignores ENTRY_SOFT; an
  issue command is priced with its render; every command is priced 1.

## Runs

- **Full checks:** pass. fmt; workspace clippy; wasm32 clippy for site and edge with and without `test-faults`, and
  ops; `cargo test --workspace` 858 tests, 78 suites, none failing; `go test` in `containers/git/server`; and the three
  `scripts/check_*.py`.
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=...`): passes in 1,346 s. Longest stretch with a
  healthy quorum, requests waiting and none completing: HARSH 15.7 s (seed 11431), GRAY 25.2 s (seed 10665), live
  24.7 s (seed 11833, 4,000 seeds from 10000), the same as main `2456b8f` (`tests/m0/results/slow-storage.md`), each
  under the absolute 30 s bound.
- **Porcupine** (`go run . -seg 2000`): Ok on all three histories, 156,325, 2,230,496 and 6,087,300 operations.
- **Plants** (`python3 tests/m0carried/plant12.py`): 4 of 4 caught. A first sweep and checks run were discarded
  because they compiled while mutant 0 was planted in the same worktree; both reran on the clean tree.

## Open, a second change

The render still crosses every AppendEntries: each issue command sends its owner's whole fold, about 10 KB, to every
follower, so B0-33's 7.6-command entries carry about 80 KB of renders against 1.9 KB stored. The leader holds the same
fold as a dependency (`IssueCall::deps` names the owner's `Authz` chain), so it could build `HostedDir.authz` itself
and the edge could send none. That would also cut the far follower's per-call bytes, a candidate for the far-leg p99
[inference].
