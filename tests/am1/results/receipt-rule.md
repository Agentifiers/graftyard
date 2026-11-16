# `receipt-rule`: a busy leader's slow receipt is waited on, and validate reads no `Names` rows

Branch `receipt-rule`, off main `aa88910a`. No hosted run yet; B0-32's rerun, with this branch's push log line, is the
hosted check.

## Why

"m3fr8 after the run" in `tests/am1/results/lm1-rows-m3f.md` traced 80 burst pushes. Three findings drive this branch:
- **The commit stage.** It took 5.5 s at the edge against the leader's own 427 ms. 110 of 121 extra `/commit` sends followed a missed receipt from a leader that answered 200 throughout. 82 `suspect` waves went out, 27 to 32% of each follower's inbound.
- **Validate.** Its 3.0 s was the network shard's two `Names` reads before its forward to the pool shard, which itself took 370 ms.
- **Two inferences.** Isolate queueing and the per-isolate stage-first signal could not be tested without an isolate id.

## The change

- **Item 1, the receipt rule.**
  - The rule is in `design/DESIGN.md`, "Commit and read paths", and `decisions/DECISIONS.md`, "`receipt-rule`"; the code is `crates/lookup/src/lib.rs`, `receipt`, `heard`, `busy`, and `RECEIPT_OPEN`.
  - A missed receipt from a member that answered this isolate 200 within `COMMIT_RECEIPT` in the current term keeps its send open up to `RECEIPT_OPEN` (8 s [guess]), with no `suspect` and no resend. Meanwhile the edge asks `/outcome_by_request`, without the pack (`workers/edge/src/lib.rs`, `lookup_body`).
  - Only the send's own 200, or a `done`, `NotLeader` or `pending` by request id, is a reply.
  - A member that never answered, or that answers an error status, is reported as before.
  - Nothing in `crates/raft` changed. The replay check needs only the push's ref names, so no digest is needed.
- **Item 2, validate and `touch`.** The edge's `/validate` carries `record`. A shard writes `last_request` and its alarm at most once per `TOUCH_EVERY` (1 min, `graftyard_git::touch_due`).
- **Item 5, the push log line.** A test-faults edge logs one line per push: `{"push", "isolate", "open", "status", "timing"}`, where `isolate` is a token drawn once per isolate and `open` is the pushes that isolate held at the push's start (`workers/edge/src/lib.rs`, `push_log`).

## Checks

- `a_slow_200_from_a_busy_member_is_the_reply_with_no_resend_or_report` and `a_member_that_never_answered_is_still_reported` (`crates/lookup`). With main's rule in the same harness, where every miss at `COMMIT_RECEIPT` is reported, both fail; with the branch's rule, both pass.
- `a_resent_request_is_answered_from_its_row_and_applied_once` (`crates/git/tests/push.rs`) now also asks with the pack removed and gets the same refs and report-status.
- `a_push_asks_for_its_outcome_without_its_pack` and `a_validate_url_names_the_packs_trailer_its_class_and_the_record` (`workers/edge`). On main the second has no record to find, and `validate_url` takes no repository record.
- `a_shard_records_its_last_request_at_most_once_a_minute` (`crates/git/tests/shard.rs`). On main there is no `touch_due`, and every request wrote.
- `tests/m0carried/plant24.py`: 14 of 14 caught.
- Full checks on the branch: see the commit's report.
