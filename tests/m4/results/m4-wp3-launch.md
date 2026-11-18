# M4-WP3 slice A, the launch service: work in progress

Status (2026-11-18): **paused before any code change**, at the lead's request so the history rewrite can happen. Slice A is approved (lead thread, 2026-11-18): the launch service that resolves `uses:`, code-only, design text first. Nothing below changes a reply or the design yet.

## Step 1 done: main's failure, reproduced

The unmodified `actions/runner` 2.338.2 (release tarball `actions-runner-linux-x64-2.338.2`) ran `tests/m4/wp3/run/uses.yml` under `crates/actions/examples/runsvc_host.rs` on main `e9985573`:

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

## What the code must match (read, not built)

- Route: `POST {launch_endpoint}/actions/build/{planId}/jobs/{jobId}/runnerresolve/actions`, `Authorization: Bearer <the job token>` (`Sdk/WebApi/WebApi/LaunchHttpClient.cs:35,42,81-93`).
- Request: `{"actions": [{"action": "owner/repo", "version": "ref", "path": "sub/dir"}], "actions_dependencies": [...]}` (`Sdk/WebApi/WebApi/LaunchContracts.cs:8-27`).
- Reply 200: `{"actions": {"<name>@<ref>": {"name", "resolved_name", "resolved_sha", "tar_url", "zip_url", "version", "authentication": {"token", "expires_at"}?, "package_details"?}}}` (`LaunchContracts.cs:31-85`). The runner looks each action up by `"{Name}@{Ref}"` as written in the workflow, ordinal (`ActionManager.cs:1589-1617`, `1140`), so the key must echo the request's case.
- Reply 422: `{"errors": {"<key>": {"message"}}}`, joined with ". " into the job's error (`LaunchHttpClient.cs:52-66`, `LaunchContracts.cs:89-106`).
- Download: the runner GETs `tar_url` with `Basic x-access-token:<token>` (Bearer only for a `codeload.` or `/_codeload/` URL under the `actions_use_bearer_token_for_codeload` feature), and takes the archive's single top directory (`ActionManager.cs:1619-1642`, `1366-1374`). With no `authentication` it uses the job's `github.token` (`ActionManager.cs:1117-1128`), which is empty on main (`workers/edge/src/actions.rs`, slice B).

## Next, when the rewrite is done

Design text (SHARED Runner protocol, a DECISIONS entry, one OPEN-QUESTIONS row for the private-action archive), then `launch_endpoint` in `runsvc.rs`, the edge route, the `runsvc_host` route over a local mirror repository, the kept test that fails on main, plants after `tests/m4/wp3/plant.py`'s last number (47, so 48 on), and the full checks.
