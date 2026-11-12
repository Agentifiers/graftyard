# LF-4 through real write routes, and the `graft-m3c` site-admin fix (2026-11-12)

The follow-up to `lf4-rest` (`tests/am1/results/lf4-rest.md`): `lf4-rest` wrote LF-4's review comments and check runs through the test-only `/_bed/identity` commit, because the edge served no write for either. This branch adds the two GHES 3.22 writes, reruns LF-4's review-comment and check-run parts through them (with all eight resources, as one run), and makes the m3 admin a site admin of `graft-m3c`'s install chain so `PUT /api/graftyard/v1/agents/apps/{id}` works. Branch `lf4-rest-2`, from `origin/main` `5e6fbe2`. Hosted on `graft-m3c` only.

## Summary

- **Both writes are on the edge, validated against the pinned description.** `POST /repos/{o}/{r}/pulls/{n}/comments` and `POST /repos/{o}/{r}/check-runs`.
- **LF-4: pass, 8 of 8 resources, 16 of 16 each, with every write through a real route.** Review comments: 16 of 16 `POST pulls/{n}/comments` under the untrusted user's own PAT answered 201 with the comment as written on its line. Check runs: 16 of 16 `POST check-runs` under m3-ci's installation token answered 201, `completed`, with the summary as written. The reads: every hidden range absent from `visible`, `invisible_chars` 2, raw text unchanged, untrusted authors `trusted: false`, and the controls without the media type 200 with no `graftyard_trust` on all 8 routes. The check-run list for the tip held 16 of 16 views.
- **The site-admin gap is fixed.** `tests/am1/rest/apps.mjs` now commits `SetMember { user: m3-admin, role: Admin }` on `authz:1` as the operator, and the m3 admin's `PUT /api/graftyard/v1/agents/apps/{m3-ci}` answered 200 (403 before). B0-36's driver flags m3-ci through that route again.
- **B0-36's labels, rerun through the flag route:** pass again. `addLabelsToLabelable` outside the allowlist was refused by the allowlist rule 100 of 100 each for m3-ci (confined), `graftyard-agent` shipped, and `graftyard-agent` widened. `Graftyard MCP` was refused 0 of 100. Inside the allowlist, 5 of 5 were accepted for every kind, and the issue ended with no labels.
- **Cost:** $0.37 for this branch's hosted window (03:36Z to 04:18Z, `INSTALL=graft-m3c tests/fanout/m2bcost.py`; containers $0.09). With `lf4-rest`'s $0.33, $0.70 of the $15 cap.

## The two writes

| Route | Plans | Renders (on the member, after the entry applied) |
| --- | --- | --- |
| `POST /repos/{o}/{r}/pulls/{pull_number}/comments` | `IssueOp::Comment` on `CommentTarget::Review`: `commit_id`, `path`, `line` required; `side` `LEFT`/`RIGHT` (default `RIGHT`); `start_line` below `line`; `start_side` equal to `side`; `subject_type` `line` only. A reply (`in_reply_to`) must name a review comment of the same pull request and takes its parent's anchor, whatever the body names. `position` alone is refused (422) | `What::ReviewCommentCreated { n }` (appended): 201 `pull-request-review-comment`, the same object `GET pulls/{n}/comments` lists |
| `POST /repos/{o}/{r}/check-runs` | `IssueOp::RecordCheck`, GitHub App tokens only (any other token 403 "You must authenticate via a GitHub App.", as GHES documents). The run's App is the token's. Its id is minted by the edge: the millisecond clock shifted 10 bits with the request id's low 10 bits. A conclusion completes the run; `completed` without one is 422; `started_at` defaults to now, `completed_at` to now for a completed run; `output` needs `title` and `summary`, at most 50 annotations | `What::CheckRunCreated { id }` (appended): 201 `check-run` |

`decide` gained two refusals on `RecordCheck`: an App's token may not record a run as another App (`Invalid`), and a first version may not take another run's id, on another commit or of another App or name (`Exists`). A late re-delivery of the same run's first version still passes, and its apply drops it by version, so the existing required-check test (`crates/git/tests/issues.rs`) passes unchanged. A user's command still names any App, as before. The edge's test-faults `/_bed/identity` route is read-only on repository chains again.

`crates/wire`'s `parse_iso` reads `started_at` and `completed_at`: `Z` or a `±HH:MM` offset, optional fractions.

**Validation** (`crates/api/gh/validate.py`, `ghes-3.22.json`):

- Local: 52 GraphQL, 99 REST, 11 LFS, and 30 webhook replies, 0 failed. The 3 new REST cases are `review-comment-created`, `review-comment-reply`, and `check-run-created`.
- Hosted: `tests/am1/lf4/capture.mjs` on `m3org/lf4-t`, 18 REST replies, 0 failed. These are the 16 GET replies as before, plus a review comment created under the admin's PAT and a check run under m3-ci's token.
- The two refusals the capture also sends are not validated: a check run under a user's PAT (403 "You must authenticate via a GitHub App.") and a review comment on an issue (404). GHES 3.22's description lists only 201 for `POST check-runs` and no 404 for the review-comment route. That is F56's kind of known mismatch, and `capture.mjs` keeps them under `<out>-refusals/`.

## Tests and plants

- `crates/api/tests/surfaces.rs` `rest_writes_of_review_comments_and_check_runs_render_ghes_shapes`:
  - a thread over lines 5 to 7, and a reply that keeps its parent's anchor;
  - 14 plan refusals;
  - a check run with a hidden character and an annotation, a conclusion that completes the run, and 7 check-run refusals;
  - the duplicate id refused while a version-2 update passes;
  - both `What` arms through `hosted::render_ok`/`render_err` after a serde round trip.
- `crates/git/tests/agent_gates.rs` `a_check_run_is_recorded_only_as_its_own_app_and_under_an_unused_id`: another App's name refused; a taken id refused on another commit and under another name; a re-delivery and an update passing; a user naming any App.
- `crates/wire` `timestamps_parse_iso_with_fractions_and_offsets`.
- Plants (`tests/am1/lf4/plant.py`, mutants 16 to 25): 10 of 10 caught. They cover:
  - a reply keeping the body's line;
  - side defaulting to LEFT;
  - `start_line` past `line` accepted;
  - a conclusion not completing the run;
  - `completed_at` not defaulted;
  - a not-pull refusal answering 422;
  - the taken-id rule removed;
  - the taken-id rule refusing re-deliveries too;
  - the App check removed;
  - an offset read with the wrong sign.

  Mutants 0 to 15 are `lf4-rest`'s.
- Full checks on `ebbc1cf`, all exit 0:
  - fmt and workspace clippy;
  - wasm32 clippy for site and edge, each with and without `test-faults`, and for ops (ops has no `test-faults` feature);
  - `cargo test --workspace`: 700 passed, 0 failed;
  - `go test` in `containers/git/server`;
  - the three `scripts/check_*.py`.

  No existing wire enum variant moved; `What` gained two variants at its end.

## Hosted run

| Step | Result |
| --- | --- |
| Coordination | After m0-carried's B0-07 and learner-dumps' read-load window ("graft-m3c free" at 02:57Z and 03:35Z); deploy announced to all four sharing threads at 03:36Z |
| Site day (`tests/am1/lf4/siteday.sh`, 03:36Z to 03:49Z) | `ebbc1cf` with `--test-faults`, the install's anchor, the unchanged shard image: site a `9c870f5d`, b `21d5a511`, c `0f4f2b84`. The supervised service restarted the script after its first pass (03:36Z to 03:45Z: `95a54d22` / `375ed2f0` / `3bcc78c6`, then `41d61aca` / `69060c95` / `04fd1207`), so the same build deployed twice more; `siteday.sh` now exits early once its log says DONE. Each site-script version resets the members, so the extra versions cost resets, not a different build |
| Edge | `aa36226e` (03:49Z), `OPS = graft-m3c-ops`, the same `FORMATS_ENABLED` |
| `apps.mjs` | m3 admin made an admin of `authz:1`; the three agent Apps unchanged (262144, 327680, 393216) |
| LF-4 (`tests/am1/rest/lf4.mjs`, `m3org/lf4-t`, tag `mwd0gg5d`, untrusted user 7100677) | writes 16 of 16 for each of the 8 resources; review comments and check runs 16 × 201 each; controls: a check run under a user's PAT 403, a review comment on an issue 404 |
| Capture and validation | 18 hosted REST replies, 0 failed (`tests/am1/raw/lf4/hosted-replies2/`, kept locally) |
| B0-36 labels (`m3org/b036-m`) | flag route 200; results above |
| Teardown | `INSTALL=graft-m3c tests/fanout/teardown.py --all-nets --pools`: `git#0#net:65543`, `65545`, `65546`, 3 of 3 answered 200; no pool shard was running |

| Resource | Write | Pass | Control |
| --- | --- | --- | --- |
| Issue | GraphQL `createIssue`, untrusted | 16 of 16 | 200, no `graftyard_trust` |
| Pull request | GraphQL `createPullRequest`, untrusted | 16 of 16 | 200, none |
| Issue comment | GraphQL `addComment`, untrusted | 16 of 16 | 200, none |
| Review | GraphQL `addPullRequestReview`, untrusted | 16 of 16 | 200, none |
| Review comment | **REST `POST pulls/{n}/comments`, untrusted PAT, 16 × 201** | 16 of 16, `trusted: false` | 200, none |
| Commit | git push | 16 of 16 | 200, none |
| Check run | **REST `POST check-runs`, m3-ci installation token, 16 × 201** | 16 of 16, `trusted: false`; the tip's list 16 of 16 | 200, none |
| Release | REST `POST releases`, admin | 16 of 16 | 200, none |

## Contradictions and notes

1. GHES 3.22's description documents only 201 for `POST check-runs`. GitHub answers 403 for a non-App token, the docs' prose says "you must use a GitHub App", and the edge does the same, so the validator cannot check that refusal. This is the same kind of gap as F56.
2. The edge mints the check-run id instead of taking it from an id block, so `decide` guards against a reused id. A real collision needs two creates in the same millisecond on one repository with the same low 10 bits of their request ids. `decide` refuses that as 422 and the client retries; it never overwrites another run.
3. `siteday.sh` under a supervised service redeployed the same build twice more. The script is now idempotent.
