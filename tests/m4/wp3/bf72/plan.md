# BF-72 hosted plan: reusable workflows on graft-m4a

This row runs only on the lead's GO, after B0-07 r2 and AM2's kill row on graft-m4a. Stop at $3. The row it gates is BF-72 in `build/BUILD-PLAN.md`, M4-WP3, and the design is `decisions/DECISIONS.md`, "`reusable`".

## What it proves

An unmodified `actions/runner` 2.338.2 runs `tests/m4/wp3/bf72/caller.yml` in `m3org/bf72`. Its job `call` `uses:` `m3org/bf72-shared/.github/workflows/deploy.yml@v1`, where `v1` is both a tag and a branch whose file differs. The row passes on the same nine lines as the local row (`tests/m4/wp3/bf72/run.sh`, raw log `tests/m4/raw/wp3/bf72/run.log`), read from the job logs by `bf72.mjs`:

1. Every job succeeds: `build`, `call / deploy` and `after`.
2. The called job runs as `call / deploy`.
3. Its inputs arrive typed: `target=prod-7` (from `needs.build.outputs`), `dry=true` and `dry == true` true.
4. It gets the inherited repository secret `DEPLOY_KEY`; the job prints only its hash's first 16 hex digits, checked against the value the driver set.
5. The file ran is the tag's, not the branch's (`version=v1`, `url=…/v1`).
6. `after` reads `needs.call.outputs.url` = `https://prod-7.example/v1` and `needs.call.result` = `success`.
7. The called job's ID token has `job_workflow_ref` = `m3org/bf72-shared/.github/workflows/deploy.yml@refs/tags/v1` (F118).
8. Its `workflow_ref` stays the caller's: `m3org/bf72/.github/workflows/caller.yml@refs/heads/main`.
9. Its token carries the caller run's `actor` (the dispatching admin) and `run_number` (the run's place among `caller.yml`'s runs in `m3org/bf72`), as BF-73's rows record them for a planned run. This needs 0x15.8 enabled on the edge, which BF-73 already did on graft-m4a.

Fail lines are BF-72's: a called job that does not run, a wrong input, secret, or output, or a `job_workflow_ref` naming the caller.

## Steps

1. **Site day** from the BF-72 head, test-faults build: `bash tests/m4/wp3/bf72/days.sh <tree> site`. It keeps every flag BF-71's site day set and adds `GRAFTYARD_REUSABLE=1`, so the trigger may plan runs, and ships 0x15.9's reader. This must come before step 2, because a leader that cannot read `RunPlanned` must never see one.
2. **Edge day**: `bash tests/m4/wp3/bf72/days.sh <tree> edge`. It adds 21.9 (0x15.9) to the live `FORMATS_ENABLED` and keeps the other plain-text bindings; the binding diff may show only `FORMATS_ENABLED`. Then `days.sh <tree> check`.
3. **The row**: `source ~/.graft-m4a/env-m4a.sh; node tests/m4/wp3/bf72/bf72.mjs run <out> <runner dir>` with one runner slot (the jobs run in sequence). It creates both repositories, pushes the tag and the decoy branch, sets `DEPLOY_KEY` through `gh secret set` on stdin, dispatches `caller.yml`, and deletes `DEPLOY_KEY` after.
4. **Sheds**: the `tieroff503.py` line the driver prints, over the row's window.

The dispatch reads the called file from the other repository through the edge's dispatch path, which resolves `v1` tag-first and pins the commit. The push trigger cannot call another repository yet (F132), so the row uses the dispatch.

## Cost

One site day and one edge day as BF-71's, plus three jobs of a few seconds each on one runner, about a minute of runner time. BF-71's days and row came in under $1 [inference: from BF-71's own row on the same install]. The stop is $3: past it, stop and report.

## If it fails

- A dispatch refused with "a reusable workflow needs Actions format 0x15.9, which is not enabled": step 2 did not take. Read `days.sh check`.
- `called job not run`: read `polls.jsonl` for a job stuck `Pending`, which would be a `needs` index fault in the plan.
- Wrong `job_workflow_ref`: read the token claims printed in `call / deploy`'s log.

A second fail on the same cause stops the row, with the analysis.
