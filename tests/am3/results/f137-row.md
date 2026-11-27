# F137's hosted row on graft-m4a: issue writes (32.35), the label update (32.36) and milestones (32.37)

On the lead's GO (2026-11-27), at a **$1.50 stop** across graft-m4a and the runner account (`graft-agents-m4a-` and
`graft-runners`), counted from the row's `repo` step. Built from main `b12a07a0` with test-faults: F149 (`4fb7f48e`) and
LM-1's F150 on top of it. F150's change is site-only (`workers/site`, Actions 0x15.15), so the site day also ships its
reader and saves graft-m4a a day of its own. Its op is sent only under `GRAFTYARD_SECRETS_BRANCH`, which this row never
sets: `site` refuses to start if site a has it, and any site that has it afterwards is a `SITE DIFF`. `workers/edge`,
`crates/api` and `web/` are the same at `4fb7f48e` and `b12a07a0`. The row starts only after the lead forwards
AM2's rerun teardown line (L0-14 and L0-12 run 2 hold graft-m4a until then).
Driver: `tests/am3/hosted/f137-row.sh` with the lines in `tests/am3/hosted/f137.mjs`. Logs go in
`tests/am3/raw/f137-<tag>/`, and the days in `tests/m4/raw/wp3/f137/<tag>/`.

What it proves hosted: `decisions/DECISIONS.md`, "`issue-writes`", "`label-routes`" and "`milestones`" (head 1, head 2
and F149), each native-tested and plaindiff-checked at $0 and waiting for "the next graft-m4a row that does a site day
and an edge day enabling" its format.

## Plan

### Live state, carried forward exactly

Read on 2026-11-27 at 09:00Z (`state` reads it again before every step):
- Sites a, b and c: `GRAFTYARD_AGENT_CI=1` and `GRAFTYARD_SECRETS_WAIT=1` (LM-1's L0-04 day), with the other live
  `GRAFTYARD_*` flags. The site day keeps every one of them (BF-76's method) and adds none. `site` diffs each site's
  plain-text vars before and after, and any change is `SITE DIFF`.
- Edge `50b5577a`: `FORMATS_ENABLED` holds 41 formats, 21.14, 32.32 and 32.33 among them. 32.34, 32.35, 32.36 and 32.37
  are all off.
- `edge` adds only the formats of 32.35, 32.36 and 32.37 that are off when it reads them, and logs which ones. It never
  adds 32.34. Its check passes only when `FORMATS_ENABLED` gained exactly those, lost none, and no other plain-text var
  moved (`EDGE DIFF` otherwise), and edgever exits 0.

### Steps

1. `state`.
2. `repo`: m3org/f137-`<tag>` with branch `f`, pull request A, issue I with one comment, and the label `plain`. It also
   checks that m3-ci's installation holds `issues` and `pull_requests` write (`tests/am1/hosted/app-perms.mjs` set them).
   The cost clock starts here.
3. **`base`, the plain-client baseline, before any day.** Each request answers as main does with 32.35 to 32.37 off. Any
   difference is a FAIL, and the row stops. Steps 4 and 5 refuse to run without a passing `base`:
   - The issue `PATCH` with title and body, comment `PATCH`, lock and unlock, label `PATCH`, the milestone list, create
     and read, and an issue `PATCH` naming `milestone`: each is 404 `Not Found`.
   - One label by name: 200.
   - An issue create naming `milestone`: 422 `invalid`.
   - `Repository.milestones` and `milestone`: `undefinedField`.
   - gh's own `IssueUpdate` document: routed nowhere (`a mutation runs through plan`).
   - `createIssue` and `updatePullRequest` with `milestoneId`: "milestoneId is not served".
   - `Issue.milestone`: `null`.
   - `gh label edit` and `gh issue edit -m`: both fail.
   - Issue I, pull request A and the label read the same before and after, over REST and GraphQL.
4. `site`: one site day from `b12a07a0`, test-faults, flags kept, nothing added, `GRAFTYARD_SECRETS_BRANCH` unset. It
   ships the readers for 32.35 to 32.37 and F150's 0x15.15 to every member.
5. `edge`: one edge day from `b12a07a0`, as above. It runs only after the last site day deployed all three sites with
   their vars unchanged.
6. `octokit` (Octokit 22.0.1 and `@octokit/auth-app` 8.3.1 from `tests/m0/wp15`):
   - **Issues:** `issues.update` with title and body in one `PATCH`; `updateComment`; `lock` and `unlock`.
   - **Labels:** `getLabel` (case-insensitive); `updateLabel` with a rename, colour and description; `deleteLabel`.
   - **Milestones:**
     - `createMilestone` twice: numbered, and the creator is the caller. A duplicate title in another case is 422.
     - `getMilestone`; `updateMilestone` to close one; `listMilestones` open and closed; `listLabelsForMilestone`;
       `deleteMilestone`.
   - **An issue's milestone:**
     - An issue `PATCH` sets the milestone, and the milestone's `open_issues` reads 1.
     - An unknown number is 422 and changes nothing. `null` clears the milestone.
     - `issues.create` with a milestone.
   - **m3-ci:**
     - A `pull_requests`-only installation token creates a milestone whose creator is `m3-ci[bot]` (type `Bot`), then
       deletes it.
     - A `contents`-only token is refused 403 "Resource not accessible by integration".
7. `gh` (2.102.1, m3-admin's PAT in `GH_ENTERPRISE_TOKEN` only):
   - **Labels:** `label create`, `label edit -d` and `label delete`.
   - **Issues:** `issue edit -t -b`, `-m`, `--remove-milestone`, and `-t -b -m`; `issue create -m`; `issue view --json`.
   - **Pull requests:** `pr edit -m` and `--remove-milestone`.
   - **The lead's line:** gh's own `IssueUpdate` document (the one `gh issue edit -m` sends) with an unknown `milestoneId`,
     once a deleted milestone's id and once an issue's id. Each is `NOT_FOUND`, and issue I's title, body, milestone and
     `updated_at` read the same before and after, over REST and GraphQL. An unknown title never reaches the edge,
     because gh resolves `-m` by title first. The row logs gh's exit status for it and does not count it.
8. `teardown`: the repository is deleted, then the shard sweep runs with `--shardkill`, failing loudly on any running
   shard container. 32.35 to 32.37 stay enabled: they are gated product state.
9. `cost`, then a results section in this file.

Every write in steps 6 and 7 is read back twice: over REST (`GET issues/{n}`, `labels/{name}`, `milestones/{n}`,
`issues/{n}/comments`) and over GraphQL (`issue` or `pullRequest(number:)` with `title`, `body`, `milestone` and
`comments`; `labels`; `milestone(number:)`; `milestones(states:)`). Lock is the one exception: its read-back is REST's
`locked`, because GraphQL's `Issue.locked` is not served (next section).

### Gaps the row records but does not count

A capture of gh 2.102.1 against the pinned SDL (2026-11-27) shows three gh commands whose writes no format serves.
`gh` runs each one and checks only that it changes nothing:
- `gh issue lock` reads `Issue.locked` and `activeLockReason`, and `gh issue unlock` sends `unlockLockable`. Neither is
  served over GraphQL. REST's lock routes (32.35) are what Octokit calls.
- `gh issue comment --edit-last` sends `updateIssueComment`, which is not served. REST's comment `PATCH` (32.35) is
  what Octokit calls.
- `gh issue list -m` searches with a `milestone:` qualifier, which `search` refuses (`INVALID_QUERY`).

They are F152, F153 and F154 in `decisions/OPEN-QUESTIONS.md` (the lead's ruling, 2026-11-27: unscheduled), each
citing gh's captured documents in `tests/am3/raw/f137-capture/`.

## $0 dry run, 2026-11-27

The four lines ran against the edge's and the site's own test-faults builds, in local workerd on plaindiff's bed,
with two edges of the same build:
- Edge 0 carried graft-m4a's live 41 formats.
- Edge 1 carried the same 41 plus 32.35, 32.36 and 32.37.

`F137_LOCAL=1` skips what needs a git shard, graft-ops or TLS: branch `f` and pull request A, m3-ci's tokens, and gh's
commands. gh's own GraphQL documents still run through `api`. The builds (`/tmp/pd`) are of `35ecb9ce`, which has the
same `workers/edge`, `crates/api` and `crates/identity` as `b12a07a0`. The site changes since then are F150's, in
Actions, which these lines do not reach.

| Run | Edge | Lines | Result |
| --- | --- | --- | --- |
| dry2 | `repo` and `base` on edge 0; `octokit` and `gh` on edge 1 | as the row runs them | `repo`, `base`, `octokit` and `gh` PASS: 2, 18, 24 and 5 checks |
| control | `repo` and `base` on edge 1; `octokit` and `gh` on edge 0 | swapped | `base` FAIL on 17 of 18 checks, passing only the label read, which needs no format; `octokit` FAIL on 23 of 24; `gh` FAIL on the milestone create and both NOT_FOUND lines |

The control shows that each line tells the two states apart. A baseline taken after the edge day fails, and the write
lines fail before it.

The first dry run failed one check: the milestone's creator login was written into the driver as `m3-admin`. The
driver now reads it from `GET /user`.

## Cost

Prices are from `research/prices.json`. History is from earlier rows on graft-m4a.

| Item | Basis | Estimate |
| --- | --- | --- |
| One site day (3 sites) | `lm3-l004.md`: two site days and an edge day, $0.01 by the edge step; `lm3-batch.md`: two site days, an edge day and the agents half, $0.28 | $0.02 to $0.08 |
| One edge day | the same rows | $0.01 to $0.03 |
| One repository's shard warm while the lines run (about 30 minutes, one push) | the containers' $0.000020 per vCPU-s and $0.0000025 per GiB-s [doc, `research/prices.json`]; `lm3-l004.md`'s whole row, $0.36 on graft-m4a over an hour with three runs | $0.05 to $0.20 |
| About 300 API calls and 30 gh commands | Workers and Durable Object request prices: under a cent | $0.01 |
| Runner account | nothing runs there | $0.00 |
| **Total** | | **$0.10 to $0.35**, stop **$1.50**, read by `state` and before every line; past it the row tears down and reports |

## Result

Not run yet. The row waits for AM2's rerun teardown line.
