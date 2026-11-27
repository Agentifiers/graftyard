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

r1 on graft-m4a, 2026-11-26. Site day from `dccb7975` (sites a eb75a6b7, b a4e60331, c 109a2d06), edge day 1 from
`dccb7975` (0bb4c7e1, FORMATS_ENABLED 40 with 32.32 and 32.33), edge day 2 from main `b6a44183` (f96935c0, binding diff
empty, no format added; edgever exit 0, so no second site day). Agents half: controller 8cd13216, front ce15d844, probe
c946ed63, harness image c16fa2e7, installations 7190003 (agent) and 7190004 (workspace). Every run counts.

| Line | Runs | Result |
| --- | --- | --- |
| repo | 1 | PASS: `m3org/inrepo-r1` created, main 736c16bb |
| reader | 1 | PASS: m3-reader reads with `read` only |
| L0-12, Graftyard-run kind | 1: FAIL (driver) | 100 of 100 pushes refused naming the lease, none `NoLiveTask`; REST ref writes 99 naming the lease and 1 `422 Object does not exist`: the driver's scratch push `l012s-…-14`, which the write named, never reached the base (99 of 100 scratch branches exist) and the driver did not check it. The scratch push now retries and is logged. |
| B0-36, Graftyard-run kind | 1: FAIL (driver) | Shipped: 100 of 100 for each of the three cases (ref write into the lease, refused naming it; push after cancel; REST ref write after cancel, both `push needs a live task`). Widened: the driver's `Install` beyond graftyard-workspace's record was `BadRecord`, so no widened case ran. The driver now widens the App record first, as `tests/am1/runs/widen.mjs` does. |
| Pause and resume | 4: FAIL (driver), FAIL (product, F143), FAIL (driver), PASS | See below |
| B0-36, Graftyard-run kind | 2: no verdict | The orb's CLI restarted at 21:52Z and killed the driver mid-run (last row 21:45:55Z, the shipped task cancelled, then the push-after-cancel loop). The lead ruled it no verdict; run 3 ran under a supervised service. |
| B0-36, Graftyard-run kind | 3: INCOMPLETE | Shipped: 100 of 100 for each of the three cases. Widened (the App record widened, then restored): 100 of 100 for each. The fork push after cancel waits for AM3-WP2a (lead's ruling), so the line stays incomplete, not failed. |
| LF-9 | 1: PASS | A live task; 100 of 100 third-App task mints, other-repository mints and pushes outside the prefix refused. |
| B0-37 | 1: PASS | `graftyard-workspace` pushes the session ref under its live task; the ref is hidden from protocol v0, v1 and v2 without a matching `ref-prefix`, and from `--mirror`; a v2 fetch whose `ref-prefix` starts with it gets it. |
| A3 end to end, in-repo | 1: INCOMPLETE | The issue task read `queued`, `in_progress`, `completed` in order with the harness's pushes under the task's prefix, and the prompt-only task did the same. `pr_number` on a draft whose `AgentMeta` names the task has no product path yet (F140, built on branch `a3-controller`, not yet hosted), so the line waits for the A3 row. Step 8's verdict half is carried to AM4-WP2 (F146). |
| L0-14 | 1: FAIL, stopped at the spend line | See below. |
| L0-12, Graftyard-run kind | 2: not run | Dropped for cost (lead, 00:05Z): the row stood at $2.64 of $3.00 with L0-14 still running. Scheduled on a later graft-m4a row; not counted as a fail. |
| L0-04 | not run | Dropped for cost, as L0-12 run 2. |

### Pause and resume

On edge f96935c0. Run 4 passed: the controller created the task 14.9 s after m3-admin's assignment; the writer's edit
paused nothing; the reader's edit read `waiting_for_user` in its own reply; the box stopped its Sandbox 4.1 s later
(`Paused { reason: "the issue was edited by someone who cannot push" }`); the App's `PATCH in_progress` got 409 ("The
task is waiting_for_user") and a `graftyard_task` mint 422; the reader's reassignment resumed nothing; after the writer's
reassignment the same task read `queued`, then `in_progress` on Sandbox generation 2, and no second task exists. The
task JSON carries no snapshot, so the snapshot is read from the repository log: `TaskCreated` at index 2932,
`Assigned` with `TaskResumed` at 2982, and `TaskResumed` moves the issue snapshot to its own entry (`task.rs`).

- Run 1, driver: the controller's create came about 8 min after the assignment (right after a manual unassign and
  reassign); the writer control and the reader edit passed; then the driver read the controller token from the front URL
  argument and stopped. Fixed.
- Run 2, product: no task within 480 s. Both `issues` deliveries reached the controller and got 202 with no log line, so
  its `assigned` branch did not run (F143).
- Run 3, driver: create at once; the reader's edit stopped the box (controller log, `base waiting_for_user: Stop`, about
  19 s after the edit) and the writer's reassignment started generation 2 from the base; the driver polled the box status
  at the controller's own workers.dev URL, which is not served (1042), so it read neither. It now polls through the front.

### Findings

- **F143** (to the edge thread): the controller's create on `issues.assigned` is intermittent. Of 6 real assignments, 3
  created at once, 2 never did, and 1 came 8 min late. A signed synthetic payload, and a repository hook's capture of the
  same delivery (`assignee.login` `graftyard-agent[bot]`, type Bot, id 3801088) replayed to the front, created at once.
  Hypothesis [unverified]: the App delivery renders the assignee from a cold directory as `ghost[bot]`
  (`crates/api/src/wire.rs`, `bot_user`'s fallback). The fix matches the bot by App id and renders an installed App's
  bot login without a warm directory.
- **F144** (A3 controller head): every `graftyard_task` delivery, `created` and `cancelled`, answers 500 "TypeError:
  Can't modify immutable headers." at the controller though the box starts and stops, so the edge counts it failed.
- **F145** (A3 controller head): a cancelled task's box keeps its 30 s alarm, minting each time and failing with "422
  graftyard_task names a task that is not a live graftyard_run task". Teardown deletes the `TaskBox` namespace and shows
  none remains.

### L0-14

Run 1, from 23:25Z, stopped at 00:23Z when the row's spend read $2.86, past the $2.85 teardown line. 100 tries per
cell. Every cell's statuses are in `tests/am3/raw/inrepo-r1/l014.jsonl`.

- **`agents_enabled` off: 7 of 9 cells FAIL the gate's wording.** The gate wants 422 for every create while off. The
  valid creators (m3-admin, and the controller from an issue assigned to its bot) got 422 "Graftyard-run tasks are not
  enabled for this organization", 100 of 100. The five bad creators (installation token, the `AgentApp`'s user token,
  the read-only user, a user with `X-Graftyard-Agent-Session`, and both bad controller creates) got 403 "Only a person
  who can push to the repository creates a Graftyard-run task", 100 of 100: refused, but by the creator rule before
  the `agents_enabled` check. The demoted user got 52 of 422 and 48 of 403 (the agent class). Nothing was accepted while
  off. Whether a bad creator's 403 meets "422 for every create while off" is the lead's call; the code checks who may
  create before it checks the org's switch.
- **`agents_enabled` on: 1 cell FAIL.** The demoted user's cell accepted 52 of 100 creates (201) and refused 48 as the
  agent class. The driver demotes m3-member with 64 issue comments from 16 workers before the cell and again every 25
  creates; the 52 accepted creates came when the user's last 60 s held fewer than `DEMOTE_PER_MIN` (60) counted writes,
  so this measures the driver's pacing, not demotion (task creates do not count toward it, `admit.rs`). A rerun keeps the
  user above 60 writes in the window for the whole cell. Every other on cell refused 100 of 100: installation token,
  `AgentApp` user token, read-only user, session header, both bad controller creates, a body setting `session` (422),
  the 101st open task for one requester in both kinds (422), a `PATCH` of a cancelled task to `in_progress` in both
  kinds (409), a host-only `cancelled` state (422), and another App's `PATCH` (403). A body that sets `prefix`: 20 of 20
  accepted with the prefix derived from the id. The first 100 open tasks (control) were accepted.
- **Not reached:** the MCP token's create, a token request naming another App's task or a repository outside the token's
  list, and the 5 s create and cancel timing at the controller.

### Spend and teardown

The stop line came at 00:23Z: $2.86 of $3.00 (graft-m4a $2.74, `graft-agents-m4a-` $0.12, graft-runners $0), counted
from 17:48Z. Teardown, 00:23 to 00:28Z:
- `m3org/inrepo-r1` deleted (204), m3-reader's PAT revoked (401 after) and the user suspended, the org policy reset.
- The `TaskBox` namespace held 88 objects, and the 60 s tail before teardown held 2 alarm lines (F145). After teardown
  no namespace remains for `graft-agents-m4a-`, so no box alarm can run.
- `kill-row.sh teardown` failed loudly, as it must: one `graft-agents-m4a-controller-taskbox` Sandbox was still running
  at its 60 s check. It then deleted the container application, and a direct read afterwards shows no
  `graft-agents-m4a-` application, no script, no namespace, and 0 graft-runners Sandboxes.
- graft-m4a's shard sweep tore down 2 shard containers through `graft-m4a-shardkill`, then read none running; the
  scratch Worker was deleted.
- The runner store keeps one `graft-agents-m4a-` secret, `MODEL_KEY_A`, which the lead stored for LM-3's real-harness
  row; the teardown excluded it (`KEEP_SECRETS`), and that row's own teardown deletes it.

After teardown graft-m4a reads $2.79 since 17:48Z; the agents prefix reads $0 once its scripts are gone, so the row's
total is about $2.91, under the $3.00 stop.


## Rerun row (rerun1, 2026-11-27): L0-12 run 2 and L0-14 rerun

This row was the lead's GO after LM-1's L0-04 row (07:10Z), with a $3 stop over a baseline taken at the start. At
07:27Z, graft-m4a read $2.74 and the runner account $0.50 since 00:00Z. The drivers ran from main `897e5816`. There was
no site or edge day. LM-1's day left the sites on `3529a45f` with test-faults, which contains the F147 fix (a `d59e6458`,
b `cecd6a85`, c `afd86f1c`). The edge was `50b5577a`, with 41 formats including 32.32, 32.33 and 21.14. Every live
setting was kept, including `GRAFTYARD_SECRETS_WAIT` and `GRAFTYARD_AGENT_CI`. The agents half was kill-row's, under
tag rerun1, and its `hook` step's signed ping answered 202. The repository and reader lines passed. Logs are in
`tests/am3/raw/inrepo-rerun1/`. Every run counts.

| Line | Runs | Result |
| --- | --- | --- |
| L0-12, Graftyard-run kind | 2: **PASS** | Both refused 100 of 100 naming the lease ("`l012/<tag>/**` is leased Exclusive by user 7100002"), with none refused before the lease check: the pushes, and the REST ref writes (422). No scratch push was lost |
| L0-14 | 2: no verdict (driver); 3: no verdict (the process was killed); 4: **FAIL** | Run 4 below. One off cell fails the pass line (F151), and the 5 s delivery line is not met as measured |

**L0-14 run 2 (08:00Z)** stopped in the off phase's demoted cell. The demote step needs 72 of m3-member's comments
committed within 20 s. The repository's shard commits them at about 3 a second, and 16 workers drew 503 "the
repository's tier rows did not answer; retry" on a third or more. So no attempt passed 49, and the driver stopped the
line. The two cells before it answered 422, 100 of 100.

The driver now keeps 6 comment workers running for the whole demoted cell. Each create waits until at least 66 of
m3-member's committed comments fall in the last 50 s, which is inside the 60 s `DEMOTE_WINDOW`. Runs 3 and 4 held 66 to
129.

The other-bot issue now assigns `m3-ci[bot]` by its registered slug, because the edge serves no `GET /app`.

**L0-14 run 3 (08:25Z)** ran 12 cells, all matching run 4. Then its process was killed at about 08:51Z, with no output
and a log cut mid-line; the cause is not known. Its rows are `l014-run3.*`. Run 4 ran as a supervised orb service.

**L0-14 run 4 (08:59Z to 10:05Z), 100 tries per cell:**

- **`agents_enabled` off: 8 of 9 cells answered 422** "Graftyard-run tasks are not enabled for this organization",
  100 of 100 each. That covers the installation token, the `AgentApp`'s user token, the demoted user, the read-only
  user, both bad controller creates, the human writer, and the controller from an issue assigned to its bot. F147 holds
  on the sites.
- **The ninth cell fails.** A create from m3-admin with `X-Graftyard-Agent-Session` answered 403 "Only a person who can
  push…", 100 of 100. The edge refuses a session-header create before the commit, so the org's switch is never read
  (`decisions/OPEN-QUESTIONS.md`, F151). Nothing was accepted.
- **`agents_enabled` on: every bad case was refused 100 of 100:**
  - the installation token, the `AgentApp` user token and the read-only user (403);
  - the demoted user, refused as the agent class (403), which is run 1's failed cell, now passing;
  - the session header and both bad controller creates (403);
  - a body setting `session` (422);
  - the 101st open task in both kinds (422);
  - a `PATCH` of a cancelled task to `in_progress` in both kinds (409);
  - a host-only `cancelled` state (422);
  - another App's `PATCH` (403);
  - token requests: naming another App's task (422), with a repository outside the token's list (422), with every
    repository of the installation (422), naming an External task (422), and naming a cancelled task (422).
- **Controls:**
  - a body that sets `prefix`: 20 of 20 accepted, with the prefix derived from the id;
  - m3-member's first 100 open tasks: 100 accepted;
  - both Apps' mints on the live task: 201, 100 of 100 each.
- **Delivery timing: not met as measured.**
  - The driver timed 43 creates and cancels from its own clock to the controller's line: 42 seen, 1 missing, p50 39.6 s.
  - The orb's clock was not synchronized and read 16.3 s behind Cloudflare's, so that gap is not the delivery's.
  - On Cloudflare clocks, from each task's `created_at` or `updated_at` to the controller's `started` or `cancelled`
    line, the 20-task prefix sample took 10.9 to 30.7 s from create to start (p50 23.2 s), and 12.2 to 31.8 s from
    cancel to line.
  - The gap grew task by task through the sample, so the deliveries queued behind each other. The two creates made
    alone took 2.5 and 3.1 s.
  - The `created_at` stamps have whole-second precision, which adds up to 1 s.
  - So the controller saw a burst of creates and cancels 10 to 30 s late, and one alone in about 3 s
    [measured, `l014-tail.jsonl`, the task API].
- **No other App's webhook received a task delivery:** m3-ci has none.
- **Incomplete, as in run 1:**
  - The MCP token: graft-m4a has no graft-mcp App.
  - graftyard-insights' token: skipped, the lead's ruling.
  - Another bot's assignment: the edge answers 201 to assigning `m3-ci[bot]` but leaves the issue with no assignees,
    and `GET /users/m3-ci[bot]` answers 404. So the controller's create from that issue ran with no bot assignment.
  - **The lead's ruling (2026-11-27): this cell is judged as it ran, not as incomplete.** The design makes its
    precondition unrepresentable, so no setup can assign another bot. GHES 3.22 silently ignores an assignee that
    cannot be assigned: "Only users with push access can add assignees to an issue. Assignees are silently ignored
    otherwise" (https://docs.github.com/en/enterprise-server@3.22/rest/issues/assignees) [doc]. Graftyard drops every
    bot login but `graftyard-agent`'s: "Any other login is dropped" (`design/AGENT-LAYER.md`, "Tasks and the activity
    feed") [doc]. The cell rests on two things: the 201 with no assignee, which is that drop, and the controller's
    create from the issue refused 403, 100 of 100, with no `graftyard-agent` assignment on it. The next rerun adds
    a variant: `graftyard-agent`'s bot assigned by an App token, so no human assigner (`tiers::assigned_by` ignores an
    App's add), with the controller's create from that issue refused.

**Spend and teardown.**
- The row cost $1.30 of its $3 stop: graft-m4a $1.30, the runner account $0.03. The UTC day stands at graft-m4a $4.05
  and the runner account $0.53.
- Teardown ran 10:10 to 10:15Z:
  - m3org/inrepo-rerun1 deleted, m3-reader's PAT revoked (401 after) and the user suspended;
  - installations 7190003 and 7190004 uninstalled, agents_enabled false;
  - every `graft-agents-m4a-` script, secret and bucket deleted, the TaskBox container application deleted (0 running),
    and no box namespace left;
  - `graft-runners-runnerbox` 0 running;
  - the shard sweep tore down 1 shard container, and a second sweep found none.
