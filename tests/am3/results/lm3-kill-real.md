# LM-3 on graft-m4a: "a Sandbox killed mid-task resumes", real-harness case

The kill row's real-harness case (`build/BUILD-PLAN.md`, AM3-WP1, LM-3's second row: "The real-harness case is
AM3-WP3's"), approved by the operator with an Anthropic API key (`FRAME.md`, "Operator decision, 2026-11-26, a model key
for LM-3's real-harness kill case"). It is the scripted row (`tests/am3/results/lm3-kill.md`, r2 PASS) with the stock
Claude Code CLI as the harness. The runner account stands in as the agents account, as for LF-7 and the scripted row.

**Status: met. PASS on row run 2 (tag real1, 2026-11-27), accepted by the lead.** See "Result" at the end. The plan above it is as written
before the run.

## What it needs that the scripted row did not

- **The controller sends the key the way Anthropic takes it.** Branch `a3-controller`: on the model arm the router
  drops the Sandbox's `Authorization`, `x-api-key` and `anthropic-version`, and the host sets `x-api-key: <MODEL_KEY>`
  and `anthropic-version: 2023-06-01` (`graftyard_git::outbound::Cred::model_headers`, `MODEL_VERSION`), never
  `Authorization: Bearer`. `crates/git/tests/outbound.rs`'s positive-control test fails on main (the Sandbox's own
  `x-api-key` and `anthropic-version` pass through) and passes on the head.
- **The harness image** `tests/am3/hosted/kill/claude/`: `node:22-alpine` with git, python3 and
  `@anthropic-ai/claude-code@2.1.332`, run as a non-root user. `ANTHROPIC_API_KEY` is a dummy that the router
  overwrites. `NODE_EXTRA_CA_CERTS` points at the container's CA, so Claude Code's TLS to the model host goes
  through the router. `run.py` does what `harness.py` does around the model: it clones through the router, checks out
  `GRAFTYARD_START`, and builds `refs/heads/graftyard/<task>/work`. The difference is that each step is one `claude -p`
  turn that writes `work/<n>.md`, allowed only `Read`, `Write` and `Edit`. `run.py` commits and pushes each step
  itself, so no push can leave the task's prefix. After each turn it reports `Used` with the task's tokens so far.
  Then it reports `Finished`. The scripted image (`tests/am3/hosted/kill/Dockerfile`) is unchanged.
- **The route's model arm.** `taskbox.json` gains `route_others: [{"ModelInference": {"host": "token-proxy.workinprogress.dev",
  "path": "/v1/messages"}}]` and `secrets.MODEL_KEY` names the runner store's key secret (not null as in the scripted
  row). `harness` becomes `["python3", "/run.py", <host>, "m3org/killreal-<tag>", "6", "claude-sonnet-5-5"]`.
- **F144 and F145 fixed** in the same head (OPEN-QUESTIONS, the lead's numbering): `graftyard_task` deliveries answer
  500 today, and a cancelled box keeps its alarm. The row's teardown still deletes the `TaskBox` namespace and shows
  none remains, as LM-3's in-repo row does.

## Lines

The same Pass and Fail as the scripted row (`build/BUILD-PLAN.md`, AM3-WP1, LM-3's second row), on 10 in-repo tasks in
`m3org/killreal-<tag>`, each created by m3-admin over the product route and each Sandbox destroyed once through
`/_controller/box/{task}/_kill`. The kill points are planned as in `kill.mjs`: at least one before the first push, one
between pushes and one during a push, the rest drawn at random from the three. Each kill records the box's `pushing`,
`last_push` and generation at the kill. A "random point" inside a kind is the first poll that matches it after a delay
drawn from 0 to 20 s.

| Line | Pass | Fail |
| --- | --- | --- |
| Restart | 10 of 10 return to `Running` under a new `SandboxId` (`<task>-2`) | Any task never restarts, or one task ends with two live Sandboxes |
| From the last acked push | Every task killed after a push restarts on the killed Sandbox's last acked commit (the work branch's commit messages name each commit's Sandbox); one killed before any push restarts from the base | A restart from the base after a push |
| Work lost | At most 2 minutes from the kill back to the last acked commit | More than 2 minutes lost |
| Every task finishes | 10 of 10 `Done`, 6 commits on each work branch | A task stalls past `TASK_IDLE`, or ends `Failed` |
| Late traffic | After each restart, a `Finished` report and an outbound request from the old `SandboxId` are replayed (`/_controller/box/{task}/_late`): 20 of 20 answered 403, `late` reads 2 on every box, and the controller logs one refusal line per replay | Any late report acted on |
| The model arm | Every model call goes through the router: the controller logs no `ModelPath`, `ModelPayload` or `UnlistedHost` refusal for the model host, and each task's `Used` reports climb | A model call that bypasses the router, or a key the Sandbox chose (none can be: it holds a dummy) |

The `Used` reports also feed the box's budget (`TaskBudget`, 2,000,000 task tokens): none should come near it.

## Stop

- **Model tokens: $25**, counted by the row itself, since the operator's token proxy
  (`https://token-proxy.workinprogress.dev`, FRAME on main 70aa964b) has no limit the row can read. The router reads
  the `usage` of every model reply it forwards, JSON or streamed (`graftyard_git::outbound::ModelUsage`), and adds it
  to the task's box (`TaskBox.model`, served by `/_controller/box/{task}/status`). The driver sums every task's box at
  Sonnet's list price ($3 per million input tokens, $15 output, $3.75 cache writes, $0.30 cache reads) after every
  kill and every task, and aborts the row at $25. The model is `claude-sonnet-5-5` (`ANTHROPIC_MODEL` and
  `ANTHROPIC_SMALL_FAST_MODEL` in the image); `claude-sonnet-5`, `claude-sonnet-4-6` and `claude-sonnet-4-5-20250929`
  are fallbacks only if 5-5 errors. Estimate: 10 tasks, 6 turns each plus about 3 restarted turns, at roughly 20,000
  tokens per turn, mostly cached system prompt, comes to about 1.3 million tokens, or $1 to $5. The one-task smoke's
  counted spend goes to the lead before the 10-task row.
- **Cloudflare: $3** across graft-m4a and the runner account's `graft-agents-m4a-`, read by the row's `cost` step before
  every step. The scripted row cost $0.38. Claude Code turns are slower than `harness.py`'s 15 s interval, so the
  Sandboxes live longer (`standard-2`, about 10 to 15 minutes each).
- Past either stop the row stops after teardown and reports.

## Driver

`tests/am3/hosted/kill-row.sh` with a `REAL=1` mode, which builds `tests/am3/hosted/kill/claude/` instead of
`tests/am3/hosted/kill/`, writes the model arm and `MODEL_KEY: "graft-agents-m4a-MODEL_KEY_A"` (the key the lead stored in the runner store; the
teardown keeps it with `KEEP_SECRETS` until this row's own teardown deletes it) into `taskbox.json`, and runs `kill.mjs` with
`PUSHES=6`. A smoke of one task first (killed once, between pushes) proves the key, the model arm and the restart
before the 10.

**The smoke's gate (the lead, 2026-11-26).** The router denies by default, and the CLI makes calls besides the model
(telemetry, error reporting, update checks). The image sets `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`,
`DISABLE_AUTOUPDATER`, `DISABLE_TELEMETRY` and `DISABLE_ERROR_REPORTING` for that. The smoke passes only if both
hold:
- every `claude` turn exits 0, which `run.py` writes into each commit message (`claude exit <n>`) and the driver reads
  from the work branch;
- the controller's log for the smoke task (`wrangler tail`, `graft-controller: task <id> refused <rule>: <text>`)
  holds no refusal except on the model arm.

Any other refusal stops the row at the smoke, and the refusal goes to the lead. The route is not widened.

Teardown is the in-repo row's: the repository, the installations, every `graft-agents-m4a-` resource, the `TaskBox`
namespace (box storage and alarms), the agents containers, and the shard sweep, each failing loudly. The
key is the operator's to revoke when the results are in (`FRAME.md`).

## Result

**Verdict: PASS on row run 2.** This was the lead's GO on 2026-11-27, after LM-1's row A stopped (03:27Z). It ran on
graft-m4a and the runner account, from main `28aadfc7` plus this branch's drivers. There was no site or edge day.
Every `claude` turn ran `claude-sonnet-5-5` through the router to the operator's token proxy, and no fallback model
was needed. Logs are in `tests/am3/raw/kill-real1/`:
- `row.jsonl` and `tasks.json`;
- `branches.json`: each work branch, with the Sandbox, wall time and `claude` exit code of every commit;
- `late-logs.jsonl`: the controller's refusal lines, read from Workers observability. `wrangler tail` sampled away 8
  of the 20.

Run 1 is kept as `r1-*`, and the smoke attempts as `smoke*`.

| Line | Row run 2 |
| --- | --- |
| Restart | 10 of 10 back to `Running` under `<task>-2`, 3 to 35 s after each kill. Every task ended `Done` at generation 2, with `running` false, so no task had two live Sandboxes |
| From the last acked push | The 8 tasks killed after a push restarted on the killed Sandbox's last commit. Each work branch's numbering runs on from the first Sandbox's last commit, with no gap and no repeat. The 2 tasks killed before any push restarted from the base. As in the scripted row, the 3 during kills found `last_push` empty at the kill, but the new Sandbox built on the commit being pushed, so that push was acked [inference from `branches.json`] |
| Work lost | At most 4.4 s: each kill came 0.9 to 4.4 s after the killed Sandbox's last commit |
| Every task finishes | 10 of 10 `Done`, 6 commits on each work branch, and 60 `claude` turns, all exit 0 |
| Late traffic | 20 of 20 replays answered 403: a `Finished` report and a clone request per killed Sandbox. `late` reads 2 on all 10 boxes. There are 20 controller lines `refused late traffic from <task>-1`, two per task |
| The model arm | No `ModelPath`, `ModelPayload`, `UnlistedHost` or other refusal. 196 `/v1/messages` calls went through the router and answered 200. 4 more were `canceled`, which were the in-flight calls of killed Sandboxes. Every box's counted `model` usage grew, from $0.15 to $0.33 per task |

Kill points, each after its plan's delay of 0 to 20 s:
- 2 before the first push;
- 5 between pushes;
- 3 during a push.

Two kills planned for during a push landed between pushes, because the push finished between the poll and the kill.
The table counts the point the box reported at the kill.

**Row run 1 (04:37Z): no verdict, a driver fault.** All 10 tasks ran to `Done`, all 66 turns exited 0, and the model
spend was $2.06. But only 5 Sandboxes were killed. `kill.mjs` waited for all ten boxes to run before it started any kill
watch, and five Claude Code tasks were already `Done` by then. They had 6 commits in about 33 s, where the scripted
harness had 8 commits 15 s apart. The 5 that were killed (2 between pushes, 3 during a push) met every line: restart
under `-2`, restart from the last acked commit, at most 6.7 s lost, and late 2 with 403s. The driver now starts each
task's kill watch as soon as its own box runs, after the 0 to 20 s delay that "Lines" names.

**Smoke.** Attempt 1 (04:06Z) never started a Sandbox. The task's `graftyard_task` delivery and a signed ping both got
401 "bad signature" from the controller. A second PATCH of the same webhook secret value cleared it
(`decisions/OPEN-QUESTIONS.md`, F148). I cancelled the task. Attempt 2 (04:28Z) passed the gate: one task killed before
its first push, restarted under `-2`, both replays refused, `Done` at generation 2, 6 turns all exit 0, no refusal,
$0.34. The `hook` step now proves a signed ping answers 202 before any task, and fails loudly otherwise.

**Spend.**
- Router-counted model spend: $4.41 of the $25 stop (smoke $0.34, run 1 $2.06, run 2 $2.01).
- Cloudflare since 11-27 00:00Z: graft-m4a went from $1.84 at the 03:49Z baseline to $2.27, and the runner account from
  $0.41 to $0.47. That's $0.49 of the $3 stop.

**Teardown.** Done line 05:06Z.
- m3org/kill-real1 is deleted, agents_enabled is false, and installations 7190003 and 7190004 are uninstalled.
- Every `graft-agents-m4a-` script, the bucket and every secret are deleted, **including
  `graft-agents-m4a-MODEL_KEY_A`**. The runner store lists no `graft-agents-m4a-` secret.
- The `-controller-taskbox` container application had 0 running and is deleted. There are no `graft-agents-m4a-`
  Durable Object namespaces.
- `graft-runners-runnerbox` has 0 running.
- The install's shard sweep tore down 2 shard containers, and a second sweep found none.
- graft-m4a has one new `Keys` entry (count 587), an `agents_executor` whose seed is deleted.
