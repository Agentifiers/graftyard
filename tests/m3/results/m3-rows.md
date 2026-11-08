# Milestone 3's last rows on graft-m2b

This round makes M3-WP2's BF-2 and BF-31 and M3-WP3's BF-33, BF-34, BF-35, and BF-36 pass hosted (`build/BUILD-PLAN.md`, M3-WP2, M3-WP3, and "Milestone 3 status (2026-11-07)"). Branch `m3-rows`, cut from main `e2c67b5`. Everything ran on the **graft-m2b** test install. Nothing touched `graft-m1-*`, where BF-30's transfer is being built. Repositories 65537 and 65539 on m2b, which git cannot read, were left alone.

## Summary

| Row | Hosted result | Fail value seen |
| --- | --- | --- |
| BF-2 | 9 of 9 runs pass. 600 pushes queued behind a revocation are all refused 401; 0 acknowledged at or after the revocation's index; the 3 controls ack 300 from reads below it | None |
| BF-31 | 10 of 10: a `github_pat_` for one repository with `contents: read` is refused outside its scope and for writes; revoking either of a `ghu_`/`ghr_` pair refuses the other; `DELETE /applications/{client_id}/token` revokes | 0 uses after revocation |
| BF-33 | Leader of the new token's `Tokens` shard killed after `UseCode` committed: 99 of 100 complete with exactly one token, 1 restarts, the kill fired in 99 of 99 that reached it, and the replay is refused 100 of 100. Replay and expiry: 100 of 100 refused each (earlier run, kept) | None |
| BF-34 | 10 of 10 runs of the driver, 170 of 170 replies valid against GHES 3.22 (`collaborators`, repository invitations, teams, org invitations accepted and declined, the org-member `Collaborator` write refused 422); the GHES 3.22 description validates every documented reply. `TestAccGithubRepositoryCollaborator` cannot pass, because its config creates a `github_repository`, and `POST /orgs/{org}/repos` is not served (404). The same `github_repository_collaborator` resource, on an existing repository, applies, re-plans with no changes, and destroys | None (see Terraform) |
| BF-35 | 100 of 100: each new SSH key and PAT is in `GET /user/keys`, `gh ssh-key list`, and the `Identity` fold right after it is written, with no `Audit` consumer deployed | None |
| BF-36 | 10 of 10: a non-admin's `InstanceAuth` write is 403; a site admin's switch to OIDC is followed by a real OIDC sign-in (code flow with PKCE, verified `id_token`) and a Password sign-in is `WrongMethod`; after the switch back, `/login/oidc` is 404 | None |

16 of 16 planted bugs are caught (`tests/m3/rows/plant.sh`).

## What was deployed on graft-m2b

| Script | Version | From | When |
| --- | --- | --- | --- |
| `graft-m2b-site-a` | `c49618fa-0bf8-45ad-a436-71dbc06e0165` | `db1c6be` | site day 4, 07:53:39Z to 07:59:23Z |
| `graft-m2b-site-b` | `3864d168-d538-4615-954b-f7dae6237725` | `db1c6be` | site day 4 |
| `graft-m2b-site-c` | `93db160d-b1bf-4fad-8049-989eecc28651` | `db1c6be` | site day 4 |
| `graft-m2b-edge` | `fe066304-a18f-42e9-ab83-77dde7f11d93` | `8a9c4a2` | 07:31:31Z |

[measured, `wrangler deployments list` at 08:20Z, and `tests/m3/raw/rows/siteday4.log` and `edge3.log`]

- Container image: `graft-m1-site-gitshard-git@sha256:8fa9abd4…7f90c0891`, the one the install already ran.
- Every site day used `workers/site/deploy.mjs --test-faults --anchor … --roll-shards <edge>`, two passes per site (`tests/m3/rows/siteday.sh`). No `GRAFTYARD_AUDIT` was set, so no `Audit` consumer runs.
- Four site days in all:
  1. Main's build `e2c67b5` (23:37Z to 23:43Z), replacing `e174090`.
  2. `9e3a315`, which reads Identity 0x14.2.
  3. `2fc733b`, which reads repository 32.11.
  4. `db1c6be`, which carries the invitation-time fix below.
- The edge's `FORMATS_ENABLED`: `32.1,32.4,32.5,32.6,32.7,32.9,32.11,16.2,17.2,18.2,20.1,20.2`. That is `graft-m1-edge`'s list plus this branch's 32.11 and 20.2 (Identity 0x14.2) (`tests/m3/rows/deploy-edge.sh`).
- The edge versions before `fe066304` were `b2309a5a` (the old one), then `26ed6622` (07:22Z, from `2fc733b`).
- `graft-m2b-ops` was not redeployed.

### Seed

From `tests/m3/raw/rows/seed.log`, using `tests/m3/hosted/setup.ts` with m2b's env (`tests/m3/rows/env.sh`):
- the org `m3org` (7100001), `m3-member` (7100002, base `Write`), and `m3-admin` (7100003, admin);
- the repository `m3org/j4` (131072), plus `m3org/j4b`, which the BF-31 driver creates;
- the App `m3-ci` (196608, client `Iv1.0000000000030000`), installed on m3org as 7190001;
- one PAT each.

Every `Tokens` shard was pre-created through R20's product route `POST /api/graftyard/v1/release/tokens` (`precreate-r20.jsonl`, `precreate.done`). BF-36 makes `m3-admin` an admin of the install org (`SetMember` on `Authz(1)` as the operator), as `tests/m3/hosted/rows.ts` does.

## Formats and tags taken

| What | Value | Why |
| --- | --- | --- |
| Identity format | **0x14.2** (`IDENTITY_FORMAT_V2`) | `IdentityOp::AddKey` (op tag 7) and `IdentityEvent::KeyText` (event tag 6): a listed SSH key's public key text, which `GET /user/keys` renders and the `Tokens` row does not hold |
| Repository format | **32.11** (`REPO_FORMAT_INVITATIONS`) | `IssueOp::CancelInvitation`, for `DELETE /repos/{o}/{r}/invitations/{id}` |
| Authz | none | |

32.10 and Authz 0x12.4 are left for BF-30. origin/main (`d07f490` when checked) adds no format past `e2c67b5`.

## What was built

### Pure code

- **`graftyard_identity::token_allows`** (`crates/identity/src/access.rs`). A `github_pat_` or `ghu_` acts only inside its `resource_owner`, its `repos`, and its `permissions`. Push, issue, and merge `decide` all call it (`crates/git`), and the token's `TokenFields` come from its `Tokens` row, which the leader read as a dependency.
- **Identity 0x14.2 `AddKey`.** The leader refuses key text whose blob does not hash to the fingerprint (`KeyMismatch`), and a key whose `Tokens` row is not live (`NotCommitted`). The fold's serde form keeps 0x14.1's bytes when no key text is held.
- **`graftyard_identity::oidc`**: `verify_id_token`, which checks RS256 against the JWKS by `kid`, then the issuer, audience, nonce, `exp`, and `iat` skew, and needs `preferred_username`; `nonce_for(state)`; and `pkce_challenge`. Test keys come from a seeded RNG, so no key file is committed.
- **Repository 32.11 `CancelInvitation`**: only a repository admin may call it, and only on a pending invitation.
- **Fix: an issue command decides at the entry's time** (`db1c6be`). `IssueCall::decide` built its `RepoView` at `Timestamp(0)`, so `InviteCollaborator` set `expires` to a week after the epoch. Every hosted repository invitation was therefore already lapsed: the PUT answered 204, and the invitation never listed (first BF-34 smoke, `bf34-smoke.log`). `RepoScratch::at()` now passes the leader's `now`, or, when a follower verifies, the entry's `at`. This is the time `RepoView::new` already documented ("`at` is the entry's time"). Test: `a_repository_invitation_sent_through_the_leader_is_pending_and_accepted`, through a real leader at a 2026 clock. It fails on the old code: `expires Timestamp(604800000) is not after the entry's time Timestamp(1794000002002)`.

### Edge

Each edge piece is in its own module, apart from `ident.rs` and `precreate.rs`:
- **`usertok.rs`** (BF-31):
  - An App's authorization code or device code gives a `ghu_` and a `ghr_` through `StorePair`.
  - `grant_type=refresh_token` first revokes the old pair through its `ghr_`, which is the commit point, so a `ghr_` is used once. It then stores a new pair.
  - `POST /api/graftyard/v1/user/tokens` mints a scoped `github_pat_`. It is the test install's stand-in for the settings page.
  - `reaches()` refuses a REST read outside a token's scope.
  - `usable()` refuses a `ghr_` as a bearer, and an expired row.
- **`keys.rs`**: `GET /user/keys` and `POST /user/keys`. The key is stored with `AddSshKey` on its fingerprint shard, then listed with `AddKey`.
- **`collab.rs`**:
  - `GET`, `PUT`, and `DELETE /repos/{o}/{r}/collaborators[/{u}]`, and `GET .../collaborators/{u}/permission`;
  - `GET /repos/{o}/{r}/invitations` and `DELETE .../invitations/{id}`;
  - `GET`, `PATCH`, and `DELETE /user/repository_invitations[/{id}]?repo=o/r`;
  - `POST /orgs/{org}/teams`, `GET /orgs/{org}/teams[/{slug}]`, `PUT` and `DELETE .../memberships/{u}`, and `PUT .../repos/{o}/{r}`;
  - `GET`, `PATCH` (accept), and `DELETE` (decline) `/user/memberships/orgs/{org}`.
- **`oidc.rs`**:
  - `GET /login/oidc` sends the browser to the issuer's `authorization_endpoint` with `state`, `nonce`, and a PKCE S256 challenge. The state and verifier ride in an HttpOnly Secure cookie scoped to the callback.
  - `GET /login/oidc/callback` exchanges the code with the client secret and verifier, verifies the `id_token`, and commits `SignIn { method: Oidc }`. That leader reads `InstanceAuth` again.
  - `PUT` and `GET /api/graftyard/v1/admin/instance-auth`. GET never returns the secret.
- **`bed_rows.rs`** (test-faults): `/_bed/rows/queue` sends N pushes through `/commit_many` with the first dependency read held `hold_deps_ms`, and returns the leader's `deps_log`.
- **`ident.rs`, BF-33**: `commit_killable` puts the request's `x-graft-kill` on the exchange's own first send, on `UseCode` when `x-graft-kill-on: code` and on the new token's `StoreWith` when `token`. On a test install it returns the hedge's steps in `x-graft-trace`, so a run shows the kill fired.

## BF-2: revoke a token while 100 of its pushes wait in admission

`tests/m3/rows/bf2.mjs 9`, 07:25Z to 07:27Z, results in `tests/m3/raw/rows/bf2.jsonl`. Each run:
1. Makes a new `ghp_` for m3-member.
2. Sends 100 pushes under it to m3org/j4's leader in one `/commit_many`, with the first dependency read held 6 s.
3. Revokes the token on its `Tokens` shard at index R, `delay` ms later.
4. Reads the leader's `deps_log`, which records the index of the token's shard that every entry read.

A push acknowledged from a read at or past R is the fail value. So is one acknowledged with no read logged. The delay cycles through 2,000 ms and 0 ms, both inside the hold, and 9,000 ms, a control revoked after the queue decided.

| Runs | Delay | Pushes | Acked | Refused 401 | Acked at or after R | Acked with no read | Token read at |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1, 4, 7 | 2,000 ms | 300 | 0 | 300 | 0 | 0 | R (8) every entry |
| 2, 5, 8 | 0 ms | 300 | 0 | 300 | 0 | 0 | R (8) every entry |
| 3, 6, 9 (control) | 9,000 ms | 300 | 300 | 0 | 0 | 0 | R − 1 (7, 7, 16) |

**Pass, 9 of 9.** No push is acknowledged at an index after the revocation's. The controls show the check can tell the two cases apart: pushes decided before the revoke are acknowledged, from reads below R. Earlier smokes, 2 of 2 each, are in `bf2-smoke.jsonl`. The in-memory test is `bf2_a_revoked_token_refuses_all_100_queued_pushes` (`crates/git/tests/push.rs`).

## BF-31: scoped `github_pat_`, `ghu_`/`ghr_` refresh, `DELETE /applications/{client_id}/token`

`tests/m3/rows/bf31.mjs 10`, 08:07Z to 08:11Z, results in `tests/m3/raw/rows/bf31.jsonl`. Summary: `{"runs":10,"scope":10,"pair":10,"del":10,"useAfterRevoke":0,"failures":0}`. Every run got the same answers:

| Case | Answers |
| --- | --- |
| `github_pat_` for m3org/j4, `contents: read` | Read j4 200; read j4b (outside its scope) 404; push to j4 403; open an issue on j4 403 |
| Control: `contents: write` for j4 | Push to j4 200; push to j4b 403. The user's classic `ghp_` pushes to j4b, 200 |
| Pair | The App's code gives `ghu_` and `ghr_`. `ghu_` `/user` 200; `ghr_` as a bearer 401. Refresh: new pair; old `ghu_` 401; old `ghr_` `bad_refresh_token`; new `ghu_` 200 |
| Revoke either | `DELETE /applications/{client}/token` of the `ghr_` 204, then its `ghu_` 401 for a read and a push. On a third pair, revoking the `ghu_` (204) makes its `ghr_` `bad_refresh_token` |
| `DELETE /applications/{client_id}/token` | Wrong client secret 404, and the token still answers 200; right secret 204, then 401 for a read and a push; a second DELETE 204 |

**Pass, 10 of 10**, with 0 uses after revocation. The client secret is read from `~/.graft-m3r/app.secret` and sent only in the Basic header and the form body.

## BF-33: the device flow with the code shard's leader killed mid-flow

### Reaching "completes with one token"

The earlier hosted runs (120 of 120 restarts) killed the leader while it decided the exchange's `UseCode`. The killed entry then either committed, which spends the code and leaves no token, or was lost before the edge resent it. The edge resends under the same request id, and the re-send found the committed outcome, so the run always restarted.

The fix moves the kill one step later: the leader of the new token's `Tokens` shard is killed while it decides the exchange's own `StoreWith`, after `UseCode` committed and before the reply. The poll carries `x-graft-kill: <mode>` and `x-graft-kill-on: token`, and the edge puts the kill on its first `StoreWith` send only (`ident::commit_killable`). The modes alternate:
- `after_quorum`: the entry commits, and the leader dies before it answers. The edge's re-send under the same request id finds the outcome.
- `before_commit_early`: only the AppendEntries leave, and the entry may be lost. The re-send commits it.

Every 5th run holds the killed member down for 12 s. Each run then polls again, which is the replay, and counts the account's OAuth tokens on its `Identity` fold.

`tests/m3/rows/bf33-token.mjs 100`, 07:34Z to 07:48Z, results in `tests/m3/raw/rows/bf33-token.jsonl`:

| Mode | Down | Runs | Kill fired (trace) | One token | New listed tokens = 1 | Token works (`/user` 200) | Replay refused | Median, max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `after_quorum` | 0 | 40 | 39 | 39 | 39 | 39 | 40 | 2.5 s, 3.6 s |
| `after_quorum` | 12 s | 10 | 10 | 10 | 10 | 10 | 10 | 14.6 s, 14.7 s |
| `before_commit_early` | 0 | 40 | 40 | 40 | 40 | 40 | 40 | 2.5 s, 3.1 s |
| `before_commit_early` | 12 s | 10 | 10 | 10 | 10 | 10 | 10 | 14.4 s, 14.8 s |

Summary: `{"runs":100,"fired":99,"completed":99,"restarted":1,"other":0,"oneNewToken":99,"replayRefused":100,"tokensFromReplay":0,"extraTokens":0}`.

- "Fired" means the hedge's trace on the token's shard shows the killed member missing and a `suspect`, and a second send. With 12 s down, there are 44 to 47 sends.
- In run 77 (`after_quorum`), the first poll answered `expired_token` in 0.9 s before any `StoreWith`. Its trace is empty, so the kill was never sent. That answer comes either from the code's row read (lapsed or missing) or from `UseCode` refused as spent; the reply does not say which, and this run did not log it. The run restarted cleanly with no token, and its replay was refused too.
- **Pass.** Every run either completes with exactly one token, or restarts cleanly. No run made a second token, and no replay yielded one.

### Replay and expiry

Kept from the earlier run on this install, `tests/m3/rows/bf33-expiry.mjs`, 00:58Z to 01:32Z, results in `bf33-expiry.jsonl`:
- the first exchange gave 100 tokens;
- 100 of 100 replays were refused;
- 100 of 100 codes older than 900 s were refused.

The `UseCode` kill, from `bf33-kill.mjs`, results in `bf33-kill.jsonl`, fired 100 of 100. All 100 runs restarted with the code spent and no token, and the replay was refused 100 of 100. This matches the 120 of 120 restarts on graft-m1.

## BF-34: collaborators, repository invitations, teams, and org invitations

`tests/m3/rows/bf34.mjs 10 b`, 08:00Z to 08:34Z, results in `tests/m3/raw/rows/bf34.jsonl`. Each run makes three fresh users, `m3rb<i>-in`, `-out`, and `-no`, with a PAT each.

| Step | Expected | 10 runs |
| --- | --- | --- |
| m3-admin invites `-in` and `-no` to m3org (`Invite` on the org's `Authz`) | | |
| `GET /user/memberships/orgs/m3org` as `-in` | `pending` | 10 of 10 |
| `PATCH` it `{state: active}` | 200 `active`, role `member`, granted in the same entry | 10 of 10 |
| `DELETE` it as `-no` (decline) | 204; then GET 404 and PATCH 404: nothing granted | 10 of 10 |
| `PUT /repos/m3org/j4/collaborators/m3-member` (an org member) | **422**: the `Collaborator` write is refused | 10 of 10 |
| The same for `-in`, a member since its accept | 422 | 10 of 10 |
| m3-member (Write, not Admin) invites | 403 | 10 of 10 |
| m3-admin invites `-out` | 204, listed in `GET .../invitations` and in `-out`'s `GET /user/repository_invitations?repo=m3org/j4` | 10 of 10 |
| `-out` accepts (`PATCH`) | 204; then in `GET .../collaborators` with `push`, 204 on `GET .../collaborators/{u}`, `write` on `.../permission` | 10 of 10 |
| m3-admin invites `-no`, then `DELETE .../invitations/{id}` | 204; `-no`'s accept is 404 and `-no` is no collaborator (404) | 10 of 10 |
| `POST /orgs/m3org/teams`, `PUT .../memberships/m3-member`, `PUT .../repos/m3org/j4`, `GET .../teams/{slug}` | 201, 200, 204, 200 with `members_count` 1; m3-member lists teams 200, creates one 403 | 10 of 10 |

**Pass, 10 of 10.** Accepting grants in the same entry, declining grants nothing, and the member's write is refused. There was no overlap and no lost grant.

### Validation against GHES 3.22

`python3 crates/api/gh/validate.py tests/m3/raw/ghes/ghes-3.22.json tests/m3/raw/ghes/schema.docs-enterprise.graphql tests/m3/raw/rows/bf34-replies`, with graphql-core and jsonschema (through `uv run`):

```
0 GraphQL replies, 170 REST replies, and 0 LFS replies checked, 0 failed
```

The cases are every reply a documented operation gave, written by the driver as `<run>-<case>.rest.json`:
- `GET` and `PUT .../collaborators/{u}`, `.../permission`, and `GET .../collaborators`;
- `GET .../invitations` and `DELETE .../invitations/{id}`;
- `GET` and `PATCH /user/repository_invitations[/{id}]`;
- `GET` and `PATCH /user/memberships/orgs/{org}`;
- the four team operations and `GET /orgs/{org}/teams`.

The org-invitation DELETE (decline) has no GHES operation, so it is not validated.

### Terraform

`tests/m3/rows/tf.sh m3r-tf-ext`, 08:07Z to 08:08Z, log in `tests/m3/raw/rows/tf.log`. It used terraform 1.17.1 and terraform-provider-github built from its HEAD `1b9c5c1`, with m3-admin's token in `GITHUB_TOKEN` (the environment of those processes only), owner `m3org`, and organization mode.

1. **`TestAccGithubRepositoryCollaborator`**, run with `TF_ACC=1` and `GH_TEST_EXTERNAL_USER1=m3r-tf-ext`:
   - Two subtests failed on their first resource, `github_repository`: `POST …/api/v3/orgs/m3org/repos: 404 Not Found`.
   - The third skipped, because `GH_TEST_COLLABORATOR` was not set.
   - The edge serves no repository create (`POST /orgs/{org}/repos`). Repositories are created on the org's `Authz` chain as `CreateRepo`, as the seed does. So the acceptance test as written cannot pass on this install.
2. **The same resource on m3org/j4.** The `github_repository_collaborator` from the test's config (`permission = "triage"`, the external user) was run against the existing j4:
   - `apply`: "Creation complete after 4s [id=j4:m3r-tf-ext]" (`AddCollaborator` → 204, then a read through `ListInvitations`);
   - `plan`: "No changes. Your infrastructure matches the configuration.", exit 0;
   - `destroy`: "Destruction complete after 3s", which is `DeleteInvitation` through 32.11's `CancelInvitation`.
   - Afterwards, `GET /repos/m3org/j4/invitations` was `[]`.

## BF-35: a new SSH key and PAT, listed at once, with no `Audit` consumer

`tests/m3/rows/bf35.mjs 100`, 08:06Z to 08:15Z, results in `tests/m3/raw/rows/bf35.jsonl`. Each run:
1. Adds a fresh ed25519 key through `POST /user/keys`.
2. Makes a new PAT, stored on its `Tokens` shard and listed on `Identity` as the edge does.
3. Reads right away `GET /user/keys`, `gh ssh-key list`, and the account's `Identity` fold for the PAT. gh 2 runs with its own `GH_CONFIG_DIR` (0700), which holds m3-member's PAT. gh reads the PAT from there, never from argv.

Summary: `{"runs":100,"created":100,"keysListed":100,"ghListed":100,"patListed":100,"all":100,"failures":0}`. **Pass, 100 of 100.** No `Audit` consumer is deployed on m2b (no `GRAFTYARD_AUDIT`): each listing is written in the same request as the credential, by `AddKey` and `AddCredential`, which the leader refuses until the `Tokens` row is live.

## BF-36: a site admin switches `InstanceAuth` from Builtin to OIDC

### The IdP

`tests/m3/rows/oidc-idp.mjs` is a minimal OIDC provider:
- discovery;
- `/authorize`, which signs in the `login_hint` user with no page;
- `/token`, which checks the client id, client secret, PKCE verifier, and `redirect_uri`, and issues an RS256 `id_token` with the nonce;
- `/jwks`.

Its key is generated at start and kept in memory. It runs as the orb service `rows-idp`, port 18787. The edge reaches it through a Cloudflare quick tunnel (`rows-tunnel`, `https://foods-epinions-cartoon-transport.trycloudflare.com`). The client secret is in `~/.graft-m3r/oidc/secret` (0600).

### The run

`tests/m3/rows/bf36.mjs 10`, 08:05Z to 08:06Z, results in `tests/m3/raw/rows/bf36.jsonl`. Every run got the same answers:

| Step | Answer |
| --- | --- |
| m3-member `PUT /api/graftyard/v1/admin/instance-auth` to OIDC | **403**, and `InstanceAuth` stays `builtin` |
| m3-admin (site admin) the same | 204; GET reads `oidc` with the issuer and client, and no `secret` field |
| `GET /login/oidc` | 302 to the IdP's `/authorize` |
| IdP | 302 back with the code |
| Callback without the state cookie | 400 |
| Callback with it | **201, `method: "Oidc"`, account 7100003** (m3-admin) |
| `SignIn { method: Password }` on m3-admin's `Identity` | `WrongMethod` |
| m3-member switches back to Builtin | 403 |
| m3-admin switches back | 204, then `builtin` |
| `GET /login/oidc` after | 404 |
| A Password sign-in after | ok |

Summary: `{"runs":10,"ok":10,"fail":[]}`. **Pass, 10 of 10.** The IdP's log shows one `authorize` and one `token: issued` per sign-in.

## Planted bugs

`tests/m3/rows/plant.sh` follows `tests/m3/wp4/plant.sh`. It applies each plant alone, runs `graftyard-identity` (`--lib`, `m3`, `wp3`) and `graftyard-git` (`push`, `issues`, `agent_gates`), then reverts it. Log in `tests/m3/raw/rows/plant.log`; the rerun of `pat-any-owner` is in `plant-any-owner.log`.

| Plant | Caught by |
| --- | --- |
| `pat-reaches-every-repo` | `a_fine_grained_token_pushes_only_inside_its_scope_and_permissions` |
| `pat-read-writes` | the same |
| `pat-any-owner` | the same, after its another-owner case was added (`f86055f`); the first pass missed it |
| `push-ignores-token-scope` | the same |
| `pair-not-revoked` | `a_paired_token_revokes_with_its_partner` |
| `key-text-unchecked` | `add_key_lists_the_key_text_only_for_its_own_committed_fingerprint` |
| `key-not-committed` | the same |
| `cancel-by-anyone` | `collaborators_and_org_grants_never_overlap_and_acceptance_grants_in_one_entry` |
| `member-invited` | the same |
| `invitation-at-epoch` | `a_repository_invitation_sent_through_the_leader_is_pending_and_accepted` |
| `instance-auth-by-member` | `only_a_site_admin_writes_instance_auth_and_the_next_sign_in_follows_it` |
| `oidc-signature-unchecked`, `oidc-audience-unchecked`, `oidc-nonce-unchecked`, `oidc-issuer-unchecked`, `oidc-expiry-unchecked` | `oidc::tests::an_id_token_is_accepted_only_with_its_key_issuer_audience_nonce_and_time` |

**16 of 16 caught.** The edge's request handling (`usertok.rs`, `keys.rs`, `collab.rs`, `oidc.rs`) has no unit-test harness, because `graftyard-edge` is a `cdylib` with no test target. Its behavior is covered by the hosted drivers above.

## Checks

These ran at `63185be` (`tests/m3/raw/rows/checks.log`):

| Check | Result |
| --- | --- |
| `cargo fmt --check` | ok |
| `cargo clippy --workspace --all-targets -- -D warnings` | ok |
| wasm32 clippy, `workers/site` and `workers/edge`, each with and without `test-faults`; `workers/ops` | ok, all five |
| `cargo test --workspace` | 492 passed, 0 failed |
| `scripts/check_design.py`, `check_names.py`, `check_links.py` | OK; names OK in 1,232 files; links OK, 2,653 citations |

## Secrets

- No token, key, or secret was put on argv, in a URL, or in a log. Every driver writes rows through `lib.mjs`'s `scan`, which refuses a line holding any secret the run made or read, or any token-shaped string.
- The Terraform token was in the two processes' environment only. The IdP secret was in a 0600 file and the PUT body.
- Every raw log was scanned before this file was written: `grep -rEl 'gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9]{20,}|eyJ[A-Za-z0-9_-]{20,}\.' tests/m3/raw/rows` found no file. A second pass looked for every secret this run holds (the two PATs, the App and IdP client secrets, the bed token, and the tools token) literally in every file under `tests/m3/raw/rows`: 0 hits. The `rows-idp` and `rows-tunnel` service journals had 0 token-shaped strings.

## Design notes and contradictions (not edited)

- **`OidcConfig.secret` is stored as plaintext bytes.** `design/SHARED.md`, `InstanceAuth`: "secrets inside the configs are ciphertext." No wrap key exists for it, so `oidc.rs` stores the bytes the admin sent [inference]. The install's own `Authz` chain therefore holds the OIDC client secret in the clear until a wrap key for instance-auth secrets is designed.
- **`PUT /repos/{o}/{r}/collaborators/{u}` answers 204 for a new invitation.** GHES 3.22's description lists 204, 403, and 422 for it, and not GitHub.com's 201 with a `repository-invitation`. The edge follows the GHES description, and the Terraform provider accepted it.
- **No GHES route declines an org invitation.** `DELETE /user/memberships/orgs/{org}` is this install's choice [inference].
- **`/user/repository_invitations` needs `?repo=o/r`.** `design/SHARED.md`, `InvitationEvent`: "`Notify` puts pending invitations in the invitee's `Inbox` for `/user/repository_invitations` and `/user/memberships/orgs`." `Notify`'s `Inbox` is not built, so the edge lists one repository's invitations for the caller [inference]. Without `?repo`, the list is empty, which is not GHES's answer.
- **`IdentityEvent` gains `KeyText`** (tag 6, format 0x14.2). `design/SHARED.md` lists `pub enum IdentityEvent { Created(AccountCapture), Credential(CredentialEvent), Sso(SsoEvent), Audit(AuditEvent), Record(UserRecord), Session(SessionEvent) }`. A key's public text has no event there, and `GET /user/keys` must render it.
- **A 404 from `GET /repos/{o}/{r}/collaborators/{u}` carries GitHub's JSON error body.** GHES 3.22 describes that 404 with no body, so `validate.py` fails such a reply. The full run never produced one (every check there was 204), but the first smoke did, while the invitations had lapsed (`bf34-replies-smoke`).
- **A team's `node_id` uses the Organization node type**, because `graftyard_wire` has no Team node type.
- **A key's `created_at` is the epoch, and its `id` is the first 6 bytes of its fingerprint.** No creation time or id is stored for a key.

## Not done

- **`TestAccGithubRepositoryCollaborator` as written.** It creates a `github_repository`, and the edge serves no `POST /orgs/{org}/repos`, which is outside BF-34's routes. The collaborator resource itself was exercised against an existing repository (above).
- **Nothing was pushed or merged.**

## Commits

On branch `m3-rows`, from `e2c67b5`:

| Commit | What |
| --- | --- |
| `b579ba1` | graft-m2b drivers (env, site day, edge deploy), `BED_TOKEN_FILE`, R20 through the product route, BF-33 expiry driver |
| `feb8974` | `token_allows` on push, issue, and merge `decide`; Identity 0x14.2 `AddKey` with `KeyText`; BF-2 and BF-31 in-memory tests |
| `9e3a315` | siteday.sh comment |
| `2fc733b` | Edge routes for BF-31, BF-34, BF-35, BF-36; `CancelInvitation` at 32.11; BF-33's kill on the exchange's own commits; BF-2's queue route |
| `8a9c4a2` | Hosted drivers for BF-2, BF-31, BF-35, BF-33 and the test IdP; `x-graft-trace` on the killable commit; edge FORMATS add 32.11 and 20.2 |
| `db1c6be` | An issue command decides at the entry's time (the invitation-expiry fix); BF-34 driver |
| `96259cd` | BF-36 driver and `plant.sh` |
| `f86055f` | BF-31 push test covers a `github_pat_` for another owner |
| `63185be` | `plant.sh` `PLANTS_ONLY`; `tf.sh` |
| (this commit) | This file |
