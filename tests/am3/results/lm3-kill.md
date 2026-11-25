# LM-3 on graft-m4a: "a Sandbox killed mid-task resumes"

The kill row (`build/BUILD-PLAN.md`, AM3-WP1, LM-3's second row; the lead's GO and rulings of 2026-11-25), S4 of the
lead's plan. The runner account stands in as the agents account, as for LF-7 and LM-3's first row (`FRAME.md`,
"Operator decision, 2026-11-24, AM1 closed and the runner account stands in as the agents account"). Driver
`tests/am3/hosted/kill-row.sh` with `kill.mjs`, the agents half through `lf7-day.sh` with `ROW=kill TB=1`. Logs are in
`tests/am3/raw/kill-r1/` and `kill-r2/`, and the site and edge days are in `tests/m4/raw/wp3/lm3kill/r1/`.

## Run r2: PASS

**Verdict: PASS**, on the lead's GO after BF-72's done line (23:01:40Z), from main `e1613fc6` with no site or edge day
(sites `f7704ee3`, `b6e66c3a`, `7bbf3cbd`; edge `7b3bf05d`). The driver is r1's with the restart wait (`kill.mjs`).
Logs are in `tests/am3/raw/kill-r2/`: `row.jsonl`, `tasks.json`, `branches.json` (each work branch read from a mirror
clone, with the Sandbox and wall time of every commit), and `late-logs.jsonl` (the controller's refusal lines).

| Line | r2 |
| --- | --- |
| 10 of 10 return to `Running` under a new `SandboxId` | 10 of 10: each box named `<task>-2` after its kill, and every task ended `Done` at generation 2 |
| ... from their last acked push | 6 of 6 tasks killed after a push restarted on the first Sandbox's last commit. The 4 killed before any push restarted from the base, which is right: they had no push to lose |
| At most 2 minutes of work lost | at most 4.0 s: the between kills came 3.7 to 4.0 s after the last commit, and the during kills 1.8 to 2.0 s after the last commit, whose push the new Sandbox built on. The before kills came 0.9 to 2.6 s after the start, with no commit. The new Sandbox's first push came 24 to 41 s after each kill (`HEARTBEAT` 30 s, a clone, then one 15 s interval) |
| Every task finishes | 10 of 10 `Done`, 8 commits on each work branch |
| Every late report from an old `SandboxId` refused, counted and logged | 20 of 20 replays (a `Finished` report and an outbound request per killed Sandbox) answered 403; `late` reads 2 on all 10 boxes; 20 lines `graft-controller: task <id> refused late traffic from <task>-1 (n late)` in the controller's log, one per replay |
| Kill points: before the first push, between pushes, during a push | hit 4, 3 and 3 times, each as planned |
| No task stalls past `TASK_IDLE` or ends with two live Sandboxes | none stalled (last task `Done` 3 min 24 s after the first create); one container per object, generation 2 throughout |

The smoke before the row ran the same path with one task: it was killed before its first push, restarted under `-2`
30 s later, had both replays refused (late 2), and ended `Done` at generation 2.

The during kills found the box's `last_push` empty at the kill, but the new Sandbox started on the commit the killed
Sandbox was pushing. So that push was acked and reached the box before its alarm restarted the task [inference from
`branches.json` and the harness, which starts from the box's `GRAFTYARD_START`].

**Teardown.**
- Done line 23:35:13Z. m3org/kill-r2 is deleted, agents_enabled is false again, and installations 7190003 and 7190004
  are uninstalled. The driver's new `uninstall` step makes the same calls I made by hand at r1.
- Every `graft-agents-m4a-` resource is deleted, including the `-controller-taskbox` container application, which had
  0 running. The install's shard sweep found no shard container running.
- The teardown step exited 1 with no failure line. Its last test was `grep && exit` with no `else`, which returns 1
  when the grep finds nothing. It is now an `if`.
- graft-m4a is unchanged except for one `Keys` entry (count 583), a new `agents_executor` whose seed is deleted.
- Cost: $0.38 on graft-m4a since 23:07:57Z, $0.00 on the runner account; graft-m4a's UTC day is $9.18.

## Run r1: no verdict, because the driver replayed the late traffic too early

**Verdict: r1 is void. Neither the pass line nor the fail line was tested.** All 10 tasks were created on the product
route (201), and all 10 boxes started. Each Sandbox was killed once at its planned point: 1 before the first push, 4
between pushes, and 5 during a push. The kill point the box reported matched the plan in all 10. But
`kill.mjs` replayed the killed Sandbox's late report 0.12 s to 0.14 s after each kill (orb time). That was before the box's next
alarm (`HEARTBEAT`, 30 s) found the container stopped and restarted the task. So the killed Sandbox was still the
task's Sandbox, its replayed `Finished` was taken as current (200), and the task ended `Done` at generation 1. The
replayed request then got 409 (`Final`). No restart happened, no late report was refused, and `late` reads 0 on
every box [measured, `row.jsonl.gz`, `tasks.json`].

This is a fault in the driver, not in the product. A Sandbox the box has not yet replaced is not late, so taking its
report is right (`crates/git/src/taskbox.rs`, `TaskBox::admit`). The driver now waits until the box names a new `SandboxId`
before it replays (`kill.mjs`, `restarted`), which is the order the row means: kill, restart, then the old
Sandbox's late traffic.

| Line | r1 |
| --- | --- |
| 10 of 10 return to `Running` under a new `SandboxId` from their last acked push | not tested: each task ended on the replayed `Finished` before its alarm |
| At most 2 minutes lost; every task finishes | not tested |
| Every late report from an old `SandboxId` refused, counted and logged | not tested: the replays came before any restart |
| Kill points: before the first push, between pushes, during a push | hit 1, 4 and 5 times, each as planned (the box's `pushing` and `last_push` at the kill) |
| No two live Sandboxes for one task | held: one container per object, and `generation` 1 throughout |

**What r1 did prove on the hosted path:**
- The product create route works end to end: `POST .../tasks` without `kind` returns 201 with a host-minted v8
  session (format 32.28).
- The `graftyard_task` delivery reached `graft-controller` through `graft-agents-front`, signed with graftyard-agent's
  App hook secret, and all 10 boxes started within 18 s of the first create.
- The Sandboxes cloned and pushed through the outbound router under the tasks' installation tokens, and the box saw
  the acked pushes (`last_push` set on the between kills).
- In the smoke before the row, one task held under the stale `ControllerPointer` (below) until the roll cleared it.
  It then restarted on the new version and finished at generation 3 [measured, the smoke box's status].

## Setup

- **Site day** from main `8a4c6397`, test-faults, flags kept unchanged (BF-71's flags plus `GRAFTYARD_OIDC_HEAD_REF=1`
  and `GRAFTYARD_OIDC_CLAIMS=1`; `GRAFTYARD_APP_HOOKS=1` was already set), 20:43Z to 20:44Z: site a `d64445ff`, b
  `076f699d`, c `87aa0cfd`. Crons `[]`.
- **Edge day** 20:45Z to 20:49Z: `8961d390` at 100%. The only change is 32.28 added to `FORMATS_ENABLED`; edgever
  passes.
- **The stale `ControllerPointer`** (`5bc20f05`, left by LM-3 r2): the lead's preferred path. A real `ControllerRoll`
  to c1, the `TaskBox` controller itself (`wp3-bundle.py --controller-entry`), went through the agents `graft-ops`.
  I rolled it with the deploy token blocked, aborted it to itself at step 0 (entries 578 and 579), and unblocked it.
  The agents `graft-ops` then deployed c1 `437bbc42` at 100% with the TaskBox bindings and container kept
  (`upload_metadata` carries the deployed version's bindings and `containers`), and committed `IntentDone`. Count
  582, `controller_pin` null. So the roll carried taskbox.json's bindings, and option A was not needed. The roll was
  aborted to its own release, not walked through its steps (the lead accepted this, 2026-11-25).
- **Deploy token:** `graft-agents-m4a-deploy` was minted with Workers Scripts Write, Secrets Store Write (A54) and
  Workers Containers Write. It was deleted at teardown.
- **Keys** generation 9 at entry 576, a new `agents_executor`. Apps 3801088 and 3866624 were re-registered (8 of 8),
  installed on m3org as 7190003 and 7190004, and agents were enabled in-repo. graftyard-agent's webhook was set to
  the front through `PATCH /app/hook/config` from the probe Worker.
- **Harness:** `tests/am3/hosted/kill/` (Alpine, git, python3): 8 pushes of 256 KiB, one every 15 s, then
  `Finished`. The orb's docker has no buildx, so I built the image with the legacy builder and pushed it to the
  runner account's registry by digest. Secret names were passed in `taskbox.json`'s `secrets` map (A58).
- **New code:**
  - The test-faults routes `/_controller/box/{task}/{status|_kill|_late}`.
  - `/status` reports the box's in-flight receive-packs (`pushing`).
  - The late-traffic answer is pure, `taskbox::refused`, with a native test and plant257 (4 of 4 caught).

## Teardown, state left, cost

- **Done line 21:53Z.** m3org/kill-r1 is deleted. Every `graft-agents-m4a-` script, secret, token, bucket and the
  `-controller-taskbox` container application are deleted. The agents container sweep found 0 running, and the
  install's shard sweep found no shard container running. The m3org installations 7190003 and 7190004 are uninstalled
  and agents_enabled is false again.
- **graft-m4a:**
  - Sites and edge stay on this row's days. `graft-m4a-ops` was untouched (`ac4cd9a5`, schedules `[]`).
  - Release count 582, no intent current, `Keys` generation 9, whose `agents_executor` seed is deleted.
- **Cost:** the row (site day to teardown) $0.14 on graft-m4a and $0.00 on the runner account; graft-m4a's UTC day
  $7.96 [measured, `acctcost.py`].

## Found

- OPEN-QUESTIONS A58: `agents init`'s secret names and `deploy.mjs --taskbox`'s differ.
- OPEN-QUESTIONS A59: the box never `PATCH`es the base's task. Not a line of this row.
