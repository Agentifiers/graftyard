# M4-WP3 slice A, the launch service

Status (2026-11-18): **built** on branch `launch-svc` from main `dcef0f59`, code-only, with no hosted run. The design is `design/SHARED.md`, "Actions", Runner protocol (the `launch_endpoint` bullet), and `decisions/DECISIONS.md`, "M4-WP3 slice A: the launch service". A private action repository is `decisions/OPEN-QUESTIONS.md`, F90. It ships with the next site day (the job message) and the next edge day (the route).

## Main's failure, reproduced

The unmodified `actions/runner` 2.338.2 (release tarball `actions-runner-linux-x64-2.338.2`) ran `tests/m4/wp3/run/uses.yml` under `crates/actions/examples/runsvc_host.rs` on main `e9985573`, and again on main `c2b7eac4` (2026-11-18 10:19Z, `tests/m4/raw/wp3/launch/main/`), the same three attempts:

```
tests/m4/wp3/run-host.sh <runner dir> tests/m4/wp3/run/uses.yml /tmp/launch-main 8791
```

The job ended `Completed { conclusion: Failure }`. Its log (`logs-job`, verbatim):

```
Prepare all required actions
Getting action download info
Failed to resolve action download info. Error: Launch client is not initialized.
Retrying in 24.758 seconds
Failed to resolve action download info. Error: Launch client is not initialized.
Retrying in 17.246 seconds
##[error]Launch client is not initialized.
##[error]Failed to resolve action download info.
```

That is three attempts: two retries with a 10 to 30 s random backoff, then the failure. The step before the action (`run: echo "before the action"`) never ran, because the runner resolves actions before it runs any step [measured].

Why, from the runner source at 2.338.2 (`actions/runner` `d1eb12d`):
- `Runner.Worker/JobRunner.cs:72-82`: the launch client is created only when the job message has the variable `system.github.launch_endpoint` and `SystemVssConnection` has an `AccessToken`. `runsvc::acquire_message` sets no `launch_endpoint`.
- `Runner.Common/LaunchServer.cs:44-58`: with no client, `ResolveActionsDownloadInfoAsync` throws `InvalidOperationException("Launch client is not initialized.")`.
- `Runner.Worker/ActionManager.cs:1058-1110`: three attempts. A 422 (`UnresolvableActionDownloadInfoException`) or a 429 (`NonRetryableActionDownloadInfoException`) is not retried.

## The fix, run in memory

`tests/m4/wp3/run-host.sh` with the same unmodified runner, on `launch-svc`. `runsvc_host` now serves the launch route and `/_codeload/` over two local repositories, `Mirror/Hello` (public) and `Mirror/Private` (private), both with tag `v1` and branch `main` at commit `24829c76e1163366bf63cb7415a07b99f433a4ff`. Logs are in `tests/m4/raw/wp3/launch/`, with job tokens replaced by `<token>`.

**`tests/m4/wp3/run/uses.yml`: Success.** One resolve call, 200, then three archive downloads, and every step ran:

```
Download action repository 'Mirror/Hello@v1' (SHA:24829c76e1163366bf63cb7415a07b99f433a4ff)
Download action repository 'mirror/hello@main' (SHA:24829c76e1163366bf63cb7415a07b99f433a4ff)
Download action repository 'Mirror/Hello@24829c76e1163366bf63cb7415a07b99f433a4ff' (SHA:24829c76e1163366bf63cb7415a07b99f433a4ff)
before the action
hello tag from Mirror/Hello
hello branch from Mirror/Hello
hello sha from Mirror/Hello
after the action
```

`mirror/hello@main` is the same repository in another case, resolved case-insensitively; the reply keys it as the workflow wrote it, which the runner's ordinal lookup needs.

**`tests/m4/wp3/run/uses-refused.yml`: five jobs, each Failure at once.** Each made one resolve call, got a 422, and logged no "Retrying" line:

```
Unable to resolve action 'Mirror/Hello@24829c7', the provided ref '24829c7' is the shortened version of a commit SHA, which is not supported. Please use the full commit SHA instead
Unable to resolve action 'Mirror/Private@v1', repository not found
Unable to resolve action 'actions/checkout@v4', repository not found
Unable to resolve action 'Mirror/Hello@0000000000000000000000000000000000000001', unable to find version '0000000000000000000000000000000000000001'
Unable to resolve action 'Mirror/Hello@v9', unable to find version 'v9'
```

`actions/checkout@v4` is not on the in-memory install, and nothing is looked up outside it. The private repository is refused as not found, as for an anonymous reader (F90).

**Without `actions_display_helpful_actions_download_errors`** (the variable removed for one run), the abbreviated-SHA job failed with `##[error]Value cannot be null. (Parameter 'Actions')`: the runner parsed the 422 body as a reply. So the job message sets it [measured].

## The runner source the code matches

Runner 2.338.2 (`actions/runner` `d1eb12d`), each cited in the code:
- `Runner.Worker/JobRunner.cs:72-82`: the launch client exists only with `system.github.launch_endpoint` and an `AccessToken`.
- `Sdk/WebApi/WebApi/LaunchHttpClient.cs:42`: `new Uri(m_launchServiceUrl, "/actions/build/{planId}/jobs/{jobId}/runnerresolve/actions")`, an absolute path, so the endpoint is the install's root. `:81-93`: `POST`, `Authorization: Bearer <job token>`, `Accept: application/json`.
- `LaunchHttpClient.cs:45-66`: 2xx is read as `ActionDownloadInfoResponseCollection`; 422 as `ActionDownloadResolutionErrorCollection`, its messages joined with ". " into `UnresolvableActionDownloadInfoException`; 429 as non-retryable; anything else throws and is retried.
- `Sdk/WebApi/WebApi/LaunchContracts.cs:8-27` (request), `:31-85` (reply: `name`, `resolved_name`, `resolved_sha`, `tar_url`, `zip_url`, `version`, `authentication`, `package_details`), `:89-106` (errors).
- `Runner.Common/LaunchServer.cs:44-58` and `Runner.Common/Constants.cs:172`: the V2 client, which reads the 422, is used only under `actions_display_helpful_actions_download_errors`; `Runner.Worker/ActionManager.cs:1067`.
- `ActionManager.cs:1058-1110`: three attempts; a 422 or 429 is not retried. `:1117-1128`: with no `authentication`, the job's `github.token` is used. `:1589-1617`: the lookup key is `"{Name}@{Ref}"`, ordinal. `:1619-1642`: Basic `x-access-token:<token>` to `tar_url`, Bearer only for a codeload URL under `actions_use_bearer_token_for_codeload`. `:1366-1374`: the archive must hold exactly one top directory.
- `Sdk/Common/Common/VssHttpRetryOptions.cs:28-34`: the HTTP layer itself retries 502, 503, 504 and 429, so a launch 503 is retried there before `ActionManager`'s three attempts.

## Checks

- Kept test that fails on main: `runsvc::tests::a_job_message_names_the_launch_endpoint_and_asks_for_its_errors`. Main's job message has neither variable.
- `launch::tests`: resolution order (a tag over a branch of the same name, a hex tag still a tag, an abbreviated SHA refused, a full SHA in either case), the reply's keys and both shapes, a failed read as 503, and a job token taken only for its own job.
- Plants 48 to 57 (`tests/m4/wp3/plant.py`), all caught; see the branch report.
