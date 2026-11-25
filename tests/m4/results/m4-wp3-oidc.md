# BF-70 on graft-m4a: Actions OIDC across an `ActionsOidcKey` rotation

Status (2026-11-25 10:01Z): **r2 passes every line.** r1 (08:06Z) stopped after the first job (F127; below). r2 ran on the lead's ruling, from same-repository pull requests, after a site day with `GRAFTYARD_JOB_TOKENS=1` and an edge day adding 21.5 and 18.14. Its raw logs are in `tests/m4/raw/wp3/oidc-hosted/r2/`. r2 spent $0.69 of its $5 stop (graft-m4a's UTC day went from $2.19 to $2.88). `ActionsOidcKey` is now on slot B, with both slots bound and published, so the next rotation goes back to A.

## r2: verdict

| Line (the lead's) | Result |
| --- | --- |
| The granted job assumes the role and reads S3 through unmodified configure-aws-credentials v6.2.0 | **Met**, twice. `job before` (09:37Z, run 1507366, PR #6) and `job after` (09:59Z, run 1507371, PR #7) each: the runner exported both variables, the action came from the install (`m3org/bf70-r2-cac@v6.2.0`, tree `ed22c69c`), `AssumeRoleWithWebIdentity` against MinIO's STS succeeded, and `s3 ls s3://probe/` read the bucket with the credentials |
| A second token verifies RS256 against the published JWKS | **Met**, twice. The job fetched discovery and the JWKS from the issuer named in `iss`, checked RS256 under the token's kid, `aud` `sts.amazonaws.com` and `exp`, and MinIO took the same token (`second token ok`). The claims: `sub` `repo:m3org/bf70-r2:pull_request`, `event_name` `pull_request`, `ref` `refs/pull/N/merge`, `repository_owner` `m3org`, `runner_environment` `self-hosted` |
| The ungranted job and the fork-PR job each see no URL and get 403 at run/idtoken | **Met.** `denied` (both runs): no URL, and its own token got 403 at `run/idtoken`. The fork job (09:40Z, a dispatch marked fork on `m3org/bf70-r2-fork`, run 1194862): no URL, and 403 naming the fork |
| After the rotation, the kid is B's and MinIO takes the token without refetching | **Met.** `job before`'s kid was A's (`xGTXBDFS…`); `job after`'s was B's (`g-7DvKNw…`). MinIO started at 09:10:58Z, read the issuer's documents then, and was not restarted; it took slot B's tokens at 09:59Z |
| No new site or edge version from the rotation | **Met.** `versions` (10:00Z) against `state` (09:10Z): sites `91113a0f` / `2a82f589` / `8469d9ef` and edge `330560d3`, the same deployments and version counts (100, 100, 100, 47). Only `graft-m4a-ops` moved, 51 to 52, from the row's own `ops` deploy, and teardown parked it back on `ac4cd9a5` |

**Known gap, not a pass (F121).** Both tokens carry `base_ref` and `head_ref` empty. GitHub sets both on `pull_request`, so on this row the gap shows: a relying party that binds `base_ref` or `head_ref` refuses these tokens. `actor` is absent too (F120).

## r2: what ran

| Step (Cloudflare time) | Result |
| --- | --- |
| Site day, 09:03:44Z to 09:04:02Z | From main `d1cf8e365`, test-faults: a `91113a0f`, b `2a82f589`, c `8469d9ef`, with `GRAFTYARD_JOB_TOKENS=1` added to LM-1's flags (`ACTIONS_CACHE`, `PR_TRIGGER`, `APP_HOOKS`, `ACTIVITY`, `AUDIT`, `HOST`) and `--oidc` kept. Crons `[]`; reretire OK |
| Edge day, 09:07:52Z | `330560d3` at 100%. Only `FORMATS_ENABLED` changed (adds 21.5 and 18.14); edgever exit 0. It does not carry a57-errors-read, which reached main after it |
| Formats | The `Release` group needed no `FormatsEnable`: it enables only 65000.1 (relview `enabled` 1, count 543), as F124 records, so 0x15.5 and 0x12.14 are on by the sites' flag and the edge's var |
| `state`, 09:10:44Z | Discovery and both kids served; `ActionsOidcKey` on slot 0 (A) |
| `action`, 09:18:58Z | **Renamed for r2:** `m3org/bf70-cac` answered 422 "name already exists". r1 deleted it, and a deleted name stays taken until its `retain_until`, 90 days (`design/SHARED.md`, "Repository create and delete"). Each tag now takes its own names (`BF70_REPO=bf70-<tag>`), and r2 mirrored v6.2.0 as `m3org/bf70-r2-cac` |
| `precheck`, 09:25:34Z | **`ACTIONS_ID_TOKEN_REQUEST_URL set: yes`** on a same-repository pull request. The first try (`precheck-1/`) started no run: the site's trigger refused `precheck.yml` with "mapping values are not allowed … line 13, column 52", a plain YAML scalar holding `: `; the step is now a block scalar |
| `job before`, three tries before the pass | Try 1 (`job-before-1/`): the verify step's `urllib` got 403 "error code: 1010" from workers.dev for its default user agent (F130); the step now names one. The check also read `second token ok` from the runner's echo of the step's script, so it now reads output lines only. Try 2 (`job-before-2/`): the new comment held an apostrophe inside the step's single-quoted Python, a bash syntax error. Try 3 (`job-before-3/`): MinIO refused every token "Token is not valid yet": the orb's clock was 31 s behind Cloudflare's, and the token's `nbf` equals its `iat` (F129). The orb's clock was set from the edge's `Date`, and try 4 passed at 09:38:27Z |
| `fork`, 09:40:19Z | Pass |
| `ops`, 09:42:16Z | `graft-m4a-ops` `7b541d43` from `d1cf8e365`, test-faults, `oidc_published_ms` 300,000, gate pending, 1-minute cron |
| `fired`, `cron` | No run within 3 minutes of the deploy, nor within 3 minutes of the hand `cron` step's re-PUT (09:46:34Z). The first run came at 09:52:38Z (`fired-3.log`), 6:04 after the re-PUT: R52's second instance on this row, recorded there |
| `rotate`, 09:53:18Z | `Rotate { ActionsOidcKey, slot: 1 }` posted (Release index 543). Probe at 09:53:39.952Z: "every site publishes kid g-7DvKNw…". Step 1 at 09:58:42Z, Switch (step 2) at 09:58:43.977Z, `IntentDone` at 09:58:45.840Z, 304 s after the probe |
| `job after`, 10:00:19Z | Pass, kid B |
| `versions`, 10:00Z | As in the verdict; the row so far $0.65 |

Each failed try is a fault of the row's own scripts or the orb, not of the install: none touched the OIDC code, and none of them is F127's cause. The precheck's URL line and both passing jobs show F127's cause is gone where job tokens are on.

## r2: teardown, 10:01Z

MinIO removed. `m3org/bf70-r2-cac`, `m3org/bf70-r2` and `m3org/bf70-r2-fork` deleted (204 each; each read 404 by 10:02Z). `graft-m4a-ops` parked on `ac4cd9a5` at 10:01:29Z with schedules `[]`, the row token `graft-m4a-bf70-deploy` revoked, and the row's files shredded. The end sweep found no shard container. `ActionsOidcKey` is left on slot B (switched 09:58:44Z), with both slots still bound and published; the next rotation goes back to A. Two repositories this thread created by hand while finding why `action` got 422, `m3org/bf70-fork` and `m3org/zz-probe-never`, were deleted at once (204, then 404).

The local oracle passed 10 of 10 on the changed `hosted.yml` (`HOSTED=1 tests/m4/wp3/oidc/run.sh`, 10:10Z).

## r1

Stopped after the first job, not passed (2026-11-25 08:06Z), raw logs in `tests/m4/raw/wp3/oidc-hosted/r1/`. It spent $0.13 (the day $1.95 to $2.08), deployed nothing and signed no `Rotate`.

### What ran

| Step (Cloudflare time) | Result |
| --- | --- |
| `state`, 07:53:35Z | The issuer `https://graft-m4a-edge.<sub>.workers.dev/_services/token` serves its discovery document, and the JWKS lists both kids: A `xGTXBDFSTAgZPaEknItTcA4H69VLmaXS4XMnKlWWDDA` and B `g-7DvKNwFRNdKn5xF8YpU1tlE7H2n65FfIVhvJS8mLk`. Sites were on `eb5c77f2`, `c6b12562` and `3a1ea21b` (site day 3 from main `0aa3b507`), the edge on `d3d76118`, ops on `ac4cd9a5`. The view's `ActionsOidcKey` slot was 0 |
| `minio`, 07:54:04Z | A MinIO STS on the orb read the edge's openid-configuration at start, live 200. Policy `m3org` covers bucket `probe` |
| `action`, 07:56:07Z | `m3org/bf70-cac` (public) holds configure-aws-credentials v6.2.0's tree `ed22c69c` as commit `2a183bb2`. The first push answered 400 "shallow pushes are not supported", because it came from the shallow clone; the driver now pushes from a fresh bare repository |
| `job before`, 07:58Z to 08:00Z | **Granted job `sts`: Failure.** Its first step, `test -n "$ACTIONS_ID_TOKEN_REQUEST_URL"`, exited 1: the runner exported no token URL. The job without the grant, `denied`, succeeded: no URL, and its own token got **403** at `run/idtoken`. The action was resolved from the install (`Download action repository 'm3org/bf70-cac@v6.2.0' (SHA:2a183bb2…)`). No job log holds a token. No 503 retry was needed |

The first `job before` attempt (`job-before-norunner/`) started no runner, because the driver passed a relative out directory to `hosted-run.mjs`, which starts the runner in the runner's own directory. It is fixed (`$PWD/$O`), and its jobs ran on the second attempt.

### Why the granted job got no URL

`runsvc::job_message` names `GenerateIdTokenUrl` only when the job's declared permissions grant `id-token: write` (`oidc::id_token_granted`). It reads them from the fold's `JobTokens` row, which only `CreateRunWith` writes. On graft-m4a no run gets that row:
- **The dispatch route.** `workers/edge/src/actions.rs` `dispatch`, which this row and BF-59 use, sends `create_run` without `permissions`. It calls `job_specs`, not `job_specs_declared`. So a dispatched run is always `CreateRun`.
- **The trigger.** `ActionsTrigger` starts runs only for `merge_group` and `pull_request` (the install has no push trigger), and `create_run_body` adds `permissions` only where a site day set `GRAFTYARD_JOB_TOKENS=1` (`gh-token`, format 0x15.5). graft-m4a's sites set only `GRAFTYARD_ACTIONS_CACHE`, `GRAFTYARD_HOST` and the earlier flags, and its edge's `FORMATS_ENABLED` has 21.6 but not 21.5 (the edge's settings, read 08:02Z). Slice B has stayed off on graft-m4a since site day 54 (`tests/am1/results/install-m4a.md`).

`runsvc_host` builds `CreateRunWith` from `job_specs_declared` itself, so the local oracle never ran either path [measured: `tests/m4/wp3/oidc/run.sh`, `HOSTED=1`, passed 10 of 10]. This is `decisions/OPEN-QUESTIONS.md`, F127.

What r1 does show on the hosted install: the issuer and both kids are published, `uses:` resolves v6.2.0 from the install, and a job without the grant has no URL and gets 403 from the shard leader's `may_mint`.

### Teardown, 08:05Z

MinIO removed. `m3org/bf70-cac` deleted (204; an anonymous read answered 404 at 08:05:58Z) and `m3org/bf70` deleted (204, after one 503). Runner key directories were deleted from the raw files. No token, ops deploy or `Rotate` was made, so none needed undoing.

### To run BF-70

Both ways need slice B on: a site day with `GRAFTYARD_JOB_TOKENS=1`, and the edge's `FORMATS_ENABLED` plus 21.5 and 18.14 (0x12.14), since the edge mints job tokens only once both are enabled (`decisions/DECISIONS.md`, `gh-token`, "Order"). Then r2 from `job before`, in one of these ways:
1. **`pull_request`-triggered (no code change; the lead's ruling for r2).** The driver opens a same-repository pull request on `m3org/bf70` whose head adds `hosted.yml` with `on: pull_request`, and the site's trigger sends the permissions through `create_run_body` (`CreateRunOn`).
2. **Dispatch.** The edge's `dispatch` sends `job_specs_declared`'s permissions to the site, which commits `CreateRunWith` from them: built on branch `dispatch-perms` at $0 (F127), for the next edge day after it merges; it does not block r2.
