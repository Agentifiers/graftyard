# LM-3's in-repo batched row: the moved clauses, LF-9's in-repo lines, A3 end to end in-repo, the L0-14 and L0-04 reruns, and the host pause

This is the plan for the lead's GO (AM3-WP3's rest, after part 5). AM3-WP3's in-repo mode is its done state until
AM3-WP2a merges (`decisions/DECISIONS.md`, "`forks-before-workspace`"), so every line runs in-repo. The fork-mode lines
(LF-9's fork pushes, B0-36's fork push after cancel, A3 in fork mode) wait for AM3-WP2a. The row runs on graft-m4a,
the install of LM-3's batched row r1 (`tests/am3/results/lm3-batch.md`), from the main commit the lead names, which
must contain the host pause (`am3-rest`, 32.33) and F137 (`issue-routes`, 32.32).

Driver: a copy of `tests/am3/hosted/batch-row.sh` as `tests/am3/hosted/inrepo-row.sh`, with its lines in
`tests/am3/hosted/inrepo.mjs`. It reuses r1's day, agents-half, teardown and cost steps. The L0-12, B0-36 and B0-37
lines reuse `tests/am1/hosted/rows.mjs` (`l012`, `b036`, `b037`) with `agentToken("graftyard-workspace")` as the
kind, and L0-14 reuses `tests/m0/run/agent-l014.ts`. Every hosted script exports `CF_SECRETS_STORE_ID` (M4-WP4's
store-id scrub, `2e472aa4`): the row's env file sets it, and the driver stops at its first line when it is unset. Logs
go to `tests/am3/raw/inrepo-<tag>/`, and the days to `tests/m4/raw/wp3/lm3inrepo/<tag>/`.

## Days

1. **Site day** from the named main, test-faults, every live `GRAFTYARD_*` flag of site a kept (BF-76's method), nothing
   added. It ships the readers of 32.32 and 32.33 to every member. No second site day is needed: no new site flag
   gates either format, and the edge sends neither until the edge day.
2. **Edge day** from the same main: the live `FORMATS_ENABLED` read at deploy time, all of it kept, with **32.32 and
   32.33 added together**. 32.33 needs no other format first (it carries what 32.30 and 32.14 carry, both already on
   graft-m4a). Adding 32.32 at the same time means the A3 line's controller can post its issue comment over REST.
   The binding diff must show only `FORMATS_ENABLED`, and `edgever` must exit 0.
3. **Controller**: the agents half redeploys `graft-controller` from the same main, so it routes `issues.edited` and
   `issues.assigned` to the `TaskBox` (`taskbox::follow`). This is the runner account's own deploy, as in r1, and not
   a site day.

The order is site day, then edge day, then agents half, then lines. The edge day waits until all three sites run the
new script (`state` shows three new versions), so no member that cannot read 32.33 receives it.

## Agents half

The same as r1 (`kill-row.sh` steps `init` to `hook` under this row's tag), with `graftyard-agent` and
`graftyard-workspace` installed on `m3org` and agents enabled in-repo. The harness (`tests/am3/hosted/kill/harness.py`)
runs with `HARNESS_PUSHES=2` and opens a draft pull request for the A3 line (`HARNESS_DRAFT=1`, to add: one REST `POST
.../pulls` with `draft: true` under the workspace token, then `Finished`).

## Lines

Each line names its row's text from `build/BUILD-PLAN.md` and runs the row as written, in-repo.

| Line | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| L0-12, Graftyard-run kind | On `m3org/inrepo-<tag>` under `AgentsExclusive`: m3-member holds an Exclusive lease on `l012/<tag>/**` through an external task. A live Graftyard-run task, created by m3-admin and held running by the harness, then sends 100 pushes and 100 REST ref writes into the scope under its `graftyard-workspace` token | 100 of 100 of each refused naming the lease, not `NoLiveTask` | Any accepted, or any refused before the lease check |
| B0-36, Graftyard-run kind | The same token: a REST ref write into another task's Exclusive scope (100), a push after its task is cancelled so it has no live task (100), each over git and REST, once with the shipped permissions and once with widened ones (`everything`). The fork push after cancel waits for AM3-WP2a | 100 of 100 refused for each, the ref write naming the lease | Any accepted, or the ref write refused before the lease check |
| B0-37, fetch half | `graftyard-workspace` pushes `refs/graftyard/sessions/<task>` under the live task. Then v0, v1 and v2 fetches, `git clone --mirror`, and v2 `ls-refs` with and without a matching `ref-prefix` | The session ref appears only to a v2 `ref-prefix` that starts with it | It appears to any other listing, or not to the matching `ref-prefix` |
| LF-9, in-repo lines | A `graftyard_task` token request from a third App (m3-ci) naming the task: 100. A request naming a repository that is not the task's: 100. A `graftyard-workspace` token pushing to the base outside its task's prefix: 100 | 100 of 100 refused for each | Any accepted |
| A3 end to end, in-repo | m3-admin assigns `graftyard-agent[bot]` to an issue. Then the controller creates the task, the harness pushes twice under its prefix and opens a draft pull request, and the task finishes. A second, prompt-only task created by m3-admin over the task API | The issue task reads `queued`, then `in_progress`, then `completed` with `pr_number` set to a draft pull request whose `AgentMeta` names the task and its requester. The prompt-only task completes the same way. No step by hand after the assignment | Any state missing or out of order, a non-draft pull request, or `AgentMeta` without the task |
| L0-14 rerun | `agent-l014.ts` as written, with `agents_enabled` off, then on, on graft-m4a | The row's Pass, every create and cancel at the controller within 5 s | The row's Fail |
| L0-04 rerun | Under `ci: auto_read_only` (`agent-ci`, 0x15.13, already on graft-m4a), an agent push with a workflow that reads a secret. A runner registered for the row (`graft-runners`, as BF-73) | The agent-push workflow starts within 30 s with a read-only token and no secrets; the job that references a secret waits | A secret reaches an agent-push job before `AgentGate` holds for its head |
| Pause and resume | An issue opened by m3-member (read on this repository, made read by the row), assigned to the bot by m3-admin, its task running. m3-member edits the body. Then m3-admin reassigns the bot. Controls: a writer's edit, and a reader's reassignment | After the reader's edit: the base reads `waiting_for_user` within the edit's own reply, the box stops its Sandbox within 30 s, the App's `PATCH in_progress` gets 409, and a `graftyard_task` mint gets 422. After the writer's reassignment: the same task id reads `queued` with a larger `snapshot`, then `in_progress` on a new Sandbox, and no second task exists. The writer's edit pauses nothing, and the reader's reassignment resumes nothing | The task runs on after the edit, a resume by anyone but a writer, a new task id, or an unchanged snapshot |

The L0-12 line runs only the Graftyard-run kind: the moved clause is that kind's. The other kinds ran in
`tests/am1/results/am1-hosted.md` and LM-1's rows. L0-14 and L0-04 run as written, in full, because LM-3 reruns them.

## Teardown

The row's repositories deleted; the policy reset to `approve`; agents disabled and both installations removed; every
`graft-agents-m4a-` resource deleted; the runner for L0-04 deregistered and its container stopped; agents container
applications listed with their running instances, failing loudly while any runs; then the install's shard sweep
(`teardown.py --sweep --shardkill`), failing loudly on any running shard container. 32.32 and 32.33 stay enabled: they
are gated product state, not row residue.

## Cost and stop

r1 cost $0.28 for two site days, an edge day and the agents half. This row has one site day and one edge day, an
agents half with about six Sandboxes, an L0-04 runner for a few minutes, and the L0-14 and L0-12 counts (a few
thousand API calls). Estimate $0.5 to $1.2. **Stop: $3.00** across graft-m4a and the runner account's
`graft-agents-m4a-`, read by `state` before every step. Past it the row stops after teardown and reports.

## Result

Not run yet.
