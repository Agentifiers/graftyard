# AM2-WP2a: the MCP door, the static toolsets, and `ToolError`, code only

Branch `am2-wp2a`, from main `f59ae900`. Code only, nothing hosted. This is the half of AM2-WP2 that L0-08 needs
(`build/BUILD-PLAN.md`, AM2-WP2, "Split"). WP2b (`coordination.overlaps`, `provenance.why`, and their edge reads) is not
here.

## What was built

- **The door** (`workers/mcp/src/rpc.rs`): JSON-RPC over Streamable HTTP, stateless and JSON-only, in both lanes the
  four L0-08 clients speak.
  - **2026-07-28.** The lane is picked by `params._meta["io.modelcontextprotocol/protocolVersion"]`, as the TypeScript
    server's `classifyInboundRequest` picks it. The envelope must carry the version and the client capabilities. A
    version graft-mcp does not serve gets 400 -32022 with `data.supported`, which TypeScript v2 and Go retry with.
    `MCP-Protocol-Version`, `Mcp-Method`, and (on `tools/call`) `Mcp-Name` must agree with the body, else 400 -32020.
    A claimless request whose header names 2026 or later gets 400 -32602. `server/discover` and `tools/list` carry
    `resultType`, `ttlMs`, and `cacheScope`, because TypeScript v2 and Python fail without them. `listChanged` is
    false, because `true` opens `subscriptions/listen`, which needs SSE.
  - **2025-11-25.** `initialize` echoes a version it knows, otherwise answers 2025-11-25; `notifications/initialized`
    and every notification get 202 with no body; `ping` answers `{}`; a method it lacks is in-band -32601 with 200.
  - **Both.** GET and DELETE answer 405 with `Allow: POST`. A content type other than JSON gets 415, bad JSON 400
    -32700, and a batch 400 -32600. No `Mcp-Session-Id` is issued, so no client sends DELETE.
- **The tools** (`workers/mcp/src/tools.rs`), 20 in the `context`, `repos`, `issues`, `pull_requests`, `checks`, and
  `coordination` toolsets:
  - Reads: `get_me`, `list_branches`, `get_commit`, `list_issues`, `list_issue_comments`, `get_pull_request`,
    `list_pull_request_reviews`, `list_pull_request_review_comments`, `list_check_runs`, `get_check_run`.
  - Writes: `create_issue`, `add_issue_comment`, `create_pull_request` (a draft unless asked), and on
    `WriteApproveMerge` only, `approve_pull_request` and `merge_pull_request`.
  - Coordination: `coordination.lease`, `.renew`, `.release`, `.leases`, and `.activity`, under `/api/graftyard/v1`.
    The feed's wait is capped at 25 s, and a lease names its session in `X-Graftyard-Agent-Session`.
- **Who sees what.** `tools/list` shows a tool only when its toolset is granted and its write scope is at or below the
  token's. A hidden tool called by name gets -32602 and makes no edge call. A repository outside the grant answers
  404 with no edge call. Every other argument goes into the path as one percent-encoded segment.
- **The trust view.** Every read asks for `application/vnd.graftyard.agent+json`. Each body, commit message, and
  check-run output becomes the edge's `graftyard_trust.visible`, with `hidden` and `author` kept beside it.
  `body_html` is dropped, and so is any text field the edge did not view.
- **`ToolError`** carries the edge's status and message, `retry_after_s` from `Retry-After`, and the `X-RateLimit-*`
  fields. To read them, `Env` gains `edge_answer`, which returns the answer's headers; `edge` wraps it.

## Choices (each tagged in the code)

- **Writes are GraphQL mutations.** The edge serves no REST route for creating an issue, a comment, or a pull request,
  and serves `createIssue`, `addComment`, `createPullRequest`, `addPullRequestReview`, and `mergePullRequest` on
  `/api/graphql` [verified by reading `workers/edge/src/api.rs`]. GraphQL `{status}` and `errors[].type` map to 401,
  404, 403, or 422, so a write error reads like a REST one.
- **An issue's node id** comes from paging the repository's issue list, because the edge has no single-issue REST read.
  A pull request's comes from `pulls/{n}`.
- **A `ref` with a slash** (a branch such as `feature/x`) is sent as `feature%2Fx` and 404s. The edge's `commits/{ref}`
  route matches one segment and does not decode `%2F`, so allowing the slash would not reach the commit either.
  Recorded, not fixed: `get_commit` takes a SHA or a one-segment branch name.
- **Dotted tool names** (`coordination.lease`) are as AGENT-LAYER names them. All four SDKs accept them; Python's
  check allows `[A-Za-z0-9._-]`.

## Local four-client run

A scratch server ran `flow::serve` on 127.0.0.1 with a stub edge and a pre-sealed token. Each client is the unmodified
SDK the hosted drivers use, with 30 tool calls:

| Client | Lane | `tools/list` | Calls ok |
| --- | --- | --- | --- |
| `@modelcontextprotocol/client` 2.4.0, pin | 2026-07-28 (`server/discover` first) | 12 tools | 30 of 30 |
| `@modelcontextprotocol/sdk` 1.32.2 | 2025-11-25 (`initialize`, 405 on GET) | 12 tools | 30 of 30 |
| Go SDK v1.8.0 | 2026-07-28, and 2025-11-25 when pinned there | 12 tools | 30 of 30 each |
| Python `mcp` 2.3.0, `mode="2026-07-28"` (no discover) and `mode="auto"` | 2026-07-28 | 12 tools | 30 of 30 each |

The token held `read` scope, so the 12 read tools of the 20 were listed. The wire shape each lane needs was
read from the pinned SDKs' sources first, then run as above [measured, local only; not through OAuth].

## The hosted drivers for day 2

`tests/am2/hosted/ts-client.mjs`, `goclient/main.go`, and `py_client.py` now call `list_issues` on `m3org/j4` 30 times
instead of `tools/list`. Each call is interleaved with the same REST read sent straight to the edge with the member's
PAT, and each prints `clientP50`, `directP50`, and `overheadP50`. That is M0-WP15's method: a Worker's `Date.now()`
moves only across I/O, so a server-side timing reads 0 (`tests/m0/results/m0-wp15.md`, "Tool overhead p50").

## Tests (`workers/mcp/tests/wp2a.rs`, 9)

- The 2026 lane discovers and lists with the fields its clients require, and refuses what the TypeScript server
  refuses: header and body disagreeing, a missing envelope, an unknown version (with `data.supported`), a claimless
  2026 request, a batch, and a bad content type.
- The 2025 lane initializes statelessly, answers notifications 202, and answers GET and DELETE 405.
- A token lists and runs only its toolsets at or below its write scope; a hidden tool makes no edge call.
- A repository outside the grant is 404 with no edge call, and a ref or lease id cannot add path segments or a query.
- Every text field is the trust view; `body_html` and unviewed text are dropped.
- `ToolError` carries the status, `retry_after_s`, and the rate fields.
- Writes are GraphQL mutations on the scoped token, and a pull request is a draft unless asked.
- A lease names its session, and the feed wait is capped.

`workers/mcp/tests/a7.rs` now reaches the door through a legacy `ping` (200), where it asserted 501 before.

## Plants (`tests/m0carried/plant90.py`)

23 of 23 caught. The first run caught 22: mutant 6 (a repository name adding path segments) survived until the
path-segment test above was added.

| # | Mutant |
| --- | --- |
| 0 | a viewed body passes through raw |
| 1 | `body_html`, which renders the hidden text, reaches the model |
| 2 | a check run's raw summary reaches the model |
| 3 | text the edge did not view passes through |
| 4 | reads do not ask for the trust view |
| 5 | a repository outside the grant is called |
| 6 | a repository name may add path segments |
| 7 | a read token sees write tools |
| 8 | a token sees toolsets it was not granted |
| 9 | a hidden tool runs when called by name |
| 10 | a pull request is opened ready for review by default |
| 11 | `ToolError` drops `Retry-After` |
| 12 | `ToolError` drops the rate fields |
| 13 | a lease's session is not named to the edge |
| 14 | the feed's wait is not capped |
| 15 | the header version may disagree with `_meta` |
| 16 | `Mcp-Method` may disagree with the method |
| 17 | an unsupported version gets no `data.supported` to retry with |
| 18 | `tools/list` lacks the cache fields TypeScript v2 and Python require |
| 19 | discover advertises `listChanged`, which opens an SSE subscription |
| 20 | a notification gets a JSON body instead of 202 |
| 21 | GET is served instead of 405 |
| 22 | `initialize` does not echo a supported legacy version |

`tests/m0carried/plant86.py`, rerun because `flow.rs` changed: 20 of 20 caught.

## Checks

`bash tests/am1/runs/checks.sh`: all 13 steps answered RC=0.
- fmt and workspace clippy;
- wasm32 clippy for the site and the edge, each with and without test-faults, and for ops and `graft-mcp`;
- `cargo test --workspace`, 1,461 passed and 0 failed;
- `check_design` (agent block OK), `check_names`, and `check_links`;
- the shard's Go tests.

The edge and every crate it links are untouched, so edgeboot was not rerun.
