# M4-WP1: Webhooks, the pure half

BUILD-PLAN M4-WP1 asks for `SUBSCRIPTIONS`, `HookDelivery` with one automatic attempt and the `retry` opt-in, `HookCursor`, `HookRecord` on `RepoEvent::Hook`, `HOOK_OWNER_CONCURRENCY`, the `Webhooks`, `Audit`, and `Search` consumers on the M2-WP5 pipeline, `HookSequencer` as the delivery store, signing through `graft-ops`, and the `webhooks` surface at T0 including `merge_group`, `pull_request.enqueued`, and `pull_request.dequeued`. This round builds the pure, in-memory half of the `Webhooks` part and runs BF-57 and BF-58 on real raft members. Nothing was deployed. `Audit` and `Search` as consumers, Basin Pipelines, the `HookSequencer` Durable Object class, delivery Workers, and the site day are not built here.

Branch `m4-wp1`, cut from `main` at `c8eda69`. `main` at `aa9e097`, the `am1-runs` merge with `89cf205` and `dfa5410`, is merged in as `8d9034a`, and `origin/main` at `cadf83d` as `f0faea7`. `cadf83d` changed only `README.md`, `build/`, `decisions/`, and `design/` (no `.rs` or `.toml`), none of it about M4-WP1, so the checks below hold across it, and the three scripts pass after it.

## What was built

- **Records and the fold** (`crates/git/src/hooks.rs`). SHARED's `HookRecord`, `HookContentType`, and `HookCursor`. Core-tier rows `hook` (as `HookRow`: the record plus the times of the entries that created and last changed it, which the REST object renders) and `hook_cursor`.
  - `RepoEvent::Hook { id, record, actor, created }` is tag 40 and `RepoEvent::HookCursor` tag 41, both appended after `fanout`'s `Fanout` (39). A create's id is the core tier's next id, and the fold now advances the tier's id cursor past it, as a release's does. Before that fix, two creates took the same id and the second overwrote the first; `hook_commands_are_admin_only_and_a_create_alone_is_pinged` found it.
  - `IssueOp::PutHook { id, record }` (38) and `DeleteHook { id }` (39), appended. Admin only; an `Agent` is refused (`AgentRule::Rules`, "refused: agents may not edit webhooks"); an App needs `repository_hooks`. An identical edit commits nothing, a second hook on one URL is `Exists`, and a repository holds at most `HOOKS_PER_REPO_MAX` (20).
  - `HookCursorCmd`: each half moves only forward, `replay_from` stays at most `read + 1`, and a late or re-sent command is a no-op.
  - `EVENTS` is GHES 3.22's 78 event names plus `merge_group`. `SUBSCRIPTIONS` lists the deployed consumers only.
- **What an entry carries** (`webhooks`). `push`, `create`, and `delete` from `Refs`, skipping `refs/pull/` and queue refs; `pull_request` `opened`, `closed`, `edited`, `reopened`, `ready_for_review`, `converted_to_draft`, and `synchronize` for the pull requests a pushed branch heads; `issues` and `issue_comment.created`; `status`; `check_run` `created` and `completed`; and from `QueueEvent`, `merge_group` `checks_requested` and `destroyed` and `pull_request` `enqueued` and `dequeued`. A new hook's `Hook` entry carries a `ping` to that hook only.
- **Delivery identity.** `delivery_guid(generation, chain, index, slot, hook)`: SHA-256 over those fields, printed as a UUIDv8. A replay derives the same GUID, and a new restore generation never reuses one.
- **The sequencer** (`crates/git/src/sequencer.rs`). The `HookSequencer`'s rows and logic.
  - `HookDelivery` rows by GUID. `record` is idempotent by GUID.
  - One queue per (chain, hook), plus one per hook for pings and manual redeliveries. At most one attempt in flight per queue.
  - `start` picks round robin after the last queue served, at most `HOOK_OWNER_CONCURRENCY` (50) in flight, and failing hooks at most 25.
  - One automatic attempt per delivery. With `retry`, backoff from 10 s doubling to 1 h, for 3 days, under the GUID.
  - An attempt unreported for 30 s is re-sent under its GUID, and a late report of it is ignored.
  - `replay_from`, `cursor`, `redeliver` (`NotFound`, `Expired` past 3 days), `list`, `get`, and `prune`.
  - Attempt ids start from the store's start time, so a rebuilt store never reuses an id a client saw.
- **The consumer's pure half.** `plan(core, repo, generation, entry, render)` turns one committed entry into one `NewDelivery` per (webhook, hook that takes it), rendering each payload once. A hook gets only entries from its own creation on. `consumer_start(cursor, recorded)` starts a store that holds nothing of a chain at the committed `replay_from`. `cursor_to_commit` commits when the batch recorded rows, when `replay_from` moved, or every `CURSOR_STRIDE` entries, so a cursor entry never chases its own tail.
- **Payloads** (`crates/api/src/webhooks.rs`). `payload` renders every `Subject` from the fold the consumer reads with the entry. The `repository` object is `full-repository` without `permissions`, and the `sender` is the actor, or `ghost` for a host bot. `pull_request.dequeued`'s `reason` is dotcom's upper-case enum (`MANUAL`, `CI_FAILURE`, `MERGE_CONFLICT`, ...), and `merge_group.destroyed`'s is `merged`, `invalidated`, or `dequeued`.
- **The request.** `request` sets GHES's headers: `User-Agent: GitHub-Hookshot/graftyard`, `X-GitHub-Delivery`, `-Event`, `-Hook-ID`, `-Hook-Installation-Target-Type` and `-ID`, `X-GitHub-Enterprise-Version`, and `-Host`. It sets `X-Hub-Signature-256` over the bytes it sends (the JSON, or `payload=` and the escaped JSON for `form`) when the hook has a secret.
- **Signing through `graft-ops`** (`crates/identity/src/sealed.rs`, `workers/ops`). `Wrapped` and `HookSecret` print no secret through `Debug`. `HookSecrets { wrap, unwrap }` is the `graft-ops` interface. `WrapKeySource` implements it with AES-256-GCM over the `SecretsWrapKey` slots, and the tests stub it in memory (`Ops` in `crates/api/tests/webhooks.rs`, XOR under one slot) the way M3-WP4 stubbed `KeySource`. `graft-ops` gained `POST /wrap`, not deployed. `hmac_sha256` matches RFC 4231 and GitHub's documented example.
- **REST** (`crates/api/src/webhooks.rs`). `hook_json` is GHES's `hook` object; `config.secret` is `********` when set and is never the value. `hook_body` parses `POST` and `PATCH` bodies: GHES's defaults (`push`, `form`), `add_events` and `remove_events`, and `config.retry` as Graftyard's opt-in. `What::Hook { created }` renders 201 with `Location` or 200 from `&Acked`, and `refused` covers the refusals. `sequencer_route` serves `POST .../pings` (204), `GET .../deliveries` (`hook-delivery-item`, newest first, `Link` with `cursor`), `GET .../deliveries/{id}` (`hook-delivery`), and `POST .../deliveries/{id}/attempts` (202, 404 for another hook's delivery, 422 past 3 days) on a `Sequencer`.
- **The edge** (`workers/edge/src/hooks.rs`, one route line in `api.rs`, one `mod` line in `lib.rs`). This covers `GET`/`POST /repos/{o}/{r}/hooks` and `GET`/`PATCH`/`DELETE .../hooks/{id}`.
  - Reads are admin only; anyone else gets 404.
  - A body's secret is wrapped by `graft-ops` over the `OPS` binding before the command leaves the edge.
  - The sequencer routes answer 503 until the `HookSequencer` exists.
  - Until `FORMATS_ENABLED` lists 32.16, `/hooks` falls through to the reply it had before.

## Byte identity

No reply changes for a client that uses no webhooks.

- Every new type, event, and op is appended, and no earlier command takes format 32.16, so earlier entries keep their bytes. `agent_bytes` and `wire_tags` pass unchanged.
- The edge's `/hooks` routes fall through until 32.16 is enabled, so even a client that calls `/hooks` gets the old reply until then.
- No existing renderer calls a new function. `full_repository` is unchanged; the webhook `repository` object is a copy with `permissions` removed.

## The gates

Both run on three real `graftyard_raft::Member<RepoCmd>`s per repository (`crates/git/tests/hooks_gates.rs`). The consumer reads each leader's committed log from the chain's committed `HookCursor`, `plan` records rows in the owner's `Sequencer`, cursors commit as `HookCursorCmd` through the leader, and simulated delivery Workers run each attempt. Time is simulated in 100 ms steps; healthy hooks answer 2xx in 30 to 400 ms (xorshift, fixed seed). `cargo test -p graftyard-git --test hooks_gates -- --nocapture` prints the lines below.

### BF-57: one hook timing out at 10 s beside 49 healthy hooks, 10 events/s. Pass.

There are 50 hooks of one owner over three repositories (20, 20, and 10, because a repository holds at most 20). Hook 1000 on the first repository never answers before the 10 s deadline. The load is 600 push entries in 60 s, one per 100 ms, round robin over the three repositories, plus each hook's ping.

| Measure | Pass | Result [measured, simulated time] |
| --- | --- | --- |
| Healthy hooks' first attempt, entry commit to attempt start | p99 at most 2 s | p50 100 ms, p99 700 ms, max 1,100 ms over 9,849 deliveries |
| The slow hook's deliveries | every one, in chain order | 201 of 201, started in strictly increasing (index, slot); drained 1,950 s after the load at one per 10 s |
| Every (chain, hook) queue | in chain order | all 50 in order |
| Webhooks cursor lag, entry commit to the committed `read` passing it | at most 5 s, not growing | max 1,000 ms in the first 30 s, 1,000 ms in the second |
| In flight | at most 50, failing at most 25 | at most 50, 1 failing |

The cursor lag is the consumer's 1 s cursor-commit pacing (`CURSOR_EVERY_MS` in the harness [guess]). It does not depend on the slow hook: `read` passes an entry once its rows are recorded, whatever their attempts do.

### BF-58: kill the site holding the owner's `HookSequencer` with deliveries pending. Pass.

Three repositories with 5 hooks each, and hook 2 of the second repository times out, so it keeps a backlog. Every leader is on site 0, and so is the sequencer.

1. 60 push entries are committed while deliveries run.
2. Site 0 is killed: its members, the `Sequencer`, and every attempt in flight. At the kill the store held 315 rows, 38 of them never attempted, with 15 attempts in flight. The committed cursors (read, replay_from) were (26, 25), (26, 4), and (26, 25).
3. Leaders fail over to site 1 after `ELECTION_AFTER`. The owner's new home site starts an empty store 11 s after the kill. The consumer starts each chain at the committed `replay_from`.
4. 30 more pushes are committed, then the pipeline drains.

| Measure | Pass | Result [measured, simulated time] |
| --- | --- | --- |
| Every (event, hook) delivered | at least once | 465 of 465 |
| GUIDs | unchanged | every attempt's GUID is the one its (chain, index, slot, hook) derives; 155 rows recorded by both stores, same GUIDs and entries; 0 under a new GUID |
| Redeliveries | allowed (at-least-once) | 117 delivered more than once, all under their own GUIDs |

The expected set is derived from what the test committed (each push to every hook of its repository, and each hook's ping), not from `plan`, so a planner that dropped a row would fail the gate.

### Other checks

- `crates/api/tests/webhooks.rs`, 8 tests. A repository with a hook, pushes, a branch made and deleted, an issue opened, edited, closed, and reopened, a comment, a pull request opened, synchronized, drafted, readied, edited, closed, and reopened, a status, a check run created and completed, a bot push to a queue ref, and a queue's enqueue, group, and ejection. Each entry is rendered against the fold as of that entry.
  - With `GRAFTYARD_REPLIES`, `crates/api/gh/validate.py` checks 30 payloads against `ghes-3.22.json`'s `x-webhooks`. For `merge-group-*` and `pull-request-enqueued`/`-dequeued`, which GHES 3.22 lacks, it checks against dotcom's description at the pinned commit (`$DOTCOM_OPENAPI`). It also checks 7 REST replies (`hook` from GET and from 201, the 422, ping 204, the deliveries list, one delivery, and redelivery 202). The result is 0 failed.
  - Mismatches found and fixed on the way: `external_id` must be a string, `verified_at` was missing, `edited` requires `changes`, and `dequeued`'s `reason` is the upper-case enum.
  - The signature is HMAC-SHA256 over the exact body sent, JSON or form, and no rendering or `Debug` print contains the secret.
- `crates/git/tests/issues.rs` covers the hook commands' `decide`. `agent_gates.rs` checks that an `Agent` may not create or delete a hook. The sequencer has 10 unit tests.
- `crates/git/tests/hooks_gates.rs::the_hook_cursor_moves_only_forward_on_the_group` runs `HookCursorCmd` on the group.

## Planted bugs

`tests/m4/wp1/plant.py` plants 34 mutants one at a time and restores each file byte for byte. Seven were missed in the first run: redelivery to another hook, attempt ids after a rebuild, round robin from the start, `HookCursorCmd`'s two halves, a queue ref sending `push`, and `edited` without `changes`. Each now has a test that observes the behaviour, and the final run catches all 34:

| # | Mutant | Verdict | Caught by |
| --- | --- | --- | --- |
| 0 | two attempts in flight per queue (order lost) | CAUGHT | `bf57_one_slow_hook_beside_49_healthy_ones`, `bf58_the_sequencer_site_is_killed_with_deliveries_pending`, `one_attempt_per_queue_in_order_and_record_is_idempotent` |
| 1 | failing hooks not held to half the slots | CAUGHT | `failing_hooks_share_at_most_half_the_slots` |
| 2 | no owner-wide cap (HOOK_OWNER_CONCURRENCY ignored) | CAUGHT | `at_most_hook_owner_concurrency_attempts_at_once`, `round_robin_serves_every_queue_before_any_twice` |
| 3 | record is not idempotent by GUID | CAUGHT | `one_attempt_per_queue_in_order_and_record_is_idempotent` |
| 4 | replay_from ignores unfinished deliveries | CAUGHT | `bf58_the_sequencer_site_is_killed_with_deliveries_pending`, `a_rebuilt_store_replays_from_replay_from_with_the_same_guids`, `one_attempt_per_queue_in_order_and_record_is_idempotent` |
| 5 | a rebuilt store starts past the committed read (replay_from ignored) | CAUGHT | `bf58_the_sequencer_site_is_killed_with_deliveries_pending`, `a_rebuilt_store_replays_from_replay_from_with_the_same_guids` |
| 6 | an unreported attempt is never re-sent | CAUGHT | `one_attempt_per_queue_in_order_and_record_is_idempotent` |
| 7 | a default hook is retried (retry opt-in ignored) | CAUGHT | `bf57_one_slow_hook_beside_49_healthy_ones`, `bf58_the_sequencer_site_is_killed_with_deliveries_pending`, `manual_redelivery_lists_and_expires`, `one_attempt_per_queue_in_order_and_record_is_idempotent` |
| 8 | a retry hook is never retried | CAUGHT | `a_retry_hook_holds_its_queue_and_retries_under_the_guid` |
| 9 | redelivery past its 3 days | CAUGHT | `ping_deliveries_and_redelivery_through_the_sequencer`, `manual_redelivery_lists_and_expires` |
| 10 | redelivery of another hook's delivery | CAUGHT | `a_redelivery_names_its_own_hook`, `manual_redelivery_lists_and_expires` |
| 11 | a rebuilt store reuses the lost store's attempt ids | CAUGHT | `a_rebuilt_store_never_hands_out_an_attempt_id_the_lost_one_did` |
| 12 | round robin restarts at the first queue | CAUGHT | `round_robin_serves_every_queue_before_any_twice` |
| 13 | a hook gets entries from before it existed | CAUGHT | `bf57_one_slow_hook_beside_49_healthy_ones`, `bf58_the_sequencer_site_is_killed_with_deliveries_pending` |
| 14 | a ping goes to every hook | CAUGHT | `bf57_one_slow_hook_beside_49_healthy_ones`, `bf58_the_sequencer_site_is_killed_with_deliveries_pending` |
| 15 | the cursor commits only on the stride (read passes nothing it recorded) | CAUGHT | `bf57_one_slow_hook_beside_49_healthy_ones` |
| 16 | HookCursorCmd moves read backward | CAUGHT | `the_hook_cursor_moves_only_forward_on_the_group` |
| 17 | HookCursorCmd lets replay_from pass read + 1 | CAUGHT | `the_hook_cursor_moves_only_forward_on_the_group` |
| 18 | the GUID ignores the hook | CAUGHT | `bf57_one_slow_hook_beside_49_healthy_ones`, `guids_are_stable_distinct_and_print_as_uuids`, `at_most_hook_owner_concurrency_attempts_at_once`, `round_robin_resumes_after_a_queue_that_emptied`, `round_robin_serves_every_queue_before_any_twice` |
| 19 | the GUID ignores the slot | CAUGHT | `guids_are_stable_distinct_and_print_as_uuids` |
| 20 | a queue ref sends push | CAUGHT | `every_webhook_the_entries_carry_renders_against_its_schema_key` |
| 21 | an edit pings (created ignored) | CAUGHT | `a_new_hook_is_pinged_once_and_an_edit_is_not` |
| 22 | no create event for a new branch | CAUGHT | `every_webhook_the_entries_carry_renders_against_its_schema_key` |
| 23 | a writer may edit hooks | CAUGHT | `hook_commands_are_admin_only_and_a_create_alone_is_pinged` |
| 24 | an agent may edit hooks | CAUGHT | `an_agent_may_not_edit_webhooks` |
| 25 | two hooks on one URL | CAUGHT | `hook_commands_are_admin_only_and_a_create_alone_is_pinged` |
| 26 | an identical edit commits an entry | CAUGHT | `a_new_hook_is_pinged_once_and_an_edit_is_not`, `hook_commands_are_admin_only_and_a_create_alone_is_pinged` |
| 27 | a create's id does not advance the id cursor | CAUGHT | `hook_commands_are_admin_only_and_a_create_alone_is_pinged` |
| 28 | the secret is rendered in the hook object | CAUGHT | `the_request_is_signed_over_the_body_it_sends_and_the_rest_object_hides_the_secret` |
| 29 | no X-Hub-Signature-256 | CAUGHT | `the_request_is_signed_over_the_body_it_sends_and_the_rest_object_hides_the_secret` |
| 30 | the signature is over the JSON, not the form body sent | CAUGHT | `the_request_is_signed_over_the_body_it_sends_and_the_rest_object_hides_the_secret` |
| 31 | the payload names the caller's permissions | CAUGHT | `a_push_names_both_shas_and_a_synchronize_the_pull_request_it_moved` |
| 32 | edited carries no changes | CAUGHT | `every_webhook_the_entries_carry_renders_against_its_schema_key` |
| 33 | HMAC with the wrong inner pad | CAUGHT | `signature_matches_githubs_example_and_rfc_4231` |

34 of 34 caught.

## Formats and wire tags

- **Repository format 32.16** (`REPO_FORMAT_HOOKS`, `0x20.16`): `PutHook`, `DeleteHook`, `HookCursorCmd`, and the two events. No Authz format was needed, and 0x12.10 is not taken. 32.15 and 0x12.9 are left to AM1.
- **Tags appended** (`crates/git/tests/wire_tags.golden`, re-pinned with `GRAFTYARD_PIN=1`; `git diff aa9e097 -- crates/git/tests/wire_tags.golden` is 9 added lines and 0 removed):
  - `RepoEvent` 39 `Hook`, 40 `HookCursor`
  - `IssueOp` 38 `PutHook`, 39 `DeleteHook`
  - new enums `git::hooks::HookContentType` (0 `Json`, 1 `Form`) and `git::sequencer::Outcome` (0 `Ok`, 1 `Failed`, 2 `TimedOut`)
- `RepoCmd::HookCursor` and `RepoOut::HookCursor` are JSON-tagged `hookCursor`, as the other `RepoCmd` variants are. `api::hosted::What::Hook` is JSON by name.

## Checks run

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 `cargo clippy -- -D warnings` for `graftyard-site`, `graftyard-site --features test-faults`, `graftyard-edge`, `graftyard-edge --features test-faults`, and `graftyard-ops`: clean.
- `cargo test --workspace`: 72 suites, 628 passed, 0 failed, 3 ignored.
- `python3 scripts/check_design.py`, `check_names.py` (1,846 files), and `check_links.py` (2,869 citations): OK, after the merge of `cadf83d`.

## What the hosted half needs

- The site's `reads()` to list 32.16 (`workers/site/src/family.rs` already renders `RepoOut::HookCursor`), then a site day and `FormatsEnable` for 32.16.
- A `HookSequencer` Durable Object class in the site scripts, one per hook owner on its home site. It would store the `Sequencer`'s rows in SQLite, serve `sequencer_route`, and hold the leader-side consumer that calls `plan`, `consumer_start`, and `cursor_to_commit`. DESIGN's Stack, "Failure modes" (`event pipeline`), and "Deployment modes" rows already name it.
- Delivery Workers that take `Start`s, POST `webhooks::request` with a 10 s deadline, and call `report`. The secret is unwrapped through `graft-ops`, so `graft-ops` needs an unwrap route; only `/wrap` is built.
- `graft-ops`'s Secrets Store bindings for `SecretsWrapKey` (`SECRETS_WRAP_KEY_A`/`_B`, `SECRETS_WRAP_KEY_CURRENT` [inference: the design names the secret, not its bindings]).
- The edge's sequencer routes wired to the object, in place of the 503.
- The `Audit` and `Search` consumers and Basin Pipelines, which this round did not build.
- A hosted BF-57 and BF-58, against real hooks and a real site kill.

## Contradictions and open points

- SHARED, "Event fan-out": "`pub struct HookDelivery { pub guid: Uuid, ... pub entry: (ChainId, Index, u16)`". Here `guid` is a derived UUIDv8 (`Guid`), as SHARED's own "Delivery identity" bullet asks ("derives from (restore generation, chain, index, event position, hook)"), and `entry` is `Option`, because "`POST .../hooks/{id}/pings` queues one with no chain entry".
- SHARED, "Event types": "`Hook { id: HookId, record: Option<HookRecord>, actor: Actor },`". The build adds `created: bool`, because SHARED's "The consumer queues a `ping` delivery when it reads a new `Hook` entry" needs the entry to say it is new: the fold the consumer reads may be ahead of the entry. The actor is `Option<AccountId>`, as the other repository events carry it.
- BUILD-PLAN M4-WP1: "The `webhooks` surface at T0, including `merge_group`, `pull_request.enqueued`, and `pull_request.dequeued`". GHES 3.22's description has none of the three (`x-webhooks` lacks `merge-group-*`, `pull-request-enqueued`, and `pull-request-dequeued`), so these payloads are checked against dotcom's description at the pinned commit, not the T0 oracle.
- DESIGN, Shape, `HookSequencer`: "Delivery Workers take attempts from the site's queue." The pure model hands out attempts from the sequencer (`start`), which enforces one in flight per queue and the owner's cap. A Queues message per attempt would have to carry the attempt the sequencer started, not pick work itself.
- Not a contradiction: `repository_hooks`, the App permission the hook commands take, is in GHES 3.22's `app-permissions` schema (checked against `ghes-3.22.json`).
