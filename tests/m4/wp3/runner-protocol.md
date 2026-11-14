# What an unmodified `actions/runner` 2.338.2 needs from the run service

This file is the measured contract M4-WP3's server is built against. It comes from running the unmodified `actions-runner-linux-x64-2.338.2` binary against `runner_mock.py` in this folder, a stdlib Python server, on 2026-11-13. Citations are to the runner source at tag v2.338.2 (commit `d1eb12d4b115c301201de27d667284e1a80f2ab5`), under `src/`. A claim marked [measured] was seen in a recorded run; [doc] was read in the source and not exercised.

Run it: `python3 runner_mock.py --port 8790 --mode success --out /tmp/out`, then `./run.sh --jitconfig "$(cat /tmp/out/jitconfig.txt)"` in an extracted runner. Modes: `success`, `kill`, `cancel`, `errors`, `renewfail`; flags `--minimal`, `--opaque-token`, `--results-url none`.

## Two credentials

- **Runner OAuth token.** From the JIT config's RSA key, by a client-assertion exchange. It authorizes the broker and `acquirejob`.
- **Job token.** `SystemVssConnection.authorization.parameters.AccessToken` in the job message. It authorizes `renewjob`, `completejob`, the results service, and the live-log socket. `completejob` comes from the worker over this connection, not the listener's [measured].

## JIT config

`run.sh --jitconfig <b64>`, where `<b64>` is base64 of a JSON object mapping `.runner`, `.credentials`, and `.credentials_rsaparams` to base64 of each file's JSON (`Runner.Listener/Runner.cs:234-268`).

- `.runner`: `useV2Flow: true` selects the broker listener (`Runner.cs:393-404`). `serverUrlV2` must end in `/`, because every call is `new Uri(base, "message")` (`Sdk/WebApi/WebApi/BrokerHttpClient.cs:70`). `IsHostedServer` defaults to `true` unless `gitHubUrl`'s host says otherwise, so a JIT config sets `gitHubUrl` to the install and `IsHostedServer: false` (`Runner.Common/ConfigurationStore.cs:63-121`). `ephemeral`, `disableUpdate`, `agentId`, `agentName`, `poolId`, `poolName`, `workFolder` are optional.
- `.credentials`: `{"scheme":"OAuth","data":{"clientId":"<guid>","authorizationUrl":"<url>"}}` (`Runner.Listener/Configuration/OAuthCredential.cs:25-53`).
- `.credentials_rsaparams`: `d`, `dp`, `dq`, `exponent`, `inverseQ`, `modulus`, `p`, `q`, each base64 of big-endian bytes, `modulus` and `d` k bytes long and the halves k/2 (`IRSAKeyManager.cs:81-107`) [measured with that padding].

## OAuth exchange

`POST {authorizationUrl}`, form `grant_type=client_credentials`, `client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer`, `client_assertion=<RS256 JWT>`, no Authorization header. The assertion's claims are `iss = sub = clientId`, `aud = authorizationUrl`, `jti`, `nbf`, and `exp = nbf + 300` (`VssOAuthJwtBearerAssertion.cs:137,147`). Answer `{"access_token": "...", "token_type": "bearer", "expires_in": 3600}`. A 400 with `error: invalid_client` makes the runner treat itself as deleted and exit (`BrokerMessageListener.cs:189-210`) [measured: one exchange per connection, and one more after each session recreate].

## Broker

All with the runner OAuth token.

- `POST {serverUrlV2}session`: the runner reads only `sessionId`, a non-empty GUID. 403 exits; 409 retries every 30 s for 4 minutes (`BrokerMessageListener.cs:115-238`).
- `GET {serverUrlV2}message?sessionId&status&runnerVersion&os&architecture&disableUpdate`: a long poll. A message is `200 {"messageId": <i64>, "messageType": "RunnerJobRequest" | "JobCancellation", "body": "<JSON as a string>"}`; an empty 200 means none, and the runner polls again at once. The HTTP timeout is 100 s (`Runner.Sdk/Util/VssUtil.cs:136-170`), so hold under that [measured: 20 s holds loop cleanly].
- A `RunnerJobRequest` body: `{"id", "runner_request_id", "should_acknowledge", "run_service_url" (ends in /), "billing_owner_id"}` (`RunnerJobRequestRef.cs`).
- `POST {serverUrlV2}acknowledge?...` with `{"runnerRequestId"}`, one attempt, errors ignored.
- `DELETE {serverUrlV2}session?sessionId`, errors ignored for an ephemeral runner, which then deletes its config files.
- **Errors on `GET message`** are matched only with `"source": "actions-broker-listener"` in the body (`BrokerHttpClient.cs:285-304`).
  - 400 `errorKind: RunnerSessionInvalid`: the runner opens a new session and keeps polling [measured].
  - 503, 502, 504, 429: the session is kept. The HTTP layer retries 3 times from a 10 s floor, and **a `Retry-After` under 10 s is replaced by a random 10 to 40 s** (`Sdk/Common/Common/VssNetworkHelper.cs:226-262`) [measured: 503 retried after 10.9 s and 12.4 s, `429 Retry-After: 2` after 30.4 s]. Above that, the listener backs off 15 to 30 s for 5 errors, 30 to 60 s for 4 more, and exits code 2 on the 10th, which `run-helper.sh` restarts with a new session (`BrokerMessageListener.cs:359-397`) [doc].

## Run service

- `POST {run_service_url}acquirejob`, runner OAuth, body `{"jobMessageId": "<runner_request_id>", "runnerOS", "billingOwnerId"}`. Errors `{"source": "actions-run-service", "statusCode": 404 | 409 | 422}` are skipped (`Runner.cs:735-757`).
- **The job message.** `messageType` must be `"RunnerJobRequest"` inside this answer. The minimal message that completes [measured, `--minimal`] is `plan.planId`, `timeline.id`, `jobId`, `resources.endpoints[{name: "SystemVssConnection", url (ends in /), authorization: {scheme: "OAuth", parameters: {AccessToken}}}]`, `contextData.github` with `repository`, and `steps`. Optional: `variables` (`system.github.token` becomes `github.token`, masked), `contextData.matrix`, `strategy`, `needs`, `inputs`, `mask`, `jobDisplayName`.
- **The `AccessToken` must parse as a three-part JWT**; the runner reads `orch_id` and never checks the signature. An opaque token makes the listener exit 2, and the acquired job is lost [measured, `--opaque-token`] (`JobDispatcher.cs:105-121`).
- Context data is `{"t": 2, "d": [{"k", "v"}]}` for a dictionary, `{"t": 1, "a": [...]}` for an array, and bare scalars (`PipelineContextDataJsonConverter.cs:35-56`). Template tokens are `{"type": 0..7, ...}` with `lit`, `seq`, `map: [{"Key", "Value"}]`, `expr`, `directive`, `bool`, `num`, or bare scalars (`TemplateTokenJsonConverter.cs:31-53`). A script step is `{"type": "action", "reference": {"type": "script"}, "id", "name", "contextName", "condition", "inputs": {"type": 2, "map": [{"Key": "script", "Value": "..."}]}}`.
- `POST {SystemVssConnection.url}renewjob`, job token, `{"planId", "jobId"}`, answer `{"lockedUntil"}`. The first renew must succeed before the worker starts, then **one every 60 s** [measured: +0.07 s, +60.0 s, +60 s] (`JobDispatcher.cs:776`).
- **A renew outage of about 40 s kills the job**: after the HTTP and run-service retries, a null dereference in the runner's own grace path abandons it, and the worker posts `completejob` `canceled` [measured, `renewfail`] (`JobDispatcher.cs:750,810`).
- `POST {SystemVssConnection.url}completejob`, job token, `{"planId", "jobId", "conclusion", "outputs", "stepResults": [...], "annotations", "billingOwnerId"}`. `conclusion` is `succeeded`, `failed`, `canceled`, `skipped`, `abandoned`, or `succeededWithIssues` [measured: `succeeded`, `canceled`]. A 401 or 404 stops the worker's retries (`JobRunner.cs:345-351`).
- **A killed runner sends nothing more**: no renew, no `completejob`, no session delete. Only the server's missing-renew timeout ends the job [measured, `kill -9` of the whole tree]. With only the listener killed, the orphaned worker finishes and posts `completejob` about 2 minutes after the last renew [measured].

## JobCancellation

A broker message with body `{"jobId", "timeout": "00:00:30"}`. The runner cancels the matching job, posts `completejob` `canceled`, deletes its session, and exits 0 [measured, `cancel`].

## Results service and logs

- Started only when `variables["system.github.results_endpoint"]` is set. Without it the job still succeeds, and its step output goes nowhere [measured, `--results-url none`]. All uploads are best effort.
- Twirp: `POST {results_endpoint}twirp/results.services.receiver.Receiver/<Method>`, job token, snake_case JSON. `GetStepLogsSignedBlobURL` and `GetJobLogsSignedBlobURL` answer `{logs_url, blob_storage_type, soft_size_limit}`; `CreateStepLogsMetadata` and `CreateJobLogsMetadata` must answer `{"ok": true}`, or the worker throws. `twirp/github.actions.results.api.v1.WorkflowStepUpdateService/WorkflowStepsUpdate` carries step status (3 in progress, 6 completed) and conclusion (2 success, 3 failure, 4 cancelled, 7 skipped), and also needs `{"ok": true}` [measured].
- A blob `logs_url` is PUT with no Authorization header and `Content-Type: text/plain` [measured: one PUT per log]. A multi-block log appends `&comp=appendblock` and, on the last block, `&seal=true`, so the signed URL must already hold a `?` [doc]. Azure headers are sent only when `blob_storage_type` is `BLOB_STORAGE_TYPE_AZURE`.
- **Live log** [measured, `tests/m4/wp3/run-host.sh`]: `SystemVssConnection.data["FeedStreamUrl"]` names a WebSocket, which the runner opens with `Authorization: Bearer <job token>` (30 s connect, 3 retries). Each batch is one text message of `{"count", "value": [lines], "stepId", "startLine"}`, keys in camelCase, in 1,024-byte frames (`Runner.Common/ResultsServer.cs:56-296`). Delivery is best effort.

## What a server must get exact

- A trailing `/` on `serverUrlV2`, `run_service_url`, `SystemVssConnection.url`, and `results_endpoint`.
- `source` in every error body, or the runner treats the error as generic.
- A JWT-shaped job token whose `orch_id` holds no spaces: the runner puts it in its `User-Agent`, and a space throws `FormatException` before the first renew [measured].
- `{"ok": true}` from the metadata and step-update calls.
- `Retry-After` of at least 10 s to slow a runner at all; DESIGN's `Retry-After: 15` and SHARED's re-poll in 15 to 30 s are consistent with this.
- The lease: the runner renews every 60 s and stops on a kill, so a 10-minute lease without a renew ends the job (BF-4) with no help from the runner.
