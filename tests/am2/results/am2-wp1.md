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

## Hosted plan and prep (2026-11-23)

The operator approved the plan with a $10 stop, metered from the site day's start (`tests/fanout/m2bcost.py`). It runs
on graft-m3f, which the lead handed to this work (`tests/am1/results/install-m3f.md`, site-day log). There will be
**one hosted day, after AM2-WP2 merges**, so L0-08 passes or fails whole on one site day. Branch `am2-hosted-prep`
holds the $0 prep. Nothing hosted has run.

### Rulings

1. **App 327680.** `graftyard-mcp` reuses App 327680, which `tests/am1/rest/apps.mjs` registered on 2026-11-14. Its
   old pem dies, and L0-12's "Graftyard MCP" kind can no longer mint on graft-m3f. `install-m3f.md` records the
   takeover.
2. **One site day.** It runs from merged main as it stands on the day, with a shard image built from the same head.
3. **Formats.** 20.4 (0x14.4) and 17.4 (0x11.4) are added to graft-m3f's `FORMATS_ENABLED` only, with no signed
   Release-group `FormatsEnable`.
4. **Teardown.** It deletes the four new store secrets, `graft-m3f-mcp`, and `graft-m3f-mcp-aux`, unless a later row
   needs them.

### What the prep changed

- **`graftyard apps init` flags.** They are product flags; without them the command behaves as before.
  - `--store <id>` writes to that store. Secrets Store's open beta allows one store and 100 secrets per account
    [doc, `research/cloudflare.md`], and the test account's one store is shared.
  - `--secret-prefix <p>` names the four secrets `<p>MCP_APP_KEY_A` and so on.
  - `--mcp-origin <url>` registers `<url>/callback` instead of `https://mcp.<host>/callback`, because AM2-WP3 does
    not route `mcp.<host>` yet.
  - `apps.json` records the prefix and origin. A test checks the named store, the prefixed names, the moved callback,
    the secret hash, and the refusals of a bad prefix or origin (`cli/src/apps.rs`).
- **The install's `Authz` read.** It takes `{"app": id}` and answers the App record's slug, callbacks, and client
  secret count, never the hashes (`workers/site/src/family.rs`). It ships with the site day.
- **`tests/am1/rest/apps.mjs`** refuses to touch an App whose record has a callback or a client secret, which only
  `apps init` writes, or whose state entry is marked `moved`. `agentToken` refuses a `moved` App.
  `tests/am1/hosted/rows.mjs` skips L0-12's "Graftyard MCP" kind when it is `moved` and logs why.
- **`workers/mcp/deploy.mjs`** deploys `graft-mcp` from `apps.json`, as a product build:
  - the `EDGE` service binding;
  - `MCP_CLIENT_SECRET` and `MCP_SEAL_KEY` from Secrets Store under the recorded prefix;
  - `MCP_CLIENT_ID` from `--app-id`;
  - `MCP_ORIGIN`, `HOST_ORIGIN`, and `DENY_ZONES`.

  `--delete` removes it.
- **`tests/am2/hosted/`:**
  - `apps.mjs` commits the claims, prints each slug's App id (from its route when the claim answers `Taken`), and
    commits the registration in file order. It marks `graftyard-mcp` `moved` before it sends anything.
  - `mcp.mjs` and `browser.mjs` play the user's browser. The browser goes through consent and the edge's stand-in
    `/login/oauth/authorize` as `m3-member`.
  - `l008.mjs` covers L0-08's cells that need no SDK client: wrong audience, refresh scope over 100, replay, framed,
    cross-site or skipped consent, and the held `ghu_`. It also runs LF-6 through the edge, 100 of each.
    graft-mcp's client secret is in Secrets Store only, so LF-6 puts a test secret beside it in the record's second
    slot (`CLIENT_SECRETS_MAX` 2) and restores the record after.
  - `ts-client.mjs` (SDK v2 2.4.0 and v1 1.32.2), `goclient/` (go-sdk v1.8.0), and `py_client.py` (`mcp` 2.3.0) run
    the SDK clients, unmodified. Each takes `--drop-token` for A43: the client's HTTP layer drops the first code
    redemption's answer after graft-mcp answered it. The row records every `/token` call, the consents, and whether
    the session came back.
  - `day.sh` runs the day's steps one at a time, one-shot per tag: init, claims, register, deploy, smoke, l008,
    clients, a43, and teardown. `aux.jsonc` is the CIMD host, M0-WP15's `aux.ts`, as `graft-m3f-mcp-aux`.
- **Records.** Secrets Store's limits are in `research/cloudflare.md`. `decisions/OPEN-QUESTIONS.md` C25 asks whether
  an install's keys fit under 100 and what a managed account holding several installs does. graft-m3f's handover and
  the App takeover are in `install-m3f.md`.

### Checks

`bash tests/am1/runs/checks.sh`: every step RC=0, and `cargo test --workspace` ran 1,446 passed and 0 failed. The
drivers load both TypeScript SDKs, the Go client builds and vets, and Python `mcp` 2.3.0 imports, each run against a
dead port. The aux config passes `wrangler deploy --dry-run`. Edgeboot does not apply: nothing the edge links changed.

### The day, in order

1. **Site day and edge day** from merged main, with a shard image from that head. The flags are m3fr37's:
   test-faults, `--anchor`, standard-4, `--ssh`, `--edge graft-m3f-edge`, and `--export-bucket graft-m3f-export`.
   FORMATS adds 20.4 and 17.4. Edgeboot runs on both builds first, and a one-shot launcher runs the deploy. The check
   is that `GET /api/graftyard/v1/mcp/grants` answers 401, not 503. About $0.05 to $0.20.
2. **Apps.** `day.sh <tag> init`, `claims`, then `register <insights id>`. This writes four secrets, so the store holds
   21 of 100. Under $0.01.
3. **Deploy and smoke.** `day.sh <tag> deploy` and `smoke`. $0.
4. **L0-08 and A43.** `day.sh <tag> l008`, `clients`, then `a43`. About $0.10 to $0.30.
5. **Teardown.** `day.sh <tag> teardown`, then list every shard container and tear them all down, failing loudly if
   any remain. The records are pushed and graft-m3f is handed back.

The total is about $1 against the $10 stop. The verdict lines are due within 30 minutes of the done line.

## Hosted, auth-only day m3fa2 (2026-11-23)

The lead cleared this auth-only day when WP2 ran past a day. It covers L0-08's auth cells, LF-6 through the edge, and
A43. The 30-call cell and the tool-overhead p50 wait for the second day, after WP2a merges. It ran on graft-m3f, taken
from B0-02 (`tests/am1/results/install-m3f.md`), under a $3 stop for the day.

### Verdicts

- **LF-6 hosted: pass**, 100 of 100 each (run m3fa2g, done line 04:32:20Z).
  - **422:** `token/scoped` without `graftyard_grant`, with another user's grant, and with `graftyard_write` above the
    grant, 100 of 100 each.
  - **409, revoked:** a stale generation got 409 in 100 of 100. Each time the grant left the list, and its scoped
    token and the user's unscoped `ghu_` answered 401 after.
  - **401:** wrong client credentials on the grant list, on `/generation`, and on `token/scoped`, 300 of 300.
  - **Control:** the right request answered 200 with GHES's `token/scoped` keys (`app`, `created_at`, `expires_at`,
    `fingerprint`, `hashed_token`, `id`, `installation`, `note`, `note_url`, `scopes`, `token`, `token_last_eight`,
    `updated_at`, `url`, `user`).
- **L0-08's auth cells: pass** (run m3fa2c, through graft-mcp `eb673cf8` and the edge `67c9c2f5`).
  - **Metadata:** RFC 9728 and RFC 8414 answer 200, with S256 only, CIMD, and `iss`. The door answers 401 with
    `resource_metadata`.
  - **Wrong audience:** 400 of 400 bad tokens refused, 100 each of the host's `ghu_`, a refresh token presented as
    access, another key, and garbage. graft-mcp's token is 401 at the host's `/api/v3/user` and 404 at the host's
    origin, and a suffix host gives no answer.
  - **Refresh keeps its scope:** 100 of 100 chained refreshes kept `write:write tools:context,repos,issues,pull_requests`,
    and 100 of 100 wider asks (`write_approve_merge`, an extra toolset) were `invalid_scope`.
  - **Replayed refresh:** `invalid_grant`. The chain's newest refresh token is refused after it, and the grant leaves
    the settings list.
  - **Framed or skipped consent:** no code. The page sends `frame-ancestors 'none'` and `X-Frame-Options: DENY`.
    Cross-site, no-Origin, and framed (`Sec-Fetch-Site: cross-site`) POSTs get 403. Skipped consent, another flow's
    state, and a replayed callback get 400. A real consent issues a code.
  - **Held `ghu_`:** the refresh answers 200, and the user's live unscoped rows go 6, 7, 7: the login adds the held
    `ghu_`, and the refresh adds the next one and deletes the held one. A tail on the edge for a second login shows
    the `DELETE /applications/{id}/token` answering 204 six seconds after the refresh's 200.
- **A43: a lost code answer costs the user a second consent, in all four clients** (m3fa2e, m3fa2f).
  - TS v2 (`@modelcontextprotocol/client` 2.4.0), TS v1 (`@modelcontextprotocol/sdk` 1.32.2), and Python (`mcp`
    2.3.0): `finishAuth` failed on the dropped answer, and none redeemed the code again. Each client's next connect
    got 401, went back through consent, and redeemed the new code with 200. That is two consents and two redemptions
    each.
  - Go (go-sdk v1.8.0): two consents. x/oauth2 probes the token endpoint's auth style, so each consent's first
    redemption is a 400 and the second a 200. The first a43 run dropped that 400, which x/oauth2 treats as the probe,
    so that run is void (m3fa2e). The driver now drops only a 200, and m3fa2f shows the second consent.
  - No client retried a code, so A43's flip (a once-row on a `Tokens` shard) is not met. The stateless rule costs a
    lost answer nothing a once-row would save, because a client that never retries needs a new consent either way.
- **Not run on this day, as cleared:** the 30-call cell and the tool-overhead p50. Every client got through auth and
  stopped at the door's 501 (`not_implemented`, WP2's tools), as expected. Python's session opened on 2026-07-28 and
  answered 0 of 30 `tools/list`.

### Fixes the day found (branch `am2-hosted-day`)

- **graft-mcp's CIMD fetch** answered `invalid_client: client metadata fetch did not answer 200` for a client id on
  the account's own `workers.dev`. That is Cloudflare's error 1042, a same-account fetch without
  `global_fetch_strictly_public` [doc, Workers "Errors and exceptions"]. M0-WP15's `graft-wp15-mcp` set the flag, and
  `workers/mcp/deploy.mjs` now does too. A CIMD document on another account never hits it.
- **graft-mcp's edge calls** all answered 403. The edge refuses a request with no `User-Agent`, as GHES does, and the
  service binding sends none. `workers/mcp/src/worker.rs` now sets `graft-mcp`. The local beds call the edge's code
  directly, so neither showed it.
- **`tests/am2/hosted/l008.mjs`:** LF-6's stale rounds reused one `ghu_`. The first stale generation correctly revokes
  it, so every later round answered 401 (m3fa2c, m3fa2d). Each round now signs in again, the list and the wrong
  credentials use a live token, and the rounds run five at a time.
- **`tests/am2/hosted/goclient/main.go`:** the A43 dropper drops only a 200 redemption.

### Runs

| Tag | What | Result |
| --- | --- | --- |
| m3fa2 | init, claims, register, deploy, smoke, teardown | ok. graftyard-mcp answered `Taken` as App 327680, and graftyard-insights is new as App 4784128 |
| m3fa2 l008 | all cases | stopped at the first login: CIMD's 1042 |
| m3fa2b l008 | all cases | stopped at the first callback: the edge's 403 with no `User-Agent` |
| m3fa2c l008 | all cases | meta, aud, refresh, replay, consent, and held pass. LF-6 stopped at the second grant (the driver's revoked `ghu_`) |
| m3fa2d l008 | lf6 | the same driver bug |
| m3fa2e l008 | lf6 | killed near stale round 111 by the orb's executor restart; void |
| m3fa2e clients, a43 | the four SDK clients | the A43 result above. Go's drop is void |
| m3fa2f a43go | Go with `--drop-token` | two consents |
| m3fa2g l008 | lf6 | pass, as above |

The restart killed m3fa2e before its `finally` put graftyard-mcp's record back. The record kept LF-6's test client
secret (two secrets) until about 04:21Z, when it was restored from `apps-register.request.json` and read back with one
secret. m3fa2g's `finally` restored it again at its end.

### Versions, cost, and teardown

- **Versions.** Sites a `8e3084cf`, b `b10d26f5`, and c `ba8635f4`, and the edge `67c9c2f5`, from `8dc9b691` (main
  `8d390b0f`'s tree plus the install row). The shard image is `sha256:91a19f9f…`, built from that head's
  `containers/git`. graft-mcp ran as `cc09e6ff`, then `2bcace1d` (the flag), then `eb673cf8` (the header), and the aux
  as `4e8f0f06`.
- **Cost.** graft-m3f's cost from the site day's start (02:53:41Z) to 04:33Z was $0.51 by `acctcost.py`
  (`prefix_usd`) and $0.47 by `m2bcost.py`, against the day's $3 stop and the $10 for both days. No shard container
  ran.
- **Teardown** at 04:32:51Z. `graft-m3f-mcp` and `graft-m3f-mcp-aux` are deleted (both answer 404). The four secrets
  are deleted from store `bfaa0c15…`, which holds 17 secrets after. The second day redeploys them with `apps init`,
  after moving `~/.graft-m3f/apps` aside, because step 1 refuses while `apps.json` exists. `teardown.py --sweep`: no
  shard container running.
