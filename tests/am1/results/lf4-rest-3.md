# A check-run create's id from the `IdBlock` cursor in `decide` (2026-11-12)

The follow-up to `lf4-rest-2` (`tests/am1/results/lf4-rest-2.md`), whose contradiction 2 was an edge-minted check-run id. The edge made it from its millisecond clock and the request id's low 10 bits, and `decide` refused a collision with 422. Every other id in the repository comes from the tier's `IdBlock` cursor inside `decide`. This branch does the same for check runs, and reruns LF-4's check-run part on `graft-m3c` with the 16 creates sent at once. Branch `lf4-rest-3`, from `origin/main` `e0bbe8a` (the `lf4-rest-2` merge).

## Summary

- **The id comes from the block.** `POST /repos/{o}/{r}/check-runs` plans `IssueOp::CreateCheck`. `decide` gives the run its id from the metadata tier's cursor (`next_id(Tier::Metadata)`), and gives the suite the same id. The tier's `CheckRun` write moves the cursor past it, so the next create in the same entry or a later one takes the following id. A repository with no ids left refuses with `NeedIdBlock`; the site's lease loop re-sends the command once a block commits, and a client that still sees the refusal gets 503. The 201 renders the id from `&Acked`. A re-sent request replays its first id. The edge mints nothing.
- **Hosted, LF-4: pass, 8 of 8 resources, 16 of 16 each.** The 16 check runs were sent concurrently under m3-ci's installation token. They answered 16 × 201 with 16 distinct ids, 917520 to 917535. That is one contiguous run of offsets 16 to 31 in block 14 of cell 0, every one from the repository's leased block. Each run's `check_suite.id` equals its id. The tip's check-run list held 16 of 16 views. The other seven resources also passed 16 of 16. All eight controls without the media type answered 200 with no `graftyard_trust`. The write controls were a check run under a user's PAT (403) and a review comment on an issue (404).
- **Validation:** 18 hosted REST replies, 0 failed. These include the capture's own `check-run-created` 201, id 917536, the block's next id after the run's 16.
- **Cost:** $0.05 for the window (06:09Z to 06:30Z; containers $0.02). Cumulative across `lf4-rest`, `-2`, and `-3`: $0.75 of the $15 cap.

## The change

| Where | What |
| --- | --- |
| `crates/git/src/issue_ops.rs` | `IssueOp::CreateCheck(CheckRunUpdate)`, appended (tag 40). It decides on the same path as `RecordCheck`: Write permission, the App check, the taken-id rule, and `RequiredCheck`. The run and suite ids come from `next_id(Tier::Metadata)`, and the version is forced to 1. It answers `IssueOut::Id`, and `replay` answers that id again. The App permission is `checks: write`, and the `graftyard/` agent refusal applies to it as it does to `RecordCheck`. `RecordCheck` is unchanged, as are its wire bytes, its versions 2 and up, and Actions' writes |
| `crates/git/src/tiers.rs` | `RepoEvent::CheckRun`'s metadata write calls `assigned`, so the cursor passes a created run's id. A run id from outside the tier's block, such as an existing `RecordCheck`'s, moves nothing |
| `crates/git/src/issue_ops/call.rs` | Repository format 32.18 (`REPO_FORMAT_CHECKS`), for `CreateCheck` only. A release that reads only up to 32.17 would apply such an entry without moving the cursor, and could hand the same id out again, so the command needs its own format. It always carries an App's claims |
| `workers/site/src/family.rs` | The site reads 32.18 |
| `crates/api/src/rest.rs`, `hosted.rs` | `plan_create_check_run` takes no id. `check_run_created` renders the acked id. `What::CheckRunCreated { id }` keeps its field and is sent as 0; an `lf4-rest-2` edge's minted id is read only when the ack carries none |
| `workers/edge/src/api.rs` | The mint is gone |
| `crates/git/tests/wire_tags.golden` | One line appended: `IssueOp` 40 `CreateCheck`. No other line changed |

The edge needs `32.18` in `FORMATS_ENABLED` once every member reads it (`tests/am1/lf4/siteday.sh`). `build/BUILD-PLAN.md`'s formats line and F61 record the format.

## Tests and plants

- `crates/git/tests/agent_gates.rs` `check_run_creates_take_distinct_ids_from_the_block_in_decide`, on three real members:
  - two creates in one entry and a third in the next get distinct ids, all inside the leased block, whatever ids the command carried;
  - each run's row holds its own run and suite id;
  - another App's name is refused;
  - the command crosses as format 32.18 and decodes back;
  - the same request re-sent answers its first id.
- `crates/api/tests/surfaces.rs` `rest_writes_of_review_comments_and_check_runs_render_ghes_shapes`:
  - two creates in one entry and one in the next, with distinct ids from block (0, 1);
  - the 201 renders the acked id, and the suite id equals it;
  - `NeedIdBlock` renders 503;
  - a version-2 `RecordCheck` under the created id updates the run;
  - `What::CheckRunCreated` through `render_ok` renders the acked id, and 500 with no id.
- Plants (`tests/am1/lf4/plant.py`, mutants 25 to 30): 6 of 6 caught. With `lf4-rest-2`'s 16 to 24 and 31 rerun, 16 of 16. The new plants are:
  - the create keeps the edge's id;
  - the `CheckRun` write does not move the cursor;
  - the suite keeps the edge's id;
  - the 201 renders the minted id instead of the acked one;
  - a re-sent create answers `Done`;
  - the create is sent in format 32.5.
- Full checks on `3ee7e69`, all exit 0:
  - fmt and workspace clippy;
  - wasm32 clippy for site and edge, each with and without `test-faults`, and for ops;
  - `cargo test --workspace`: 701 passed, 0 failed;
  - `go test` in `containers/git/server`;
  - the three `scripts/check_*.py`.

## Hosted run

| Step | Result |
| --- | --- |
| Coordination | After m0-carried-2's "graft-m3c free" (06:13Z), announced to the sharing threads; hot-shards, merge-budget-2, and narrow-reads queued after |
| Site day (`tests/am1/lf4/siteday.sh siteday3.log`, 06:10Z to 06:19Z) | `3ee7e69`, `--test-faults`, the install's anchor, the same shard image: site a `01ab8c31`, b `94dc8839`, c `43f36e9d`. One deploy, no restart |
| Edge | `e13f08a2`, `OPS = graft-m3c-ops`, `FORMATS_ENABLED` as before plus `32.18` |
| LF-4 (`tests/am1/lf4/hosted2.sh 3 lf4-c3`, `tests/am1/rest/lf4.mjs`; tag `mwd5sc1q`, untrusted user 7100514) | writes 16 of 16 for each of the 8 resources; check runs 16 × 201 sent concurrently, ids 917520 to 917535, 16 distinct, 1 block; reads 16 of 16 for each; controls 200 with no `graftyard_trust`; write controls 403 and 404 |
| Capture and validation | 18 REST replies, 0 failed (`tests/am1/raw/lf4/hosted-replies3/`, kept locally) |
| Teardown | `git#0#net:65547` and `65548`, 2 of 2 answered 200; no pool shard was running |

## Contradictions and notes

1. `lf4-rest-2`'s contradiction 2 is gone. A REST create's id comes from the block, so concurrent creates never collide. The taken-id rule stays for `RecordCheck`, whose ids come from outside (Actions' `RunOwner`).
2. GHES 3.22's description still lists only 201 for `POST check-runs`, so the 403 for a user's token is not validated (`lf4-rest-2`, note 1; F56's kind).
3. `What::CheckRunCreated { id }` keeps a field that only an `lf4-rest-2` edge fills. Removing it would break that edge's commands mid-rollout, so it stays until no such edge runs.
