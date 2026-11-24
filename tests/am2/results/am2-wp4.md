# AM2-WP4: the AM2 gate (LM-2)

LM-2 (`build/BUILD-PLAN.md`, AM2-WP4): the MCP row passes, a bad `graft-mcp` at 1% swaps back with the API blocked,
and L0-15's `AgentGate` half holds. State on 2026-11-24, after the lead's rulings:

| Part | State | Evidence |
| --- | --- | --- |
| MCP row (L0-08) | **met** | auth cells on m3fa2, the 30-call cell and tool-overhead p50 on m3fa3 (`am2-wp1.md`); sessions through `mcp.<host>` at N and under the pin on AM2-WP3 day d1 (`am2-wp3.md`) |
| Swap-back, the undo | measured | AM2-WP3 day d1: the pre-signed `McpPointer` with the deploy token invalid, 119,218 of 119,218 answers from N while pinned |
| Swap-back, the trigger | **met** | R47 row r1 on graft-m4a (below): `graft-ops` breached `GATE_ERRORS` on its own (`GATE` unset) and committed the `McpPointer` with the deploy token invalid; the 1% gap stays open (`decisions/OPEN-QUESTIONS.md`, R47) |
| L0-15's `AgentGate` half | **met** | the in-memory test below, and the hosted day l015 below |

## L0-15's `AgentGate` half, in memory

`crates/git/tests/agent_gates.rs`, `an_approval_through_graftyard_mcp_writes_no_agent_gate`: a writer approves an
agent PR through the install's `Graftyard MCP` (an `AgentApp` of kind `Mcp`) on a scoped `ghu_` whose grant is
`write_approve_merge`. 100 of 100 reviews are accepted and write no `AgentGate`, the merge is refused "0 of 2", and
the same writer's own PAT and a second writer's approval then merge it. With the AgentApp-credential condition taken
out of the review rule (`crates/git/src/issue_ops.rs`) the test fails at the gate count. The named-requester case is
`crates/git/tests/task_gates.rs` (AM1-WP3).

## Hosted, day l015 on graft-m3f (2026-11-24)

The lead's go ($3 stop) after `am2-wp4` merged (main `e0b1be9e`). Driver `tests/am2/hosted/l015.mjs` from this
branch; graft-mcp from `e0b1be9e` through `tests/am2/hosted/day.sh l015` (init, claims, register, deploy, smoke).
Raw rows in `tests/am2/raw/l015-l015d.jsonl`. Start 05:39:40Z; **done line 06:13:07Z** (end sweep).

**State before.** Start sweep: no shard running. No edge or site day: the edge was `81eb863f` at 100% with 20.4 and
17.4 in its own `FORMATS_ENABLED`, sites a `9122106d`, b `884e864a`, c `56b7bf42` (2026-11-23 23:01Z, after
AM2-WP1), every site, edge and ops schedule `[]`, no `graft-m3f-mcp`. RERETIRE OK (`tests/am2/raw/reretire-l015.log`).
`m3org/am2-l015` = repo:65630 (201 in 35 s). Both apps slugs answered Taken (graftyard-mcp 327680, graftyard-insights
4784128), as on m3fa4; the seven register commands ok; smoke: both metadata documents 200, the door 401 with
`resource_metadata`, CIMD 200.

**The run.** m3-ci flagged ThirdParty with `confine` `refs/heads/claude/**` (200), and two external tasks: requester
m3-admin (7100003) and requester m3-member (7100002), each confirmed by account id in the task list. With no
`extra_human_approvals`, each agent PR needs one approval from a human writer who is not its requester, so one counted
approval would merge it. 20 PRs per case, each on its own `claude/` branch, pushed by m3-ci and opened in its task's
session. graft-mcp's grant: `write:write_approve_merge tools:pull_requests`.

| # | Case | Verdict | Evidence |
| --- | --- | --- | --- |
| 1 | An MCP client with `WriteApproveMerge` approves an agent PR (m3-member through `approve_pull_request` on m3-admin's task) | **pass, 20 of 20** | each review accepted (`APPROVED`); each `merge_pull_request` through graft-mcp `isError`, 403 "refused: graftyard/agent-gate is not satisfied for … 0 of 1 approvals from human writers"; each admin REST merge the same 403 |
| 2 | A vendor's named requester approves its PR (m3-member's PAT on its own task's PR) | **pass, 20 of 20** | each review accepted; each admin REST merge 403 "0 of 1" |
| 3 | Controls: an approval that counts merges | **pass, 2 of 2** | PR #1 (case 1) after m3-member's own PAT: merged 200; PR #2 (case 2) after m3-admin's PAT: merged 200 |
| 4 | Teardown | **pass** | unflag 204; `graft-m3f-mcp`, `graft-m3f-mcp-aux` and the four `graft-m3f-` store secrets deleted (`day-l015-teardown.log`); net:65630's 3 shards torn down (200 each); repo:65630 retired (4 names, 4 were joined, all not joined, retired rows) and purged (0 objects, second pass 0; no key of 65630 in any of the three buckets); end sweep 06:13:07Z: no shard running; schedules `[]` |
| 5 | Spend | **pass** | $0.17 on the graft-m3f prefix, 05:39:40Z to 06:13:10Z (`acctcost.py`), under the $3 stop |

**L0-15's `AgentGate` half passes hosted, 42 of 42.** With the in-memory test, LM-2 now waits only on R47's trigger.

**What the day found (harness and one product note).**
- **The task create reply renders a named requester as `ghost`.** Both creates answered 201 with `requester.login`
  `ghost`, and the unfiltered task list right after named m3-admin (7100003) for the first and `ghost` with id
  7100002 for the second; a later list named m3-admin for both of the first runs' tasks. The stored requester was
  right each time (the gate used it: "0 of 1"), so the login is a directory miss in the reply's rendering
  (`crates/api/src/tasks.rs`, `simple_user` through `r.dir`), not a wrong requester [inference]. The list's
  `requester=<login>` filter answered 503 twice. The driver now checks the requester by account id; runs l015 and
  l015c stopped on this check before any PR (rows kept), and l015d is the run above.
- `retirenets.mjs` under `timeout 400` was stopped during purge's wait for the 06:10Z sweep window, after the retire
  had finished; `purge.py` alone then ran to its end. Purge logs went to `tests/am2/raw/` (`PURGE_LOG`,
  `RETIRE_LOG`), not the tracked m2b logs.

## The swap-back's trigger (R47)

The lead approved the design on 2026-11-24 with three changes (flush homed by script name, the floor pooled as
`gate-pool`, a missing version header counted apart and alarmed, not breached). Its code, plants plant121 to plant129,
and the graft-m4a row follow on their own branch.

### Hosted, row r1 on graft-m4a (2026-11-24)

Cleared by the lead from main `46edc385` ($8 row stop; stop if the install's UTC day would pass $35). Driver
`tests/am2/hosted/r47-day.sh`, logs in `tests/am2/raw/r47-r1/`. Start 09:06:28Z, done line 10:01:45Z (end sweep).

| Check | Result |
| --- | --- |
| `graft-ops` notices the bad build by itself | **pass**: `GATE` unset; at the 1% step its run read `gate mcp: N+1 29 answers, 29 5xx, 0 thrown; N 5650 answers, 0 5xx (whole intent 8754, 0) from 3 sites -> Breach("GATE_ERRORS: N+1 29 5xx of 29 answers (0 thrown), N 0 of 5650")` [measured, `logs.log`] |
| The swap-back commits with the API blocked | **pass**: `McpPointer` committed at `Release` entry 389, with `graft-m4a-ops-deploy-token` invalid; the run then waited on "no deployment of Mcp read" |
| Answers after the pointer | **pass**: sampler on `mcp.m4a.graftyard.dev` (concurrency 4): every one of 29,474 answers from 10 s after the pointer to the end came from N `c2c4ec61` with pin `c2c4ec61`; front status `mcpPin c2c4ec61`, 0 mismatches |
| Before the pointer | at 1%, `02fca362` (N+1) answered 76 of the sampler's 14,138 answers, all 500; N answered the rest 200 |

Sampler answers by the view count the front reported (`mcp.jsonl.gz`): 388 (before w5's intent) 6,020 from N; 389 (the
1% split) 7,192 from N and 76 500s from N+1; 390 (the `McpPointer`) 22,618 pinned to N, plus 5 unpinned from N while
the isolate refreshed; 391 to 392 (the undo's `IntentStep` and `IntentDone` after unblock) 6,852 pinned to N. The 1%
split was deployed at 09:52:13Z, the first N+1 answer came at 09:52:06Z by the sampler's clock, and the first pinned
answer at 09:53:05Z, one cron run later. Workers Logs stamp the breach line 6 s after that first pinned answer, so
the sampler's counts, not the log stamps, give the order.

Setup, in the order the lead set:

1. Site day (test-faults, site day 70's flags, image m4a60 `e55ab2ad`) 09:21:38Z to 09:22:34Z, exit 0, so each site
   has `/errors`: a `b2bb49ec`, b `5f9e7a9f`, c `74869876`. Site crons reset to `[]`; reretire OK.
2. `graft-m4a-mcp` N `35ec218a` and `graft-m4a-front` `e6a5bff8` (secrets `graft-m4a-MCP_CLIENT_SECRET_A` and
   `-MCP_SEAL_KEY_A` made fresh, no `apps init`), custom domains `m4a.graftyard.dev` and `mcp.m4a.graftyard.dev`.
   `graft-m4a-ops` `75708f21` from `46edc385` (`exec-r47.json`: `GATE` unset, `OPS_SCRIPTS` sites plus front and mcp,
   60 s dwells, `FIRST_DAY` 1,800,000 ms, 1-minute cron). The cron fired only at 09:45:31Z, after two remove-and-re-add
   cycles (R52); that run committed `ReleaseUploaded` for w4 (`r47r1-w4/`, `2310a9a4…`, front and mcp) and w5
   (`r47r1-w5/`, `aa14c304…`, mcp only: `tests/am2/hosted/r47-bad/index.js`, which answers 500 and reports its version).
3. w4 as N: `FrontRoll` (entry 381) and `McpRoll` (385) each blocked first by the token, then the step-0 check passed
   (relview `next` 0; front still `e6a5bff8` 100, mcp still `35ec218a` 100), then `Abort` to w4 (382, 386). After
   unblock each ran to `IntentDone`: `rolled.Front` and `rolled.Mcp` = `2310a9a4`, front `fb3d4dfe`, mcp `c2c4ec61`.
4. w5 `McpRoll` (intent `01M86FG8J4JDA3AJDBR0PGBFGA`, entry 388) with `GATE` unset; `graft-ops` deployed
   `02fca362` 1% / `c2c4ec61` 99%, then the token was blocked. Its first two gate reads were `Starved(0)` (no N+1
   answer flushed yet); the third breached as above. Unblocked 09:55:50Z: the undo returned mcp to `c2c4ec61` 100,
   `IntentStep` and `IntentDone` (count 392).

Teardown from 09:57:48Z: sampler stopped; ops cron reset (a yearly cron, then `[]`) and `graft-m4a-ops` deployed as
`ac4cd9a5` **by version id** (its own bindings: `GATE=pending`, `OPS_SCRIPTS` sites only, `OPS_STEP_MIN_ENTRIES=50`,
60 s dwells); `graft-m4a-front` and `graft-m4a-mcp` deleted, with both domains; both mcp secrets deleted; token
`graft-m4a-r47-deploy` revoked and its files shredded. Zone `graftyard.dev` before: 0 custom domains, 0 DNS records, 2
universal packs; after: the same, plus one advanced pack for `mcp.m4a` in `pending_deletion`. End sweep 10:01:45Z: no
shard container running. Ops schedules read `[]`, but Idle runs kept firing after the reset (last seen 10:02:13Z), as
on M4-WP4's rows. Cost: the row $0.04 (09:06Z to 10:02Z); graft-m4a's UTC day $16.97 [measured, `acctcost.py`].
