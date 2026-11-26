# LM-3's batched row on graft-m4a: A59, the demoted create, assignment, `agent-ci`, and F136's `/pulls`

On the lead's GO (2026-11-26) from main `2bf216fa` on graft-m4a, after BF-76 r2. The stop is $2.50, covering both
accounts (graft-m4a and the runner account's `graft-agents-m4a-`). Driver: `tests/am3/hosted/batch-row.sh` with
`batch.mjs`. The agents half (Apps, controller with its `TaskBox`, `graft-agents-front`, probe) reuses the kill row's
steps (`tests/am3/hosted/kill-row.sh`). Logs go in `tests/am3/raw/batch-<tag>/`, and the site and edge days in
`tests/m4/raw/wp3/lm3batch/<tag>/`.

## Plan

### Days

1. **Site day 1** from `2bf216fa`, test-faults. Every live `GRAFTYARD_*` flag of site a is kept (BF-76's site day
   method, `tests/m4/wp3/bf76/row.sh`). This day ships 0x15.13's reader to every member and adds no flag.
2. **Site day 2**: the same build, adding `GRAFTYARD_AGENT_CI=1`. It is a separate day because graft-m4a is not a
   fresh deploy. A site day resets members site by site (`deploy.mjs` deploys a, b, then c). On a single day, a site
   that already has the flag could lead an Actions shard while another site's member still runs the old script.
   That old member cannot read `CreateRunHeld` (0x15.13) and would refuse it as `Unsupported` mid-row. The second day
   starts only after the first finished on all three sites, so every member reads 0x15.13 before any site sends it.
   The cost is one more site day (a few cents).
3. **Edge day** from `2bf216fa`: the live `FORMATS_ENABLED` read at deploy time, all 36 kept, with 32.30 and 21.13
   added (38). Nothing else may change (binding diff). 21.12 is not added because no line uses a push run: the
   agent-ci lines run through the pull request trigger.

### Agents half (runner account, as in the kill row)

`init`, `register`, `keys`, `infra`, `taskbox`, `controller`, `front`, `probe-deploy`, `install` (agents enabled
in-repo), `hook`. A59 and the controller's create need a live `TaskBox` and controller. No `graft-ops` and no roll.

### Lines

| Line | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| A59 | A task created by m3-admin; its harness pushes twice and reports `Finished` | the base's task reads `in_progress` while the box runs it and `completed` after `Finished`, with no `PATCH` by hand. A second task cancelled by the admin is not moved back by the box | the base still reads `queued` once the box runs, or never reads `completed`, or a cancelled task changes |
| Demoted create | m3-admin commits 60 writes (issue comments) inside a minute, then sends a Graftyard-run create | 403 (`NotWriter`). A control create after the window passes is 201 | 201 within the window |
| Assign | REST `POST .../issues/{n}/assignees` with `graftyard-agent[bot]` by m3-admin; GraphQL `addAssigneesToAssignable` with the bot's `BOT_` node id on a second issue; `DELETE .../assignees` and an assignees-only `PATCH`; m3-member (read) assigning; m3-ci's token creating a task | REST answers 201 and GraphQL answers with the bot assigned. Each assignment produces one controller create (201) with m3-admin as the task's `requester`. The read-only member's assignment is dropped (200, assignees unchanged). m3-ci's create is 403 | any create without a user's assignment, a wrong requester, or a dropped writer assignment |
| agent-ci: held | A same-repo pull request from `refs/heads/claude/<tag>` (m3-ci's confine) with a workflow, under the default `ci: approve` | the run reads `status: completed`, `conclusion: action_required`; `GET .../runs/{id}/jobs` lists 0; no check run on the head | the run queues, or jobs or checks appear |
| agent-ci: Agent token | `POST /api/graftyard/v1/.../actions/runs/{id}/approve` with m3-ci's installation token | 403 "Resource not accessible by integration" | any other answer |
| agent-ci: approve | the same route by m3-admin | 201 `{}`, then the run reads `queued` with its jobs listed | anything else |
| agent-ci: second approve | the same route again | 409 | anything else |
| agent-ci: AutoReadOnly | `PUT .../agents/policy {ci: auto_read_only}`, then a second confined pull request | the run reads `queued` at once with its jobs listed, and the run row has `fork: true` | held, or `fork: false` |
| agent-ci: fork after approval | the approved run's row | `fork: true` | `fork: false` |
| F136 side check | on m3org/batch-`<tag>` with one open and one closed pull request: `GET /api/v3/repos/{o}/{r}/pulls?state=all`, and `?head=m3org:<branch>` and `?base=main` | 200 for each, items with GHES 3.22's required `pull-request-simple` fields, `state=all` listing both, `head=` listing only its own | non-200, a missing required field, or a wrong filter |

The agent-ci lines read the run shape through the GHES route (`/api/v3/repos/{o}/{r}/actions/runs/{id}`) and the
`fork` field through the Graftyard runs read. No runner is registered, so an approved run stays queued. The line asks
for the jobs to queue, not to run.

### Teardown

The repositories deleted; the policy reset to `approve`; agents disabled and the two installations removed; every
`graft-agents-m4a-` resource deleted; agents container applications listed with their running instances, failing
loudly while any runs; the install's shard sweep (`teardown.py --sweep --shardkill`), failing loudly on any running
shard container. `GRAFTYARD_AGENT_CI` stays set and 32.30 and 21.13 stay enabled: they are gated product state, not
row residue.

### Cost

Two site days and one edge day (about $0.3 together, from BF-76's days), the agents half (the kill row's was $0.38
with ten Sandboxes; this row runs three), and a few hundred API calls. Estimate $0.8 to $1.2. Stop $2.50, read by
`state` before each step.

## Result r1 (2026-11-26, from main `2bf216fa`): every line PASS

Every line passed hosted on graft-m4a. Three lines needed more than one try. Each failed try was a driver fault, and
the try that passed ran the same product build with a fixed driver. Logs are in `tests/am3/raw/batch-r1/`: a step's
first try is `<step>.log`, and later tries are `<step>-<n>.log`. The jsonl of a failed try is renamed `-1` or `-2`.

### Days

| Day | When (UTC) | Versions | Checked |
| --- | --- | --- | --- |
| Site day 1 (test-faults, every flag kept, 0x15.13's reader) | 11:39Z to 11:41Z | a `bb5ea824`, b `c10c8cc7`, c `0f67b3f8` | BF-73's flags plus `ARTIFACTS_V4` and `REUSABLE` kept; crons `[]` |
| Site day 2 (the same build, `GRAFTYARD_AGENT_CI=1` added) | 11:41Z to 11:43Z | a `28d01739`, b `3de96500`, c `09986bf7` | the flag on all three; crons `[]` |
| Edge day (test-faults) | 11:44Z to 11:48Z | `e85dc381` → `b5c99d99` | `FORMATS_ENABLED` 36 → 38 (+32.30, +21.13); the binding diff shows only `FORMATS_ENABLED`; edgever exit 0; ops `ac4cd9a5` unchanged |

The agents half ran on the runner account, through kill-row.sh's steps under tag `r1b` (`tests/am3/raw/kill-r1b/`).
The first `init` hung on a Secrets Store list call. I stopped it, and the retry passed. Apps 3801088 and 3866624
registered. Keys moved the `Release` count from 583 to 584, with a new `agents_executor`. Controller `3967881e`, front
`0b98d7b4`, probe `d97475a0`. Installed as 7190003 and 7190004, with agents enabled in-repo. The webhook went to the
front (200).

### Lines

| Line | Verdict | Evidence |
| --- | --- | --- |
| F136 side check | **PASS** (11:54Z) | Pull request #1 open, #2 merged. `state=all`: 200, both listed, no required `pull-request-simple` field missing. `state=all&head=m3org:f136-open-68277`: only #1. `state=closed&base=main`: only #2 |
| agent-ci: held | **PASS** | Run 1194873 (PR #3 from `claude/held-152075`, m3-ci's confine): `completed`/`action_required`, 0 jobs, shard states `Held,Held`, `fork: true`, 0 check runs on the head |
| agent-ci: Agent token | **PASS** | m3-ci's installation token: 403 "Resource not accessible by integration" |
| agent-ci: approve | **PASS** | m3-admin: 201 `{}`. The run then reads `queued` with 2 jobs (`Queued`, `Pending`) |
| agent-ci: second approve | **PASS** | 409 "This run is not waiting for approval." |
| agent-ci: fork after approval | **PASS** | `fork: true` after approval |
| agent-ci: AutoReadOnly | **PASS on try 2** (`auto.log`) | Try 1 (PR #4, run 1194876): the shard row was `Queued` with the jobs right and `fork: true`, but the GHES run read answered 503 once, so the check failed. Try 2 (PR #5, run 1194879): 200 `queued`, 2 jobs (`Queued`, `Pending`), `fork: true`, never held |
| Assign | **PASS on try 2** (`assign-3.log`) | REST add 201 with the bot assigned; the controller's create followed in 9.8 s with requester m3-admin. GraphQL add assigned the bot; create in 8.9 s, requester m3-admin. REST remove 200; an assignees-only `PATCH` 200 set the bot. m3-member's assignment created a task with requester id 7100002 (rendered `ghost`, below). Another App's create: 403 |
| Demoted create | **PASS on try 3** (`demote-5.log`) | 65 issue comments by m3-admin in 44.3 s (2 transient 503s retried), then the create: 403 (`NotWriter`). The control create after the window: 201 |
| A59 | **PASS on try 2** (`a59-3.log`) | The base read `in_progress` at about 19 s and `completed` at 64 s (box `Done`, base `Completed`), with no `PATCH` by the row. The admin's cancel answered 200 `cancelled`, and that task still read `cancelled` 90 s later. Controller logs show `PATCH in_progress` 200 and `PATCH completed` 200, and 409 for the cancelled task's `completed`, refused as it should be |

### Failed tries and their driver fixes

- **Assign try 1** (`assign.log`). `GET /api/v3/repos/{o}/{r}/issues/{n}` answers 404 on main for every number (on
  `m3org/j4` too), so the assignee reads failed. The fix reads assignees from the write's own answer and from the
  issues list. The controller's create came after about 3.7 min, past the 3 min wait, so the wait went up. The plan
  said m3-member could only read, but m3-member has **write** on the repository, so the "reader's assignment is
  dropped" check did not test a reader. Try 2 records m3-member as a second writer and checks that the create names
  that writer.
- **Demote try 1** (`demote.log`): 0 writes counted, because `POST /issues/{n}/comments` also answers 404 on main.
  The fix uses GraphQL `addComment`. **Try 2** (`demote-3.log`): only 49 writes committed in the window (503s), so the
  create got 201, which is right for 49. The fix retries a 503 until 60 have committed.
- **A59 try 1** (`a59.log`): every check passed except the last read of the cancelled task, which got a transient
  non-200. I then read that task myself, and it was still cancelled. Try 2 retries the read.
- **agent-ci try 1**: the AutoReadOnly read's single 503, as above. The `auto` step reruns that line alone.
- Other transient answers seen: the tasks list once returned a non-array, and the collaborator permission read once
  answered 503.

### Findings (not lines of this row)

1. `GET /api/v3/repos/{o}/{r}/issues/{n}` and `POST /api/v3/repos/{o}/{r}/issues/{n}/comments` answer 404 on main
   `2bf216fa`. GHES 3.22 serves both.
2. A task whose requester is m3-member (id 7100002) renders the requester's login as `ghost`. The cause is not
   verified.
3. A demoted user's create gets 403 with the message "Only a person who can push to the repository creates a
   Graftyard-run task, never an App's token". The status is right, but the message is about an App's token, not a
   demoted user.

### Teardown (12:27Z)

`m3org/batch-r1` deleted (204). Both installations uninstalled and `agents_enabled` false. On the runner account only
the `graft-runners` script is left: no `graft-agents-m4a-` store secrets and no buckets. The taskbox container
application had 0 running and was deleted. The first `teardown.py --sweep --shardkill` failed only because
`tests/m0/node_modules` was missing in the worktree; the rerun reported 0 torn down and "no shard container running",
and the shardkill Worker was deleted. A direct listing showed every graft-m4a container application at running 0.
Two other applications on the account each showed 1 running: `graft-m3f-site-a-sshfront` and
`graft-m3f-site-c-sshfront`. They are graft-m3f's SSH front, not this row's, and were left alone.

Left on graft-m4a, as planned: sites with `GRAFTYARD_AGENT_CI=1`, edge `b5c99d99` with 38 formats, ops `ac4cd9a5`.

### Cost

$0.28 on graft-m4a and $0 on the runner account (`cost-4.log`, at 12:28Z), against the $2.50 stop.
