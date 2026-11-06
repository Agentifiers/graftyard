# AM1-WP1: base agent load changes (pure half)

These results are from 2026-11-06, on branch `am1-wp1` from `main` at `64530ea`. Nothing was deployed. AM1-WP1 has no gate row of its own: it carries LM-1's rerun of B0-29 to B0-35 on real code (`build/BUILD-PLAN.md`, AM1-WP1). This is the in-memory half of that rerun. B0-29, B0-31, B0-32, and B0-33 ran on three real `graftyard_raft` members with admission. B0-30 ran on M3-WP5's install bed. B0-34 and B0-35 ran on the pure functions the hosts call. The hosted reruns come later.

| Row | In-memory result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-29 | 60 s of journey 9's mix, 1,667 commands at 27.6/s, enam leader: commit p50 165 ms, p99 245 ms; merge p99 215 ms; 0 `Stale` on distinct refs; 0 refused; 0 leader changes. 12 of 12 1 MiB agent pushes staged, and 1 of 232 40 KB pushes, by `AGENT_INLINE_ENTRY`. At most 118,349 agent inline bytes in an entry [measured, `crates/raft/tests/admission.rs`] | Commit p99 at most 600 ms, merge p99 at most 2 s, 0 `Stale` | Pass on the modeled network |
| B0-30 | 720 hourly entries (30 days), with the cap scaled to 6,000 bytes. 679 ranges archived to two buckets. Peak stored log on any member was 6,139 bytes, against a cap of 6,000 plus one entry of 144. 680 archived entries read back with an intact digest chain. A follower offline for all 30 days rejoined by `InstallSnapshot` from a snapshot plus the archives, with the same fold digests [measured, `crates/fold/tests/install.rs`] | Member log under the cap, archived entries readable | Pass (scaled cap; no SQLite) |
| B0-31 | apac leader, every agent push at 1 MiB. Buckets up: commit p99 935 ms, 148 of 148 agent pushes staged, 0 leader changes. Placement failing: Human and MergeQueue p99 770 ms, with 0 of 35 refused (1 MiB pushes kept inline). 140 of 140 agent pushes over `INLINE_MAX_HOT` refused at once with a retry hint, including 100 KiB ones that only the outage rule refuses. 0 leader changes [measured] | p99 at most 1 s buckets up. While failing: Human and MergeQueue p99 at most 3 s, 0 of them refused, agent pushes over `INLINE_MAX_HOT` refused at once | Pass |
| B0-32 | 500 agent pushes of 256 KiB in 1 s on a hot group, with 5 queue merges: 500 of 500 acknowledged within 5 s (client p99 715 ms, 500 ms of it the modeled stage). 0 refused. Each merge in the next entry (110 to 130 ms). Queued agent bytes peaked at 1,924, because every pack was staged. 0 leader changes [measured] | Every push within 5 s or `ng`, every merge in one of the next 2 entries, 0 leader changes | Pass |
| B0-33 | Capacity pinned at 300 commands/s, as M0's `cap300r1`, for 60 s. 252 agent commands/s offered, 150.0/s admitted (the bucket's share). 5,027 refused, every one an agent's, at once at `AGENT_QUEUE_MAX`, with a retry hint (the queue peaked at exactly 1,000). Human p99 225 ms, MergeQueue p99 225 ms (max 225 ms). The user token was classed Agent on 40 of 40 writes after its 60th. 0 leader changes. At measured capacity (17,130/s in this model), 252.6/s admitted and 0 refused [measured] | Human and MergeQueue p99 at most 1 s, at least 140 agent commands/s, user token Agent, refusals in the shape, 0 leader changes | Pass |
| B0-34 | `fanout` each minute for 180 min from two warm sites' `NetworkLoad`, with `pool_grants` holding owners to plan shares (6, 2, 2, as M0's run). The monorepo's k went 1, 3, 7 by 20 min and held 7. Max k 7, set by the monorepo owner's share of 6 pool shards (k = 8 needs all 7 of the pool). It was never reduced, through a site kill, a second owner's network, and three hostile networks at 30 vCPU each. Every owner stayed within its share at every minute [measured, `crates/git/tests/agent_load.rs`] | k from `NetworkLoad` and the pool, at most 8, hostile owner held, no API call | Pass (CPU model, not containers) |
| B0-35 | 600 s: head pushes at 4.17/s (each with its dispatch), GETs at 100/s, lists at 10/s, and merge cost drawn from M0's torvalds/linux spread. Test-merge vCPU averaged 0.527 against a 1.00 vCPU budget (k = 1). 2,500 push merges, 0 shed. 8,691 GET computes, and 51,309 GETs answered null. 0 list computes. Dispatch merge p99 300 ms. Forced over budget: 500 of 500 pushes served, GETs null 11,295 to 705, use 1.00 of 1.00 vCPU [measured] | Within `TEST_MERGE_BUDGET`, dispatch p99 at most 10 s, 0 list computes | Pass (CPU model, not git) |

What the in-memory rows do not show: the network is a model (one-way delays of 40 to 110 ms plus 0.3 ms per KB, from B0-01's echo). Storage confirms at once, so the leader-storage stalls that set M0's B0-29 tail and `STEP_DOWN` are absent. So the latency numbers measure admission's queueing and entry sizes on top of that model, not Cloudflare. LM-1's hosted rerun decides the gates.

## What was built

- **`crates/raft/src/admit.rs`** defines `AdmitClass` and the design's constants: `AGENT_SHARE`, `AGENT_FLOOR`, `DEMOTE_PER_MIN`, `AGENT_WAIT`, `AGENT_QUEUE_MAX`, `INLINE_MAX_HOT`, `AGENT_INLINE_ENTRY`, and `HOT_PUSHES`. It also defines `Admit` (what a host sends beside a command), `AdmitError::{Full, Stage}`, per-actor fair-queued `Lane`s, and `retry_after_s`.
- **The leader** (`crates/raft/src/member/leader.rs`) has one lane per class.
  - **Entry order.** Each entry is cut as Human and MergeQueue, then Agent up to `AGENT_FLOOR` x capacity over the time since the last cut, then Bot, then Agent while the `AGENT_SHARE` x capacity token bucket holds.
  - **Capacity.** It is commands per entry at the moving byte mix times entries/s at the moving commit round. `set_admit_capacity` pins it.
  - **Admission order.** It checks the outage refusal, then `AGENT_QUEUE_MAX` before any staging, then staging. A command is staged when it is over `INLINE_MAX_HOT` on a hot group, when it is an agent pack under pressure, or when it would take queued agent inline bytes past `AGENT_INLINE_ENTRY`.
  - **`AGENT_WAIT`.** Expiry on `tick` and on admit answers `CommitError::Full`.
  - **`Member::commit` is unchanged.** It is Human with no actor: never demoted, staged, or refused, and FIFO as before. `Member::commit_as(now, id, cmd, Admit)` is the new entry point.
- **The group fold** (`GroupFold`, `crates/raft/src/fold.rs`) keeps the last minute of committed writes from `Fold::write_mark`: per-actor counts for `DEMOTE_PER_MIN`, and pushes for `hot`. A new leader reads both from what committed. `Member::restore` rebuilds the window from the stored log's applied entries. The default `write_mark` is `None`, so existing folds never demote and never turn hot.
- **Refusal shapes.**
  - `smart::Refusal::agent_full(n)`: HTTP 200, `ng <ref> agent admission full, retry after <n>s` on every ref, and a `remote:` line.
  - `Reply::secondary_rate_limit(n)`: 429, `retry-after: n`, and GitHub's secondary rate limit message.
  - `PushOutcome::Full` carries the refusal over the member boundary. The edge's git push path and REST ref path render it, and the site host's `outcome_json` gives `{"full": {"retry_after_s": n}}`.
- **`crates/git/src/shard.rs`**:
  - `NetworkLoad`, `Fanout`, and `fanout`: up a step after 3 min above 0.75, or to `ceil(k x cpu / 0.75)`; down a step after 1 h below 0.4; at most `FANOUT_MAX`; judged on the larger of the two warm sites' loads.
  - `pool_grants`: each owner is held to its plan's share. Networks keep what they hold first, so another owner never reduces them.
  - `clone_cache_buckets`: both warm sites for a hot network.
- **`crates/git/src/prsync.rs`**:
  - `MergeBudget` keeps `TEST_MERGE_BUDGET` of the k shards' vCPU. Pushes and dispatch may run one merge into deficit. GETs coalesce per PR per 30 s and keep half a second's budget in reserve. Lists never compute.
  - `commands` takes the shed merges as `unmerged`, so a shed head's old merge ref is deleted and `mergeable` reads null (GraphQL `UNKNOWN`).
  - The site consumer (`workers/site/src/prsync.rs`) applies the budget at k = 1 per isolate.
- **`crates/fold/src/snapshot.rs`**: `LOG_MAX_BYTES`, `LOG_KEEP` (30 days), and `archive_due`, the next range to archive. It is chosen by age and then by bytes, only committed entries, and only once a snapshot exists.

`R2Key::InlineIdx` and `R2Key::Archive` already existed (M1, M3-WP5). The edge's bed route already archives to two buckets and the site host already truncates only after HEADing two (M3-WP5). This package adds the rule that decides when.

## Planted bugs

`tests/am1/wp1/plant.py` plants each mutant, runs the tests that gate it, and restores the file byte for byte. 23 of 23 were caught:

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | Agent cut before Human and MergeQueue (agents starve humans) | `humans_and_the_merge_queue_are_never_behind_agent_commands` |
| 1 | No `AGENT_FLOOR` before Bot | `a_bot_flood_cannot_take_the_agent_floor` |
| 2 | Agent bucket never empties | B0-33 pinned, `agent_wait_answers_full_after_thirty_seconds` |
| 3 | `DEMOTE_PER_MIN` ignores the fold (leader memory only) | B0-33 pinned, `demotion_counts_live_in_the_fold_...` |
| 4 | The fold marks no writes | B0-33 pinned, demotion, `hot_is_read_from_the_log_...` |
| 5 | Outage refusal off | B0-31 |
| 6 | Outage refusal applied to Human pushes | B0-31 |
| 7 | `AGENT_QUEUE_MAX` not enforced | B0-33 pinned |
| 8 | `AGENT_WAIT` never expires | `agent_wait_answers_full_after_thirty_seconds` |
| 9 | `hot` not from the log | `hot_is_read_from_the_log_so_a_new_leader_stages_at_once` |
| 10 | `AGENT_INLINE_ENTRY` not applied | B0-29, B0-31 |
| 11 | 429 without `retry-after` | `secondary_rate_limit_is_a_429_with_retry_after` |
| 12 | 429 with the primary rate limit message | same |
| 13 | `ng` without the retry hint | `agent_admission_refuses_in_the_ng_shape_with_a_remote_line` |
| 14 | `mergeable` computed past the budget | `a_head_push_past_the_test_merge_budget_leaves_mergeable_null` |
| 15 | GETs ignore the budget | B0-35, `over_budget_pushes_go_first_and_gets_are_null` |
| 16 | List fields compute | B0-35 |
| 17 | GETs not coalesced per PR | B0-35 |
| 18 | `fanout` past `FANOUT_MAX` | `fanout_never_passes_fanout_max` |
| 19 | `fanout` from one site's load | `fanout_steps_up_after_three_minutes_above_and_down_after_an_hour_below` |
| 20 | Owner share not enforced | `an_owner_is_held_to_its_share_and_cannot_reduce_another_owners_network` |
| 21 | Another owner can reduce a held network | same |
| 22 | `LOG_MAX_BYTES` ignored (30-day rule only) | B0-30 |

## Raft sweep and Porcupine

Group commit changed, so the long sweep ran: `RAFT_HISTORY=/tmp/am1wp1sweep/hist.jsonl RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -p graftyard-raft --release --test sim -- --ignored --nocapture` passed in 341.85 s.

- **Fault half:** 2,000 seeds and 20,109,327 steps. 49,655 writes acknowledged, 226,062 crashes (97,860 mid-input), 31,225 partitions, and 3,525 leaders.
- **Gray half:** 2,000 seeds and 20,082,389 steps. 951,337 acknowledged, 17,109 gray faults, and 1,534 step-downs.

The sim commits through `Member::commit`, so it exercises the admission path at its default: one Human lane. Its `CommitError::Full` arm panics, and it never fired.

`go build` in `tests/m0/porcupine`, then:

- `hist.jsonl`: `{"operations":156742,"result":"Ok"}`
- `hist.jsonl.gray`: `{"operations":2226521,"result":"Ok"}`

## Checks

- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy of `workers/site` and `workers/edge`, with and without `--features test-faults`: clean.
- `cargo test --workspace`: 396 passed, 0 failed.
- `scripts/check_design.py`, `check_names.py`, and `check_links.py`: pass.

## Assumptions, and what the design leaves open

- **`DEMOTE_PER_MIN` counts writes the leader refused.** It counts committed writes from the fold, plus the leader's unresolved ones, plus writes it refused in the last minute. Without the refused ones, a demoted token whose writes are refused falls back under 60 committed and is classed Human again. The refused count is in leader memory only [inference].
- **The fold's admission window is derived and not in the host's tables.** A restored member rebuilds it from the applied entries in its stored window. A window shorter than a minute undercounts until a minute passes [inference].
- **Entry times are each leader's clock.** The window counts by entry `at`, so across a leader change it counts approximately [inference].
- **Capacity measurement.** Capacity is `ENTRY_MAX` over the moving command size, times 1,000 over the moving commit round (prior 250 ms, floor 50 ms). This is the M0 prototype's rule without its least-squares fit [guess].
- **Owner pool shares.** `pool_grants` takes each owner's share as a function. The design says "its plan's share" and defines no plan field, so the tests use M0's 6, 2, 2.
- **The test-merge budget is per isolate in the site consumer, at k = 1,** until `fanout` is wired to the consumer and the shard objects.

## Not built

- **Host wiring of admission.** The edge does not yet call `commit_as`. Nothing classes commands (that needs `AgentApp`, trailers, and `AgentMeta`, AM1-WP2), and `Admit` does not cross the member route. Hosts call `commit`, so default behavior is unchanged.
- **The archive alarm.** The leader alarm that calls `archive_due` and runs promote, place, `Archived`, and two-bucket put is not built. The bed route does those steps on request.
- **The rest of `fanout`.** The network leader's per-minute `fanout` commit, the shard's `NetworkLoad` report, and clone-cache writes to the second warm bucket are not built.
