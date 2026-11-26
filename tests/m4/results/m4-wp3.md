# M4-WP3, the Actions engine: results

Branch `m4-actions`, from main 2b42214e. The shapes are in `tests/m4/results/m4-wp3-design.md`, the runner contract in `tests/m4/wp3/runner-protocol.md`, and the decisions in `decisions/DECISIONS.md`, "M4-WP3: the Actions shard's commands, and `RunOwner`'s writes, 2026-11-14".

## Status

| Row | In memory | Hosted (graft-m4a) |
| --- | --- | --- |
| BF-4 | Pass, also with the real runner killed | Pass (run 1179649): failed at its lease end, 10 min after the renew answered, 10 min 4 s after it was sent |
| BF-37 | `ActionsTrigger` for `merge_group` (`crates/actions/src/trigger.rs`); BF-21 leaves no queue ref | **Pass** (03:21Z to 03:25Z, site day 35): a run on each of the 4 queue refs built, every ref deleted 10 s after its landing or ejection, all 18 deliveries parsed by go-playground/webhooks v6.4.0 |
| BF-54 | Not built | Waits for the step-3 estimate's approval (lead, 2026-11-14) |
| BF-55 | `Owner` scope on shard 0, `MatchOwner`, resend, `moved`, `Taken` (`crates/actions/tests/bf55.rs`) | **Pass** (run 2, 11:30Z to 12:17Z, site days 39 to 41): first poll on the new leader's site 11.5 s after the old leader died; 256 of 256 jobs ran once, each matched and acquired once; longest dispatch gap 36.4 s |
| BF-56 | Pass (both kills) | **Pass**, both kills (23:18Z to 23:24Z, site day 34): every required check concluded, matching its job, at most 10.4 s after the job's `Completed` |
| BF-59 | `graft-ops` open, reseal, and rewrap (PyNaCl oracle); key rules (`crates/identity/tests/sealed.rs`) | **Pass** (19:07Z to 19:31Z): repository, environment, and org secrets read right before and after both rotations; a fork-PR job sees none; no plaintext in 1,296 committed entries, 34 run files, runner diagnostics, or Workers Logs |
| BF-69 | `actions-cache` rules (`crates/actions/src/cache.rs`, plant231) | **Pass** (2026-11-25): every line of the row 04:27Z to 05:55Z, and the parallel-save line in its own row 07:07Z to 07:11Z (one dispatch, four jobs, both saves of each key reserving within 1 s, one committing per key), no job failed |
| BF-71 | `actions-artifacts`: unit tests in `crates/actions/src/artifacts.rs`, plant246, the gzip edgeboot, and the local runner run below | **Pass** (2026-11-25 12:52:48Z to 12:55:33Z, edge day `3c453149`): every line; 0 elect and 0 tierOff sheds; F125's drop seen hosted. Below |

A row passes only hosted, with an unmodified `actions/runner`. The in-memory halves below check the engine's rules on real members first.

## What is built

- `crates/actions`: `JobState`, `transition`, `derive_run_status`, `LabelSet`, `actions_shard`, the shard's fold (`ActionsFold`, an overlay scratch) and commands (`ActionsCmd`, format 0x15.1), and `RunOwner`'s pure round (`run_owner`).
- `crates/git/src/run_checks.rs`: `RepoCmd::RunChecks`, format 32.21, `RunOwner`'s check suite, check runs, and required checks on the repository chain.
- `crates/actions/src/workflow`: the workflow parser, a Rust port of the runner's `WorkflowParser` with its expression grammar, `MatrixBuilder`, and the `on:` filters. See "The parser" below.
- Wire tags: the Actions enums and `RunChecksOut`, appended to `crates/git/tests/wire_tags.golden`. Main's last `RepoEvent` tag is still 42. No `RepoEvent` or `IssueOp` variant was added.

## In memory: `crates/actions/tests/gates.rs`

Three real `Member<ActionsCmd>`s of one owner's shard and three real `Member<RepoCmd>`s of the repository. Simulated runners commit what the run service will commit. The leader's host commits `ExpireLeases` at the fold's next deadline, and `RunOwner` runs `run_owner::round` against both groups. Time moves in 500 ms steps.

- **BF-4.** One job on one runner renews every 60 s for 7 minutes, which commits 1 or 2 lease rewrites (`LEASE_REWRITE_UNDER`). Then the runner is killed and the group left idle. The job completes `Failure` at its lease end, within 10 minutes of the last renew. After the kill the only entries are the lease alarm's and `RunOwner`'s.
- **BF-56, commit path.** Two runs of 6 jobs on two open PR heads, every job required by the branch rule, with `needs` chains: build, then test, then deploy, and lint, then report, then notify. Lint fails, so report and notify are skipped. The second run is cancelled mid-run. The repository's leader site is killed before `RunOwner` writes anything and recovers by election. Every required check has its conclusion within 60 s of its job's `Completed`, and every check-run version committed at most once, in order.
- **BF-56, `RunOwner`.** The same, with `RunOwner` killed for 20 s after a round's repository writes and before its shard half. The cursor stays at 0, the replayed round's repository writes commit nothing new, and the row's pass line holds.
- **One runner at a time.** Two runners matched to one job in one entry: the second is refused `Stale`. A runner whose labels lack the job's is refused, and a second run for one trigger is refused `Duplicate`.
- **The abort path.** A repository deleted mid-run answers `Gone`, and `RunOwner` cancels the run's unfinished jobs.

`cargo test -p graftyard-actions`: 24 unit tests, 6 gate tests, and the parser's differential test pass.

### Plants: `tests/m4/wp3/plant.py`

| # | Mutant | Verdict |
| --- | --- | --- |
| 0 | a lease never expires a running job | caught |
| 1 | an expired lease from `InProgress` requeues the job | caught |
| 2 | every renew rewrites the lease | caught |
| 3 | a renew sets the lease past `JOB_LEASE` | caught |
| 4 | a completed job takes a cancel | caught |
| 5 | a match commits against a stale version | caught |
| 6 | a match ignores labels | caught |
| 7 | a second run for one trigger | caught |
| 8 | a stall moves a lease by more than its length | caught |
| 9 | a stall moves no lease | caught |
| 10 | a stall revives a lease that had ended before it | caught |
| 11 | a failed need still releases its dependents | caught |
| 12 | a skip does not release its own dependents | caught |
| 13 | the `RunOwner` cursor moves back | caught |
| 14 | `RunOwner` never cancels a gone repository's run | caught |
| 15 | a replayed check run moves its row back | caught |
| 16 | a completed check writes no conclusion | caught |
| 17 | every job's check row is version 1 | caught |

The first run missed four plants. Each miss led to a fix: the stall test in `shard.rs`, a `notify` job behind `report` with lint finishing last, the cursor test, and the abort-path test. One plant was dropped: "a completed run takes another status" cannot happen, because a completed run's jobs take no event, so the guard was removed instead.

## The run service with an unmodified runner: `crates/actions/examples/runsvc_host.rs`

The run service's pure half is `crates/actions/src/runsvc.rs` (JIT config, the OAuth assertion, job tokens, the job message, steps, cancellation) and `crates/actions/src/relay.rs` (LogRelay's tail and its viewer messages). The example host runs them in memory on three real `Member<ActionsCmd>`s behind the run service's HTTP and WebSocket routes, and `tests/m4/wp3/run-host.sh` drives `actions/runner` 2.338.2, unmodified, against it.

- **A matrix completes** (`tests/m4/wp3/run/ci.yml`, 2 jobs, ephemeral runners): both jobs `Completed Success`, step logs PUT to the blob route, and 15 live-log frames arrived over `FeedStreamUrl` [measured].
- **BF-4 with the real runner** (`long.yml`, `sleep 1200`): the runner tree was killed with `kill -9` at 09:28:15Z. Its last `renewjob` had answered `lockedUntil` 09:37:56Z, and the leader's `ExpireLeases` alarm completed the job `Failure` at 09:37:56.766Z, 10 minutes after that renew, inside the row's 10 [measured]. After the kill the only commit was the alarm's.
- **Routing.** A runner's `clientId` holds its owner, registration shard, and id, and each token names its Actions shard, so the edge routes every run-service call with no lookup. Job GUIDs carry the full 53-bit id. The matrix run passed again after the change: 2 jobs `Success`, 154 live-log frames, 12 blobs [measured].
- **Found:** a job token's `orch_id` must hold no spaces, because the runner sends it in its `User-Agent` (`tests/m4/wp3/runner-protocol.md`).

## Hosted on graft-m4a: the run service and LogRelay

Site day 28 (14:15Z) put the `Actions` family, `/runsvc`, `RunnerBroker`, `LogRelay`, and `RUN_SERVICE_KEY` (Secrets Store) on all three sites. Edge days 10 and 11 by M4 web put `workers/edge/src/actions.rs` on the edge, with `RUN_SERVICE_PUBLIC`. Site day 29 (14:45Z) fixed the one hosted failure. `tests/m4/wp3/hosted-run.mjs` drives an unmodified `actions/runner` 2.338.2 through product routes only: it registers an ephemeral runner (`POST /api/graftyard/v1/repos/{o}/{r}/actions/runners`), writes its JIT config with `examples/jitconfig.rs`, and dispatches a workflow read from the git shard's `/ui/blob`.

- **Found on day 28:** a new Actions shard's first command answered 422 `NeedIdBlock`. `/runsvc` waited on its ticket without handing over the command, so the host's lease loop had nothing to re-send after leasing a block. The command now goes with the ticket, as `/commit`'s does (`f0282e80`).
- **Live log, 14:47Z** (`tests/m4/wp3/hosted/live.yml`, one line a second for 10 minutes): the runner took job 1114114 of run 1114113 on `m3org/wp3-live` (shard `actions:7100001.8`) 4 s after dispatch. A viewer on `/_t/logrelay/repos/m3org/wp3-live/jobs/1114114` got `hello` with the tail, then `line` messages p50 339 ms and at most 375 ms after the runner printed them, over 23 lines [measured]. The relay's `line.at` is to the second.
- **GHES reads** on the same run: `actions/runs/{id}`, `runs/{id}/jobs`, and `jobs/{id}` answer 200, with the runner's step report as `steps` (1 and 2 `completed`/`success`, 3 `in_progress`, 4 `queued`) [measured].
- **BF-4 hosted** (`tests/m4/wp3/hosted/bf4.yml`, one long step): the runner took job 1179650 of run 1179649 on `m3org/wp3-bf4` (shard `actions:7100001.11`). Its last `renewjob` was at 14:52:47Z and answered `lockedUntil` 15:02:50Z. The runner tree was killed at 14:53:22Z. The job read `Completed Failure` at 15:02:50.958Z, the lease end: 10 min 4 s after the renew was sent, 10 min after its answer set the lease. The row's fail line, still running at 11 minutes, did not happen [measured]. The driver process exited at about 15:01:49Z for a reason not found, so the job's state was read directly with the GHES `jobs/{id}` read.
- **Found by BF-4:** the GHES `job.completed_at` showed the last step report (14:52:47Z), because a runner killed mid-step reports no end. It is now the job's own completion time (`crates/actions/src/rest.rs`).
- **A running job's `/logs`** now answers the step logs uploaded so far, in upload order, instead of 404, as GHES serves a log so far (`workers/edge/src/actions.rs`). A step still running has no lines there until it ends.
- **BF-61's relay kill** (M4 web runs the row; this is the relay side). Site day 30 (15:40Z) added `LogRelay` `/abort`, `ctx.abort()` reached only through a binding. Job 1114120 of run 1114119 on `m3org/wp3-live` streamed through `logrelay:1114120` on site 0 with two viewers attached, and a scratch Worker bound to the `LogRelay` classes (`graft-m4a-relaykill`, deleted after) aborted it at 15:47:05.05Z. The runner noticed on its next send, about 27 s later, retried twice at once (two harmless `ResultServer` exceptions as the attempts raced), and reconnected within 1 s to a fresh relay with no tail. Its lease kept renewing and the job completed `Success` at 15:52:15Z. Its `/logs` then held all 600 counter lines with none missing (37,382 B) [measured]. M4 web's spec stalled on a page action after saving the log, so the lead asked for one clean printed rerun with no new site day: job 1114123 of run 1114122, relay on site 0, aborted at 16:20:00.113Z, behaved the same (the runner reconnected about 29 s later on its next send; `Success` at 16:25:37Z; all 600 lines in `/logs`) [measured]. The spec then printed a clean pass (`1 passed`, M4 web's `tests/m4/results/m4-wp5.md` on `m4-web-7` `f12a64e3`): 220 live lines with none over 2 s, the socket closed with no `end`, the page on saved logs 2.2 s later, and the final log holding every line the page saw live.
- **The `line.at` skew is the orb's clock.** The orb's clock runs about 5 s behind Cloudflare's: the kill Worker's `Date.now()` read 5.03 s after the orb's send, and the edge's `Date` 5 s after the orb's. The runner ran on the orb, so its timestamps, not the relay's, were behind [measured on two readings].
- **Not yet checked:** a viewer's close arrives about 10 s after `no-stream` (M4 web, edge day 10), or not within 12 s (edge day 11). The web viewer falls back on `no-stream` itself; BF-61's kill path falls back on the close, so it is checked there.

## BF-59 hosted: secrets across both rotations

Driver `tests/m4/wp3/bf59.mjs`, workflow `tests/m4/wp3/hosted/bf59.yml` (one job with `environment: bf59env`; each step prints a secret only as the first 16 hex of its SHA-256 and its length, and the variable as text). Repository `m3org/wp3-bf59` (65554), unmodified runner 2.338.2. Site day 32 (`17dff43c` / `57066014` / `30444906`), edge day 15 (`1524eb37`), then edge day 16 (`b9a67f48`) for the rotation fix below. Three random 29-character values were kept only in the run directory (mode 600).

| Step | Result |
| --- | --- |
| `gh secret set` 2.102.1: `BF59_REPO` (`-R`), `BF59_ENV` (`--env bf59env`), `BF59_ORG` (`--org m3org --visibility selected --repos wp3-bf59`); `gh variable set BF59_VAR` | each exit 0; the three list routes show the names and no value; one owner key `1478349346864660411` |
| Job 1179653 | Success; repo, env, and org each print the right hash and length 29; `var bf59-plain-variable` |
| Fork-PR job 1179656 (run marked `fork` through the dispatch route) | Success; all three print the empty value's hash `e3b0c44298fc1c14` and length 0; the variable still reads |
| `rotate-seal-key` (run 2) | 200: new key `3052770262353804737`; 4 records resealed (3 repository and environment, 1 org); keys `1478349346864660411` and `6243795946503046962` retired and dropped; afterwards no repository row or the org's fold names either |
| Job 1179659 after it | Success, all three right |
| Wrapping key: slot B created (`graft-m4a-secrets-wrap-key-b`); `graft-m4a-ops` `cb75d656` with A and B, B current; `rewrap-secrets` | 200: 1 `SealKey` and 65 hook secrets rewrapped under slot 1 across 16 repositories; a second run rewraps nothing |
| Slot A cleared: `graft-m4a-ops` `459c7362` with B alone | job 1179662 Success, all three right; a ping to hook 229398 arrived signed (`x-hub-signature-256`), 200 |
| Scan (`bf59.mjs scan`) | 0 hits for any value, its base64, or its hex in every committed entry of `repo:65554` and `authz:7100001` on all three members (1,296 entries) and in 34 run files including runner logs and diagnostics |
| Workers Logs (`tests/m4/wp3/bf59-logs.mjs`, 18:40Z to 19:31Z) | 0 hits in messages or request URLs of the edge, ops, and three site scripts (423, 148, 17,511, 7,211, 3,289 events); a control query for strings that are logged matched 6 and 18 events |

Found on the way, both fixed and redeployed before the run that counts:

- **An empty 201 broke `gh secret set`.** go-gh decodes every non-204 body, so a new secret answered 201 with no body made `gh` report "unexpected end of JSON input" although the write committed. A new secret or variable now answers 201 `{}` (`What::ActionsConfig`, kept test in `crates/api/tests/surfaces.rs`). Site day 32 and edge day 15.
- **Rotation reseals shared one request id.** The first `rotate-seal-key` resealed the org secret and one repository secret of three, and kept the old key: every repository command it committed carried the edge request's id, so the leader answered the second and third as replays of the first. Each command a rotation commits now has its own id (`commit_issue_as`). Edge day 16; the rotation run again finished the reseal and dropped both older keys.
- The driver's first job read `/logs` before the job's log was joined and through a JSON helper that cut a text body to 300 characters; it now reads raw and waits for the joined log. Jobs 1179653 and 1179656 were checked from a raw re-read of the same `/logs`.

## BF-56 hosted: required checks after both kills

Driver `tests/m4/wp3/bf56.mjs`, workflow `tests/m4/wp3/hosted/bf56.yml` (build, then test, then deploy; lint, which fails, then report, then notify), repository `m3org/wp3-bf56` (`repo:65555`, shard `actions:7100001.12`), unmodified runner 2.338.2 on two runner directories. Results in `tests/m4/raw/bf56/`. Each phase pushes fresh heads to branches `a` and `b`, whose open pull requests (#1, #2) target `main`, whose rule (GHES `PUT .../branches/main/protection`, new on edge day 17) requires all six job names. Run B is cancelled with GHES `POST .../actions/runs/{id}/cancel` (202) while its `build` runs. The kills go through `graft-m4a-memberkill`, a scratch Worker bound to the `Member` classes (`tests/m4/wp3/bf56-kill`), to a member's `/abort` and an Actions member's `/owner_fault`, which the edge does not forward (site day 33). Conclusion times are Cloudflare's, from the members' `/status` clock (the orb ran 5.9 s behind), polled every 2 s from the repository leader's `required_check` rows, so each lag is up to 2 s high.

- **Commit path** (runs 1376296 and 1376303). `RunOwner` held on the shard leader, both runs created, no check run or required row on either head; then the repository's leader member (site 0, term 2) was reset, the group read leaderless 0.4 s later, and the hold was lifted, so `RunOwner`'s first round met a dead commit path and retried until a leader answered. All 12 required checks concluded and match their jobs: A's build, test, and deploy `success`, lint `failure`, report and notify `skipped`; every B job `cancelled`. Largest lag 10.4 s [measured].
- **`RunOwner`** (runs 1376317 and 1376324). `/owner_fault {kill}` armed on the shard leader before the runs; its first round committed the repository's checks, then reset the leader before its shard half (site 0's member booted again at 23:21:53Z and was re-elected), and the next round replayed under the same request ids. The same 12 conclusions; largest lag 7.7 s [measured].
- **Found by run 1** (day 33, commit path, `tests/m4/raw/bf56/commit-run1.json`, which also passed, largest lag 9.3 s): an ephemeral runner finishing a cancelled job polled once more before it exited, was matched to run A's `test`, and exited, so `test` sat `Assigned` until its 10-minute lease requeued it. The leader now matches an ephemeral runner at most once (`workers/site/src/actions.rs`, site day 34). The first `RunOwner` run on day 33 was cut by an orb executor restart after its kill fired and was not counted.
- **Not BF-56's:** shard `actions:7100001.12`'s site 1 voter (`#2#f1`, a replacement) read commit 1 under its first-generation name while the leader was at 156; under its own name it was at the leader's index throughout. The driver's polls use each voter's own name.

## BF-37 hosted: `merge_group` runs and the queue's deliveries

Driver `tests/m4/wp3/bf37.mjs`, workflow `tests/m4/wp3/hosted/bf37.yml`, repository `m3org/wp3-bf37c` (`repo:65558`), site day 35 and edge day 18, unmodified runner 2.338.2 on two runner directories. Results in `tests/m4/raw/bf37/` (`result.json`, `deliveries.json`, `parsed.jsonl`, `polls.jsonl`, `run3.log`). `main`'s rule requires `queue-ci` with a merge queue (the protection `PUT`'s `merge_queue`). Pull requests #1 and #3 pass; #2 sets `BF37_FAIL: "1"` in its copy of the workflow, so every queue ref holding it fails. A hook on the repository sends every event to `graft-m4a-bf37sink`, and each delivery is replayed through go-playground/webhooks v6.4.0's unmodified `github.Parse` with the hook's secret (`tests/m4/wp3/bf37/main.go`). Times are Cloudflare's (the orb ran 6.4 s behind); the driver lists the queue refs over smart HTTP every 2 s or so, so a deletion time is up to one poll late.

| Queue ref | Built | Run | Conclusion | Ended by | Deleted |
| --- | --- | --- | --- | --- | --- |
| `pr-1-2d542013` | 03:22:08 | 1114172 | Success | #1 landed, 03:23:39 | 03:23:49 |
| `pr-2-d0f46248` | 03:22:08 | 1114174 | Failure | #2 ejected (`dequeued`), 03:23:39 | 03:23:49 |
| `pr-3-a1d081dc` | 03:22:08 | 1114176 | Failure (holds #2) | rebuilt without #2, 03:23:39 | 03:23:49 |
| `pr-3-d0f46248` | 03:23:49 | 1114184 | Success | #3 landed, 03:24:59 | 03:25:07 |

- **Runs.** `ActionsTrigger` created one `merge_group` run on each built ref, at the ref's tip, and none on any other ref. #1 and #3 merged; #2 did not.
- **Deletion.** Every queue ref was gone within 10 s of the delivery that ended it, against the gate's 5 minutes, and `ls-remote` lists no `gh-readonly-queue` ref after the run.
- **Deliveries.** 18, none failed: `pull_request` `opened` 3, `enqueued` 3 (one per PR), `dequeued` 1 (#2), `closed` 2; `merge_group` `checks_requested` 4 and `destroyed` 4 (one each per ref); and the ping. `Parse` accepted every signature. For `merge_group`, which v6.4.0 has no type for, it answers "unknown event merge_group" after the HMAC check, and the harness decodes the body itself (DECISIONS, "M4-WP3: BF-37's merge queue setting and `ActionsTrigger` for `merge_group`").
- **Found by run 1** (`tests/m4/raw/bf37/run1/`, `m3org/wp3-bf37`): every run failed, #1's too, because the job message dropped job-level `env`: the runner's log of job 1507329 shows the step with no `env`, and `test "$BF37_FAIL" = 0` exited 1. `crates/actions/src/specs.rs` now merges the workflow's and the job's `env` into each step (DECISIONS, "M4-WP3: job and workflow `env` ride on each step"); that fix is not on graft-m4a yet, so the passing run's workflow sets `env` on its step. Run 2 (`run2b/`, `m3org/wp3-bf37b`) was stopped: the driver passed a relative output path to runners started in their own directories, so none started. Its three queued runs were cancelled.

## BF-55 hosted: killing the `Owner` registration shard's leader site

Driver `tests/m4/wp3/bf55.mjs`, workflow `tests/m4/wp3/hosted/bf55.yml` (a 16 × 16 matrix, 256 jobs, `RUN_JOBS_MAX`), repository `m3org/wp3-bf55b` (`repo:65560`, jobs on `actions:7100001.4`), unmodified runner 2.338.2 on two runner directories, each ephemeral and registered `Owner` scope on `actions:7100001.0` (`"scope": "owner"`), so its broker reads all 16 shards. The kill is a site day of the registration shard's leader site alone through the production deploy path (`tests/m4/wp3/bf55-kill.sh down`: `deploy.mjs --only <site> --down <site>`, `GRAFTYARD_SITE_DOWN=1`), once 64 jobs completed. The driver polls every site's broker object's `/stats` through `graft-m4a-memberkill`'s `BROKER_<n>` bindings and the run's jobs every 2 s; "assigned once" is read from each job's final version (3 is matched, acquired, finished, once each).

### Run 2 (the row): site days 39 to 41, 2026-11-15 11:30Z to 12:17Z

Times are Cloudflare's, from the members' `/status` clock (the orb ran 7.5 s behind). The driver ran as the supervised orb service `bf55-run2`, and the day 40 and 41 deploys were its children.

- **Before.** Site day 39 put `a3d57564` on all three sites at 10:45Z. The registration shard `actions:7100001.0` was led by site a (term 4) when the kill fired. A first start at 11:26Z read site b as the leader at setup, before day 39's resets moved it back, so it was stopped and its run (1769729) cancelled before any kill. The driver now reads the leader at kill time. Run 1769986 started 11:30:16Z.
- **Kill.** 64 jobs completed at 11:41:19Z. Site day 40 deployed site a's down version `6147b5df` by 11:41:29Z. Site a's registration-shard member kept running its old version until it first answered as other than the leader at **11:46:27.2Z**, which the row times from (`dead`).
- **Polls on the new leader's site.** Site b's member became leader (term 5), and site b's broker took its first poll at **11:46:38.8Z, 11.5 s after `dead`** (pass: within 15 s).
- **Restore.** Site day 41 brought site a back on `47a5705f` at 11:49:29Z (the driver's clock), 3 minutes after `dead`.
- **Every job once.** All **256 of 256 jobs concluded `Success`**, and every job ended at **version 3**: one `Matched`, one `Acquired`, one `Finished`. **No job was assigned twice.** 256 ephemeral runners ran, one job each.
- **No stall.** The longest gap with no job leaving `Queued` was **36.4 s** (11:48:19Z to 11:48:55Z, after the kill), against the fail line of 60 s. The steady-state gaps are about 21 s: two runners, each re-registered after its job.
- Raw data `tests/m4/raw/bf55/run2/` (`result.json`, `polls.jsonl`, `run2.log`); the first start's in `run2a/`.

### Run 1 (does not count): site days 36 to 38, 08:11Z to 09:00Z

- **Kill.** At 08:23:33Z (64 done) the driver deployed site a's down version `f339b2e6` (08:23:43Z). Site a's broker took its last poll at 08:28:41Z and site b's its first at 08:29:07Z, with the registration shard's leader then on site b (term 3), so the first poll on the new leader's site came within about 26 s of the old broker's last, not of a known kill time. A running object keeps its old version for minutes after a deploy (BF-58), and the driver read the old leader's 503 "site down" as alive, so it never marked the kill and could not time the row.
- **Restore.** Site a back on `8102e109` at 08:32:12Z, 8.5 minutes after the down deploy.
- **What failed.** Four jobs (`build (4, 14)`, `(4, 15)`, `(5, 11)`, `(5, 12)`) sat `Assigned` at version 1 to runners 1704011, 1704012, 1704025, and 1704026, which had each run one job and exited; 89 jobs finished at version 3 and none above it, so no job was assigned twice before the run was cancelled at 09:00Z (`POST .../cancel`, 202). Dispatch stalled 87 s (08:30:14Z to 08:31:42Z on shard 0), and from 08:37Z it stopped: both runner slots were matched to jobs they never took, and the driver, seeing them busy, started no new ones.
- **Why.** Each runner's listener leaves one poll held when its job ends (`status=Busy` before the job's `Online` event, `tests/m4/wp3/runner-protocol.md`), then deletes its session and record. The broker kept a set of the ephemeral runners it had matched, but site b's broker had matched none of the runners already running when site a went down, so it matched their last polls. Separately, site b's summary reads of the 13 shards whose groups do not exist held each poll for the lookup's whole 20 s while site a's members answered 503, because the lookup answers "no group" only when every member says "not joined".
- **Fixed** in `a3d57564` (DECISIONS, "M4-WP3: BF-55's `Owner`-scope matching", amended): a broker never matches an ephemeral runner it matched or saw holding a job; it matches only when every shard with a group was read in that poll; each read is cut at 3 s, and a failed shard counts as having no group only when a majority of its members answer "not joined". The in-memory gate now runs ephemeral runners that poll while busy and leave a last poll; with them it found the second rule (a broker on the new site matched a runner on another shard while the shard holding its lost-answer match had no leader yet). Plants 46 and 47 are caught. The driver now marks the kill at the old leader's first answer that is not a leader's, and restores the site 180 s after it. Raw data `tests/m4/raw/bf55/run1/`.

## The parser: `crates/actions/src/workflow`

- **`template`** reads YAML (`yaml-rust2`, default features off, which builds for wasm32) into template tokens and validates them against `workflow-v1.0.json`, the oracle's schema with descriptions removed, so allowed contexts, required properties, and error text come from the same data the runner uses.
- **`expr`** ports `@actions/expressions`' lexer, parser, and evaluator, with the oracle's error text, and JavaScript's and .NET's number printing (`G15` names matrix jobs).
- **`mod`** converts as `convertWorkflowTemplate` does: events and their filters, jobs, `needs` checked for unknown jobs and cycles, `if` given `success() &&` unless it calls a status function, step ids (`__run`, `__run_2`, `__owner_repo`, `__self`, a docker image's), and reusable-workflow calls (parsed, not run).
- **`matrix`** ports the C# `ConvertToStrategy` and `MatrixBuilder`. The npm oracle has none, so its tests are GitHub's documented include and exclude examples plus id and name rules read from the C# source, with expected values written by hand.
- **`filter`** decides `on:` for push branches, tags, and paths, `pull_request` and `merge_group` activity types and base branches, and `workflow_dispatch`. Pattern rules are act's `workflowpattern`, checked against GitHub's cheat-sheet rows.

**Differential test** (`crates/actions/tests/workflows.rs`): 59 workflows, 43 of them public (ripgrep, tokio, cargo, serde, TypeScript, node, cli, uv, ruff, rust-analyzer, home-assistant, next.js, react, deno, cpython, vscode, docker/build-push-action, workerd, and others) and 16 written for the port, each parsed by the port and compared with `@actions/workflow-parser` 0.3.61's JSON (`*.oracle.json`, written by `tests/m4/wp3/oracle.mjs`), errors included. All 59 match. Two differences are allowed and recorded: a multi-line string's expressions are read from its value, as the C# parser does, not from its raw YAML source (`07-scalars.yml`), and a YAML syntax error must match in position, not in the YAML library's wording (`14-bad-yaml.yml`).

**Plants** 18 to 40 in `tests/m4/wp3/plant.py`: all 23 caught. The first run missed one, "a job name falls back to its id only when absent", because no workflow had `name: ''`. `16-names.yml` covers it now.

## Slice B: the job's `GITHUB_TOKEN` (`gh-token`), in memory

Built on branch `gh-token` from main `81ab3b9c`, code-only (`decisions/DECISIONS.md`, "`gh-token`"). No site day, edge day, or hosted run.

- **What a job gets.** At `acquirejob` the edge reads the job's run and declared `permissions:` (`{"jobToken": job}` on the shard), commits the owner's Actions installation if its `Authz` lacks it, and has `graft-ops` sign `token::job_grant`: `ACTIONS_APP` on `actions_installation(owner)`, the run's repository alone, GHES's permissions (a fork's writes read), 6 h 10 min, and `jti = job_jti(job, attempt)`. The shard commits `AcquireToken` (`Acquired` plus `TokenIssued`), and the job message carries the token only from such an entry.
- **The deny.** `RunOwner` revokes each issued job's `jti` on its `Tokens` shard when a `Finished`, `Cancel` or `LeaseExpired` completes it, before the cursor that passes the entry, with `exp` 24 h past the end.
- **Visibility.** The edge routes no installation listing; the token route reads `NameKey::Installation`, which a negative id never holds; App deliveries skip Apps with no `AppRecord`. So no GHES 3.22 reply changes for a client that does not run Actions.

**Tests.**
- `a_vanished_runners_job_token_is_denied_at_its_lease_end` (`crates/actions/tests/gates.rs`): a runner killed after its acquire; nothing is revoked while the lease is live, the `jti` is revoked within 5 s of the lease failing the job, and the revocation outlives `JOB_TOKEN_MAX`. On main it fails to compile (no `AcquireToken`, `token::denies`, or deny in `round`); with the deny removed (plant 68) it fails at "the token is still live a minute after the lease failed its job".
- `a_finished_cancelled_or_deleted_repositorys_job_token_is_denied`: finished, failed, cancelled by an admin, and cancelled by `RunOwner` after the repository was deleted each revoke; a skipped job and a running job's token are not revoked.
- `the_actions_installation_commits_without_a_route_and_a_job_token_reaches_one_repository` (`crates/identity/tests/wp4.rs`): no deps, format 0x12.14, only the owner's own id and `actions_row()`, idempotent, `Uninstall` refused, `RegisterInstallation` of id ≤ 0 refused, and a minted token reaches its one repository at its permissions.
- `key_rotation_keeps_both_keys_valid_through_the_overlap`: a 24 h job token signed by an isolate's cached old key verifies until its `exp`, and the old key retires after it.
- Unit tests: `only_the_actions_apps_tokens_outlive_an_hour_and_none_outlives_a_day`, `a_mapping_replaces_the_default_and_a_fork_gets_reads`, `a_jobs_grant_is_its_repository_its_permissions_and_its_jti`, `a_jobs_jti_is_its_own`, `an_old_fold_keeps_its_bytes_until_a_token_row` (main's derived `ActionsFold` and this one write the same bytes until a 0x15.5 row).
- `wire_tags.golden`: four lines appended (`ActionsEvent` 6 `JobTokens`, 7 `TokenIssued`; `ActionsOp` 11 `CreateRunWith`, 12 `AcquireToken`), nothing else changed.

**Plants** 58 to 75 in `tests/m4/wp3/plant.py`: all 18 caught. The first run missed 63 ("a token's `jti` is drawn, never the job's": no test read the minted `jti`) and 70 ("every ended job is denied, issued or not": no test cancelled a job no runner held). `only_the_actions_apps_tokens_outlive_an_hour_and_none_outlives_a_day` now checks the grant's `jti`, and the cancel test cancels a queued run; both reruns are caught.


## BF-69 hosted: `actions/cache@v4` and `@v5`

Driver `tests/m4/wp3/bf69/bf69.mjs`, workflow `tests/m4/wp3/bf69/cache.yml` (jobs `v4` and `v5`, each 200 MiB of random bytes, 7 chunks), mirror `tests/m4/wp3/bf69/mirror.mjs`. Repository `m3org/bf69` (public), unmodified runner 2.338.2 on two slots. Actions shard `actions:7100001.11`. The `actions` org (7100010) held `actions/cache` (v4 `0057852b`, v5 `caa29612`) and `actions/checkout` (v4 `11d5960a`), pushed from github.com's tags; workflows say `uses: actions/cache@v4` and `@v5` unchanged.

**Install.** Site day 1 (03:46Z, main b37ca7a8, no flag): a bc36f89c, b 059383f0, c cebda537; every live shard leader answered `cache_list` 404 "actions-cache is not enabled". Site day 2 (04:27Z, main 281683f6, `GRAFTYARD_ACTIONS_CACHE=1`, `--oidc bfaa0c15…` for M4-WP4): a 1a489208, b e844af52, c 805df109; 13 of 13 live shard leaders answer `cache_list` "Cache not found". Edge day (04:50Z, main 281683f6, test-faults): d3d76118, R57's diff against e9197bf3 is FORMATS_ENABLED +21.6 and GRAFTYARD_CACHE_LIMIT=1073741824 only, edgever exit 0. Crons [] on every site; reretire OK after each site day.

**Formats.** 0x15.6 is enabled by the sites' flag and the edge's FORMATS_ENABLED, as 21.2 to 21.5 were, not by a Release-group `FormatsEnable`: m4a's `SiteRelease` names R9's uploads (main 801d63ad), which do not report 0x15.6, so `check_reported` refuses it as `NotReported`. A release roll of an upload that reads 0x15.6 would make `FormatsEnable` possible; it was rejected for this row on cost (OPEN-QUESTIONS F124).

**Toolkit.** `isGhes()` is true for any host but github.com, `*.ghe.com` and `*.localhost`; on GHES `getCacheServiceVersion` is v1, whose `isFeatureAvailable` reads only `ACTIONS_CACHE_URL`; `GHESNotSupportedError` is defined but not thrown on this path; `downloadCache` is one plain GET for any non-blob host (dist of v4 and v5). Every job's log shows the v1 calls: `Cache not found`, `Cache saved with key`, `Cache restored from key`.

| Line | Jobs | Result |
| --- | --- | --- |
| Save on `main` | 1194827, 1194828 | both `Cache saved` (209,722,421 and 209,722,426 B, 7 chunks each) |
| Exact restore on `main` | 1194777, 1194778 | both `cache-hit` true, SHA-256 equal to the saved bytes |
| `f` through restore-keys `bf69-main-` | 1194782, 1194783 | both restored from `v4-`/`v5-bf69-main-1`, `cache-hit` false, SHA-256 equal; then saved `bf69-f-1` |
| Save on `f`, then `main` | 1194787/88, then 1194792/93 | `f` saved `bf69-fonly-1`; `main` misses it on both majors |
| Two saves of one key | 1194797/98 (run A), 1194800/01 (run B); then the parallel-save row below | **not met in this row**: every job succeeded, but run B's jobs started 16 s after A's ended, so B restored instead of saving. **Met** in the parallel-save row below |
| PR into `rel` (restore key `bf69-`, `rel` holds `bf69-rel-1`, `main` holds `bf69-main-1`) | PR #1, 1194817/18 | both restored `rel`'s entry, SHA-256 equal to `rel`'s save |
| PR into `main` | PR #3, 1194832/33 | both restored `main`'s entry, SHA-256 equal |
| LRU at 1 GiB | 1194837 to 1194858 | after `bf69-lru-1` and `-2`, R2 holds ids 17, 19, 20, 21, 22 (the five newest commits by the `Cache saved` times; id 18, `v4-bf69-pr-main-miss`, committed 0.16 s before id 17 and was evicted first [inference: ids are assigned at reserve, `created` at commit]; 1,048,612,178 B, each id's bytes equal to its `Cache Size`); every older id's chunks are gone; `lru-1` and `lru-2` restore, `rel-1` misses, on both majors |

No job failed. The 35 objects of the 5 live entries stay in `graft-m4a-site-a-packs` after the repository's delete: a delete does not drop its cache, which the 7-day sweep removes (not waited for).

Found on the way:

- **`actions/checkout@v4` failed every job.** The runner evaluates an action input's default (`repository: ${{ github.repository }}`) only when the message carries every context `input-default-context` names, and `runsvc::job_message` sent `strategy` and `matrix` only for a matrix job: "Unexpected type 'BasicExpressionToken' encountered while reading 'default value for input 'repository''" (job 1194757). Reproduced with `run-host.sh` and fixed on branch `runsvc-ctx` (2caeaee3: both contexts always, empty when there is no matrix; tests/m4/wp3/plant.py #76 CAUGHT); not deployed, so this row's workflow carries its case in `env` and uses no checkout. Checkout's `token: ${{ github.token }}` also needs job tokens (21.5), which m4a does not enable.
- **The `parallel` case could not prove overlap with two runner slots**, so the line ran again as its own row (below).
- `main-not-f`'s v5 lookup took 6 min 7 s to answer "not found" (05:27:14Z to 05:33:21Z); the shard's three members were followers when probed before the row and needed a `/campaign` (cf. LM-1's B0-35 note).
- Neither major declares a `cache-matched-key` output; the driver reads the restored key from the toolkit's log line.

Cost (`acctcost.py`, prefix graft-m4a, UTC day): $0.56 after site day 1, $0.64 at 04:12Z, $1.51 at 05:56Z after teardown. Site day 2, the edge day and the row: **$0.87**, against the $6 stop.

### The parallel-save line, its own row (07:07Z to 07:11Z)

The lead's ruling: the same install, `par.yml` (`tests/m4/wp3/bf69/par.yml`) from one `workflow_dispatch` fanning out to four matrix jobs, two per major, on four runner slots, every leg seeding key `bf69-par-2` and holding its bytes until one time `AT`, so each key's two reserves overlap by construction. Repository `m3org/bf69p`, Actions shard `actions:7100001.12`.

- **Install.** Site day 3 (06:54Z, main 0aa3b507, which carries runsvc-ctx 2caeaee3, `GRAFTYARD_ACTIONS_CACHE=1`, `--oidc`): a eb5c77f2, b c6b12562, c 3a1ea21b; crons []; reretire OK. Edge d3d76118 unchanged.
- **Mirror name.** The actions are `actions-mirror/cache` (v4 `0057852b`, v5 `caa29612`) and `actions-mirror/checkout` (v4 `11d5960a`) under org `actions-mirror` (7100011), and the workflow says `uses: actions-mirror/cache@v4`, `@v5` and `actions-mirror/checkout@v4`, because m4a holds `actions/cache` and `actions/checkout` `Deleted` for 90 days after BF-69's teardown (a create answers 422).
- **Checkout, the context fix hosted.** All 8 jobs ran the unmodified `actions/checkout@v4`, whose `repository: ${{ github.repository }}` default now evaluates: every job checked out its run's commit (`bf69.env` hash `aca81e4805a2d979` in the four seeding jobs, `8f339d98176b37ad` in the four reading jobs, each its own commit), and no job log or runner diagnostic holds `BasicExpressionToken`. Deviation, approved by the lead: m4a enables no job tokens (21.5 is off), so checkout's `token` is the repository secret `BF69_READ`, an m3-ci installation token limited to `contents: read` on `bf69p`, set with `gh secret set` from stdin and kept by the driver's scrubber; at the end the driver deleted the secret (204) and revoked the token (`DELETE /installation/token` 204; the token then reads 401).

| Line | Result |
| --- | --- |
| Overlap | `AT` 07:09:36Z. The edge's four reserves (`POST …/artifactcache/caches`, Workers Logs): 07:09:46.888Z 201, 07:09:47.086Z 201, 07:09:47.188Z 409, 07:09:47.883Z 409, all inside 1 s. Each key's two reserves came 0.30 s and 0.80 s apart |
| One save per key commits | v4: job 1376338 (leg b) saved `v4-bf69-par-2` at 07:09:50.413Z (209,722,409 B); job 1376337 (leg a) got "Unable to reserve cache with key v4-bf69-par-2, another job may be creating this cache" (the 409). v5: 1376339 (leg a) saved (209,722,420 B); 1376340 (leg b) got the 409. R2 holds exactly two entries on the shard, ids 1 and 2, 7 chunks each, bytes equal to the saved sizes |
| Both jobs succeed | all 4, and all 4 of the read after |
| The read after gets one whole entry | jobs 1376346/47 (v4) and 1376348/49 (v5): `cache-hit` true from `v4-`/`v5-bf69-par-2`, SHA-256 equal to the bytes of the leg whose save committed (`94d003cd…` and `05e134ae…`), not the refused leg's |

**F126 rode along at $0.** The cache steps ran with `ACTIONS_STEP_DEBUG` and `RUNNER_DEBUG`. No lookup or reserve ran past 30 s: the edge served every cache call of the row (33) in at most 10,047 ms (the 200 MiB download), lookups in 1,141 to 1,519 ms. The driver flagged two restore steps over 30 s (jobs 1376346 and 1376349, 34 s each), and the edge logged 5 of their calls inside each gap, so those were a download, not a hold. The toolkit printed no `##[debug]` lines through the runner although both variables reached the step's env; the runner's own debug switch is the job message's `ACTIONS_STEP_DEBUG` variable, which the driver does not set, so a rerun that needs the attempt lines sets it as a repository variable. F126 stays open.

Teardown: `actions-mirror/cache`, `actions-mirror/checkout` and `m3org/bf69p` deleted (404); the `actions-mirror` org stays; the sweep tore down 4 shard containers and found none running after. Cost (`acctcost.py`, prefix graft-m4a, UTC day): $1.58 at 06:50Z, before site day 3; $1.78 at 07:19Z after teardown: **$0.20**, against the $2 stop.

## Checks

`cargo test -p graftyard-git -p graftyard-actions -p graftyard-api`, `cargo clippy -p graftyard-actions -p graftyard-git --all-targets -- -D warnings`, `cargo clippy -p graftyard-site --target wasm32-unknown-unknown -- -D warnings`, `cargo fmt --all -- --check`, and `scripts/check_design.py`, `check_names.py`, and `check_links.py` all pass.

## Cost

`INSTALL=graft-m4a python3 tests/fanout/m2bcost.py`, 2026-11-14 14:10Z to 14:50Z (site days 28 and 29, the edge days, and the live run): $0.53, of which containers $0.51 (git shards started by the resets). The whole install 00:00Z to 14:50Z: $8.47, containers $7.66. 13:00Z to 23:30Z (BF-61's reruns, BF-4, BF-59, BF-56, site days 30 to 34, edge days 14 to 17): $12.17, containers $11.78.

BF-55, 2026-11-15 06:35Z to 12:27Z (both teardowns, site days 36 to 41, edge day 19, runs 1 and 2): **$3.14**, containers $2.50. Of that, 06:35Z to 06:53Z, before the first teardown, was $0.55 (all containers), and run 2 (11:26Z to 12:27Z) was $0.75. With the $24.1 metered before 06:35Z, the hosted total for steps 1 and 2 is **about $27.2 of the $30 cap**. Every git shard was torn down at 12:26Z, so graft-m4a sits at $0/h for containers.

## BF-71, `actions-artifacts`: the local run (2026-11-25)

`tests/m4/wp3/run-host.sh <runner dir> tests/m4/wp3/bf71/art.yml <out>`, with upload-artifact v3.2.2 (c6a366c9) and download-artifact v3.1.0 (a9bc5e6e) mirrored unchanged, against `runsvc_host`'s on-disk mirror of the edge's routes (`crates/actions/examples/runsvc_host.rs`, `artifacts`). An unmodified runner 2.338.2 ran six jobs and each succeeded:

- `up`: artifact `a`, holding a 40,000 B file (gzipped in memory), 30 MiB of text (gzipped on disk), 50 MiB of random bytes (sent plain in 7 chunks of 8 MiB), and an empty file.
- `leg (1)` and `leg (2)`: one file each into artifact `m`.
- `re`: `r/x.bin` uploaded at 9,000,000 B, then again at 12,000,000 B in the same job. Its record names the second upload (`size` 12,000,000); both uploads' chunks were on disk until the replaced one was deleted.
- `probe`: `BF71 PROBE {"create":201,"put":200,"listed":["a","m","r","sums-a","sums-m-1","sums-m-2","sums-r"],"otherRunPipe":403,"otherRun":403,"otherRepo":403,"otherShard":403,"noToken":401}`. The artifact `unfinal`, uploaded but never finalized, does not list.
- `down`: `download-artifact` by name, then all of them; every file `OK` against its writer's SHA-256, the empty file empty, no `unfinal`, then `BF71 ALL SUMS OK`.

The local host is not the edge: it proves the client's protocol and the pure half (`graftyard_actions::artifacts`), not Workers' streaming or the gzip passthrough, which `tests/edgeboot/run.sh` checks locally and BF-71 checks hosted.

**Driver fix found on the way.** `runsvc_host` handed a queued job to a runner that had finished its one job but not yet deleted its session (the runner polls once between `completejob` and the delete), and that job was stranded. Every runner the host registers is ephemeral, so it now hands each runner one job only.

## BF-71 hosted: the plan

On graft-m4a after BF-70 r2, $3 stop, on its own edge day from main at adfdaf4a or later (a57-errors-read, F128's elect-again, dispatch-perms F127, oidc-nbf F129, and this branch), with `GRAFTYARD_ARTIFACTS=1`:
1. `node tests/m4/wp3/bf71/bf71.mjs lifecycle`: the `actions/artifacts/` rule (delete at 90 days) merged into each site bucket's rules and read back; recorded in `tests/am1/results/install-m4a.md`.
2. F130's setup step from M4-WP4 (Browser Integrity Check off for `/_services/token/*`), with its before and after Python-urllib fetch.
3. Mirror upload-artifact v3.2.2 and download-artifact v3.1.0 under `actions-mirror` (`tests/m4/wp3/bf69/mirror.mjs`, `MIRROR_ORG=actions-mirror`).
4. `node tests/m4/wp3/bf71/bf71.mjs run <out> <runner dir> <runner dir>`, then `python3 tests/fanout/tieroff503.py <from> <to> graft-m4a-edge` for the elect and tierOff counts.
5. Teardown: delete `m3org/bf71`, and once a site day ships F125, check its prefix is gone (`bf71.mjs prefix`).

## BF-71 hosted: the result (2026-11-25)

**PASS on every line.** Run 1120326 on `m3org/bf71` (repo 65604, shard `actions:7100001.8`), 12:52:48Z to 12:55:32.945Z (the done line), unmodified runner 2.338.2 on two runner directories, upload-artifact v3.2.2 (c6a366c9) and download-artifact v3.1.0 (a9bc5e6e) mirrored unchanged as `actions-mirror/*` (repos 4128770 and 4128771; m4a holds `actions/*` Deleted since BF-69, so the driver rewrites `uses:` to the mirror). Driver `tests/m4/wp3/bf71/bf71.mjs`, days `tests/m4/wp3/bf71/days.sh`, raws in `tests/m4/raw/wp3/bf71/`.

**What the days carried.** Both built from main `c40141d8` (a detached worktree, test-faults).
- Site day, 12:41Z (sites a `1d6a9c52`, b `2a294df5`, c `ed3481e0`): BF-70 r2's flags unchanged (`GRAFTYARD_JOB_TOKENS=1`, `GRAFTYARD_ACTIONS_CACHE=1`, `--oidc`, PR_TRIGGER, APP_HOOKS, ACTIVITY, AUDIT, the run key, the audit stream). It ships F125's member half and `oidc-base-ref`. Crons reset to `[]`; reretire 35 of 35 not joined with the retired row.
- Edge day, 12:46Z (edge `3c453149` at 100%, `edgever` exit 0): a57-errors-read (`cfa269df`), F128's elect-again (`338e1501`), dispatch-perms F127 (`e4167de2`), oidc-nbf F129 (`adfdaf4a`), oidc-base-ref, and actions-artifacts (`9f415115`), each an ancestor of the build. The plain-text binding diff is `GRAFTYARD_ARTIFACTS=1` alone; `FORMATS_ENABLED` (29 formats, 21.5, 21.6 and 18.14 among them), `RUN_SERVICE_PUBLIC` and `GRAFTYARD_CACHE_LIMIT` kept from `330560d3`.
- Lifecycle, 12:33Z: `graftyard-actions-artifacts-90d` (prefix `actions/artifacts/`, delete at 7,776,000 s) merged beside each bucket's `Default Multipart Abort Rule` and read back on all three (`lifecycle.log`).

| Line | Measured |
| --- | --- |
| Every job succeeds | 6 of 6 `Success`: `up`, `leg (1)`, `leg (2)`, `re`, `probe`, `down` |
| SHA-256 identical, gzipped ones included | `down` printed `OK` for every file, by name and in the all-artifacts download (`small.txt`, `dir/big.txt`, `dir/deep/random.bin`, `empty.txt` twice each; `leg-1.bin`, `leg-2.bin`, `x.bin`), then `BF71 ALL SUMS OK`. The records show what crossed the edge gzipped: `small.txt` 73 B for 40,000 (`gzip: true`), `dir/big.txt` 91,625 B for 31,457,280 (`gzip: true`), `random.bin` 52,428,800 B plain in 7 chunks. So Workers served the stored gzip as stored, hosted: the DECISIONS `actions-artifacts` [inference] on hosted behaviour is now [measured] |
| The empty file arrives empty | `test -f empty.txt && test ! -s empty.txt` passed; its record is `size` 0, upload 0 |
| Both legs list under one name | `m` holds `leg-1.bin` (job 1120328) and `leg-2.bin` (job 1120329) |
| A re-upload replaces whole, never a mix | `r/x.bin` was uploaded at 9,000,000 B then 12,000,000 B in job 1120330. Its record names the second upload (`size` 12,000,000), the chunks under `f/1120330/` are that upload's two (8,388,608 + 3,611,392 B) and nothing of the first, which was deleted, and `x.bin` matched the second upload's SHA-256. No 503 |
| The unfinalized artifact never lists | `probe` uploaded `unfinal/x.txt` (PUT 200, its record written) without the finalize: the run's list held `a, m, r, sums-a, sums-m-1, sums-m-2` and no `unfinal`, R2 holds no `m/unfinal`, and `down`'s all-artifacts download made no `all/unfinal` |
| Cross-run, cross-repository, cross-shard 403; no token 401 | `BF71 PROBE {"create":201,"put":200,…,"otherRunPipe":403,"otherRun":403,"otherRepo":403,"otherShard":403,"noToken":401}`; the edge logged 4 answers 403 and 3 answered 401 in the window |
| Stop | $0.25 (m4a's UTC day $3.06 before, $3.31 after teardown), against $3 |

**Sheds (F128's hosted read).** `tieroff503.py 12:52:48Z 12:56:30Z graft-m4a-edge`: `{"tierOff": 0, "elect": 0, "edge503": 0}`, and the same over 12:40Z to 12:57Z, which takes in the repository's create at 12:50Z. `m3org/bf71` was new at 12:50Z and its first calls (create, push, dispatch, run reads) answered without a 503, so there is no spread line to record. The edge's answers 12:49Z to 12:57Z: 200 ×299, 201 ×98, 403 ×4, 401 ×3, 202 ×1, 422 ×1, no 5xx.

**F125 hosted.** `DELETE /repos/m3org/bf71` at 12:57:48Z answered 204, then the repository read 404. Its prefix `actions/artifacts/actions:7100001.8/65604/` held 48 objects at 12:57:54Z and at 12:58:35Z, and 0 at 12:59:04Z (the `Audit` finisher's `DropActions`, within one alarm and a retry of the delete). Buckets b and c held none throughout. The cache half (`CacheDropRepo`) had no entries to drop on this repository, so only the artifacts half was seen hosted.

**F130 on m4a.** `graftyard oidc check --edge https://graft-m4a-edge.<sub>.workers.dev` (12:37:00Z, exit 1): "GET …/_services/token/.well-known/jwks as Python-urllib/3.12: HTTP 403: the host is on workers.dev, whose Browser Integrity Check cannot be turned off (https://developers.cloudflare.com/workers/configuration/routing/workers-dev/), so a relying party on Python's default agent is refused 1010 before the edge runs; serve the edge on a custom host in the install's zone" (`oidc-check.log`). No zone rule applied, as ruled.

**A harness miss on the way.** The first start (12:49:58Z, run 1120309) gave the runners a relative output path, so each runner's shell, which runs in its own directory, failed its redirect and exited before taking a job. The driver now resolves the path; run 1120309 was cancelled (202, every job `Completed`) before the second start, and no job of it ran.

**Teardown.** `m3org/bf71` deleted (above); `teardown.py --sweep` tore down 3 shard containers (`git#1#net:65604`, and the two mirrors' `git#0#net:4128770` and `4128771`) and then reported no shard container running. No runner process is left. The mirrors stay as BF-69's did. m4a is left with sites `1d6a9c52` / `2a294df5` / `ed3481e0`, edge `3c453149`, ops `ac4cd9a5`, every cron `[]`.


## BF-75, the REST artifact routes (F131): built at $0 (2026-11-25)

Branch `bf75-artifacts-rest`. R2-only, no fold rows and no format (`decisions/DECISIONS.md`, "`actions-artifacts`", the amendment): the finalize writes a row object `actions/artifacts/<chain>/<repo>/a/<id>` whose JSON rides its custom metadata, so a list is one R2 list; the create POST keeps `RetentionDays` and the creation time; `GET .../zip` writes a 60 s ticket under the repository prefix and answers 302 to `/_services/artifact-zip/...`, which streams one zip composed from the stored chunks (`graftyard_actions::zip`: a gzipped file's deflate stream passed through as DEFLATE with the trailer's CRC, any other file STORED). Edge code only: it rides an edge day with `GRAFTYARD_ARTIFACTS=1` and needs no site day.

Local evidence:
- `tests/edgeboot/run.sh` on the edge's own wasm: two finalized artifacts' zips read back through Python's `zipfile` SHA-256 identical (`dir/text.txt` gzipped, method 8; `bin.dat` and an empty `sub/empty`, method 0), the one-day row expires 86,400,000 ms after creation and the default 90 days, and a delete leaves 8 objects, all the other artifact's, with its row. `PRODUCT=1` passes too; 0 wasm traps in each.
- `crates/actions/src/zip.rs` tests, cross-checked once by Python `zipfile.testzip()` and `unzip -t` (no errors, hashes equal).
- Plant `tests/m0carried/plant253.py`: 8 of 8 caught (expiry from the finalize, retention past 90, a pointer key mapped to its job, a page counted as the total, a ticket for another artifact, a ticket at its expiry, a plain file's length and a gzip's ISIZE unchecked).

### BF-75 hosted: the plan

- **Days.** One edge day on graft-m4a from this branch merged on main, keeping every live var (`FORMATS_ENABLED`, `RUN_SERVICE_PUBLIC`, `GRAFTYARD_CACHE_LIMIT`, `GRAFTYARD_ARTIFACTS=1`), as `tests/m4/wp3/bf71/days.sh edge` did. No site day.
- **Driver.** `node tests/m4/wp3/bf75/bf75.mjs <out> <runner dir>`: one ephemeral runner 2.338.2 runs `tests/m4/wp3/bf75/art.yml` (upload-artifact@v3.2.2 from `actions-mirror`) uploading `keep` (3,000,000 B of text, gzipped by the client; 9,000,000 B random in two chunks; an empty file) and `short` with `retention-days: 1`. The driver then reads, as the admin user: both lists, `GET artifacts/{id}`, each zip (302, then the URL with no token, `unzip -t`, SHA-256 against the job's printed sums), `gh run download <run>` with `GH_HOST` set to the edge, the zip URL again after 65 s (403), `DELETE` of `short` (204), both lists again, a `GET` of it (404), R2's objects under the repository's prefix, and an anonymous `DELETE` (401).
- **Pass.** Both lists name both artifacts with GHES's eleven fields and the finalized sizes (12,000,000 and 4,000); every zip and `gh run download` file is SHA-256 identical to the job's; `short`'s `expires_at` is its `created_at` plus one day and `keep`'s plus 90; the delete removes `short` from both lists and leaves none of its objects, with `keep`'s three records still there.
- **Fail.** A missing or extra artifact, a file that differs, a delete that leaves objects, or a wrong expiry.
- **Also recorded.** Elect and tierOff shed counts apart (`tests/fanout/tieroff503.py` over the row's window), and the edge version.
- **Estimate.** About $0.60: the edge day and one job of under 2 minutes, a few hundred R2 operations, no shard container. **Stop $3** by `PREFIX=graft-m4a python3 tests/fanout/acctcost.py`.

## BF-75 and BF-77 hosted: one edge day, the plan (2026-11-25)

The lead's GO, after AM2's kill row frees graft-m4a, $3 stop per row.

- **Edge day.** `ROW=bf75 bash tests/m4/wp3/bf71/days.sh <worktree at main 20d761d1 or later> edge`, with test-faults and the web build. It keeps every live plain-text var as it finds it: `FORMATS_ENABLED` (with 21.7 and 21.8, and whatever the kill row's edge day added), `RUN_SERVICE_PUBLIC`, `GRAFTYARD_CACHE_LIMIT`, `GRAFTYARD_ARTIFACTS=1`, and any later flag (the days script now carries every var it does not write itself). The binding diff it prints must be empty but for the build. No site day: BF-75 is edge code, and F125's member half has been on the sites since BF-71's site day.
- **BF-75.** `node tests/m4/wp3/bf75/bf75.mjs <out> <runner dir>`, lines as in "BF-75 hosted: the plan" above.
- **BF-77.** `node tests/m4/wp3/bf77/bf77.mjs <out> <runner dir>`:
  - *Delete half.* `seed.yml` in public `m3org/bf77-del` saves two `actions-mirror/cache@v4` entries and uploads one artifact. The driver reads the shard leader's `repoCaches` for the repository (at least 2) and the R2 prefixes, `DELETE`s the repository, and polls every 30 s. Pass: `repoCaches` 0 and `actions/artifacts/<shard>/<repo>/` empty within 10 min, and the shard's `actions/cache/<shard>/` fewer by at least the two seeded entries' objects. Fail: an entry or object left at 10 min.
  - *Token half.* `perms.yml` in private `m3org/bf77-tok`, three jobs calling the API with their own `GITHUB_TOKEN`: `default` (no `permissions:`, GHES's restricted default, contents and packages read), `ro` (`contents: read`), and the control `rw` (`contents: write`, `issues: write`, `statuses: write`). Each reads the repository, opens an issue, sets a commit status, lists artifacts, and pushes a branch over HTTPS. Want: `default` and `ro` read 200, issue 403, status 403, artifacts 403, push refused; `rw` read 200, issue 201, status 201, artifacts 403 (no `actions: read`), push accepted, and only `bf77-rw` among the branches after. Fail: any write past a token's permissions.
- **"Push refused for `contents: read`" is enforced, read in code:** a push's `decide` calls `may_write` (`crates/git/src/repo.rs`), which needs `authorize_write(actor, WriteAction::Push, ...)` = `Allow`; for an installation token (`Actor::App`, `Credential::Stateless`) that is `permission` → `app_permission` (`crates/identity/src/access.rs`), which grants `Write` only for `contents: write`/`admin` in both the claims and the installation row, so `contents: read` gives `Read` < `Write` and the push is refused. `crates/git/tests/push.rs` already holds the case ("Read-only claims", refused 403 or 404, no ref written). A job's token is such a token, its claims narrowed to the job's permissions (`graftyard_actions::token::job_permissions`).
- **Also recorded.** Elect and tierOff sheds apart for each row's window (`tests/fanout/tieroff503.py`), the edge version, and the cost after each row.
- **Estimate.** About $0.60 for BF-75 and $0.60 for BF-77 (two runs of under 5 min, one cache entry pair, R2 operations, the deleted repository's shard container if one starts). **Stop $3 each** by `PREFIX=graft-m4a python3 tests/fanout/acctcost.py`. Teardown: `m3org/bf75` and `m3org/bf77-tok` deleted, `teardown.py --sweep`.

## BF-75 and BF-77 hosted: the result (2026-11-25)

**Edge day.** `graft-m4a-edge` 78130d2f at 100% (from 8961d390), built from main 6b48a6dc (BF-75's 20d761d1 plus BF-72's code, which stays inert until 0x15.9 is enabled) with test-faults, 22:05:20Z to 22:09:58Z (`tests/m4/raw/wp3/bf75/day-edge.log`). The plain-text binding diff is empty: every live var kept, `GRAFTYARD_ARTIFACTS=1`, and `FORMATS_ENABLED` unchanged, with 21.7, 21.8 and the kill row's 32.28. No site day. A first attempt at 22:00:57Z failed to build (`/tmp` tmpfs full) and deployed nothing.

**BF-75: PASS on every line (25 of 25).** Run 1194864 on `m3org/bf75` (repo 65608), 22:10:53Z to 22:13:48.375Z (the done line). Both REST lists name both artifacts with GHES's fields; both zips (302, then the URL: `keep` 9,009,135 B, `short` 4,124 B) and `gh run download` give files SHA-256 identical to the job's sums; `short`'s `expires_at` is its `created_at` plus one day and `keep`'s plus 90; the zip URL answers 403 after a minute; the `DELETE` answers 204, removes `short` from both lists, and leaves none of its objects (site-a bucket 21 → 15, `keep`'s 3 records stay); a `DELETE` with no token answers 401. Sheds over the window: elect 0, tierOff 0, edge 503 0.

**BF-77, delete half: PASS (F125's cache half hosted).** Third attempt, `m3org/bf77-del3` (run 1507375, shard `actions:7100001.14`): before the delete the shard leader's `repoCaches` read 2 with 2 cache chunk objects and 6 artifact objects; `DELETE` 204 at 22:35:21Z; at 63 s `repoCaches` 0, the artifact prefix empty, the seeded chunks 2 → 0 (want: within 10 min). The second attempt (`bf77-del2`) passed the same line at 96 s. The first (`bf77-del`) never seeded: `actions-mirror/cache` was gone (BF-69's teardown deleted it, and the name is held Deleted for 90 days), so cache v4 0057852b is mirrored as `actions-mirror2/cache` (org 7100012, repo 4456448) and the driver takes `CACHE_MIRROR=actions-mirror2` and `DEL_REPO`.

**BF-77, token half: FAIL as run, not yet a verdict on permissions.**
- First run: every probe ran with no token. `perms.yml` read `$GITHUB_TOKEN`, which the runner never exports (it sets only `github.token`), so the shell's `set -u` failed every call. Fixed: each step maps `GITHUB_TOKEN: ${{ github.token }}` into its env, as GitHub's docs do.
- Second run (`bf77-tok`, run 3020811): `default` and `ro` read 200, artifacts 403, push refused; `rw` read 200, artifacts 403, push accepted, and only `bf77-rw` exists among the branches after. So no token wrote past its permissions, and the push line holds hosted both ways. Two probes fail for reasons that are not permission checks:
  - `POST /repos/{o}/{r}/issues` answers 404 for every caller, the site admin's PAT included (probed 22:39Z): REST issue creation is not routed on the edge (`workers/edge/src/api.rs` routes only `GET issues`; `IssueOp::OpenIssue` is reached from GraphQL `createIssue` alone). That is a missing GHES route, not a BF-77 line.
  - `POST .../statuses/{sha}` answers 401 "Requires authentication" to every installation token, `rw`'s `statuses: write` included: the route takes only `Actor::User` (`api.rs`, the `statuses` arm), because `StatusUpdate.creator` is an `AccountId`. A job token (`ghs_`) is refused before its permissions are read. GHES 3.22 accepts `statuses: write` from a `GITHUB_TOKEN`, so this is a bug to fix before BF-77's token half reruns.
- Sheds over BF-77's window (22:24:37Z to 22:37:59Z): elect 0, tierOff 0; edge 503 3, none an elect or tierOff verdict (`electByWhy` empty).

**Cost.** graft-m4a's UTC day $7.98 before the edge day, $8.02 after BF-75, $8.62 after BF-77's three attempts and teardown ($0.64 for both rows, the edge day included). Under both $3 stops.

**Teardown.** `m3org/bf75` and `m3org/bf77-tok` deleted (404 after); the `bf77-del*` repositories were the rows' own deletes; `actions-mirror2/cache` stays for BF-76. `teardown.py --sweep` tore down 9 shard containers and then found none running. No runner process left.

## BF-77 token half: the two routes (`bf77-routes`, 2026-11-26)

**Built at $0, not yet hosted** (`decisions/DECISIONS.md`, "`app-bot`"; F135).
- `POST /repos/{o}/{r}/issues` is routed. It plans GraphQL `createIssue`'s `OpenIssue` from GHES 3.22's body, `decide` needs `issues: write` of a `ghs_`, and the member renders the 201 `issue` with `Location` from `&Acked` (`What::IssueCreated`). An App's REST issue is authored by its bot account, so `user` is `github-actions[bot]`; an App's GraphQL `createIssue` keeps its author-less bytes.
- `POST .../statuses/{sha}` takes an installation token. `decide` needs `statuses: write`, and the creator is the App's bot account, `AppId::bot`, the App's id negated. It renders as GitHub's bot `simple-user`: `github-actions[bot]`, `type` `Bot`, `id` 2, a `BOT_` node id and `html_url` `/apps/github-actions`.
- Both are repository format 32.29, so the rerun needs a site day before the edge day, and `FORMATS_ENABLED` must add 32.29. The edge serves neither until then: issue create stays 404 and a token's status 401, as on main.

**Evidence at $0, on main `01781349` plus this head.**
- `crates/git/tests/issues.rs`, `an_installation_token_sets_a_status_as_its_apps_bot_with_statuses_write`: fails on main (the first App status is 403), passes here, and checks the REST issue's bot author against GraphQL's none.
- `crates/api/tests/surfaces.rs`, `rest_issue_create_and_a_bot_status_render_ghes_shapes`; both replies validate against `ghes-3.22.json` (`gh/validate.py`, 0 failures).
- `tests/bf77bed/run.sh`: the edge's own wasm in local workerd, the base edge (main `01781349`) and the head edge on one bed, 32.29 enabled. Output: base `default`, `ro` and `rw` each "issue 404, status 401", the PAT's issue 404 and its status 201; head `default` and `ro` "issue 403, status 403" with "Resource not accessible by integration" on both, `rw` "issue 201, status 201", the issue `#1 user github-actions[bot] Bot` with `Location` `.../repos/cw/r/issues/1`, the status `creator` `{"login":"github-actions[bot]","id":2,"type":"Bot","node_id":"BOT_kgAC","html_url":"https://cw.example/apps/github-actions"}`, the PAT's issue 201 (user and assignee `cw`) and status 201 (creator `cw`). 0 unexpected answers on either edge.
- `tests/plaindiff/run.sh` (base main `01781349`): 7 request pairs, 0 unexpected differences.
- `tests/m0carried/plant259.py`: 10 of 10 caught, the REST issue's bot author included.
