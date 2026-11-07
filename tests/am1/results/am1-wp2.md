# AM1-WP2: agent actors and refusals in `decide` (pure half)

These results are from 2026-11-07, on branch `am1-wp2` (worktree `.worktrees/am1-wp2`), last merged with `main` at `baebca0`. Nothing was deployed or pushed. AM1-WP2's own gate is LF-2. The package also carries LM-1's rerun of B0-36 and B0-37 on real code (`build/BUILD-PLAN.md`, AM1-WP2). This is the in-memory half of that work: every gate ran on three real `graftyard_raft::Member<AuthzCmd>` of the install's own `Authz(Genesis.account)` chain and three real `Member<RepoCmd>` of one repository. The repository leader's deps were answered from the install leader's `fresh` read, as a site host answers them once per entry (`crates/git/tests/agent_gates.rs`). The hosted reruns come later.

| Row | In-memory result | Gate | Verdict |
| --- | --- | --- | --- |
| LF-2 | 25 rounds of flag, refused push outside `confine`, `DeleteAgentApp` acknowledged, then the same push. 25 of 25 first decisions after the acknowledgment classed the App `Bot` and accepted the push. An unflag admitted on the install leader before the push reached the repository leader was also seen. Control: a leader deciding on an install fold read before the unflag keeps refusing (`lf2_control_a_stale_install_read_keeps_the_old_class`) | The next `decide` after the ack no longer classes it `Agent` | Pass |
| B0-36 | A confined vendor App (`ThirdParty`, `confine: refs/heads/claude/**`) and a Graftyard-run App, each on an installation widened to every permission. 100 of 100 refused for each of 11 actions per kind: approve, merge, ready for review, rule edit, settings edit, release, a label outside the allowlist, a `graftyard/` status, a `graftyard/` check run, a tag push, and a push outside the prefix (the Graftyard-run App is refused `NoLiveTask` first, in the design's order). Also for the vendor App: protected path 100 of 100, ruleset bypass refused, an allowlisted label applied, and an ordinary push in its prefix accepted. For the Graftyard-run App: a non-draft PR refused `DraftOnly` and a draft accepted. Controls: an unflagged App, a flagged App without `confine` (`Bot`), and a user token are never refused by an agent rule | 100 of 100 refused for each, both kinds; the others never refused | Pass for the rows `decide` can see before AM1-WP3/WP4 (see "Not covered") |
| B0-37 | 10,000 pushes by a user, an unflagged App, a flagged unconfined App, and a confined App. Each push had 1 to 3 commits with hostile messages: random bytes, random trailer blocks with CR/LF/continuations, forged `Graftyard-Session` values (wrong host, `http`, upper-case origin, path traversal, `%2e`, bare or App-scoped names, bidi characters), 5,000-character values, and messages past `MESSAGE_SCAN_MAX`. Each push also had 0 to 3 hostile `refs/notes/ai` entries. Pushes went through `validate`'s parser, the edge's `link`, the member wire, and `decide` in batches of 100 per entry. 0 failed on parsing. 17,264 forged `Graftyard-Session` trailers, 0 linked. 1,675 `Session` sources forged on the wire, 0 stored. 17,264 stored `Claimed`. 12,779 provenance rows. 1,000 of 1,000 `refs/graftyard/sessions/*` updates refused `Hidden`, and 0 such refs in the fold | 0 parse failures, every forged trailer `Claimed`, every `refs/graftyard/` update refused | Pass for the parser, linking, and `decide`. The fetch half (v0, v1, v2, `--mirror`) is not run here |

Other behavior the gates check:

- **`AgentGate` counts human writers on the current head** (`agent_gate_counts_human_writers_on_the_current_head`). With no recorded requester, merges need 2 approvals, and the refusal texts read `0 of 2` and then `1 of 2`.
  - These approvals write no gate: a repeat by the same writer, a writer's approval through CLAUDE's user-to-server token, and a reader's approval.
  - Two writers satisfy the gate.
  - An App push that moves the head voids it (`0 of 2` again).
  - `extra_human_approvals: 1` makes it `2 of 3`.
- **A default PR merges as before** (`a_default_pr_merges_as_before`). A user's PR is opened and approved in default formats and in 32.8. It gets no `AgentMeta` and no `AgentGate`, and it merges with no gate. No `AgentMeta`, `AgentGate`, or `Provenance` event is in the log.
- **`Co-authored-by` alone never classes** (`co_authored_by_alone_never_classes`).
  - A user-token push with only that trailer stays `Human` and is accepted, and an unflagged App's push stays `Bot`.
  - `Assisted-by` classes the App's push `Agent` for admission only. A user's push with it stays `Human`.

## Three bugs the gates found, fixed at the cause

1. **Followers refused an entry with a provenance-carrying push** (`OwnDefect`, then `NotLeader` on every ticket). `Push::tiers` was always `CORE`, but `Provenance` is a Metadata event, so verify said "the entry is tagged TierSet(1) but the event needs TierSet(2)". The agent block already says "A `Refs` entry whose push carries `provenance` is Core and Metadata". `Push::tiers` now returns Core and Metadata when `provenance` is non-empty, and `RepoCmd::tiers` delegates to it. The harness now asserts that no member raises an alarm.
2. **`session_from_url` panicked on a forged value cut inside a multi-byte character** (`value.split_at(origin.len())`). The value is untrusted text from a commit, so one push could panic the edge's `validate`. It is now `split_at_checked`, which returns `None`.
3. **An agent refusal could come after an App permission check** (`issue_ops::decide_in`), so an App installation without, say, `administration` got a bare 403 for a settings edit instead of the rule text. The agent check now runs right after `check_credential` and before any permission check. A rule edit and a settings edit by a confined App now name the rule.

## Planted bugs

`tests/am1/wp2/plant.py` plants each mutant, runs `agent_gates`, `agent_bytes`, and the git lib tests, then restores the file byte for byte. All 19 were caught:

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | Unflag not applied: the fold keeps the old `AgentApp` row (a stale class) | LF-2 |
| 1 | A confined App is never classed `Agent` | B0-36, LF-2, LF-2 control |
| 2 | Prefix check off | B0-36, LF-2, LF-2 control |
| 3 | Protected-path check off | B0-36 |
| 4 | Ruleset bypass allowed to agents | B0-36 |
| 5 | No live task not required of a Graftyard-run App | B0-36 |
| 6 | Gate ignores the missing requester (one approval suffices) | `agent_gate_...` |
| 7 | Gate not checked in `Merge::decide` | `agent_gate_...` |
| 8 | Gate counted on any head (a head move does not void it) | `agent_gate_...` |
| 9 | `AgentGate` written by an `AgentApp`'s user-to-server token | `agent_gate_...` |
| 10 | `Co-authored-by` classes | `co_authored_by_...`, `agent::tests` |
| 11 | A forged `Graftyard-Session` links without a task | B0-37, `agent::tests` |
| 12 | A `Session` sent on the wire is kept without a task | B0-37 |
| 13 | `refs/graftyard/` not host-owned | B0-36, B0-37 |
| 14 | Label allowlist ignored | B0-36 |
| 15 | 32.8 chosen for every push (default bytes change) | `agent_bytes` |
| 16 | The agent check after the permission checks | B0-36 |
| 17 | `Push::tiers` Core only with provenance | B0-37, `co_authored_by_...` |
| 18 | `session_from_url` splits inside a character | B0-37 |

## Byte identity: default replies and bytes unchanged

- **`crates/git/tests/agent_bytes.rs`** pins the bytes `main` (49d63f6) produced before any agent code (`agent_bytes.golden`). It covers a default push, issue command, and merge command by a user and by a plain App; the events they commit; the entries' digests and both fold tiers' digests after a scripted run; and the report-status bytes. It passes on the merged branch. Mutant 15 shows that it fails when a default push changes format.
- **Every API reply the api suites render** was compared against `main`. `GRAFTYARD_REPLIES` dumps were taken from a detached worktree of `main` at `baebca0` (its own target dir) and from `am1-wp2` after the merge. `diff -r`: all 120 of 120 files identical. That dump was also identical to the one taken before the first merge.
- **`crates/identity/tests/agents.rs`**: an `Authz` fold and `Policy` events with no agent rows keep their M3 bytes.

## What was built

- **Identity** (`crates/identity`):
  - `AgentApp`, `AgentKind`, and `WorkspaceMode`.
  - `RefPattern` and `PathGlob` (`glob.rs`).
  - `AuthzEvent::AgentApp { app, row: Option<AgentApp> }` (tag 14), where `row: None` is the unflag, and `AuthzEvent::OrgAgents { enabled, workspace }` (tag 15).
  - Ops `PutAgentApp` (26), `DeleteAgentApp` (27), and `SetOrgAgents` (28), in Authz format 0x12.3 (`AUTHZ_FORMAT_V3`). Only the install org's admin may write them, and only on the install's own chain. A `GraftyardRun` row needs the install's own App.
  - `OrgPolicy.agents_enabled` and `agents_workspace` are read through `AuthzRead::policy`. They are `#[serde(skip)]`, so a `Policy` record's bytes stay M3-WP3's.
  - `install_account()` = `Genesis.account`.
- **`crates/git/src/agent.rs`**:
  - Types: `ActorKind`, `AgentActor`, `AgentMeta`, `AgentGate`, `AgentPolicy`, `CiMode`, `LeaseEnforce`, `PROTECTED_PATHS`, `ProtectedHits`, `ChangedPaths`, `PATHS_MAX`, `Provenance`, `Source`, `TrailerKey`, `SessionId`, `HOST_REFS`, `SESSION_REFS`, and `NOTES_AI`.
  - `validate`'s rules, ported from the M0-WP10 prototype: the trailer parser, `provenance_of`, `changed_of`, and `protected_of`.
  - The linking rule (`link`, `unlinked`).
  - Classing: `actor_kind` for `decide`, and `admit_class` for admission.
  - The refusal rules and texts, and `approvals_needed`.
- **`Push`** gains `provenance`, `protected`, and `changed`.
  - These are carried only in push format 32.8 (`REPO_FORMAT_AGENT`), chosen only when one of them is non-empty. Formats 32.1, 32.2, and 32.7 carry counts of zero and keep their bytes.
  - `decide` refuses an `Agent` in the design's order: host-owned ref, stale, tag, no live task, prefix, bypass, protected. In 32.8 the refusal is per ref, as `RejectReason::Agent` (tag 7). `refs/graftyard/` is hidden for every actor.
  - An App push's deps add the install chain.
- **Repository events:** `Provenance` (27, Metadata), `AgentMeta` (28), `AgentGate` (29), and `AgentPolicy` (30), with core tables `agent_meta`, `agent_gate`, and `agent_policy`, and the metadata table `provenance`. Only 32.8 commands write them.
- **Issue and merge `decide`:**
  - An `Agent` is refused approve, ready-for-review, rule and settings edits, `SetAgentPolicy`, releases, `graftyard/` statuses and check runs, labels outside the allowlist, and merge. A Graftyard-run App may open draft PRs only.
  - `OpenPr` by an `AgentApp` (as an App or through its user-to-server token) writes `AgentMeta`.
  - A qualifying approval writes `AgentGate`. `Merge::decide` and queue `Enqueue` check `gate_holds`.
  - `SetAgentPolicy` is admin only.
  - The API renders an agent refusal as 403 (REST) or `FORBIDDEN` (GraphQL) with the rule text.
- **Wiring:**
  - The edge classes commands with `admit_class` and passes the class in `AdmitWire` on `/commit` only when it is not `Human`. The site member then calls `Member::commit_as(.., Admit)`.
  - `AdmitError::Full` maps to the existing `full` reply. `Stage` returns `{"error":{"stage":true}}`, after which the edge stages the pack and re-sends.
  - A plain user token is `Human` with no extra read, so a default request's reads and bodies do not change.
  - The leader classes again from its own fresh install read (LF-2), so the edge's class only orders admission.
- **Trust view:** `AuthorTrust.is_agent` is true for a PR with `AgentMeta` and for a check run whose App has an `AgentApp` row (`Directory::agent_app`). The edge reads the install fold only when the client opted in to the trust view. Tested by `is_agent_comes_from_agent_meta_and_the_install_s_agent_app_rows`.
- **Settings endpoints** under `/api/graftyard/v1` (`crates/api/src/agents.rs`, `workers/edge/src/agents.rs`), gated on `FORMATS_ENABLED`:
  - `GET /agents/apps`, and `PUT` and `DELETE /agents/apps/{id}`;
  - `GET` and `PATCH /orgs/{org}/agents` for `agents_enabled` and the workspace mode;
  - `PUT /repos/{o}/{r}/agents/policy`.
- **Site:** reads 32.8 and 0x12.3.

## Formats and tags taken

| What | Taken |
| --- | --- |
| Repository command format | 32.8 (`REPO_FORMAT_AGENT`): an agent-validated push, `SetAgentPolicy`, or an issue command with `agent` set. `main`'s 32.9 (`REPO_FORMAT_APP_CALLS`, `m3-close`) is unchanged: a `ghs_` issue command without agent fields stays 32.9 |
| Authz command format | 0x12.3 (`AUTHZ_FORMAT_V3`) |
| `AuthzEvent` tags | 14 `AgentApp`, 15 `OrgAgents` |
| `AuthzOp` | 26 `PutAgentApp`, 27 `DeleteAgentApp`, 28 `SetOrgAgents` |
| `RepoEvent` tags | 27 `Provenance`, 28 `AgentMeta`, 29 `AgentGate`, 30 `AgentPolicy` |
| `RejectReason` tag | 7 `Agent` |

## Checks (after merging `main` at `baebca0`)

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy of `workers/site` and `workers/edge`, with and without `--features test-faults`, and of `workers/ops`: clean.
- `cargo test --workspace`: 486 passed, 0 failed, 3 ignored (the long raft sweeps), in 56 suites.
- `GRAFTYARD_REPLIES` dump against `main`: 120 of 120 identical.
- `scripts/check_design.py` (agent block OK), `check_names.py` (1,206 files), and `check_links.py` (2,631 citations): pass.

`crates/raft` was not touched, so the long sweep and Porcupine were not rerun.

## Assumptions

- **`install_account()` is `DbId::pack(0, 0, 1)`**, the install's `Genesis.account` [inference].
- **No tasks exist before AM1-WP3.** So the following hold:
  - A Graftyard-run App's push is always refused `NoLiveTask`.
  - `AgentMeta.requester` and `session` are `None`, so every agent PR's gate needs one extra approval.
  - Every `Graftyard-Session` trailer stays `Claimed`.
  - `decide` drops a `Session` source from the wire that no task of the pusher names.
- **No user pusher is excluded from `AgentGate` yet.** Pushers to an agent PR's head ref are App installations until tasks exist [inference].
- **The unflag is `AgentApp { row: None }`** rather than a new event.
- **The org settings use their own event** (`OrgAgents`), so that `Policy` bytes do not change.

## Not covered (needs later packages or the hosted rerun)

- **B0-36 rows that need tasks, dispatch, or leases:** dispatch, workflow re-run, a fork push after its task is cancelled, a REST ref write into another task's Exclusive scope under `AgentsExclusive`, and the lease controls (AM1-WP3, AM1-WP4). Also the GraphQL surface, and "an outsider commit that changes a workflow under its ref", which depends on `validate` in the git cache.
- **B0-37's fetch half:** session refs advertised only to a v2 `ref-prefix` that starts with them, over v0, v1, v2, and `--mirror`. Nothing writes `refs/graftyard/` yet, because every update is refused.
- **"A ref update of a PR's base whose new tip contains the PR's head is a `Merge` of that PR"** needs `validate` to report contained `AgentMeta` heads. It is not built.
- **`PathSet` and leases** (`changed` is carried and stored, and nothing matches it yet): AM1-WP4.

## Design sentences these findings contradict or leave open

Each quote is from `design/AGENT-LAYER.md`, Shape, with the same text in `design/DESIGN.md`'s agent block where noted. Neither file was edited.

1. "`AuthzEvent` gains `AgentApp(AgentApp)`, written only on the install's own `Authz(Genesis.account)` chain." This gives no removal form, but LF-2 and the API ("`DELETE /agents/apps/{id}`") need one. Built as `AgentApp { app, row: Option<AgentApp> }`.
2. "`OrgPolicy` gains `agents_enabled: bool` (default false), `agents_workspace: WorkspaceMode` …" This names no event and no format. Adding the fields to the `Policy` record would change M3-WP3's bytes. Built as a separate `OrgAgents` event, with the fields `#[serde(skip)]` on `OrgPolicy`.
3. "`trusted` means a human who can push: never a bot, an `AgentApp` (`is_agent` false until AM1-WP2), or a commit author." This sentence is now stale. `is_agent` is set from `AgentMeta` and `AgentApp` rows.
4. "`Push` carries `changed` beside `protected` and `provenance`, `decide` matches it against live Exclusive leases, and `PathSet.paths` is copied from it." Leases and `PathSet` are AM1-WP4's, so in this package `changed` is carried and stored, and nothing matches it.
5. "`decide` records `Session` only when the pusher is an App's installation token and the commit names … an `AgentTask` of that App, live or final." This holds trivially until AM1-WP3. The gate shows that a `Session` forged on the wire is dropped (0 of 1,675 stored).
6. "A `Refs` entry whose push carries `provenance` is Core and Metadata." This is correct. The code broke it (bug 1 above), and the code now follows it.
