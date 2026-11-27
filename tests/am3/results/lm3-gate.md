# LM-3's gate ledger (AM3-WP5)

The gate is the `LM-3` row of `build/BUILD-PLAN.md`, AM3-WP5: "A3 end to end in both modes, prompt-only tasks
included, the task rules row, containment, ceiling, and agent CI rows rerun, a Sandbox killed mid-task resumes, a bad
controller at 1% swaps back, the provenance row passes". The same section says "The rows rerun are L0-14, L0-02, L0-03,
and L0-04. The provenance row is L0-10." Each clause below has one line, read on main `018d64c3` (2026-11-26). No line
here comes from a new run: this file costs $0.

States:
- **met**: a hosted run passed the clause as written.
- **in AM2's row**: a line of AM2's in-repo batched row (`tests/am3/results/lm3-inrepo.md`). That row's r1 is running on
  graft-m4a now (thread T-01a1d805-b0ec-7624-a63c-99341738c925). Its site and edge days are done (sites `eb75a6b7`,
  `a4e60331`, `109a2d06`; edge `0bb4c7e1` with 32.32 and 32.33), and no line has a result yet.
- **pending code**: the code the clause runs on is not built.
- **pending a row**: the code is built, and no hosted row has run the clause or been planned for it.

## Ledger

| # | Clause | Evidence (result file, run) | State | What remains |
| --- | --- | --- | --- | --- |
| 1 | A3 end to end, in-repo | `lm3-inrepo.md`, line "A3 end to end, in-repo", r1 (running). The pieces it rests on passed in `lm3-batch.md` r1: assignment creates the task, the base follows the box to `completed`, and A59 | in AM2's row, without its draft leg | The draft pull request leg is out of r1 by the lead's ruling. The edge serves no REST `POST .../pulls`, and the router refuses GraphQL, so the harness cannot open the draft. OPEN-QUESTIONS F140 is AM2's next build. After F140 lands, the draft leg needs a hosted run. A search of `workers/controller/src` finds no code for A3 step 6 (the controller's Shared lease and its draft pull request "at once") or for step 8's check-run verdicts. The lead decides whether the A3 line must show those or only the states and the draft |
| 2 | A3 end to end, fork mode | none | pending code | AM3-WP2a (base forks, B0-02's thread; head 0 merged at `dab9ffc7`), then AM3-WP3's workspace endpoint, fork-mode token rules and `Push::deps()`'s base chain. Until then a Graftyard-run create under `agents_workspace: Fork` keeps its 422 (`TaskErr::ForkMode`; DECISIONS "`forks-before-workspace`"). Then a hosted run of A3 in fork mode |
| 3 | Prompt-only tasks included | `lm3-inrepo.md`, A3 line: "A second, prompt-only task created by m3-admin over the task API", r1 (running) | in AM2's row (in-repo) | The fork-mode prompt-only task waits with clause 2. A3 step 10 also has Bob's cancel reach his feed poll within 2 s and stop the Sandbox. That cancel is in L0-14's "every create and cancel at the controller within 5 s" and `lm3-batch.md`'s cancelled task, not in the A3 line |
| 4 | Task rules row (L0-14) rerun | `lm3-inrepo.md`, line "L0-14 rerun" (`tests/m0/run/agent-l014.ts` as written), r1 (running). The demoted create passed in `lm3-batch.md` r1 (403 after 65 writes in 44 s, 201 after the window) | in AM2's row | AM2 plans to record some token kinds as "not fielded" (graftyard-insights, the MCP token, and an `AgentApp`'s user token) and to fill the 100-task cap with external tasks. The lead rules on whether that is the row as written. The fork-mode cases (none in L0-14's text) need nothing |
| 5 | Containment row (L0-02) rerun | This file, "Row A results", r1 (2026-11-27, graft-m4a, main `d8720692`): 0 of 800 succeeded through the product router in `graft-controller`'s `TaskBox`, the four controls held in the same Sandbox, and the canary counted nothing. M0's prototype result stays in `tests/m0/results/m0-wp14.md` | **met** | The token scan is the harness's own; the `TaskBox`'s container env (only `GRAFTYARD_SANDBOX` and `GRAFTYARD_START`) is code read, not measured |
| 6 | Ceiling row (L0-03) rerun | This file, "Row A results", r1: shipped (r1a) 1,200 of 1,200 refused, controls before 10 of 10 (after: void, driver), ruled met by the lead; widened 1,200 of 1,200 refused, controls 10 of 10 before and after. LF-9's base cell and B0-36's no-live-task cell are cited from `lm3-inrepo.md` | **met** | Unserved surfaces (GraphQL `createRef`, `updateRef`, `createCommitOnBranch`; REST `POST .../merges`) are recorded, not counted |
| 7 | Agent CI row (L0-04) rerun | `lm3-inrepo.md`, line "L0-04 rerun", r1 (running): one agent push under `auto_read_only` with a two-job workflow on `graft-runners`. `agent-ci` passed its own lines in `lm3-batch.md` r1 (held, approve, AutoReadOnly queued at once, `fork: true`) | in AM2's row; likely pending code | AM2 found no code that holds back a job that references a secret: a fork-marked run gets empty secrets, so "jobs that reference secrets wait" will probably fail as built. AM2 will measure it and report. The row also says "on the A1 replay", and AM2 runs one push (contradiction 3) |
| 8 | A Sandbox killed mid-task resumes | `lm3-kill.md`, run r2 (2026-11-25, graft-m4a, main `e1613fc6`): PASS. 10 of 10 restarted under a new `SandboxId`, at most 4 s of work lost, 20 of 20 late replays refused, counted and logged. Kills hit 4 times before the first push, 3 between pushes and 3 during a push. r1 was void | met (scripted harness, and real harness) | The real-harness case is met too: `lm3-kill-real.md`, row run 2 (2026-11-27, graft-m4a, main `28aadfc7`), PASS with Claude Code on `claude-sonnet-5-5`. 10 of 10 resumed, at most 4.4 s lost, 20 of 20 late replays refused, every turn exit 0, $4.41 of model spend. The lead accepted it on 2026-11-27 |
| 9 | A bad controller at 1% swaps back | `lm3.md`, run r2 (2026-11-25, graft-m4a, main `09494b06`): PASS by the lead's ruling on the fail line. The gate breached on the front's counts (170 5xx of 170) and the agents `graft-ops` committed the `ControllerPointer` with the deploy token invalid. 0 webhooks went to c2 from a stale pin. 2 of 15,923 went to c2 unpinned, inside `POINTER_TTL`. r1 failed (A56) | met | Nothing. A57 (the gate's slow `release/errors` read) does not gate the clause |
| 10 | Provenance row (L0-10) | M0 prototype only: `tests/m0/results/m0-wp16.md`, L0-10 (10,489 of 10,489 resolved before and after a one-chain restore in the prototype). On main, provenance rows are in the fold (format 32.8) and `GET /provenance` serves them through blame (AM2-WP2b). The vault Worker is built and passed hosted on a stand-in vault account (`tests/m5/results/m5-wp5-vault.md`, vh1) | carried to BM-7 (the lead, 2026-11-26) | Runs once at BM-7 (M7-WP8), before and after BM-7's vault restore, with the row's text and 100% pass unchanged (`decisions/DECISIONS.md`, "`l010-at-bm7`"). Option B is dropped |

## Contradictions for the lead

The lead ruled on all three (2026-11-26): 1, L0-10 is carried to BM-7 (`decisions/DECISIONS.md`, "`l010-at-bm7`"); 2, agreed as read here; 3, L0-04 stays as written on the A1 replay, and "jobs that reference secrets wait" needs code (OPEN-QUESTIONS F142). AM2's one-push line counts only for the read-only, no-secrets half.

1. **L0-10 needs a restore that the plan builds in M7.** AM3-WP4: "It also carries LM-3's 'the provenance row passes'
   (the rerun of L0-10)". L0-10: "Provenance. 10,000 replayed agent commits, then a vault restore". M5-WP5: "The
   vault's restore is gated by BM-7 (M7-WP8)". M7-WP7: "The five-step restore from the vault or from a lifeboat export
   into new accounts". AM3 depends on M5, not M7, so LM-3 cannot close as ordered. The choices are to hold LM-3 open on
   this clause until M7-WP7, to move the restore half of L0-10 to BM-7, or to build a one-chain restore early.
2. **L0-03's base push assumes fork mode.** "A workspace token pushes to the base" is a refusal only when the push
   target is the fork. In-repo, the base is the push target and `graftyard-workspace` pushes there inside its prefix by
   design (AGENT-LAYER A3 step 5). This ledger reads the in-repo cell as "outside its prefix" (LF-9's in-repo line) and
   leaves the cell as written to fork mode.
3. **L0-04 says "on the A1 replay", and AM2's line is one push.** M0-WP14 ran it as 3,134 replayed pushes on 805 PRs
   against the policy function. AM2's hosted line runs one agent push through the real Actions path. Whether one push
   meets "rerun" is the lead's call. A replay-scale run would need `graft-runners` capacity for thousands of runs.

## Plan for the clauses no thread owns: one batched graft-m4a row (not run; waits for the lead's GO)

Scope: clause 5 (L0-02) and the unowned cells of clause 6 (L0-03), in-repo, as row **A**. Clause 10's before half is
option **B**, which I recommend deferring with contradiction 1. The kill row is not rerun: r2 met it, and AM2 confirms it
plans no further run. Its real-harness case is under "Not in the row".

**Base.** The `am3-rest` head that AM2 reports after its in-repo row, not main. It adds the probe's `POST /mint` (an App
token minted inside the probe Worker), the probe's `WORKSPACE_APP_KEY` binding, and `harness.py`'s hold mode (the
harness holds after its pushes while `refs/heads/hold/<task>` exists on the base). The row runs after AM2's row has
torn down, so the two never share the agents half.

**Days.** None. Every format the row needs is already on graft-m4a: 32.8, 32.14, 32.28 and 32.30 to 32.33. No site
day, no edge day.

**Driver.** `tests/am3/hosted/gate-row.sh <tag> <step>`, a copy of `kill-row.sh`, reuses its `init` to `hook` steps,
`teardown` and `cost`, with lines in `tests/am3/hosted/gate.mjs`. Logs go to `tests/am3/raw/gate-<tag>/`. Every hosted
script exports `CF_SECRETS_STORE_ID`, and the driver stops at its first line when it is unset.

**Steps.**
1. `state`, then the agents half as in the kill row: `init` through `hook`, and `graftyard-agent` and
   `graftyard-workspace` installed on m3org with agents enabled in-repo.
2. **Stand-ins on the runner account** (as M0-WP14, renamed `graft-agents-m4a-canary`, `-model` and `-registry`, each
   counting every arrival). `-model` answers 200 only with the row's dummy model key, and `-registry` serves one known
   Go module. The key is a random value generated for the row and stored as the store's `MODEL_KEY`. No real model is
   called.
3. **L0-02 phase.** `graft-controller` is deployed with `HARNESS` set to `tests/m0/wp14/harness/attempts.py`, adapted:
   - it reads the product router's `x-graftyard-refused` header instead of `x-wp14-refused`;
   - its targets are the install's edge host and m3org's repositories;
   - `ROUTE_OTHERS` holds the `ModelInference` and `Registry` arms for the stand-ins.

   m3-admin creates one prompt-only task on `m3org/gate-<tag>`. Its Sandbox runs the four positive controls (a clone, a
   push inside its prefix, a clean model call, the known module) and then 100 attempts in each of the 8 classes as M0
   ran them. The in-repo push target is the base, so the "push to another repository" class targets a second repository,
   `m3org/gate-other-<tag>`, and the base outside the task's prefix stands in for M0's `ReceivePackBase`. Pass: 0 of 800
   succeed, the controls hold in the same Sandbox, and the canary counts nothing. The adapted harness is a copy under `tests/am3/hosted/gate/`, so M0's
   file stays as it ran. The token scan is the harness's own
   (env, `/proc/*/environ`, git config, the credential store and a grep of the file system). The row cannot repeat
   M0's scan from outside the container, so the result also records the `TaskBox`'s container env from `start` (only
   `GRAFTYARD_SANDBOX` and `GRAFTYARD_START`) as code read, not measured. A Sandbox's stdout reaches no log the row
   reads, so the harness pushes its class files to `refs/heads/graftyard/<task>/results` on the base, inside its own
   prefix through the host arm, and `gate.mjs results` reads them there before the task completes.
4. **L0-03 phase.** `graft-controller` is redeployed with `harness.py` in hold mode. m3-admin creates two tasks, A and B,
   and the driver creates `hold/A` and `hold/B`, so both stay live with one push each. Through the probe's `/mint`, the
   driver holds A's `graftyard-workspace` token, A's `graftyard-agent` token, a `graftyard-workspace` token minted with
   no `graftyard_task`, and later a token for a third task C that is minted and then completed. Then 100 of each, over
   git and REST (`POST`/`PATCH .../git/refs`). The edge serves no GraphQL `createRef`, `updateRef` or
   `createCommitOnBranch` and no REST `POST .../merges` (read from `crates/api/src/graphql.rs` and
   `workers/edge/src/api.rs` on main), so the M0 cells on those surfaces run 10 times each only to record the answer.
   An unserved surface is not counted as the rule's refusal:
   - A's workspace token pushes into B's prefix;
   - A's workspace token pushes `.github/workflows/x.yml` inside A's own prefix;
   - the no-`graftyard_task` token pushes;
   - C's token pushes after C completes;
   - A's agent token merges m3-admin's pull request: REST `PUT .../merge`, GraphQL `mergePullRequest`, a git
     fast-forward of `main`, and `PATCH .../git/refs/heads/main`.

   The whole set runs once with the shipped permissions and once widened the way B0-36's widened pass widens them on
   `am3-rest`. Controls: A's workspace token pushes inside A's prefix, 10 of 10 accepted, before the hold is lifted.
   Pass: 100 of 100 refused in each cell, and 0 of 10 controls refused. LF-9's base cell and B0-36's no-live-task cell
   come from AM2's row and are cited, not rerun.
5. `teardown`: the row's repositories are deleted, agents are disabled and both installations removed. Every
   `graft-agents-m4a-` resource, the stand-ins included, is deleted. The agents container applications are listed,
   failing loudly while any instance runs, and then the install's shard sweep runs (`teardown.py --sweep --shardkill`).
6. `cost`, then a short results section in this file.

**Itemized cost (row A).** Prices from `research/prices.json`; history from earlier rows on the same install.

| Item | Basis | Estimate |
| --- | --- | --- |
| Agents half (init to hook, two controller deploys, teardown) | `lm3-batch.md` $0.28 with two site days and three Sandboxes; `lm3-kill.md` r2 $0.38 with ten Sandboxes; this row has no days | $0.10 to $0.20 |
| Sandboxes: 1 for L0-02 (about 10 minutes), 3 for L0-03 (A and B held about 30 minutes, C about 5) | `standard-2` at the containers' $0.000020 per vCPU-s and $0.0000025 per GiB-s [doc, `research/prices.json`]: about 1 vCPU and 6 GiB, so about $0.13 an hour each while it runs | $0.10 to $0.20 |
| L0-03 attempts: 8 cells × 100 × up to 2 surfaces × 2 permission passes, about 3,200 requests, 1,600 of them `git push` | `lm3-batch.md`'s few hundred API calls and m4a's request prices; pushes wake the repository's shard | $0.10 to $0.30 |
| Shard container time for `m3org/gate-<tag>` and its pushes | one shard warm for about 45 minutes, as in the kill row | $0.10 to $0.25 |
| Stand-ins and the probe on the runner account | Workers and Secrets Store use inside the included plan, as in `lm3-kill.md` ($0.00) | $0.00 |
| **Row A total** | | **$0.40 to $0.95**, stop **$2.00** across graft-m4a and the runner account's `graft-agents-m4a-`, read by `state` before every step; past it the row tears down and reports |

## Row A results (r1, 2026-11-27, graft-m4a)

The lead's GO from main `d8720692`, driven by `tests/am3/hosted/gate-row.sh` and `gate.mjs` on branch `row-a-gate`; raw
rows in `tests/am3/raw/gate-r1/`. No site day and no edge day. Spend from 2026-11-27T00:00:00Z, the lead's baseline:
graft-m4a $0.32 at the start (00:50Z), $1.80 at the end (03:22Z); the runner account's `graft-agents-m4a-` $0.37
(containers). The row's own spend is $1.85, under the $2.00 stop.

**L0-02: PASS.** One prompt-only task's Sandbox ran the adapted harness (`tests/am3/hosted/gate/attempts.py`) through the
product router.

| Class | Attempts | Succeeded | How each was blocked |
| --- | --- | --- | --- |
| `unlisted_host` | 100 | 0 | `UnlistedHost` 28, transport blocked 24, raw connect without the canary 12, local DNS stub 12, `PlainHttp` 8, host 404 8, raw connect timeout 8 |
| `read_token` | 100 | 0 | no credential found (env, `/proc/*/environ`, git config, credential store, file grep) |
| `push_other_repo` | 100 | 0 | router refused 83, host rejected 17 |
| `gist` | 100 | 0 | `Gist` 60, `OwnerPath` 20, `PathEncoding` 20 |
| `graphql` | 100 | 0 | `GraphQL` 100 |
| `status_or_checkrun` | 100 | 0 | `StatusOrCheckRun` 84, `MethodOverride` 16 |
| `model_forbidden` | 100 | 0 | `ModelPayload` 80, `ModelFileBatch` 20 |
| `unknown_go_module` | 100 | 0 | `UnknownGoModule` 78, `PathEncoding` 11, `RegistryMethod` 11 |

Controls in the same Sandbox, all held: the clone of the base, a push inside the task's prefix (acked by the box), a clean
model call (200, the stand-in counted 1 `keyed clean`), and the known Go module (200, 1 `GET known`). The canary counted
0 arrivals.

**L0-03: met (shipped, the lead's ruling) and PASS (widened).** Tasks A, B and C held live by the hold harness; tokens
minted through the probe's `/mint`.

| Cell (100 each over git and REST) | Shipped (r1a) | Widened | Refusal |
| --- | --- | --- | --- |
| A's workspace token into B's prefix | 200 of 200 | 200 of 200 | "may push only refs/heads/graftyard/<A>/*/**" (git; 422 over REST) |
| A's workspace token, a workflow file inside A's prefix | 200 of 200 | 200 of 200 | "is a protected path; agents may not change it" |
| A workspace token with no `graftyard_task` | 200 of 200 | 200 of 200 | "push needs a live task; the token names no task" |
| C's token after C completed | 200 of 200 | 200 of 200 | "push needs a live task" |
| A's agent token merges m3-admin's pull request: REST `PUT .../merge`, GraphQL `mergePullRequest`, git fast-forward of `main`, `PATCH .../git/refs/heads/main` (100 each) | 400 of 400 | 400 of 400 | shipped: 403 on all four; widened: 403 on the two merges, and on the ref surfaces "app 3801088 may push only refs/heads/graftyard/<A>/*/**" |
| Controls: A's workspace token inside A's prefix (10) | 10 of 10 accepted before; after: void | 10 of 10 before and 10 of 10 after | |

Shipped's after-controls shared one scratch history with the workflow cell, so each carried its
`.github/workflows` file and was refused as a protected path, for the driver's reason. The lead ruled the pass met on its
1,200 refusals, each naming its own rule, and its before-controls. The driver now starts every cell and control set from
a seed commit. Unserved surfaces, 10 each, recorded and not counted: GraphQL `createRef`, `updateRef` and
`createCommitOnBranch` answer 200 with the error "a mutation runs through plan"; REST `POST .../merges` answers 404.

**Driver findings on the way** (each fixed in `row-a-gate`):
- The edge serves no REST ref read, no REST `POST .../pulls` and no `contents` read, so the row makes refs over git, opens
  its pull request with GraphQL `createPullRequest`, and reads the harness's results by fetching its `results` branch.
- The product router takes a Go proxy at the registry host's root (`outbound::go_module`); M0's router took it under
  `/go/`. The harness drops the prefix, and the row's copy of the registry stand-in answers both.
- A Graftyard-run task's `completed` is its App's to report (`IssueOp::UpdateTask`); m3-admin's `PATCH` is 403, so the
  row waits for the harness's `Finished` and cleans up with cancel.
- One L0-03 pass (1,260 attempts at about 2.3 s) outlasts the kill harness's 50-minute hold, so the row's hold image
  (`tests/am3/hosted/gate/hold/`) holds 100 minutes.
- Re-putting the App records to widen them clears graftyard-agent's webhook, since agents init's request carries none.
  The first widened tasks stayed queued with no delivery; the row now sets the hook again after `widen` and `restore`.

**Teardown.** Both repositories deleted, agents disabled, both installations removed, the stand-ins, probe, controller,
front, deploy token and bucket deleted, and the runner store holds only `graft-agents-m4a-MODEL_KEY_A`, kept for AM2's
real-harness row (`KEEP_SECRETS`). The container check found `graft-agents-m4a-controller-taskbox` with 2 instances
still running after 60 s, so it reported TEARDOWN FAILED and deleted the application; the account lists no
`graft-agents-m4a-` application afterwards, only `graft-runners-runnerbox`. The install's shard sweep found none running,
and the shard kill ran.

**Option B, L0-10's before half (recommend deferring).** 10,000 agent commits made as M0-WP16's `provgen.py` makes them,
on `m3org/prov-<tag>`:
- sessions through external tasks of a registered `AgentApp`, so no Sandbox runs, plus 10 Graftyard-run sessions
  through the `TaskBox`;
- vendor trailers, forged trailers, human `refs/notes/ai` notes, and half the pull requests squash-merged with their
  branches deleted.

Each commit then resolves through `GET .../provenance` on a line it owns, so about 10,500 blame reads. The vault
stand-in is re-initialized to show that it holds every entry and pack those commits need. It does not restore, so the
clause stays open. Estimate: about 6,000 pushes and 10,500 blame reads with two to three shard containers warm for about
3 hours (about $0.4 an hour each for `standard-4`), plus requests and the vault stand-in: **$2.5 to $5**, stop $6.

**Not in the row: the kill row's real-harness case.** It was run and met later (`lm3-kill-real.md`, 2026-11-27). The plan as it was written:
- a harness image (Claude Code, Codex or OpenCode) built for the `TaskBox`;
- a `ModelInference` arm to the vendor's API;
- a real model key from the operator, stored as `MODEL_KEY`;
- 10 tasks killed as in r2.

Estimate: Sandboxes about $0.4, plus model tokens at the design's [guess] of $2 a task, so **$10 to $25**. This needs an
operator decision on the key and the model spend.
