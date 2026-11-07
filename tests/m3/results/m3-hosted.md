# M3-WP1 to M3-WP4: the hosted half on graft-m1

This round takes M3-WP1 to WP4's pure halves onto the graft-m1 install. It covers a site day that reads the M3 formats, the edge's identity routes, a `graft-ops` Worker that mints `ghs_` tokens, the `AccountLogin` backfill, and the hosted gates with real clients. BM-3's first clause ran as journey 4 over HTTPS, 100 rounds. Everything ran on `graft-m1-*`. Nothing touched `graft-m2b-*` or `graft-m1-lin-*`.

## What was deployed

| Script | Version | What changed |
| --- | --- | --- |
| `graft-m1-site-a` | `10430b64-d5ef-4e06-8a27-b8a4457d7eff` | Site day; was `27bc2768` |
| `graft-m1-site-b` | `307605cc-364a-48d8-b45a-2ed1fe2ae260` | Site day; was `b6143e89` |
| `graft-m1-site-c` | `9c4bf22c-9c07-43cf-bd36-a88390a5e690` | Site day; was `8ea0c173` |
| `graft-m1-ops` | `ca658d46-850d-4c02-8e0d-463a46b0f903` | New |
| `graft-m1-edge` | `07e0176d-7bde-4840-9296-14ed5aa6ebcb` | The M3 formats, `LOGINS={}`, the `OPS` binding, and the routes below |

The edge's `FORMATS_ENABLED` is `32.1,32.4,32.5,32.6,32.7,16.2,17.2,18.2,20.1`: Names 0x10.2, Tokens 0x11.2, Authz 0x12.2, Identity 0x14.1, push 0x20.7, and repository 32.6 and 32.7 [measured, `wrangler deployments list`]. The edge went through two earlier versions, `8d4c599f` (formats and `LOGINS={}`) and `164585c3` (the `OPS` binding), on the way.

Other hosted state:
- The Secrets Store secret `graft-m1-mint-wrap-key`, id `e4c5ccc99a244c35a7c5b60e3e641d37`, in store `bfaa0c1573de4775b1b92a12fb89acb4`.
- Kid 1's `PutVerifyingKey`, at index 3 on `tokens:0`. The group was created for it.
- `tests/m3/hosted/setup.ts` seeded:
  - the org `m3org` (7100001), with the private repository `j4` (1048576) and base permission Write;
  - `m3-member` (7100002), and `m3-admin` (7100003), who is an org admin;
  - the App `m3-ci` (1114112), with client `Iv1.0000000000110000` and owner `m3-admin`, installed on `m3org` as 7190001 with contents write and metadata read.

  Each account has an `AccountLogin` row, an `Account` row, and an `Identity` chain. The run's state lives in `~/.graft-m3h/`, and its token files are mode 0600.

## What was built

- **Site day.** The site release reads every M3 format. `Kind::Identity` and `Hosted::Identity` host the new `Identity` family under `Source::Owner`, and `record_for` finds an `Identity(owner)` chain's placement. The Repo family reads 32.7. The Tokens fold reads `verifyingKey`, `jti`, `sshKey`, `code`, and a token's kind, expiry, and fields. Names reads `slug`, `app`, `client`, and `installation` (commits `9c48e48`, `248a837`).
  - It ran with `--test-faults` on the same container image, with `--roll-shards`. No shard needed rolling. A clone and a push on `wp6/wp6p` worked afterward [measured].
  - Before 0x12.2 was enabled, every deployed `Authz` fold was checked for a trailing part. None holds one, so `AuthzMore.installations` decodes [measured].
- **The edge** (`workers/edge/src/ident.rs`, commit `57da664`).
  - `authenticate` takes `ghs_` tokens. A token verifies offline against `TokenShard(0)`'s `VerifyingKey`. A fresh read of `jti_shard` that holds a revocation refuses it. That read creates the shard's group if it has none.
  - `POST /app/installations/{id}/access_tokens` checks the App's JWT, narrows by `repositories`, `repository_ids`, and `permissions`, and mints through the `OPS` service binding. `DELETE /installation/token` commits `RevokeJti` and answers 204.
  - OAuth's `POST /login/device/code` and `POST /login/oauth/access_token` run `UseCode`, then `StoreWith`, then `AddCredential` (crates/identity `edge::after_credential`).
    - A wrong secret gets 401 `incorrect_client_credentials`.
    - An App's client gets `unsupported_grant_type`, because `ghu_` is not served.
  - `GET|POST /login/oauth/authorize` and `POST /login/oauth/device/approve` stand in for the browser pages. A caller with a PAT gets a code or approves a user code. They are test routes, not the pages SHARED describes.
  - Also served:
    - `DELETE /applications/{client}/token`;
    - `GET /user` and `GET /users/{u}`;
    - `GET|PUT|DELETE /orgs/{org}/memberships/{u}` and `DELETE /orgs/{org}/members/{u}`;
    - GraphQL `viewer` (`graphql::query_viewer` in `crates/api`).
  - The edge sends a push under a `ghs_` token only when 32.7 is enabled. The bed route `/_bed/identity` commits on `authz:` and `identity:` chains under the owner's `Placement`.
- **`graft-ops`** (`workers/ops`). This is a real `KeySource` over Secrets Store, not a test-install stand-in.
  - `POST /mint` unwraps `MintWrapKey`: a JSON object with the wrap key, the kid, a nonce, and the signing seed under AES-256-GCM. That format is this build's choice [inference]. The design does not specify one.
  - The Worker reads the secret only when `Minter::needs_key` says the cache needs a key. `needs_key` is new in `crates/identity/src/stateless.rs`, and `Grant` now has serde.
  - `workers/ops/deploy.mjs` deploys it. `tests/m1/integ/deploy-edge.mjs` takes `--ops <name>` to bind it. `tests/m3/hosted/tools/src/bin/mintkey.rs` wrapped kid 1's seed and planned its `PutVerifyingKey`.
- **Backfill** (`tests/m3/hosted/backfill.sh`, with the planner `tests/m3/hosted/tools/src/bin/backfill.rs`, which is `finish::account_login`; commit `e6f986f`).

## The gates

### F50: logins with `LOGINS` empty. Pass.

The seed had never claimed logins for 7000002 to 7000004, so the backfill ran `ClaimLogin` for them first. It then wrote `AccountLogin` rows for 7000001 `wp6`, 7000002 `wp6-writer`, 7000003 `wp6-reader`, and 7000004 `wp6-revoked`. A second run planned nothing [measured; the logs were in `/tmp`, which the orb's restart at 05:33Z wiped].

`tests/m3/hosted/logins.sh` lists every login the edge renders on the seeded repositories' issue, pull, comment, and GraphQL lists. It ran once before, with `LOGINS` set, and once after, with `LOGINS={}`. Both runs rendered 787 logins across 9 repositories (`main`, `wp3b`, `wp3c`, `wp3f`, `wp3m`, `wp3q`, `wp3t`, `wp6j`, `wp6p`), and `diff` of the two found no difference [measured]. The run after the orb's restart rendered the same 787 logins in the same 35 lines: `wp6` and `wp6-writer`, and no `ghost` [measured, `tests/m3/raw/logins-after.txt`]. Eighteen lines are 404s for surfaces those repositories do not serve, the same in every run.

### BF-32: `@octokit/app` against the edge. Pass, 12 of 12.

`tests/m3/hosted/bf32.mjs`, `@octokit/app` 16.1.4. It passed 12 of 12 twice, before and after the restart [measured, `tests/m3/raw/bf32/`]:

| Case | Expected | Got |
| --- | --- | --- |
| Installation access token | 201, a `ghs_` token | 201, 344 characters, matches Renovate's `ghs_` regex |
| Read with it | 200 | 200 |
| Narrowed by `repositories` and `permissions` | 201 | 201 |
| Asks for `issues`, which the App lacks | 422 | 422 |
| Asks above the installation's level | 422 | 422 |
| JWT signed by another key | 401 | 401 |
| An issue write under the `ghs_` token | fail closed | 403 |
| `DELETE /installation/token` | 204 | 204 |
| A write after the revoke | 401 | 401 |
| OAuth web flow, authorize | 302 with a code | 302 |
| Exchange with a wrong secret | 401 | 401 |
| Exchange with the right secret | 200 | 200 |

The token is 344 characters, not 40, which is the point of `research/agents.md` R1 and finding 4.3. The issue-write case fails closed because issue commands do not carry a stateless principal yet (see "Not built").

### OAuth device flow with gh. Pass, 10 of 10.

`tests/m3/hosted/gh-login.sh` ran `gh auth login` against the device flow 10 times. Each run read the user code from gh's output and approved it through the stand-in page as `m3-member`, then ran `gh api user` and `gh auth status`. All 10 logged in, and all 10 showed `m3-member`. A second set of 10 after the orb's restart also logged in 10 of 10 [measured, `tests/m3/raw/gh-login/runs.txt`]. An earlier run of gh `auth login --web` also completed 1 of 1.

`tests/m3/hosted/device-replay.mjs` ran BF-33's replay cases 10 times as gh's client, twice, before and after the restart, with the same counts [measured, `tests/m3/raw/device-replay.json`]:
- An exchange before approval got `authorization_pending`: 10 of 10.
- The first exchange after approval got a `gho_` token: 10 of 10.
- A second exchange of the same device code got no token: 10 of 10.
- A made-up device code got no token: 10 of 10.

BF-33's leader-kill case, a leader killed between `CodeUsed` and the token's row, was not run hosted.

### git with a `ghs_` token. Pass.

A stock git push to `m3org/j4`, with the token from a credential helper, was accepted. Journey 4 below repeats it 100 times.

### BM-3 clause 1: journey 4 over HTTPS. RESULT_J4

`tests/m3/hosted/journey4.sh`, run as the orb service `m3h-j4`, 100 rounds against `m3org/j4` with a stock git client. Each round:
1. A fresh PAT for `m3-member` pushes, and the push is accepted.
2. `DELETE /applications/{gh client}/token` revokes the PAT. A push with it must be refused.
3. A fresh `ghs_` from installation 7190001 pushes, and the push is accepted.
4. `DELETE /installation/token` revokes the `ghs_`. A push with it must be refused.

At the end, `m3-admin` removes `m3-member` from the org, and a push with the member's live PAT must be refused. The member is then added back. A refusal counts only if git exits non-zero and the remote ref did not move.

| Step | Count |
| --- | --- |
| PAT pushes accepted | J4_PAT_OK of 100 |
| PAT revokes, 204 | J4_REV of 100 |
| Pushes with the revoked PAT refused, 401, ref not moved | J4_PAT_REF of 100 |
| `ghs_` pushes accepted | J4_GHS_OK of 100 |
| `DELETE /installation/token`, 204 | J4_JTI of 100 |
| Pushes with the revoked `ghs_` refused, 401, ref not moved | J4_GHS_REF of 100 |
| Push after member removal | J4_MEMBER |

[measured, `tests/m3/raw/j4-100/journey4.tsv` and `journey4.summary`]. A 2-round trial before the run refused 2 of 2 PATs and 2 of 2 `ghs_` tokens. Removal answered 204, the push after it got 403, and the re-add answered 200 [measured].

The first 100-round run reached round 44 with every revoked token refused, 88 of 88. The orb restarted at 05:33Z and took the run and its `/tmp` output with it. The run above is the second, started at 05:42Z, with its output under `tests/m3/raw/` (gitignored).

### Replies against GHES 3.22. Pass.

`tests/m3/hosted/identity.sh` collected these replies:
- `GET /user`, 200;
- `GET /users/{u}`, 200;
- `GET /users/{org}`, 200;
- `GET /users/{unknown}`, 404;
- a membership, 200;
- `GET /user` with a `ghs_` token, 403;
- `DELETE /applications/{client}/token`, 204.

`tests/m3/hosted/rest-cases.py` wrote those and the BF-32 REST replies as 14 REST cases. `tests/m2/gh-replies.py` took 10 GraphQL replies from the 10 gh logins. `crates/api/gh/validate.py` checked them against the GHES 3.22 description and the enterprise GraphQL schema: 10 GraphQL and 14 REST replies, 0 failed [measured].

## Real clients

| Client | Version | Flow | Result |
| --- | --- | --- | --- |
| gh | the orb's | `auth login` over the device flow, then `api user` and `auth status` | 10 of 10 [measured] |
| gh | the orb's | `auth login --web` | 1 of 1 [measured] |
| `@octokit/app` | 16.1.4 | BF-32: App JWT, installation token, narrowing, revoke, OAuth web flow | 12 of 12 [measured] |
| git | the orb's | push with a `ghs_` token through a credential helper | 100 of 100 accepted in journey 4, J4_GHS_REF of 100 refused after revoke [measured] |
| git | the orb's | push with a PAT, then with it revoked | J4_PAT_OK accepted, J4_PAT_REF refused [measured] |
| curl | the orb's | REST replies for `validate.py` | 14 of 14 valid [measured] |

## Latency

A device-code request takes 12 to 14 s. Twelve in a row took 11.9 s to 14.4 s, with a median of 13.1 s [measured]. Twelve more, after 30 device flows had already run, took 1.0 s once and 12.4 s to 13.9 s otherwise [measured, `tests/m3/raw/device-code-latency.txt`]. A token exchange takes about 15 s. Before each commit, `ident::commit_system` calls `graftyard_lookup::create`, which probes the group and creates it only if the probe answers nothing. A code's shard is one of 3,072, chosen by the code's hash. Almost every new code therefore lands on a shard with no group yet, and pays a group's creation on its first commit [inference, from the code; the 12 timings above all fall in the same band]. gh polls every 5 s and waits 900 s, so logins still complete. Creating the code shards' groups ahead, or routing codes to fewer shards, would remove the cost. Neither was done in this round.

## Tokens and logs

- Every token reached git through a credential helper that reads a 0600 file, and curl through a header on stdin. No token went on argv or in a URL. A scan of every live process's argv during the run found none.
- `rg "gh[pousr]_[A-Za-z0-9]{20,}|github_pat_|BEGIN [A-Z ]*PRIVATE KEY"` found nothing in the raw output, before the restart (`/tmp/m3h/raw`) or after (`tests/m3/raw`), nor in `tests/m3/hosted`, `workers/ops`, or `workers/edge/src`. It does match the 0600 token holders (`~/.graft-m3h`, `tests/m3/raw/*.tok`), which is expected. gh's `GH_DEBUG=api` logs show `gho_<redacted>` and a masked `Authorization` header.
- Workers Observability for the account's last 6 h has no event matching `gh[pousr]_[A-Za-z0-9]`, `github_pat_`, `eyJ`, `PRIVATE KEY`, `client_secret`, `code=`, `token=`, or `device_code` [measured, `tests/m3/raw/obs-scan.txt`]. Observability masks the `authorization` header as `********`. Request lines name `/login/oauth/access_token` and `/app/installations/{id}/access_tokens` but carry no query or body [measured].

## Not built

- `graftyard admin bootstrap`. The CLI command and graft-ops' sign-in link are not built.
- `graftyard mint-key rotate` on the hosted install. Rotation is built and tested in memory (M3-WP4). Kid 1 was published once by hand with `mintkey.rs`.
- The App's installation flow at the edge. `RegisterInstallation` and `Install` were committed by `setup.ts` as the operator.
- REST for teams, collaborators, invitations, and SSH keys. Journey 4 needs none of them: it uses tokens and org membership.
- `ghu_` user-to-server tokens.
- A stateless wire form for issue and merge commands, so a `ghs_` issue write fails closed with 403.
- The `Audit` consumer for the hosted chains. BF-3, BF-30, BF-33's leader kill, BF-34, BF-35, and BF-36 ran in memory only (M3-WP3).
- The transfer's `Settings` command over HTTPS.
- The browser pages for the device and authorize flows. The test routes stand in for them.
