# AM1-WP4 results: leases, `PathSet`, and the activity feed (the in-memory half)

These results are from 2026-11-08, on branch `am1-wp4` (worktree `.worktrees/am1-wp4`), cut from `main` at `b767933`. Nothing was deployed, pushed, or merged. AM1-WP4's own gates are LF-3 and LF-5. The package also carries LM-1's rerun of L0-12, L0-13, and L0-16, and reruns LF-12 and LF-13 on real code (`build/BUILD-PLAN.md`, AM1-WP4).

This is the in-memory half. The repository gates ran on three real `graftyard_raft::Member<AuthzCmd>` of the install's `Authz(Genesis.account)` chain and three real `Member<RepoCmd>` per repository, using AM1's harness (`crates/git/tests/common/mod.rs`). Each repository leader read the install chain through `fresh` once per entry, and read the owner's `Authz` as a `dep`. The feed's serving rules (LF-12, LF-13) ran as pure models of the code that a serving member and the edge run. The hosted rerun comes later; "What the hosted half needs" lists it.

## Gates

| Row | What ran | Pass line | Result |
| --- | --- | --- | --- |
| LF-3 | A cursor encoding another group (`repo:8`) with `wait=40`, through `graftyard_api::leases::parse_feed`, `graftyard_git::feed::start`, and `leases::activity`. A cursor older than the ring and a malformed one. 33 `paths`, then 32 (`crates/api/tests/lease_api.rs`, `lf3_foreign_cursor_long_wait_and_too_many_paths`) | 410 with `oldest_cursor`, `wait` clamped to 25, 422 for the 33 paths | Pass (in memory). 410 with `oldest_cursor` (the encoded `{repo:7, oldest}`), `wait` 25 s, 422 on `paths` for 33, and 32 accepted. Without `contents: read` the feed is 404 |
| LF-5 | Five draft PRs opened by a writer, through the real members, with bodies holding a valid block, a repeated key, a bad glob (`/etc/passwd`), a second block after an invalid first one, and a block over 4 KiB. Each `OpenPr` was marked 32.14 as the edge marks it (`lease::marks`) (`crates/git/tests/lease_gates.rs`, `lf5_plan_blocks_make_one_lease_and_leave_bodies_alone`) | One lease from the valid block. None from the others, and the body unchanged | Pass (in memory). Exactly 1 live lease: Shared, from the valid block's PR, with its two globs and intent. 5 of 5 bodies are stored byte for byte in the metadata `issue` row. Body edits: a new valid block replaces the editor's plan lease, an invalid block changes nothing, and a body with no block releases it. Marking the PR ready ends the plan lease with no command on the lease |
| L0-13 (lease limits) | 100 of each bad acquire, each in one entry: Exclusive `**`, `*`, `*/x`, an Exclusive with no task (a user token), one on a cancelled task, a second Exclusive for one task, an App token with no session, an App token naming another App's task, and a reader. Then an `Agent` naming a ref outside its prefix. 600 plan blocks from 120 accounts with only `Read`, then 600 from 120 writers. A holder whose `Write` grant drops to `Read`. `CLAUDE` uninstalled with 5 tasks and 7 leases held through it. An uninstall and reinstall of a holder whose claim names no task here. An Exclusive lease renewed every 50 minutes for 5 h, probed by another task's push after each renew (`l0_13_*`) | 100 of 100 refused for each bad acquire. Zero plan leases from non-writers and at most 500 from writers. The removed holder's lease refuses nothing at the next `decide`. Uninstall releases every lease and cancels every task. The lease reads as Shared after 4 h | Pass (in memory). 100/100 refused for each of the 9 bad acquires, each with its own refusal (`Literal` ×3, `NoLiveTask` ×2, `OnePerTask`, `NoSession` ×2, `NoPush`), and 0 written. The out-of-prefix ref was refused `RefScope`. Plan blocks: 0 leases and 0 rows from the non-writers, and 500 from the writers (`PLAN_LEASES_PER_REPO`), with none over `LEASES_PER_HOLDER`. All 1,200 PRs opened. The removed holder's lease no longer refused the next agent push. Uninstall: 7 to 0 leases held, and 5 of 5 tasks read `cancelled`. A reinstall revived none, and none for the claim with no task. Modes by minutes since grant: 50, 100, 150, 200 Exclusive (push refused `Leased`); 250 and 300 Shared (push accepted) |
| L0-12 (leases and the feed), the pure parts | An Exclusive lease on `pkg/auth/**` held by Graftyard-run task 0. For each of five actor kinds, 100 pushes and 100 REST ref writes (the same `Push` with no pack), each touching `pkg/auth/session.rs`, crossed the member wire in 32.14. The run happened first under `Advisory`, then under `AgentsExclusive` (`l0_12_agents_exclusive_refuses_only_agents`). An idle 60 s Exclusive lease, then 10 minutes later an overlapping Exclusive acquire (`l0_12_a_late_exclusive_acquire_after_an_idle_lease_succeeds`). A 30-minute stall: the leader appended an entry whose AppendEntries were dropped, time moved 30 minutes, the leader ticked, and the group then settled (`l0_12_a_stall_moves_every_live_lease_by_its_length`) | 100 of 100 refused for each of the two `ActorKind::Agent` kinds, 0 of 100 for the other three. The late Exclusive acquire succeeds. Every lease live before the stall is live after it, with `expires` moved by the recorded stall | Pass (in memory). `AgentsExclusive`: Graftyard-run App 100/100 pushes and 100/100 REST writes refused; confined vendor App 100/100 and 100/100; flagged App without `confine` 0 and 0; `Graftyard MCP` 0 and 0; human user token 0 and 0. `Advisory`: 0 for all. The refusal text names the glob, `app 4`, the task id, and the expiry, and in a two-ref push the other ref passes (per ref in 32.14). A truncated `pkg/` directory is refused too. The late acquire succeeded; while the first lease was live, the same acquire had been 409 `Conflict` naming it. Stall: the leader recorded `stalled_since` at its unacknowledged entry. Exactly one later entry carried `ExtendLeases { by: 1,801,001 ms }`. 3 of 3 leases were live after it, with `expires` moved by exactly `by`, and `stalled_since` cleared on all three members |
| L0-16 (core tier alone) | The leader's `decide` for 5 lease-path commands (a conflicting agent push, a Shared acquire, a conflicting Exclusive acquire, a renew, a release), first on the full fold, then on the same core tier with a metadata tier that is not `loaded` and counts every read (`l0_16_lease_checks_read_the_core_tier_alone`) | Every check passes or refuses with zero metadata loads | Pass (in memory). 5 of 5 decisions were identical, with 0 metadata reads. A lease command used to be refused `Unavailable` while the metadata tier loaded (`IssueCall::decide` read both tiers first). It now decides on the core tier alone, and plant 27 checks that |
| LF-12 (pure) | `feed::Serving::admit` and `feed::route` for 5,000 polls over a leader, two followers, and two learners, with the edge asking up to three members. Before `place_learners` no learner answers. After it, 1,500 polls were sent with learners answering (`crates/api/tests/lease_api.rs`, `lf12_lf13_polls_go_to_following_members_and_never_the_leader`) | At most 1,000 polls held per member with the rest answered 429 with `Retry-After`, zero polls on the leader, and zero on followers after placement | Pass (pure model; the commit p99 half needs hosted members). Held [leader 0, follower 1,000, follower 1,000, learner 0, learner 0] with peaks the same, and 3,000 answered 429 with `Retry-After` 10 s (`FEED_BATCH_ALL`). After placement: [0, 0, 0, 750, 750] |
| LF-13 (pure) | A follower with 1,000 polls held whose AppendEntries leave a gap, then an `InstallSnapshot` catch-up, then lagging by 2 s, then following again | Every held poll returns with its cursor and no events, new polls are refused until it catches up, and no event is lost | Pass (pure model). `drop_all` answered 1,000 of 1,000 at once. New polls were `Lagging` with a gap, while installing, and at 2 s behind, and held again once following. No event is lost because a poll answered without events keeps its cursor (`activity` renders `max(through, after)`) |
| Byte identity (LM-1's L0-11 rerun) | `agent_bytes.golden`. The `GRAFTYARD_REPLIES` dump of `cargo test -p graftyard-api` on this branch, compared with `diff -r` against `main`'s | Unchanged | Pass. `agent_bytes` passes against the golden file, untouched. 120 of 120 reply files identical. The new `lease_api` suite writes no reply files |

Also on the real members:

- **Renew and release** (`renew_and_release_follow_the_holder_and_admin_rules`): only the holder renews (`NotHolder`), the TTL must be 60 to 3,600 s, and a renew moves `expires` to the entry's `at` plus the TTL. Another writer may not release, but the repository admin may. After release, renew and release are 404.
- **A cancel releases the task's leases, and the feed reports each change** (`a_cancel_releases_the_tasks_leases_and_the_feed_reports_each_change`, `feed::events` over the leader's real entries):
  - an acquire gives `Acquired`, and an explicit release gives `Released`;
  - a cancel marked 32.14 writes `LeaseRelease` for both of the task's leases in its own command, and the feed shows 2 `Released` and the task `cancelled`;
  - a 60 s lease gives `Expired` at the first entry past its expiry;
  - a 32.14 agent push writes `PathSet` and an `agent_push` event that matches `f/**` and not `docs/**`.
- **The lease endpoints** (`the_lease_endpoints_serve_the_reference_shape`):
  - a Shared acquire renders 201 `{lease, overlaps}` from the `&Acked`, with the overlapping Exclusive lease in `overlaps`, `intent` as a `TrustView` (an HTML comment hidden), and `holder.session` in its text form;
  - an overlapping Exclusive acquire is 409 with `conflicts`;
  - `GET .../leases` is filtered by `paths`, paged by `per_page` with `Link` `rel="next"` and an opaque `after`, and is 404 without `contents: read`;
  - the task GET's `leases` lists the task's live lease, and leaves it out once expired (before any prune) or released;
  - renew is 200 and release is 204;
  - every route is under `/api/graftyard/v1`, and `/agents/leases` routes nowhere.

## Planted bugs (`tests/am1/wp4/plant.py`)

40 of 40 caught. Each mutant is planted alone, the gating tests run, and the file is restored byte for byte. The first run missed 6: a plan lease outliving its draft (3), a reinstall reviving a lease (4), the Exclusive index missing leases under the glob (7), a non-writer's plan block (14), an explicit release reported as `Expired` (36), and the task GET listing an expired lease (38). It also counted 10 mutants as caught on a cargo usage error (bug 3). Each of the six got the test the gates were missing: a readied draft, a claim with no task row reinstalled, an acquire above a lease and a truncated directory, no row for a non-writer, an explicit release on the feed, and a read past expiry before a prune. On the second run, with the runner fixed, all six and all 10 were caught by named tests. That run missed 31 (a non-canonical cursor), and 26 did not compile because of a second runner argument bug. A one-spelling cursor test and the fixed arguments catch both (`plant.py 26 31`).

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | holds ignores expiry | `l0_12_a_late_exclusive_acquire_after_an_idle_lease_succeeds`, `the_lease_endpoints_serve_the_reference_shape` |
| 1 | holds ignores the holder losing push access | `l0_13_removed_uninstalled_and_old_holders_lose_their_hold` |
| 2 | a final task does not release its leases in holds | `l0_13_removed_uninstalled_and_old_holders_lose_their_hold` |
| 3 | a plan lease outlives its draft | `lf5_plan_blocks_make_one_lease_and_leave_bodies_alone` |
| 4 | a reinstall revives an installation's leases (not_before ignored) | `l0_13_removed_uninstalled_and_old_holders_lose_their_hold` |
| 5 | Exclusive never reads Shared (EXCLUSIVE_MAX ignored) | `l0_13_removed_uninstalled_and_old_holders_lose_their_hold`, `lease::tests::exclusive_reads_shared_after_exclusive_max` |
| 6 | a holder conflicts with its own session | `l0_12_agents_exclusive_refuses_only_agents` |
| 7 | the Exclusive index misses leases under the glob | `l0_12_a_late_exclusive_acquire_after_an_idle_lease_succeeds`, `l0_12_agents_exclusive_refuses_only_agents` |
| 8 | Exclusive accepts a wildcard first segment | `l0_13_bad_acquires_are_refused_100_of_100` |
| 9 | a task may hold two Exclusive leases | `l0_13_bad_acquires_are_refused_100_of_100` |
| 10 | an App token may acquire with no session | `l0_13_bad_acquires_are_refused_100_of_100` |
| 11 | Exclusive without a live task | `l0_13_bad_acquires_are_refused_100_of_100` |
| 12 | Shared conflicts like Exclusive (overlap check on every acquire) | `l0_12_a_late_exclusive_acquire_after_an_idle_lease_succeeds`, `the_lease_endpoints_serve_the_reference_shape` |
| 13 | anyone may release (admin rule dropped) | `renew_and_release_follow_the_holder_and_admin_rules` |
| 14 | a plan block from an account without push access makes a lease | `l0_13_plan_blocks_need_push_and_stop_at_the_repository_cap` |
| 15 | PLAN_LEASES_PER_REPO not enforced | `l0_13_plan_blocks_need_push_and_stop_at_the_repository_cap` |
| 16 | a repeated key in a plan block is accepted | `lease::tests::plan_blocks_follow_the_grammar`, `lf5_plan_blocks_make_one_lease_and_leave_bodies_alone` |
| 17 | the 4 KiB plan-block cap off | `lease::tests::plan_blocks_follow_the_grammar`, `lf5_plan_blocks_make_one_lease_and_leave_bodies_alone` |
| 18 | a later block is read after an invalid first one | `lease::tests::plan_blocks_follow_the_grammar`, `lf5_plan_blocks_make_one_lease_and_leave_bodies_alone` |
| 19 | an invalid block releases the plan lease | `lf5_plan_blocks_make_one_lease_and_leave_bodies_alone` |
| 20 | prune never drops expired rows | `l0_12_a_late_exclusive_acquire_after_an_idle_lease_succeeds` |
| 21 | ExtendLeases moves nothing | `l0_12_a_stall_moves_every_live_lease_by_its_length` |
| 22 | the leader does not apply extend before its commands decide | `l0_12_a_stall_moves_every_live_lease_by_its_length` |
| 23 | Advisory refuses too (policy ignored) | `l0_12_agents_exclusive_refuses_only_agents` |
| 24 | a lease refusal refuses the whole 32.14 push, not the ref | `l0_12_agents_exclusive_refuses_only_agents` |
| 25 | a cancel does not release its task's leases in the same command | `a_cancel_releases_the_tasks_leases_and_the_feed_reports_each_change` |
| 26 | glob overlap ignores a trailing ** after a literal | `glob::tests::two_globs_overlap_when_some_path_matches_both`, `l0_12_agents_exclusive_refuses_only_agents`, `l0_13_removed_uninstalled_and_old_holders_lose_their_hold` |
| 27 | a lease command on an unloaded metadata tier is refused (L0-16) | `l0_16_lease_checks_read_the_core_tier_alone` |
| 28 | wait is not clamped | `feed::tests::the_query_clamps_wait_and_caps_paths`, `lf3_foreign_cursor_long_wait_and_too_many_paths` |
| 29 | 33 paths are accepted | `feed::tests::the_query_clamps_wait_and_caps_paths`, `lf3_foreign_cursor_long_wait_and_too_many_paths` |
| 30 | a foreign cursor is served | `lf3_foreign_cursor_long_wait_and_too_many_paths` |
| 31 | a cursor decodes a non-canonical spelling | `lf3_foreign_cursor_long_wait_and_too_many_paths` |
| 32 | the leader serves feed polls | `feed::tests::learners_serve_and_full_or_lagging_members_send_polls_elsewhere`, `lf12_lf13_polls_go_to_following_members_and_never_the_leader` |
| 33 | followers serve while a learner answers | `feed::tests::learners_serve_and_full_or_lagging_members_send_polls_elsewhere`, `lf12_lf13_polls_go_to_following_members_and_never_the_leader` |
| 34 | FEED_HELD_MAX not enforced | `feed::tests::learners_serve_and_full_or_lagging_members_send_polls_elsewhere`, `lf12_lf13_polls_go_to_following_members_and_never_the_leader` |
| 35 | a lagging member holds polls | `feed::tests::learners_serve_and_full_or_lagging_members_send_polls_elsewhere`, `lf12_lf13_polls_go_to_following_members_and_never_the_leader` |
| 36 | a release reports Expired, not Released | `a_cancel_releases_the_tasks_leases_and_the_feed_reports_each_change` |
| 37 | the lease list ignores contents | `read: the_lease_endpoints_serve_the_reference_shape` |
| 38 | the task GET lists released leases | `the_lease_endpoints_serve_the_reference_shape` |
| 39 | intent is raw text, not a TrustView | `the_lease_endpoints_serve_the_reference_shape` |

## Bugs the gates found in my own code

1. **The first entry after a stall judged leases before the stall was added back.** The fold applies `extend` before an entry's events (`Tiered::on_extend`). But the leader's `decide` ran on the fold at `applied`, without the extension, so a renew sent right after a 30-minute stall was refused `NotFound`, and a lease the stall had covered read expired. The leader now takes `extend` before it decides and applies it to the scratch view (`Scratch::extend`, default no-op; `RepoScratch` runs `lease::extend` on the core tier). The entry carries the same value it was decided with. `verify` already sees it, because a follower applies the entry's `extend` when it applies the entry. Plant 22 checks this. The raft suite passes unchanged (9 suites, 0 failed).
2. **A lease command waited for the metadata tier.** `IssueCall::decide` refused `Unavailable` whenever the metadata tier was not loaded. L0-16 needs lease checks on the core tier alone, so a lease command now decides with an empty metadata view.
3. **Plant runner.** The first plant run reported 10 mutants as CAUGHT with no failing test. Its combined argument list passed `--lib` twice, and cargo's usage error was counted as a failure. The runner now builds one argument list per package set, and treats any `error:` with no failed test as DID NOT COMPILE. Every count above is from the corrected run.

## What was built

- **Globs** (`crates/identity/src/glob.rs`): `glob_overlap` (two globs overlap when some path matches both, walking segment lists; a trailing `/` covers everything under it, for `truncated` directories), `literal_prefix`, and `PathGlob::overlaps`.
- **Leases** (`crates/git/src/lease.rs`):
  - Types: `LeaseId` (a ULID from the entry's `at` and the repository's `lease_seq`, so every member derives it), `ActorId`, `LeaseHolder`, `LeaseScope`, `LeaseMode`, `Lease` (`mode_at`: Shared after `EXCLUSIVE_MAX`), `LeaseRow { lease, plan }`.
  - Constants: `LEASE_TTL`, the TTL range, `EXCLUSIVE_MAX`, `LEASES_PER_HOLDER`, `LEASES_PER_APP`, `PLAN_LEASES_PER_REPO`, `LEASES_PER_REPO`, the per-lease limits, and `PLAN_BLOCK_MAX`.
  - Core tables: `lease`, `lease_seq`, `lease_by_holder`, `lease_by_app`, `lease_by_session`, `lease_expiry`, `lease_plan`, and `lease_exclusive` (Exclusive globs by literal prefix).
  - `prune` drops expired rows in `on_entry`. `extend` applies `ExtendLeases` in `on_extend`, before `on_entry`.
  - `LeaseView::holds`: not expired; the holder still has push access by the owner's `Authz` (an installation also needs `granted >= not_before`); its task does not read final; and a plan lease's PR is still an open draft. `live`, `holder_live`, `session_live`, and `exclusive_conflicts` (the literal-prefix index, other holder by session, else by actor).
  - `decide` for `AcquireLease`, `RenewLease`, and `ReleaseLease`, with the design's rules and caps (`IssueCmd::decide_lease`).
  - `graftyard-plan` blocks: `find_plan`/`parse_plan` by the API reference's grammar, and `plan_events`, which replaces, releases, or keeps the editor's Shared plan lease in the `OpenPr` or body edit's own command.
  - `marks`: which issue commands the edge sends as 32.14.
- **`decide` wiring:**
  - `Push::decide` matches `changed` against live Exclusive leases last among the agent rules, under `AgentsExclusive` only, for `ActorKind::Agent` only (`AgentRule::Leased`, text from `agent::leased`). It is per ref in 32.14, and refuses the whole push in an earlier format.
  - A 32.14 push writes one `PathSet` per applied create or move with `changed`, carrying `pr` (the ref's open PR) and `planned` (the paths of the live leases of the pusher's task, else of the pusher). `PathSet` is deleted when its ref's PR closes or merges.
  - A task cancel, finish, or derived-final record releases the task's leases in the same command (32.14).
  - The leader applies an entry's `extend` before its commands decide.
- **`ExtendLeases` in raft** (`crates/raft/src/member/`):
  - `stalled_since` is marked at a follower's `suspect`, at a campaign start (not a transfer's), and by the leader's tick when its entry in flight is unacknowledged past `CALL_DEADLINE`.
  - The leader's next entry carries `extend { since: max(marker, last at), by: now - since }`, unless an unapplied entry already carries one.
  - The marker clears when an entry with `extend`, or stamped after the marker, applies.
  - `Scratch::extend` is the leader's pre-decide hook.
- **Activity feed** (`crates/git/src/feed.rs`):
  - The `FEED_*` constants and `FEED_PATHS_MAX`. `LeaseChange`, `PrAction`, `ActivityEvent` (`Lease`, `Task`, `AgentPush`, `AgentPr`), and `matches(paths)`.
  - `FeedCursor` as 17 bytes of base64url, canonical spelling only.
  - `parse_query` (`wait` in whole seconds, clamped at 25; `paths` repeated, at most 32). `start` (410 for another group, outside the ring, or malformed). `ring_floor`.
  - `events` for one entry (lease diffs as Acquired, Released, or Expired; tasks; agent pushes from `PathSet`; agent PRs opened, readied, closed, merged).
  - `Following`, `FeedRole`, `Serving` (`admit`, `done`, `drop_all`, `peak` for `place_learners`), and `route` (the edge's choice over up to three members).
- **API** (`crates/api/src/leases.rs`): routes for `/repos/{o}/{r}/leases`, `/leases/{id}`, `/leases/{id}/renew`, and `/activity` under `/api/graftyard/v1`. `lease_json` (intent as a `TrustView`), `task_leases` (the task GET's `leases`, previously `[]`), `list`, `parse_acquire` (the session header wins), `parse_renew`, `render_ok` from `&Acked`, `refused` (409 with `conflicts`), and the feed's `event_json`, `parse_feed`, `gone`, `too_many`, and `activity`. Also `hosted::What::Lease`.
- **Edge** (in `workers/edge/src/agents.rs`): `agents::leases` serves the lease endpoints once `FormatsEnable` holds 32.14, and 503 before. `GET .../activity` checks its query (422), then answers 503, because this edge reads only leaders and the feed is served by followers and learners. Outside the `agents` module: `api.rs`'s `commit_issue_in` sets `call.leases` from `lease::marks` once 32.14 is enabled, and `Read::view` became `pub(crate)`. `lib.rs` and `bed_rows.rs` got `leases: false` on the `Push` literals.
- **Site:** `Repo::reads()` adds 32.14; `snap.rs` got `leases: false`.

## Formats and tags taken

| What | Taken |
| --- | --- |
| Repository command format | 32.14 (`REPO_FORMAT_LEASES`). 32.13 is `m3-repos`'s |
| Authz format | none (0x12.8 not needed) |
| `RepoEvent` tags | 35 `LeaseAcquire`, 36 `LeaseRenew`, 37 `LeaseRelease`, 38 `PathSet`. They were 33 to 36 in `844ecbe`; they moved in `0477a8b`, because `m3-repos` (`5066a6f`) holds 33 `Created` and 34 `Deleted`. Checked free on `main`, `m3-repos`, and every worktree. No golden file or test pinned them |
| `IssueOp` | `AcquireLease`, `RenewLease`, `ReleaseLease` (serde variants, 32.14 only) |
| `IssueOut` / `IssueErr` | `Lease(LeaseId)` / `Lease(LeaseErr)` |
| `AgentRule` | `Leased` (per-ref reason only in 32.14) |
| Wire fields | `Push.leases` (32.14 only when also validated), `IssueCall.leases` |
| Core tables | `lease`, `lease_seq`, `lease_by_holder`, `lease_by_app`, `lease_by_session`, `lease_expiry`, `lease_plan`, `lease_exclusive`. Metadata: `path_set` |
| Raft | `MemberState.stalled_since` and `LogEntry.extend` now set (fields existed). `Scratch::extend` (default no-op) |

## Checks

All run on the branch head after the last code commit (`3558066`):

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy (`--target wasm32-unknown-unknown -- -D warnings`): clean for 5 of 5 (`workers/site` and `workers/edge`, each with and without `--features test-faults`, and `workers/ops`).
- `cargo test --workspace`: 539 passed, 0 failed, and 3 ignored (the long raft sweeps), in 62 suites. Main at `b767933` gave 518, 0, and 3; the 21 new tests are AM1-WP4's.
- The raft suite (`cargo test -p graftyard-raft`), rerun after `Scratch::extend` and the leader's pre-decide `extend`: 9 suites, 0 failed, including the harsh simulator and the scenarios that a separate `Noop` and an extra state write had broken.
- The `GRAFTYARD_REPLIES` dump of `cargo test -p graftyard-api` on this branch, compared with `diff -r` against the same dump from `main` (`b767933`): 120 of 120 files identical.
- `crates/git/tests/agent_bytes.rs` against `agent_bytes.golden` (unchanged): pass.
- `python3 scripts/check_design.py` (agent block OK, cost figures OK), `check_names.py` (1,255 files), and `check_links.py` (2,701 citations): pass.
- Build directory `target-am1` only. The release profile it briefly held was removed with `cargo clean --release`.

## Assumptions

- **`LeaseAcquire.plan` is `Option<IssueNumber>`**, not the design's `plan_block: bool`, because a plan lease's liveness reads its PR (open draft). The API still renders `plan_block: bool`.
- **A plan lease is held only while its PR is an open draft.** The design says "in a draft PR body" and "each body edit replaces"; it does not say what ending the draft does. Readying, closing, or merging ends the lease, with no command.
- **A plan holder's `session`** is the editor's token claim. User tokens have none.
- **`PathSet.merge_base` is `Option`**, `None` until the git cache reports it. `Push` carries no merge base.
- **Lease expiry in refusal text** is epoch milliseconds.
- **`ActivityEvent::Verdict`** is left out until AM4's `InsightVerdict` exists.
- **`FeedCursor.group`** is the `RepoId` (the 8 bytes the cursor text names), not a `ChainId`.
- **A stall marker rides the member's next state write.** It does not get a write of its own (see contradiction 4).
- **The edge's activity GET answers 503** after checking its query, until feed serving on followers and learners is hosted.
- **The stall gate dropped messages instead of blackholing member calls for 30 minutes with real DO storage.** The leader's own marker and tick are real. The suspect and campaign-start markers are covered by the raft suite's existing scenarios, which pass.

## What the hosted half needs

1. A site day with this release (the site reads 32.14), then `FormatsEnable` for 32.14. Until then the lease endpoints answer 503, no command is marked 32.14, and no push writes `PathSet` or is refused `Leased`.
2. The edge sending `changed` (and `validate`'s other outputs) on agent pushes, and setting `Push.leases` once 32.14 is enabled. Today `receive_pack` sends no `changed`, so hosted pushes neither match leases nor write `PathSet`. REST ref writes (`PATCH`/`POST .../git/refs`) need the same `changed`.
3. Feed serving: the `activity` read on followers and learners from their applied log (`feed::events` per entry, the `ring_floor` ring), held polls with `Serving`, the edge's `route` across members, `FEED_POLLS` per token and IP, and each member's `peak`, refusals, and read rate reported to `place_learners` (M3-WP5).
4. Durable Object wiring for `stalled_since`: the site's `suspect` path and the leader's tick, persisted with the member's state row, plus the hosted 30-minute stall with member calls blackholed and real clock offsets (L0-12's `DRIFT` line).
5. The hosted runs: L0-12 (the A1 replay with a lease per agent and leader kills, 500 long-pollers within 3 s p99, the per-kind refusal counts over the edge), LF-12 (5,000 held polls, commit p99), LF-13 (a real gap and `InstallSnapshot`), and L0-16 on the A1 replay with the metadata tier unloaded.
6. Plan blocks on the GHES PR body paths. The edge serves no PR body edit yet (no REST `PATCH /pulls/{n}` or GraphQL `updatePullRequest` builds `IssueOp::Change { Body }`). When it does, `commit_issue_in` marks it 32.14 through `lease::marks`, and each path needs a hosted check. A draft opened through GraphQL `createPullRequest` already marks.
7. MCP's `coordination.lease`, `.renew`, `.release`, and `.activity` (AM2), and the controller's Shared lease at task start (AM3).

## Design sentences these findings contradict or leave open

Neither `design/` nor `decisions/` was edited.

1. `design/AGENT-LAYER.md`, Shape (and `design/DESIGN.md`'s agent block): "`LeaseAcquire { lease: Lease, plan_block: bool },              // Core`". A plan lease is live only while "a fenced `graftyard-plan` block in a draft PR body" says so, and "each body edit replaces the editor's plan lease". That needs the PR number on the row: to find the editor's lease on that PR, and to tell the draft is still open. The code stores `plan: Option<IssueNumber>`, and the API still renders `plan_block: bool`.
2. Same file, "Overlaps and prediction": "`pub struct PathSet { pub refname: RefName, pub pr: Option<PrNumber>, pub head: Oid, pub merge_base: Oid,`", "Written … in the command that moves the ref". The command that moves the ref (`Push`) carries no merge base, and `validate` does not compute one today. The code has `merge_base: Option<ObjectId>`, `None` until the git cache reports it.
3. Same file, the feed block: "`?cursor=` is base64url without padding of 17 bytes: a version byte (1), the group's 8-byte `RepoId`, and the 8-byte big-endian index" against "`pub struct FeedCursor { pub group: ChainId, pub index: Index }`". A `ChainId` does not fit 8 bytes with its kind. The code takes `group: RepoId`, which matches the 17 bytes.
4. `design/DESIGN.md`, Shape, "Chains": "A member persists `stalled_since` at the earliest durable marker: a follower's first `suspect` report, a candidate's campaign start, or the leader's first entry that no quorum acknowledged within CALL_DEADLINE." A separate durable write at each `suspect` report changed the seeded harsh simulator's crash points: acknowledged writes fell to 1,138 against a floor of 1,200. A separate `Noop` entry to carry `extend` broke two scenarios tests. The code records the marker in memory and persists it with the member's next state write (a campaign start writes at once). So a crash before that write loses the marker, and leases lapse by that stall, which the advisory slack accepts.
5. The same sentence, with "The next entry the group commits carries `ExtendLeases`" and "`decide` compares `expires` with the entry's `at`". It does not say that `decide` for the carrying entry must see the extension. Without it, the first command after a stall judges every lease the stall covered as expired (bug 1). The code applies `extend` to the leader's scratch view before its commands decide.
6. `design/AGENT-LAYER.md`, the feed block: "`Verdict { refname: RefName, verdict: Verdict /* worst of Conflict, PathOverlap, Stale, Clean */ }`". Its only source is AM4's `InsightVerdict` ("the feed's `Verdict` event read[s] only that row"). It is left out until AM4.
7. Same block: "Learners serve it, followers only while no learner answers, never a leader or witness", and DESIGN's "Activity feed polls, 45/s, go only to followers and learners". Today's edge reads only leaders, so the hosted feed needs a follower and learner read path first. Until then the edge answers 503 after checking the query.
8. Same file, A2: "`refused: pkg/auth/** is leased Exclusive by graftyard-agent (task <id>) until <time>`". `decide` cannot read an App's slug (it is on the `Names` chain, which a push does not read), so the text says `app 4` and gives the time as epoch milliseconds.
9. Same file, the lease block: "Plan-block leases are Shared, held by the editor, who needs push access". It does not say what ending the draft (ready, close, merge) does to the lease. The code ends it.
10. Carried from AM1-WP3: contradictions 1 to 3 still hold (the `…/<uuid>/**` prefix, `TaskUpdated` always writing Core, and the core task row). The lease checks read only that core row.
