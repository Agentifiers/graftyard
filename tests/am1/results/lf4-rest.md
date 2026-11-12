# LF-4's other six resources and B0-36's GraphQL labels on `graft-m3c` (2026-11-12)

The two LM-1 items left after `am1-runs` (`build/BUILD-PLAN.md`, AM1-WP6): LF-4 for pull requests, comments, reviews, review comments, commits, and check runs, which had no REST read on the edge, and B0-36's label mutation over GraphQL, fixed in `89cf205` but never rerun hosted. Branch `lf4-rest`, from `origin/main` `9a6d955`, rebased onto `199972d` (docs and results only) before the push. Hosted on `graft-m3c` only; `graft-m1-*` and `graft-m2b-*` were not touched.

## Summary

- **LF-4: pass, 8 of 8 resources, 16 of 16 each.** Gate: "100% absent from `visible` and counted"; fail: "Any present". One payload per `HIDDEN_CHARS` range (the last character of each of the 16, twice in each payload) in each resource, read back over REST with `Accept: application/vnd.graftyard.agent+json`: in 128 of 128 objects no hidden character is in `visible`, `invisible_chars` is 2, and the raw text (`body`, `commit.message`, `output.summary`) is byte for byte what was written. Every object the untrusted writer wrote reads `author.trusted: false`. The control read of each route without the media type carries no `graftyard_trust` (8 of 8 routes, all 200).
- **B0-36, label mutation over GraphQL: pass.** Gate: "100 of 100 refused for each, for both agent kinds". `addLabelsToLabelable` naming a label outside the repository's agent allowlist: 100 of 100 refused by the allowlist rule for the confined vendor App (m3-ci), 100 of 100 for `graftyard-agent` with its shipped permissions, and 100 of 100 for `graftyard-agent` widened to every permission. `Graftyard MCP`, the control, was refused 0 of 100. A label inside the allowlist was accepted 5 of 5 for every kind, so the refusals came from the rule, not from permissions. The outside label never reached the issue (its labels at the end: none).
- **Installation tokens on `graft-m3c`:** no longer blocked. This branch stood up `graft-m3c-ops` and bound it to `graft-m3c-edge` as `OPS`; m3-ci's mint answered 201.
- **Cost:** $0.33 for this branch's hosted window, after which `m0-carried` started its own site day at 02:05Z (00:15Z to 02:02Z, `INSTALL=graft-m3c tests/fanout/m2bcost.py`; containers $0.20). Cap $15.

## Routes added (F61)

Seven GETs in `rest_repo` (`workers/edge/src/api.rs`), each rendered in `crates/api/src/rest.rs` from the fold, and each reply still passed through `trust::layer`:

| Route | Renders | Notes |
| --- | --- | --- |
| `GET /repos/{o}/{r}/pulls/{n}` | `pull-request` | `webhooks::pull_request`, now shared; an issue number answers 404 |
| `GET /repos/{o}/{r}/issues/{n}/comments` | `issue-comment[]` | the webhook's comment object now uses the same renderer, byte-identical before and after |
| `GET /repos/{o}/{r}/pulls/{n}/reviews` | `pull-request-review[]` | pending reviews left out; a dismissed review reads `DISMISSED` |
| `GET /repos/{o}/{r}/pulls/{n}/comments` | `pull-request-review-comment[]` | oldest first, by (created, id): a reply's Metadata id can be lower than its thread's Core id |
| `GET /repos/{o}/{r}/commits/{ref}` | `commit` | the raw commit read from the network's git cache shard, a v2 `upload-pack` fetch of the one object (`deepen 1`, `filter tree:0`; `crates/git/src/commit_fetch.rs`) checked against its SHA-1. No shard, site, or container change. An unknown sha answers 422 "No commit found for SHA" |
| `GET /repos/{o}/{r}/commits/{ref}/check-runs` | `{total_count, check_runs}` | newest run first; an unresolvable ref answers 422 |
| `GET /repos/{o}/{r}/check-runs/{id}` | `check-run` | the webhook's check-run object now uses the same renderer |

The commit reply has no `files` or `stats`: both are optional in GHES 3.22's `commit` schema, and computing them needs a tree diff, which LF-4 does not read.

**Validation against the pinned description** (`crates/api/gh/validate.py`, `ghes-3.22.json`, with `DOTCOM_OPENAPI` for the webhook keys GHES does not describe):

- Local: `GRAFTYARD_REPLIES=… cargo test -p graftyard-api` then `validate.py`: 52 GraphQL, 96 REST, 11 LFS, and 30 webhook replies, 0 failed. That includes 14 new `trust-*.rest.json` cases, one plain and one `.agent` for each route (`crates/api/tests/surfaces.rs`, `rest_reads_of_the_trust_resources_render_ghes_shapes`).
- Hosted: `tests/am1/lf4/capture.mjs` read the seven routes from the deployed edge on `m3org/lf4-s`, plain and with the media type, plus the 422 for an unknown sha: 16 REST replies, 0 failed (`tests/am1/raw/lf4/hosted-replies/`, kept locally). For each route, the agent reply with `graftyard_trust` removed equals the plain reply.
- Byte identity for clients that do not opt in: each local reply goes through `keep_rest`, which runs the `agent_bytes` check (the plain reply is the agent reply minus the agent key, byte for byte).

## Code, tests, and plants

- `4803fb9` (was `dd219a8` before the rebase; the hosted site day and first edge deployed `dd219a8`): the renderers, `view.rs`'s `review_comments_of` and `check_run`, `commit_fetch`, the edge routes, the tests, the plants, and the drivers.
- The follow-up commit (`lf4-rest` head): the edge's test-faults `/_bed/identity` now commits on a `repo:` chain as well as reading it, under the `RepoOwner`'s record. LF-4's driver needs that to write review comments and check runs, which no REST or GraphQL route on the edge writes (`IssueOp::Comment` on a review line, `IssueOp::RecordCheck`), with the writer's own credential, which `decide` checks as it checks any write. The commit also adds the driver fixes found hosted, `capture.mjs`, `siteday.sh`, F61's entry, and this file.
- Tests: `surfaces.rs` covers ids, order, a pending and a dismissed review, cross-PR isolation, check-run filtering by sha, 404s, the 422, commit parsing with a `gpgsig` header and two parents, and the trust view per route (a reader's comment untrusted, the owner's trusted). `commit_fetch` has unit tests, checked against real `git upload-pack`: a 282-byte response whose cut object equals `git cat-file`.
- Plants (`tests/am1/lf4/plant.py`): 16 of 16 caught. They cover an issue served as a pull, an authorless comment, pending reviews listed, a dismissed review's state kept, review comments of every PR, id order instead of oldest first, check runs of every commit, oldest-first runs, a check run found by the wrong id, the commit message's trailing newline, only the first parent, the author read from the committer line, and four in the fetch: no SHA-1 check, progress packets read as pack data, a one-byte size header, and a request that asks for history.
- Full checks on the final code (`tests/am1/lf4/checks.sh`): fmt, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and for ops (ops has no `test-faults` feature), `cargo test --workspace` (0 failed), the three `scripts/check_*.py`, and `go test` in `containers/git/server`, all exit 0. The first run's `check_links.py` failure was only the site day's untracked build directory under `.worktrees/`; `siteday.sh` now builds outside the repository. The rebase changed no file under `crates/`, `workers/`, or `containers/`, and the three doc checks pass after it. No wire enum changed. `crates/raft` did not change.

## Hosted run

| Step | Result |
| --- | --- |
| `graft-m3c-ops` | `mintkey` kid 1; Secrets Store secret `graft-m3c-mint-wrap-key` in store `bfaa0c1573de4775b1b92a12fb89acb4`; script version `ee85c0b0`; `PutVerifyingKey` kid 1 on `tokens:0` at index 7; the private half shredded |
| Site day (`tests/am1/lf4/siteday.sh`, 00:58Z to 01:07Z) | `dd219a8` (now `4803fb9`) with `--test-faults`, the install's anchor, and the unchanged shard image `…git@sha256:172216ad…`: site a `d4874bbc`, b `6c69e111`, c `f4c0ca98` (were `9684ae63` / `c58c97c9` / `eadf3310`) |
| Edge | `ad4c8460` (01:09Z), then `296cc2b0` (01:35Z, edge only, the `/_bed/identity` repo commit, in the second commit), each with `OPS = graft-m3c-ops` and the same `FORMATS_ENABLED` (was `ba54c862`, no `OPS`) |
| Agent Apps (`tests/am1/rest/apps.mjs`) | `graftyard-workspace` 262144, `graftyard-agent` 327680, `Graftyard MCP` 393216, installations 7190011 to 7190013 on m3org; m3-ci given issues, pull requests, and checks write (`tests/am1/hosted/app-perms.mjs`) |
| LF-4 (`tests/am1/rest/lf4.mjs`, `m3org/lf4-s` = `repo:65542`, tag `mwcvmwpv`) | writes 16 of 16 for every resource; reads below |
| B0-36 labels (`tests/am1/lf4/b036-labels.mjs`, `m3org/b036-l` = `repo:65541`) | below |
| Teardown | `INSTALL=graft-m3c tests/fanout/teardown.py --all-nets --pools`: `git#0#net:65540`, `65541`, `65542`, and `git#0#pool:5`, 4 of 4 answered 200 |

LF-4 reads (`tests/am1/raw/lf4/lf4.jsonl`, last row):

| Resource | Writer | Read | Pass | `trusted` | Control (no media type) |
| --- | --- | --- | --- | --- | --- |
| Issue | untrusted user, GraphQL `createIssue` | `GET issues` | 16 of 16 | false | 200, 0 with `graftyard_trust` |
| Pull request | untrusted user, GraphQL `createPullRequest` | `GET pulls/{n}` ×16 | 16 of 16 | false | 200, 0 |
| Issue comment | untrusted user, GraphQL `addComment` | `GET issues/{n}/comments` | 16 of 16 | false | 200, 0 |
| Review | untrusted user, GraphQL `addPullRequestReview` (COMMENT) | `GET pulls/{n}/reviews` | 16 of 16 | false | 200, 0 |
| Review comment | untrusted user, `IssueOp::Comment` on a review line, its own PAT | `GET pulls/{n}/comments` | 16 of 16 | false | 200, 0 |
| Commit | admin, git push (author `am1 <am1@x>`, no account) | `GET commits/{sha}` ×16 | 16 of 16 | false | 200, 0 |
| Check run | App m3-ci, `IssueOp::RecordCheck` under the admin's PAT | `GET check-runs/{id}` ×16; the list for the tip held 16 of 16 views | 16 of 16 | false | 200, 0 |
| Release | admin, `POST releases` | `GET releases/{id}` ×16 | 16 of 16 | (not checked, the admin wrote them) | 200, 0 |

The untrusted writer is a fresh user (7100807) with no org membership and Read on the public repository, under its own PAT.

B0-36 (`tests/am1/raw/lf4/b036-labels.jsonl`): the policy `{labels: [lf4-ok-…]}` was set through `PUT /api/graftyard/v1/repos/{R}/agents/policy` (200).

| Kind | Token | Outside the allowlist, 100 calls | Inside, 5 calls |
| --- | --- | --- | --- |
| m3-ci (confined vendor App, `ThirdParty`, `confine refs/heads/claude/**`) | 201, issues write | 100 refused: "label … is not in this repository's agent label allowlist" | 5 accepted |
| `graftyard-agent`, shipped | 201, issues write | 100 refused, same message | 5 accepted |
| `graftyard-agent`, widened to every permission | 201 | 100 refused, same message | 5 accepted |
| `Graftyard MCP` (control) | 201 | 0 refused (100 accepted, each removed by the admin) | 5 accepted |

## Contradictions and notes

1. **The edge serves no write for a review comment or a check run** (no REST `POST pulls/{n}/comments`, no GraphQL `addPullRequestReviewThread`, no `POST check-runs`). LF-4's row needs a payload in each, so the driver committed those two through the test-faults `/_bed/identity` route on the repository's chain, with the writer's own credential hash, and `decide` checked them as it checks any write. That route only reaches test installs. Adding those writes is F61's scope, not LF-4's.
2. **`graft-m3c` had no `graft-ops`**, so it could not mint installation tokens (`tests/am1/results/install-m3c.md`). This branch added it, as above. `graft-m3c-ops` and the edge's `OPS` binding are now part of the install; a later edge deploy keeps them only with `--ops graft-m3c-ops` (`tests/am1/lf4/siteday.sh`).
3. **On `graft-m3c` the m3 admin is not a site admin of the install chain `authz:1`**, so `PUT /api/graftyard/v1/agents/apps/{id}` answers 403. `apps.mjs` opened `authz:1` as the install's org with only the operator acting for it. The driver flags m3-ci as the operator (`PutAgentApp` on `authz:1`), the same row the PUT writes. It is a test-install setup gap, not a product one.
4. Commits and check runs read `trusted: false` even though the admin pushed and recorded them. The commit's author is a git identity with no account, and the check run's actor is App m3-ci. Both follow `design/AGENT-LAYER.md`'s rule (trusted means a person with Write or above), so neither is a contradiction.
