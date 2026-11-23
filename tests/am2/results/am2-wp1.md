# AM2-WP1: `graftyard apps init` and the `graft-mcp` authorization server, the pure half

Branch `am2-wp1`, from main `e31c8c1b`. Code only, nothing hosted. LF-6 and L0-08's local checks pass in memory, over
real `Identity`, `Tokens`, `Authz`, and `Names` members (`crates/identity/tests/common`, included by path).

## What was built

- **`graftyard apps init`** (`cli/src/apps.rs`), in two steps.
  - Step 1 (`--host`) draws the `Graftyard MCP` and `graftyard-insights` RS256 keys, the MCP client secret, and
    `graft-mcp`'s seal key. It writes all four straight to the install's Secrets Store (`graft-ops-<install>`) and
    keeps no copy. It writes `apps.json` (public keys, the client secret's SHA-256) and the two `ClaimSlug` requests.
  - Step 2 (`--app-ids`) writes the registration requests: `PutApp`, `RegisterApp`, and `RegisterClient` for each App,
    and `PutAgentApp(Mcp)` for `Graftyard MCP`.
  - A test commits both steps' files on real members in file order. It checks the stored secrets match the records:
    the public key halves and `secret_hash`.
- **The host's half** (`crates/api/src/mcp.rs`, routed by `workers/edge/src/mcp.rs`):
  - `/api/graftyard/v1/mcp/grants` (POST, GET), `/{id}/generation` (POST), and `/{id}` (DELETE). Every call takes
    `Graftyard MCP`'s client id and secret as Basic credentials (401 otherwise). POST, GET, and DELETE also take a
    live `ghu_` of that App in `X-Graftyard-User-Token`.
  - `POST /api/v3/applications/{client_id}/token/scoped`, served for `Graftyard MCP` only. It requires
    `graftyard_grant` and `graftyard_write` and checks them against a live grant of the token's user, else 422. It
    narrows by GHES's `target`, `repositories`, and `permissions`, and stores the scoped `ghu_` with `resource_owner`,
    `repos`, `grant`, and `write` on its `Tokens` row. Every other client gets the edge's 404, as before.
- **The grant rows** (`crates/identity/src/identity.rs`, Identity format 0x14.4):
  - Events: `IdentityEvent::McpGrant { grant, revoked }` (tag 8) and `IdentityEvent::McpScoped` (tag 9). `McpScoped`
    records each scoped token under its grant, with the unscoped `ghu_` it narrowed.
  - Ops: `PutMcpGrant`, `McpGeneration`, `RevokeMcpGrant`, and `McpScoped` (tags 10 to 13). The leader assigns grant
    ids. A generation other than the grant's own revokes the grant in the same entry, and `McpScoped` re-checks the
    grant and the write scope at the commit point.
  - Tokens format 0x11.4 lets a `ghu_` carry `resource_owner` and `repos`.
- **`graft-mcp`** (`workers/mcp`, a new Worker crate; its failure and deployment rows were already in AGENT-LAYER):
  - The types `Consent`, `McpToken`, `McpRefresh`, `Toolset`, and `ToolsetMask`. `WriteScope` is the host's.
  - AES-256-GCM sealing, with one key per kind derived from `MCP_SEAL_KEY`. The slots are newest first, so a
    rotation keeps old tokens open.
  - RFC 9728 and RFC 8414 metadata, `/authorize`, `/consent`, `/callback`, `/token`, `/settings`, and the MCP door.
  - `flow.rs` is plain Rust over an `Env` trait; `worker.rs` implements it with `EDGE`, Secrets Store, and `fetch`.
- **Wire tags** pinned in `crates/git/tests/wire_tags.golden`: the four Identity enums' new variants and
  `IdentityOut`.

## Choices the design left open (each tagged in the code)

- **Code use is the host's, not an isolate's.**
  - Redeeming our code spends the grant's generation (0 to 1). A second redemption is then a stale generation,
    which revokes the grant, as RFC 6749 4.1.2 asks of a code used twice.
  - A replayed callback fails because the host spends its own code once.
  - The prototype's per-isolate "used" sets are gone, so "once" holds across isolates [inference].
- **The held `ghu_`** stays sealed in the `McpRefresh` (`held`). Only after a refresh spent the `ghr_` does
  `graft-mcp` send `DELETE /applications/{client_id}/token` for it (A7 step 5, A4's fallback, built as the
  behavior).
- **Generation first.** A refresh spends the generation before the upstream refresh, so a replayed `McpRefresh` never
  reaches `/login/oauth/access_token` (M0-WP15's finding 2, now a test).
- **Revoking a grant** revokes every scoped token recorded under it, plus the last unscoped `ghu_`, which takes its
  `ghr_` with it. The edge reads the row fresh after the commit, so a repeated `DELETE` finishes a revocation whose
  edge died partway.
- **Store, then bind.** `token/scoped` stores the scoped token first, then commits `McpScoped`. If the grant was
  revoked in between, the leader refuses, the edge revokes the token it just stored, and answers 422. No scoped token
  of a revoked grant is ever live or shown.
- **Repositories** are named on the consent page (`owner/name`, one owner, at most 4) and sent as `token/scoped`'s
  `target` and `repositories` [inference: `McpToken.repos` is a `SmallVec<[RepoId; 4]>`; one owner because
  `token/scoped` takes one `target`].
- **A scoped token lives an hour**, capped at the unscoped token's expiry [inference: the `McpToken`'s hour].
- **The MCP door** checks the token's prefix, key, audience, and expiry. A good token answers 501 until AM2-WP2's
  tools land.

## LF-6 (`workers/mcp/tests/lf6.rs`)

| Case | Result | Pass |
| --- | --- | --- |
| `token/scoped` from `Graftyard MCP` without `graftyard_grant` | 422 `missing_field`, 100 of 100 | 422 |
| With another user's grant | 422 `invalid` on `graftyard_grant`, 100 of 100 | 422 |
| With `graftyard_write` above the grant (`write` and `write_approve_merge` on a `read` grant) | 422 `invalid` on `graftyard_write`, 100 of 100 | 422 |
| Without `graftyard_write` (extra) | 422 `missing_field`, 100 of 100 | 422 |
| A stale generation (0, 2, 7 after one good refresh) on `POST /mcp/grants/{id}/generation` | 409 `revoked: true`, 100 of 100. The grant row is revoked, its scoped token and the unscoped token are revoked, the right generation is refused after, a new `token/scoped` is 422, and the grant leaves the list | 409, revoked |
| Grant calls (all four routes) with no, wrong, empty, another client's, or Bearer credentials | 401, 100 of 100. The grant is untouched | 401 |

Controls and extras:
- No refused request stored a token.
- The good request answers 200, with the token's row as described above.
- The `Identity` leader refuses an `McpScoped` above the grant, under no grant, or from another user.
- A revocation that races `token/scoped` leaves no live token.
- A user token of another App gets 401 on the grant endpoints and 404 on `token/scoped`.
- Another client's `token/scoped`, with or without the fields, gets the edge's own 404.

**Validated against the pinned GHES 3.22 description.** `GRAFTYARD_REPLIES=… cargo test -p graftyard-mcp --test lf6`
then `crates/api/gh/validate.py`: "0 GraphQL replies, 5 REST replies, 0 LFS replies, and 0 webhook payloads checked,
0 failed". Those are the 200 `authorization` and four 422 `validation-error` bodies.

## L0-08's local checks (`workers/mcp/tests/a7.rs`)

| Check | Result |
| --- | --- |
| Wrong-audience tokens (the host's origin, another server, a suffix host, no trailing slash) | 100 of 100 refused, 401 with `resource_metadata`. Extras, 100 each, all 401: our audience under another key, a refresh token presented as access, the upstream `ghu_`, expired |
| A refreshed token keeps its scope | 100 of 100 chained refreshes. Each answers the same scope, tools, and repositories, calls `token/scoped` again (a new upstream row with the grant's write), and moves the generation by one |
| No refresh wider than its grant | `write_approve_merge`, an extra toolset, `write:admin`, and `repo` are each `invalid_scope`; a narrower scope is granted |
| A replayed refresh revokes the grant | `invalid_grant`, with no upstream refresh call. The chain's newest refresh token is then refused, and the live access token's upstream token, the held `ghu_`, and its `ghr_` are revoked |
| Framed or skipped consent issues no code | The page sends `frame-ancestors 'none'` and `X-Frame-Options: DENY`, and sets no cookie before consent. POSTs with no Origin, another Origin, `null`, or `Sec-Fetch-Site: cross-site` get 403 with no redirect or cookie. A skipped consent, another flow's cookie, and a replayed callback each get 400 with no redirect |
| A refresh succeeds with the unscoped `ghu_` held, which is deleted after it | 100 of 100. The held `ghu_` is live before each refresh, and the `DELETE .../token` naming it comes after the `ghr_` was spent |

Also covered:
- The end-to-end flow, with RFC 9207 `iss` and the client's `state` on the answer.
- A wrong verifier, client, or redirect spends nothing.
- `/authorize` refuses plain PKCE, a wrong `resource`, and an unknown scope by redirect. The CIMD cases are shown,
  never redirected: a 301 or 302 with a good body, over 5 KB, a mismatched `client_id`, no redirect URIs, not JSON,
  `http:`, a root path, the install's own zones, and an unregistered `redirect_uri`.
- The CIMD cache follows `max-age`.
- The consent page shows the client host, every redirect host, and the localhost warning, and escapes a hostile
  `client_name`.
- Settings signs in through the host's OAuth, lists the grants, refuses a cross-site revoke, and a revoke kills the
  client's upstream token.

What this bed does not show: a browser engine (framing is checked by header) and real SDK clients. L0-08 hosted
measures those, after AM2-WP3's routing.

## Plants (`tests/m0carried/plant86.py`)

20 of 20 caught. Eight target the host's rules:
- a write above the grant;
- another user's grant;
- no grant;
- any client secret;
- another App's user token;
- a stale generation that moves on;
- `McpScoped` above the grant at the leader;
- a revocation that leaves tokens live.

Twelve target `graft-mcp`'s rules:
- a raced scoped token left live;
- no PKCE;
- another audience;
- cross-site consent;
- an unbound state;
- a widening refresh;
- no generation on refresh;
- no delete of the held `ghu_`;
- a CIMD id mismatch;
- over 5 KB;
- a redirect followed;
- every write scope advertised.

The first run caught 13 of 19 and showed six weak tests, each fixed at the test or the code:
- **Missing field.** A missing `graftyard_grant` was refused only incidentally, so the test now checks the error
  code.
- **Revocation.** It returned the tokens in the commit's answer, which a replay lost. The edge now reads the row
  fresh.
- **PKCE.** A wrong verifier was never tried, and now it is.
- **The held `ghu_`'s delete.** It was only implied by the pair's revocation, so the test now checks the `DELETE`
  and its order.
- **CIMD size and redirects.** Each body failed another check first, so the cases are now good but for one rule.
- **Race.** A scoped token stored before `McpScoped` could outlive a racing revocation, so the order is now store,
  then bind, then revoke on refusal.

## Checks

`bash tests/am1/runs/checks.sh`, which now also runs wasm32 clippy for `workers/mcp`. Every step answered RC=0:
- fmt;
- workspace clippy;
- wasm32 clippy for the site and the edge, each with and without test-faults, and for ops and `graft-mcp`;
- `cargo test --workspace`, 1,412 passed and 0 failed;
- `check_design` (agent block OK), `check_names`, and `check_links`;
- the shard's Go tests.

Edgeboot (`tests/edgeboot/run.sh`) passes on the test-faults build and with `PRODUCT=1`, with 0 wasm traps.
