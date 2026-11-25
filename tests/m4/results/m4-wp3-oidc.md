# BF-70 on graft-m4a: Actions OIDC across an `ActionsOidcKey` rotation

Status (2026-11-25 08:06Z): **stopped after the first job, not passed.** The run is r1. The driver is `tests/m4/wp3/oidc/hosted-row.sh` on branch `bf70-driver`, and raw logs are in `tests/m4/raw/wp3/oidc-hosted/r1/`. The row spent $0.13 of its $5 stop: graft-m4a's UTC day went from $1.95 at the start to $2.08. Nothing was deployed. graft-m4a-ops stayed on `ac4cd9a5` with schedules `[]`, and no `Rotate` was signed. `ActionsOidcKey` is still on slot A, with both slots bound and published.

## What ran

| Step (Cloudflare time) | Result |
| --- | --- |
| `state`, 07:53:35Z | The issuer `https://graft-m4a-edge.<sub>.workers.dev/_services/token` serves its discovery document, and the JWKS lists both kids: A `xGTXBDFSTAgZPaEknItTcA4H69VLmaXS4XMnKlWWDDA` and B `g-7DvKNwFRNdKn5xF8YpU1tlE7H2n65FfIVhvJS8mLk`. Sites were on `eb5c77f2`, `c6b12562` and `3a1ea21b` (site day 3 from main `0aa3b507`), the edge on `d3d76118`, ops on `ac4cd9a5`. The view's `ActionsOidcKey` slot was 0 |
| `minio`, 07:54:04Z | A MinIO STS on the orb read the edge's openid-configuration at start, live 200. Policy `m3org` covers bucket `probe` |
| `action`, 07:56:07Z | `m3org/bf70-cac` (public) holds configure-aws-credentials v6.2.0's tree `ed22c69c` as commit `2a183bb2`. The first push answered 400 "shallow pushes are not supported", because it came from the shallow clone; the driver now pushes from a fresh bare repository |
| `job before`, 07:58Z to 08:00Z | **Granted job `sts`: Failure.** Its first step, `test -n "$ACTIONS_ID_TOKEN_REQUEST_URL"`, exited 1: the runner exported no token URL. The job without the grant, `denied`, succeeded: no URL, and its own token got **403** at `run/idtoken`. The action was resolved from the install (`Download action repository 'm3org/bf70-cac@v6.2.0' (SHA:2a183bb2…)`). No job log holds a token. No 503 retry was needed |

The first `job before` attempt (`job-before-norunner/`) started no runner, because the driver passed a relative out directory to `hosted-run.mjs`, which starts the runner in the runner's own directory. It is fixed (`$PWD/$O`), and its jobs ran on the second attempt.

## Why the granted job got no URL

`runsvc::job_message` names `GenerateIdTokenUrl` only when the job's declared permissions grant `id-token: write` (`oidc::id_token_granted`). It reads them from the fold's `JobTokens` row, which only `CreateRunWith` writes. On graft-m4a no run gets that row:
- **The dispatch route.** `workers/edge/src/actions.rs` `dispatch`, which this row and BF-59 use, sends `create_run` without `permissions`. It calls `job_specs`, not `job_specs_declared`. So a dispatched run is always `CreateRun`.
- **The trigger.** `ActionsTrigger` starts runs only for `merge_group` and `pull_request` (the install has no push trigger), and `create_run_body` adds `permissions` only where a site day set `GRAFTYARD_JOB_TOKENS=1` (`gh-token`, format 0x15.5). graft-m4a's sites set only `GRAFTYARD_ACTIONS_CACHE`, `GRAFTYARD_HOST` and the earlier flags, and its edge's `FORMATS_ENABLED` has 21.6 but not 21.5 (the edge's settings, read 08:02Z). Slice B has stayed off on graft-m4a since site day 54 (`tests/am1/results/install-m4a.md`).

`runsvc_host` builds `CreateRunWith` from `job_specs_declared` itself, so the local oracle never ran either path [measured: `tests/m4/wp3/oidc/run.sh`, `HOSTED=1`, passed 10 of 10]. This is `decisions/OPEN-QUESTIONS.md`, F127.

What r1 does show on the hosted install: the issuer and both kids are published, `uses:` resolves v6.2.0 from the install, and a job without the grant has no URL and gets 403 from the shard leader's `may_mint`.

## Teardown, 08:05Z

MinIO removed. `m3org/bf70-cac` deleted (204; an anonymous read answered 404 at 08:05:58Z) and `m3org/bf70` deleted (204, after one 503). Runner key directories were deleted from the raw files. No token, ops deploy or `Rotate` was made, so none needed undoing.

## To run BF-70

Both ways need slice B on: a site day with `GRAFTYARD_JOB_TOKENS=1`, and the edge's `FORMATS_ENABLED` plus 21.5 and 18.14 (0x12.14), since the edge mints job tokens only once both are enabled (`decisions/DECISIONS.md`, `gh-token`, "Order"). Then r2 from `job before`, in one of these ways:
1. **`pull_request`-triggered (no code change; the lead's ruling for r2).** The driver opens a same-repository pull request on `m3org/bf70` whose head adds `hosted.yml` with `on: pull_request`, and the site's trigger sends the permissions through `create_run_body` (`CreateRunOn`).
2. **Dispatch.** The edge's `dispatch` sends `job_specs_declared`'s permissions to the site, which commits `CreateRunWith` from them: built on branch `dispatch-perms` at $0 (F127), for the next edge day after it merges; it does not block r2.
