# M4-WP4, hosted runners on Sandboxes: results

The hosted rows ran on 2026-11-15 on graft-m4a (site days 42 to 45, edge day 20) and in the runner account, from main `aa4a9d9c` and then `c7970488`. The status table below gives the results. The next paragraph is the record from before the merge.

Branch `m4-runners`, rebased onto main 4833094d. The choices are in `decisions/DECISIONS.md`, "M4-WP4: how hosted runners register, count against the cap, and go stale, 2026-11-15". The operator approved the estimate below with a $25 stop. The runner account exists: "Graftyard Runners", `<cf-runner-account-id>`, on Workers Paid, workers.dev subdomain `graftyard-runners`. Its account-owned token is the project secret `CLOUDFLARE_RUNNER_API_TOKEN`. On 2026-11-15 it verified at `/accounts/{id}/tokens/verify` and read Workers scripts, Durable Object namespaces, Containers, the registry, and Secrets Store. Its policy grants Workers Scripts, Containers (Cloudchamber), Secrets Store, Workers Observability, Workers Routes, and Account Settings Write. The account then held no script, no container application, and no store. Nothing is deployed yet: the hosted rows wait for the lead's merge to main.

## Status

| Row | In memory | Hosted |
| --- | --- | --- |
| BF-60 | **Pass** (`workers/runners/tests/bf60.rs`) | **Pass**, run 1966335 on F80's alarm start (`graft-runners` from main `a73871ce`, a cold burst after 8 idle hours): 50 of 50 jobs `Success`, at most 20 at once, nothing left after 10 minutes, no runner deleted while holding a job, no delivery pass cut. Run 1966284 was met at 90% (49 of 50, F79); run 1966182 passed on the sweep alone; run 1966233 broke the delete invariant (F78) |
| BM-4 | (M4-WP3's matrix with an unmodified runner) | **Pass**, run 1900546, site a down mid-run (site days 44 and 45) |
| BF-54 | (M4-WP3's broker cap) | **Pass**, run 3 on site day 47 and edge day 21 (main `1d90863a`, F77's fix): all 512 jobs succeeded, job start p99 16.6 s after the run committed (gate 35 s), max 19.2 s, none unstarted after 60 s, 0 `RunnerSessionInvalid`, re-polls within 30 s. Runs 1 and 2 failed: a lost `Repo`-scope match waited out its lease (F76), then the acquire path (F77) |

## What is built

- **The install side** (`crates/actions/src/hosted.rs`, additive calls in `crates/actions/src/shard.rs`):
  - The hosted label table, which is SHARED's.
  - `decide`'s rule for a hosted `PutRunner` (`admit`): the runner is ephemeral, `Owner`-scoped, and carries exactly one row's labels. Past `HOSTED_CAP` (20) hosted runners it is refused `TooMany`, and the count includes the entry's earlier commands.
  - The rule for a hosted `DeleteRunner` (`may_delete`): a delete by anyone but the runner itself is refused while the runner holds a job.
  - `ShardSummary` and `Broker::demand` (`demand`, `HostedDemand`, `LabelDemand`), with `Seen` in the broker object's storage (see "The smoke run").
- **The site** (`workers/site/src/actions.rs`, `broker.rs`):
  - `/read {"hosted": true}` answers a shard's summary.
  - `/runsvc` adds two ops. `register_hosted` answers 201, or 409 at the cap. `delete_hosted` answers 204, or 422 while the runner holds a job.
  - `RunnerBroker` adds `/demand`. It reads all 16 of the owner's shards at once, and any shard that fails to answer fails the read with 503.
- **The edge** (`workers/edge/src/hosted.rs`):
  - `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand`.
  - `POST .../hosted/runners` `{label, key, name}`, which answers the runner, its client id, and the URLs the JIT config needs.
  - `DELETE .../hosted/runners/{id}`.
  - All three need an App installation token on the owner with `organization_self_hosted_runners` write. `workers/edge/src/actions.rs` only made three helpers `pub(crate)`.
- **`graft-runners`** (`workers/runners`):
  - `control::pass` is one pass. It reads every owner's demand. For each stale runner it deletes the registration, then destroys the Sandbox only if the delete committed. It then registers and starts one runner per `start`, until the cap's 409 or `RUNNER_VCPU_STOP_PCT` of the account's vCPU, counting the runners already registered.
  - `worker.rs` is the Workers host. It signs the App JWT with `HOSTED_RUNNERS_APP_KEY` (Secrets Store) and gets an installation token per owner from the front. WebCrypto makes the runner's RSA-2048 key, and `runsvc::jit_config` writes the JIT config. Each runner gets one Sandbox, `runner-<id>`, on `SANDBOX_STD3` or `SANDBOX_STD4`, called over Durable Object RPC the way `getSandbox` does, with `keepAlive`. The Sandbox runs `run.sh --jitconfig "$JIT"; kill -TERM 1`, so the runner's exit stops it.
  - Triggers: an HMAC-verified `workflow_job` webhook runs a pass for its owner after answering 202, and the 5-minute cron runs a pass for every owner.
  - `entry.mjs` exports the SDK's `Sandbox` class twice, one class per instance type. `worker-build --release` builds `index.js` with `fetch` and `scheduled`, and esbuild bundles `entry.mjs` against `@cloudflare/sandbox` 0.12.14.
  - `deploy.mjs` writes the wrangler config. It has been run only with `--config-only`.
- **The runner image** (`containers/runner/Dockerfile`): the `cloudflare/sandbox` 0.12.14 base plus `actions/runner` 2.338.2, checked against its release SHA-256. It builds in the orb: 2.04 GB, with `Runner.Listener --version` answering 2.338.2 and `/container-server/sandbox` present. `tests/m4/wp4/image.sh <tag>` builds it and pushes it to the runner account's registry, then prints the digest-pinned name. The orb's user is not in the docker group, but passwordless `sudo` reaches the daemon, so the script runs `sudo docker build` and gives wrangler's push a `WRANGLER_DOCKER_BIN` wrapper around `sudo docker`. `wrangler containers build` cannot run here, because it passes `--load` and the engine has no buildx. This is the same `docker build` and `wrangler containers push` that `tests/am1/hotshards/image.sh` used for the git image.
- **The bundle.** `node workers/runners/deploy.mjs <runners.json> --config-only`, then `wrangler deploy -c <config> --dry-run --containers-rollout=none --outdir <dir>`, writes `entry.js` and one `.wasm` module, about 1.2 MiB. The Sandbox SDK imports `node:path/posix`, so both deploy paths set `nodejs_compat`, as the SDK's templates do.
- **CLI** (`cli/src/runners.rs`). Both commands use `CLOUDFLARE_RUNNER_API_TOKEN`, so the install's deploy token never reaches the runner account.
  - `graftyard runners init --account $R` writes `HostedRunnersAppKey` to the runner account's Secrets Store `graft-runners`. It uploads `graft-runners` with its bindings, its `v1` migration, the 5-minute sweep, and the workers.dev route for the App's webhook. Then it creates the `standard-3` and `standard-4` container applications from the image's digest. A rerun creates nothing that already exists.
  - `graftyard runners upgrade` deploys only a `release.json` that verifies under the release key (Ed25519 over the raw record, as `ReleaseUploaded.record_sig`) and lists every bundle file's SHA-256 and the image digest. Its first run uploads a version tagged with the release id and sends it 10%. A run at least a day later sends it 100% and rolls both container applications to the image (DESIGN, "Failure modes", `actions state` [guess]).
  - Gap: a container application holds one image, so the 10% half runs the old image. The release registry's record has no image list yet (SHARED, "The registry"); `images` is this command's field.
  - Stub tests, 5: init writes the key only to Secrets Store and creates both instance types on their namespaces; a tag-pinned image is refused; upgrade refuses a wrong key, a changed module, and another image; the canary stays at 10% until a day has passed.
- `HOSTED_OWNERS` (owner to installation id) stands in for listing the App's installations, which the install does not serve yet.

## BF-60 in memory: `workers/runners/tests/bf60.rs`

The owner's registration shard is three real `Member<ActionsCmd>`s, so the cap is `decide`'s. Demand is `hosted::summary` and `hosted::demand` over the shard's fold with one broker's `Seen`, a registration commits `PutRunner`, and a delete commits `DeleteRunner` as the operator. `graft-runners` is `control::pass`, the code the Worker runs. The Sandboxes are a fake host:

- Each simulated runner boots in 20 s, opens its session, takes the oldest queued job its labels fit, runs it for 1 to 8 minutes, finishes, deletes its own registration, and exits.
- Every 7th runner hangs after its job, and every 11th exits without deleting its registration.
- Every call a pass makes yields once. While a pass is suspended, the other passes run and every booted runner polls, so the passes interleave at each call.
- Three passes start at the queueing (the webhook and two sweeps). After that, one pass runs per `completed` webhook and two per 5-minute sweep.

Result: 50 jobs, every one `Success`. The last finished 1,020 s after queueing. 50 Sandboxes started. At most 20 runners were registered and 20 Sandboxes ran at once. The cap refused 29 registrations from overlapping passes. 10 minutes after the last job, no Sandbox and no hosted registration was left [measured, in memory].

Two more tests in the same file:

- A runner that the demand read listed stale but that took a job before its delete keeps both the job and its Sandbox.
- The cap holds inside one entry, and `decide` refuses a `Repo`-scope runner and a runner with a partial row as hosted registrations.

## The smoke run on graft-m4a

Repository `m3org/wp4-smoke`, run 1900544, one `ubuntu-latest` job (`tests/m4/wp4/hosted/smoke.yml`), dispatched at about 16:37Z after site day 42, edge day 20, and `graftyard runners init` (`tests/am1/results/install-m4a.md`). It found two bugs.

1. **The Sandbox did not start.** The 16:40:51Z pass registered runner 1704299, then `setSandboxName` failed: `TypeError: The RPC receiver does not implement the method "apply"`. `call_js` in `workers/runners/src/worker.rs` called the RPC stub's method with `Function::apply`, which the stub proxies as an RPC method named `apply`; it now uses `Reflect::apply`. Redeployed with a second `runners init` at 16:43:46Z (`tests/m4/raw/wp4/runners-init.log`).
2. **The runner that never started never went stale.** 1704299 had no Sandbox and never polled. Demand listed it idle against the one queued job, so every pass after 16:45Z answered `start []` and started nothing. It should have gone stale `HOSTED_IDLE` (600 s) after the broker first saw it, but at 17:13:52Z it still had not, with demand read every 5 minutes by the sweep and every 34 s by a probe (`tests/m4/raw/wp4/smoke-demand-34s.log`). With the probe reading every 6 to 11 s from 17:22:37Z, it went stale at 17:32:42Z, 10 min 5 s later (`smoke-demand-8s.log`). So the broker object left memory between reads 34 s apart and lost its in-memory `Seen`, and each read saw the runner afresh [inference from the two cadences; Cloudflare documents hibernation after 10 s idle, Durable Objects, "Lifecycle of a Durable Object" [doc]]. The fix keeps `Seen` as one JSON value in the object's storage (`workers/site/src/broker.rs`, `SEEN_KEY`). The test is `hosted::tests::seen_ages_runners_across_stored_reads`, and plant 21 drops `first` from the stored value. The fix is site code and ships on a site day.

Then the 17:35:50Z sweep deleted 1704299, destroyed its Sandbox, registered 1704300, and started its Sandbox. The job completed `Success` at 17:36:22Z, 32 s after the sweep. At 17:38Z both Sandboxes, `runner-1704299` and `runner-1704300`, were `inactive` in `graft-runners-sandboxstd3` (1704300's since 17:36:33Z, 11 s after its job), and demand answered no runners and no queue.

## BF-60 hosted: `tests/m4/wp4/bf60.mjs`

50 `ubuntu-latest` jobs (`tests/m4/wp4/hosted/bf60.yml`, a 10 × 5 matrix of 61 to 480 s `sleep`s) on `m3org/wp4-bf60` (`repo:65562`), with `graft-runners` on its 5-minute sweep alone: the install delivers no `workflow_job` to Apps yet (`decisions/OPEN-QUESTIONS.md`, R24), and the driver forges none. Every sweep is a Cron Trigger of the one deployed script, so two sweeps overlap whenever one outlives five minutes, as the 19:00:35Z sweep did (233 s) and the 18:40:35Z one (336 s). The driver samples the runner account's Sandbox instances every 10 s and records each job's queue-to-start time.

| | Run 1966182, **the row** | Run 1966080, evidence only |
| --- | --- | --- |
| Install | site day 43, main `c7970488`: the broker's `Seen` in storage | site day 42, `aa4a9d9c`: `Seen` in memory |
| Demand reads by the driver | every 300 s, as often as the sweep, so the broker can leave memory between reads | every 10 s, which keeps the broker in memory |
| Every job `Success` | 50 of 50 | 50 of 50 |
| Hosted registrations at once, max | 16 (8 samples, one per demand read) | 18 (every 10 s) |
| Running Sandboxes at once, max | 18 | 18 |
| Last job after dispatch | 1,680 s | 1,700 s |
| Queue to start, p50 / p90 / max | 588 / 1,170 / 1,179 s | 593 / 1,183 / 1,207 s |
| 10 minutes after the last job | 0 running Sandboxes (122 `inactive`), no hosted registration | 0 running (52 `inactive`), none |
| Sweeps that started runners | 19:04:31Z 20, 19:07:20Z 12, 19:11:51Z 13, 19:16:00Z 5 | 18:09:17Z 20, 18:11:51Z 12, 18:16:20Z 12, 18:21:16Z 6 |

**Pass** on run 1966182 [measured, `tests/m4/raw/wp4/bf60-rerun/`]. Run 1966080 passed the same checks on the old code with a warm broker, so it is evidence only (`tests/m4/raw/wp4/bf60-run1966080/`; its `driver.log` was overwritten when the supervised service restarted the driver; `result.json` and `samples.jsonl` are intact).

- **What the sweep costs.** The first runners start at the first sweep after the dispatch, 3 to 8 minutes; with the cap at 20, the 30 later jobs wait for a sweep after a slot frees, so queue-to-start is about 10 minutes at p50 and 20 at p90. A `workflow_job` webhook pass would start the first 20 within seconds and refill a freed slot at the job's `completed` delivery. That is R24's gap, measured.
- **A sweep that outlives five minutes.** A pass registers and starts runners one at a time; the start waits for the Sandbox to be placed, and the Sandbox SDK answered `There is no container instance that can be provided to this Durable Object, try again later` for up to a minute while new instances were placed (the 20-runner sweeps took 221 to 336 s). The cap held, because the two overlapping sweeps share `decide`'s count.
- **No Sandbox outlived its runner.** Each runner's exit stopped its Sandbox (`kill -TERM 1` after `run.sh`), within about 10 s of its job's end.

## BM-4 hosted: `tests/m4/wp4/bm4.mjs`

A 2 × 4 matrix of `ubuntu-latest` legs (`tests/m4/wp4/hosted/bm4.yml`: a step, a 300 s step, a step) on `m3org/wp4-bm4`, jobs on `actions:7100001.7`, unmodified runner 2.338.2 in `graft-runners`' Sandboxes, sweep only. Once all 8 legs were running (7 `InProgress`, 1 `Assigned`), at 19:43:00Z, the driver took the jobs' shard's leader site, site a (also the registration shard's leader), down through site day 44 (`tests/m4/wp3/bf55-kill.sh down 0`, version `91c83eea`). Site a's member stopped answering as leader at 19:48:09Z, the running object having kept its old version for five minutes as in BF-55; site b took over both shards (term 3 on `.7`, term 7 on `.0`).

Result [measured, `tests/m4/raw/wp4/bm4/result.json`]: every leg `Success`, at version 4 (matched, acquired, and finished once, no lease requeue); the run `success`; each job held by exactly one runner (1704421 to 1704428). Legs a1 to b1 finished at 19:46:09 to 19:46:15Z while the old version still served; b2 to b4 finished at 19:47:21Z, 19:47:28Z and 19:48:28Z, the last after the old leader died, so its renewals and `completejob` crossed the leader change. **Pass.**

The driver's restore was due 3 minutes after the death, but the legs finished first and the driver exited, so site a came back by hand through site day 45 at 19:52:32Z to 19:52:42Z (`bf55-kill.sh up 0`, version `a5426497`), 9 minutes 42 s after the kill, inside the announced 10. Its members rejoined as followers by 19:58:28Z. The driver now waits for its restore before it exits.

## BF-54 hosted: `tests/m4/wp4/bf54.mjs`

2,500 unmodified `actions/runner` 2.338.2 listeners, all ephemeral `Repo`-scope runners of `m3org/wp4-bf54` (`repo:65565`, shard `actions:7100001.11`, labels `self-hosted, linux, x64, bf54`), 100 to each of 25 `standard-4` Sandboxes from the scratch Worker `graft-runners-bf54` (`tests/m4/wp4/bf54-box/`) in the runner account. These are self-hosted-style runners on Sandboxes, not `graft-runners`' hosted ones. The run was on site day 43's install (main `c7970488`), with `BROKER_HELD_MAX` 2,000 on one broker. Raw data: `tests/m4/raw/wp4/bf54/` (the listeners' `_diag` lines in `diag.tar.gz`; `jobs.jsonl.gz` and `samples.jsonl.gz` gzipped). `jobs.jsonl` is a second sampler (`tests/m4/wp4/bf54-jobs.mjs`) that recorded every job's state about every 90 s from 21:22Z. It was started because the driver counts a job as started once it is `Assigned`.

| Phase | Time (UTC) | Measured |
| --- | --- | --- |
| Keys made in parallel | 20:42:38 to 20:50:25 | 2,500 in 467 s |
| Registered through the admin route, 10 a second | to 20:54:36 | 2,500 in 251 s, 0 failed: the shard took 10 registrations a second with no error |
| 25 Sandboxes, listeners started 2.5 s apart in each | 20:54:36 to 21:03:13 | all 2,500 listening, 0 exits. A box was at 2,389 MiB of 12,215 with 100 idle listeners (about 22 MiB of box `used` per listener; RSS was 88 MB in the pilot, mostly shared pages) |
| Hold | to 21:08:48 | 503s from 21:00Z, once about 2,000 were listening |
| 32 runs of 16 20-s jobs, one run every 15 s | 21:08:48 to 21:39:22 | 512 jobs, 510 `Success` and 2 `Failure`. At most 27 workers at once; busiest box 3,406 MiB |

| Check | Result |
| --- | --- |
| Polls past the cap get 503 | **Pass**: 2,495 of 2,500 runners got at least one 503, as held slots turned over |
| Those runners re-poll within 30 s | **Pass** on the runner's own wait (`tests/m4/wp4/bf54-repoll.mjs`): in a retry chain it waits the edge's `Retry-After: 15`, and after a chain's last attempt it backs off 0 to 15 s (11,893 back-offs, max 14.998 s). Of 19,472 in-chain retries, 777 took over 30 s from one failed attempt to the next, because the install answered late (p99 79 s, max 114 s) |
| Sessions valid | **Pass**: 0 `RunnerSessionInvalid`, 0 recreated sessions, 0 listener restarts, 0 runners exited without a job |
| Job start p99 at most 35 s for 503 runners | **Fail**. The driver reports p99 60 s, but it counts `Assigned` as started. Of 310 matches, 277 were still `Assigned` 45 s or more after the match (p50 368 s, max 572 s) |
| No job unstarted after 60 s | **Fail**: 70 jobs started more than 60 s after their match. 18 of them were then taken by another runner, and none by its first runner. The other 52 completed between two samples |

**What failed** [measured, with the cause an inference]. A match committed but never reached its runner:

- In 68 of the 277 stuck matches, the matched runner logged its poll timing out at 100 s within 105 s after the match (`The HTTP request timed out after 00:01:40`).
- The broker holds a poll for at most `POLL_HOLD` (50 s), so these polls were answered more than 50 s late. That fits a leader slowed by about 2,000 held polls, each asking it every `ASK_EVERY` (5 s), about 400 asks a second.
- A `Repo`-scope runner whose match answer is lost never gets that job again. The shard's `match` answers a runner that holds a job with nothing (`workers/site/src/actions.rs`, `"match"`). The resend after `RESEND_AFTER` exists only on the `Owner`-scope path (`graftyard_actions::broker::Taken`, BF-55).
- So the job sits `Assigned` until its 10-minute lease ends, and `ExtendLeases` pushes that end back further. The stuck jobs' lease ends moved later from sample to sample while the runner stayed the same, which only a recorded stall does.
- The 2 `Failure` jobs: job 1182509 went `InProgress` under runner `bf54-0002`, whose listener never ran a job, and failed at its lease end. Its acquire committed, but the answer was lost the same way. Job 1182240 completed `Failure` before the sampler started; nothing records why.

This is the `Repo`-scope half of BF-55's lost-answer finding, which was fixed only for `Owner` scope. It is `decisions/OPEN-QUESTIONS.md` F76. F26, the runner's back-off and session on a 503, is answered: it backs off 15 s and keeps its session.

**Teardowns, BF-54.**

- 21:45:25Z: services `bf54` and `bf54-jobs` stopped.
- 21:45:34Z to 21:45:51Z: in each of the 25 boxes, every listener was sent SIGINT (0 left), then the box was destroyed (`{"ok":true}` for each).
- 21:46:10Z: the containers API listed 0 non-`inactive` instances in every runner-account application.
- 21:46:40Z: scratch Worker `graft-runners-bf54` deleted.
- 21:46:50Z: its container application `a03d94f1-…` deleted.
- Left in the runner account: the `graft-runners` script and its two Sandbox applications, with 0 running instances.

**Teardowns, m4a git shards.** 7 shards were still running from BF-60's, BM-4's, and BF-54's clones:

- Site b: `git#1#net:65563`.
- Site a: `git#0#net:` 65536, 65561, 65562, 65563, 65564, and 65565.

They were stopped at 21:59:16Z to 21:59:20Z through GitShard `/teardown` (all 200 `torn_down`) from a scratch Worker `graft-m4a-shardkill` bound to the three site scripts' `GitShard` classes. The Worker was then deleted, and at 21:59:49Z the API listed 0 running on sites a, b, and c.

## F76: the lost `Repo`-scope matches, measured

Branch `f76-repo-resend`. Measured from the BF-54 run's Workers Logs (the telemetry API's exact count view, by second) and `durableObjectsPeriodicGroups`. `tests/m4/raw/wp4/f76/trace.md` keeps the queries' output.

**Why a held 50 s poll was answered after 100 s.** The broker died. It was not a slow leader.

- From 21:09:20Z, the first minute with jobs, the broker `broker:actions:7100001.11` on site a ended `exceededMemory` 14 times in 25 minutes. Each death failed 618 to 1,836 held polls in the same second. Sites b and c had 7 more as the edge moved polls there.
- Its peak memory was 47 to 109 MiB while it held 2,000 polls with no jobs (20:58Z to 21:08Z, no deaths). With jobs it was 256 to 270 MiB.
- A death drops every held poll, including any whose match the leader had just committed. The edge's `GET broker/message` then waited out the runner's 100 s timeout: 84 to 346 edge polls a minute ended at 100 s from 21:10Z, against 0 before 21:09Z.
- **The leader's load did not cause it.**
  - The registration shard's leader member used 7 to 18 s of CPU a minute, 12 to 30% of one thread, the same before and after the jobs.
  - `/runsvc` asks reached the leader at 17,425 to 26,050 a minute during the hold, about 290 to 430 a second, which is 2,000 polls asking every `ASK_EVERY` (5 s).
  - Before the jobs it answered them at p50 1 to 17 ms and p99 under 600 ms. Once jobs ran, it answered at p50 84 to 1,298 ms and p99 2.0 to 3.0 s, while it also committed matches, acquires, renewals, and finishes. Its leader stepped down 8 times between 21:10Z and 21:35Z (`StepDown { CommitErrors }`, 6 on site a and 2 on site b).
- **What filled the memory is an inference.** Every hint after a queue move woke all 2,000 held polls at once, and each ask holds its request, the lookup's future, and its answer. With answers at 0.3 to 1.3 s instead of a few ms, about ten times as many asks were in flight at each wake. The trace has no per-object heap breakdown.

**The fixes, one rule each:**

- **Resend in both scopes.** The `Repo`-scope `match` now answers with `ActionsFold::repo_pick`, which is `Taken::pick`, BF-55's `Owner`-scope rule, over the one shard. A job its runner holds `Assigned` past `RESEND_AFTER` is handed to it again, a runner holding any other job gets nothing, and an ephemeral runner takes one job. `MATCHED` is replaced by a per-shard `Taken`.
- **At most `ASKS_MAX` (64) asks in flight per broker.** A poll past them waits in line.
- **Test.** `a_repo_scope_runner_whose_match_answer_was_lost_gets_the_job_on_a_later_poll` (`crates/actions/tests/gates.rs`) passes. With main's rule swapped into `repo_pick`, it fails with "no answer after 2 minutes: Busy".
- **Plants** 22 to 24 are caught.
- The BF-54 rerun on this branch's site day decides whether `ASKS_MAX` is enough.

## BF-54 run 2, on F76's fix (2026-11-16)

Site day 46 put main `cab0b332` on all three m4a sites, with git image `m4a46` (`tests/am1/results/install-m4a.md`). Then the BF-54 driver ran unchanged, with the same shape as run 1: 2,500 ephemeral `Repo`-scope listeners of `m3org/wp4-bf54` on 25 `standard-4` Sandboxes, and 32 runs of 16 20-s jobs. Raw data is in `tests/m4/raw/wp4/bf54-run2/`:

- `diag.tar.gz`, `jobs.jsonl.gz`, and `samples.jsonl.gz` are as in run 1.
- `jobs-fast.jsonl.gz` is a second sampler of the run's own runs, every 60 to 90 s.
- `rest-jobs.jsonl` holds each job's REST `started_at`, the runner's first step.
- `dispatch-times.txt` holds each dispatch request's start time and wall time, from the edge's Workers Logs.
- `run-created.txt` holds each run's `created_at`.

| Phase | Time (UTC) | Measured |
| --- | --- | --- |
| Keys, then registration at 10 a second | 00:27:56 to 00:39:32 | 2,500 in 444 s, then 2,500 in 251 s, 0 failed |
| 25 Sandboxes, listeners 2.5 s apart | 00:39:32 to 00:48:45 | all 2,500 listening, 0 exits |
| Hold | to 00:53:52 | 503s once about 2,000 listened |
| 32 runs, dispatched about every 20 s | 00:53:58 to 01:06:42 | 512 jobs: 511 `Success`, 1 `Failure` (below). At most 33 workers at once; busiest box 3,001 MiB |

**Job start.** A job's start is its first step's `started_at`, the runner's own clock, and none is missing except job 1185331's. It is timed from the run's dispatch.

- The jobs sampler never once saw a job `Queued`, because every job was matched within one round of samples. Each run's `created_at` came out later than its jobs' matches. So neither marks the queue time.
- The edge's Workers Logs do. A dispatch request's event timestamp is when it answered 201, so `RunCreated` had committed by then.
- It is the end, not the start: a probe request's event time was 01:51:16.273Z with wall 6,174 ms, and the response's `Date` header read 01:51:16. The orb's own clock ran about 3.5 s behind, so no orb time is used here.
- The request's wall time was 2.1 to 6.8 s (p50 2.7 s), so the job was queued at most that long before the event time.

| Measure | Result | Gate |
| --- | --- | --- |
| Start after the dispatch answered 201, so after `RunCreated` committed (511 jobs) | p50 12.0 s, p95 26.6 s, **p99 39.0 s**, max 62.0 s (job 1185242); 8 over 35 s, 1 over 60 s | p99 at most 35 s: **fails** |
| Start after the dispatch request arrived (event time minus wall time) | p50 14.8 s, **p99 43.4 s**, max 66.2 s; 13 over 35 s, 1 over 60 s | — |
| Runners that got a 503 | 2,485 of 2,500; 505 of the 512 jobs ran on one of them | — |
| Re-poll within 30 s on the runner's own wait | **Pass**: 2,636 back-offs, max 14.999 s. Of 12,815 in-chain retries, one took 35 s from one failed attempt to the next (run 1: 777, max 114 s) | — |
| Sessions | **Pass**: 0 `RunnerSessionInvalid`, 0 listener restarts, 0 runners exited without a job | — |
| A job unstarted after 60 s | **Fails**: job 1185331 (below) | — |

The fail line reads "a job unstarted after 60 s". Two jobs cross it. Job 1185242 started 62.0 s after its run committed, after 3 acquire retries (below). Job 1185331 never ran a step at all.

**The three items the lead asked for:**

1. **Matches `Assigned` 45 s or more: 0** in the samples (run 1: 277 of 310). **Jobs started more than 60 s after their match: 1, job 1185242** (and job 1185331, which never started). In run 1 it was 70.
   - The samples caught 20 matches with their time (`lease_until` minus `JOB_LEASE`). The latest was sampled 15.6 s after its match.
   - The samples also caught 159 acquires with their time. Their p99 is 37.1 s after the dispatch answered, and their max is job 1185242's 60.2 s.
   - Every job that ran except 1185242 started within 60 s of its run's commit, so within 60 s of its match. Job 1185242's match is not sampled. Its run committed by 00:54:37.0Z, and it was matched to `bf54-2427`. That runner's `acquirejob` got 503 at 00:54:44Z, 00:55:00Z, and 00:55:16Z by its own clock, and the edge logged 503s at 00:55:01Z and 00:55:16Z. The fourth try was acquired 60.2 s after the commit.
2. **Resends by scope: `Repo` 0 seen, `Owner` 0.** Every runner is `Repo`-scope. **No job got two runners.**
   - No sample shows a job `Assigned` more than 30 s after its match, and no job shows a second runner or a second match.
   - 511 runners ran one job each, and 1,989 ran none, as the driver's per-runner count shows.
   - A resend is not logged, so the samples could miss one. Only 11 jobs started, or were acquired, more than 30 s after their run committed, which a resend needs. For job 1185242 those 30 s went to 503 retries of one acquire, not a lost match.
3. **The broker. `exceededMemory`: 0** on sites a, b, and c, 00:39Z to 01:08Z (run 1: 14 on site a, 21 across the three).
   - Peak memory per minute was 13 to 69 MiB (run 1: 256 to 270 MiB), by `durableObjectsPeriodicGroups` on `broker:actions:7100001.11`.
   - All `broker/poll` calls ran on site a. No edge `broker/message` took 100 s or longer; its p99 was 52 to 61 s a minute.
   - **Asks in flight against `ASKS_MAX` (64), and how long a poll waited for a slot: not measured.** The broker logs neither. The edge's `broker/message` p50 stayed at 50.3 to 51.5 s, the `POLL_HOLD` of 50 s plus about 1 s, every minute of the run. Before the fix, during run 1's job minutes, it was 1.3 to 96 s. So no poll waited long for a slot.
   - **64 did not throttle the gate.** The time that decides it is the acquire, below, not the match.

**What failed.**

- **Job 1185331** (run 1185326, committed by 00:56:26.8Z) was matched to runner `bf54-1017` and acquired at 00:56:41.2Z, 14.4 s later: it went `InProgress`. No step ever started. The driver keeps only 503 and broker lines of each runner's log, so the log cannot show whether this runner's `acquirejob` got the 200 or a 409. Its diag log has nothing after 01:01:05Z but broker polls. At its lease end, 01:06:41.2Z, the job failed `Failure`: `LeaseExpired` on an `InProgress` job. This is run 1's job 1182509 again: an acquire that commits but whose answer the runner never gets. In the edge's logs:
  - Between 00:56:35Z and 00:56:54Z, `run/acquirejob` answered 200 in 2.8 to 21.9 s.
  - One call answered 409 after 24.0 s, at 00:56:52.8Z: the only 409 of the run. Its body is not logged. A 409 from `acquire` is a refused `Acquired` (`workers/site/src/actions.rs`), and a second acquire of a job already `InProgress` gets exactly that.
  - The likeliest reading [inference]: the runner gave up on a slow first `acquirejob`, whose commit landed anyway, retried, and got the 409. The runner then drops the job, so it sits `InProgress` with nobody until its 10-minute lease.
  - The REST jobs list shows job 1185331 with no steps, `started_at` equal to its `created_at`, and `completed_at` 01:06:41Z.
- **74 `run/acquirejob` 503s.** The edge answered "acquirejob actions:7100001.11: secrets: the repository's rows did not answer" (`workers/edge/src/actions.rs`, `job_config`). It reads the repository's secret rows before the acquire, and on a failed read answers 503 without committing anything. The runner retries every 15 s, up to 4 times. `bf54-2427` retried 3 times, 3 others twice, and 69 runners logged at least one failed `acquirejob`. These retries, not the broker, put job 1185242 past 60 s and the p99 past 35 s: 15 s per retry on a job that otherwise starts at about 12 s (p50). The edge's `acquirejob` wall was p50 3.8 to 8.6 s and p99 9.6 to 28.9 s a minute.
- **What this settles, and what it opens.** F76 is settled. Lost `Repo`-scope matches are gone, with 0 stuck and no 100 s polls, and the broker no longer dies, with 0 `exceededMemory` and 69 MiB peak. What is left is the acquire path: a slow secrets read before the acquire, and an acquire answer lost after its commit. That is `decisions/OPEN-QUESTIONS.md` F77.

**Teardowns, BF-54 run 2.**

- 01:12:07Z: services `bf54run2` and `bf54jobs2` stopped. `bf54jobsfast` had already stopped, at 01:07Z.
- 01:12:31Z to 01:12:33Z: in each of the 25 boxes, every listener got SIGINT (0 left), then the box was destroyed, `{"ok":true}` for each.
- 01:13:19Z: application `a032924c-…` listed 25 `inactive` and nothing else.
- 01:13:31Z: the scratch Worker `graft-runners-bf54` was deleted.
- 01:13:32Z: its container application `a032924c-…` was deleted (`success: true`).
- Left in the runner account: the `graft-runners` script and its two Sandbox applications, with 0 running instances.
- m4a git shards: site b's `git#1#net:65565` and site a's `git#0#net:65565` and `git#0#net:65536`, started by site day 46's verify clone and the run, were stopped at 01:14:24Z through GitShard `/teardown`, all 200 `torn_down`. The calls came from a scratch Worker `graft-m4a-shardkill`, deleted at 01:14:50Z. At 01:14:38Z the API listed 0 running on sites a, b, and c.
- The symlink `tests/m0/.m0-token` was removed.

## BF-54 run 3, on F77's fix (2026-11-16)

Site day 47 and edge day 21 put main `1d90863a` on m4a, with git image `m4a47` (`tests/am1/results/install-m4a.md`). The driver ran unchanged, with run 2's shape: 2,500 ephemeral `Repo`-scope listeners of `m3org/wp4-bf54` on 25 `standard-4` Sandboxes, and 32 runs of 16 20-s jobs. The orb restarted at 04:20Z, 9 minutes after the driver's first start, and took the driver and the box token with it before any key or registration; the box Worker got a new token and the driver started again at 04:24:32Z. Raw data is in `tests/m4/raw/wp4/bf54-run3/`, as in run 2; `acquires.txt` is the edge's acquire path (`tests/m4/raw/wp4/f77/acquires.mjs`), and the start times are from `tests/m4/raw/wp4/f77/start.py`, which reads run 2's files the same way.

| Phase | Time (UTC) | Measured |
| --- | --- | --- |
| Keys, then registration at 10 a second | 04:24:41 to 04:36:21 | 2,500 in 448 s, then 2,500 in 251 s, 0 failed |
| 25 Sandboxes, listeners 2.5 s apart | 04:36:21 to 04:44:18 | all 2,500 listening, 0 exits |
| Hold | to 04:49:23 | 503s once about 2,000 listened; 2,490 runners saw one |
| 32 runs, dispatched about every 17 s | 04:49:24 to 04:58:35 | 512 jobs, all `Success`. Busiest box 2,858 MiB |

| Measure | Run 3 | Run 2 | Gate |
| --- | --- | --- | --- |
| Start after the dispatch answered 201 (`RunCreated` committed) | p50 8.7 s, p95 14.2 s, **p99 16.6 s**, max 19.2 s (job 1188709); 0 over 35 s, 0 over 60 s (512 jobs) | p50 12.0 s, p99 39.0 s, max 62.0 s; 8 over 35 s, 1 over 60 s (511) | p99 at most 35 s: **passes** |
| Start after the dispatch request arrived | p50 11.3 s, p99 19.0 s, max 22.1 s | p50 14.8 s, p99 43.4 s, max 66.2 s | — |
| The driver's own start (its job sampler) for the 511 jobs on runners that got a 503 | p50 11.2 s, p99 19.1 s | — | — |
| A job unstarted after 60 s | **0** | 2 (1185242 at 62 s; 1185331 never ran) | fail line |
| `acquirejob` answers (edge Workers Logs, 04:44Z to 05:03:30Z) | **512, all 200**; wall p50 3.7 s, p99 10.5 s, max 13.0 s | 586 tries: 74 503, one 409 | — |
| `job_config` failures ("secrets: the repository's rows did not answer") | **0** | 74 | expected 0 |
| Repeated acquires answered, and 409s | **0 repeated, 0 409**: 512 acquires for 512 jobs, so no runner asked twice and F77 (b) was not exercised | 1 409 (job 1185331) | — |
| Jobs with two runners | 0 (job sampler); 512 runners ran one job each, 1,988 none | 0 | — |
| Re-poll within 30 s on the runner's own wait | **Pass**: 2,294 back-offs, max 14.992 s; in-chain retries max 21 s, none over 30 s (`repoll.json`) | Pass | — |
| Sessions | **Pass**: 0 `RunnerSessionInvalid`, 0 listener restarts, 0 runners exited without a job | Pass | — |
| Broker `exceededMemory`, sites a to c, 04:36Z to 05:04Z | 0 | 0 | — |

- **F77 (a) holds in production.** The narrow `actions` read answered every acquire: no `job_config` failure in 512, where run 2 had 74 in 586. The p99 fell from 39.0 to 16.6 s because no runner waited out a 15-s acquire retry.
- **F77 (b) is not exercised by this run.** No acquire was lost, so no runner retried one. It is covered by `crates/actions/tests/gates.rs` and plants 25 to 27, not by a hosted measurement.
- **Teardowns, BF-54 run 3.** 05:03:26Z: services `bf54run3` and `bf54jobs3` stopped. 05:03:41Z to 05:03:45Z: in each of the 25 boxes every listener got SIGINT (0 left), then the box was destroyed, `{\"ok\":true}` each (`teardown.log`). 05:04:21Z: application `a03fbdb5-…` listed 0 active. 05:04:37Z: the scratch Worker `graft-runners-bf54` deleted; 05:04:38Z: its application `a03fbdb5-…` deleted (`success: true`). Left in the runner account: `graft-runners` and its two Sandbox applications, 0 active. m4a git shards `git#0#net:65565`, `git#0#net:65536`, `git#1#net:65565`, and `git#1#net:65536`, started by the verify clone and the run, were stopped at 05:06:48Z to 05:06:50Z through GitShard `/teardown`, all 200 `torn_down`, from a scratch Worker `graft-m4a-shardkill`, deleted at 05:07:21Z; at 05:07:18Z the API listed 0 active on sites a, b, and c. The symlink `tests/m0/.m0-token` was removed.

## R24 hosted: `graft-runners` prompted by the App's `workflow_job` delivery (2026-11-16)

Site day 48 and edge day 22 put main `77c9fa23` on m4a, with 32.24 and 18.13 enabled and `GRAFTYARD_APP_HOOKS=1` (`tests/am1/results/install-m4a.md`). A new `WEBHOOK_SECRET` was generated in the orb with `secrets.token_hex(32)`. `graftyard runners init` set it on `graft-runners` and `app.mjs` sent it to `PATCH /app/hook/config` (200), from the same file, which was then deleted. The hosted-runners App now has `actions: read`, `events: [workflow_job]`, and its webhook at `graft-runners`. `GET /app/hook/config` shows `secret` as `********`. Run 1966233 ran `bf60.mjs` unchanged, with `DEMAND_EVERY_S=300` (raw data in `tests/m4/raw/wp4/r24-bf60/`; `prompted.jsonl` is `tests/m4/wp4/prompted.py` over the runner account's Workers Logs).

| | Run 1966233 (deliveries and sweep) | Run 1966182 (sweep alone) |
| --- | --- | --- |
| Passes that started runners | 24 `fetch` (App delivery) passes started 24 runners. 6 `scheduled` sweeps started 0 | sweeps only: 20, 12, 13, 5 |
| First runners running | 5 jobs started 46 s after the dispatch, before any sweep | at the first sweep, 3 to 8 min |
| Queue to start, p50 / p90 / max | **261 / 594 / 767 s** | 588 / 1,170 / 1,179 s |
| Last job after dispatch | 1,262 s | 1,680 s |
| Hosted registrations / running Sandboxes at once, max | 20 / 19 | 16 / 18 |
| 10 minutes after the last job | 0 running Sandboxes (182 `inactive`), no hosted registration | 0, none |
| Every job `Success` | **49 of 50**: job 1966235 `Failure` (below) | 50 of 50 |

**R24's proof holds:** every runner of the run was started by a `workflow_job` delivery, and none by the sweep [measured, `prompted.jsonl`]. **The run fails BF-60's "every job finishes" check** on one job, through a bug in the delete guard that this run found (F78).

- **Job 1966235 ("build (0, 1)").** Runner 1704433 acquired it at 18:52:41Z (`acquirejob` 200, `renewjob` 200, live log opened). At 18:52:43.988Z and 18:52:44.136Z, two delivery passes deleted the runner's registration (`DELETE .../hosted/runners/1704433`, both 204). At 18:52:44 to 18:52:45Z they destroyed its Sandbox (`sandbox.destroy success`). The job got no further renew and failed at its `JOB_LEASE` end, 19:02:41Z, 600 s after its acquire (a renew within `LEASE_REWRITE_UNDER` writes no new lease). It had no steps and no runner [measured, edge and runner-account Workers Logs, REST].
- **Why the delete was not refused.** `DELETE .../hosted/runners/{id}` commits `DeleteRunner` on `OWNER_SCOPE_SHARD` (shard 0), and `hosted::may_delete` checks only that shard's jobs. `m3org/wp4-bf60` (`repo:65562`) hashes to shard 5, so the guard could not see the job [measured: `actions_shard(65562)` is 5; code: `workers/edge/src/hosted.rs`, `crates/actions/src/hosted.rs`]. The in-memory BF-60 test puts its repository on shard 0, so it never exercised this.
- **Why the runner was listed `stale`: not settled.** It held a job, so a demand read that included shard 5's summary after the match should have kept it. One candidate is two overlapping `/demand` calls in one broker. A read whose shard reads began before the match would then load the `Seen` that a later read stored, with the runner marked as having held a job, and list it stale as "its job completed". Twenty deliveries reached the broker in under 30 s. This is an inference: no log line names the read that listed it.
- **Demand 503s.** The first 20 deliveries (18:52:14Z to 18:52:43Z) got 503 from `Broker::demand`: 15 on site b, then 5 on site a, all with outcome `ok`. That is the every-shard rule refusing a partial view. Which shard did not answer is not logged, so the cause is open; a broker cold after the site day is a guess.
- **Sandbox placement.** About 1,921 "no container instance that can be provided", 17 "too many containers per second", and 2 "Container failed to become ready" (163 s and 180 s). The pass that hit them registered a runner whose Sandbox did not start, and the cap answered `full` 24 times. So the 20 slots were seldom all running (17 to 19), which accounts for most of the p90.
- **One `access_tokens` 503**, "authz:7100003: every member asked shed the read: overloaded". The next delivery's pass got a token.
- **Teardowns.** Driver service `r24bf60` stopped after the run. The runner account listed 0 running Sandboxes (182 `inactive`) and the demand listed no hosted registration. m4a git shards `git#0#net:65562` and `git#1#net:65562`, started by the run's clones, were stopped at 19:48:11Z through GitShard `/teardown` (both 200 `torn_down`) from a scratch Worker `graft-m4a-shardkill`, deleted at 19:48:20Z. At 19:48:43Z the Containers API listed 0 running on sites a, b, and c.

## BF-60 rerun after F78 (2026-11-17)

Site day 49 and edge day 23 put main `0de37416` on m4a (F78's fix; lookup-share reverted), with day 48's flags, `GRAFTYARD_APP_HOOKS=1`, and edge day 22's `FORMATS_ENABLED` (`tests/am1/results/install-m4a.md`). The git shard image stays `m4a47`; `graft-runners` was not redeployed (no change under `workers/runners/src`). Run 1966284 ran `tests/m4/wp4/bf60.mjs` with `DEMAND_EVERY_S=300` from 04:36:15Z (`tests/m4/raw/wp4/f78-bf60/`). The lead granted the slot with a $2.50 budget and stops of $5 for graft-m4a plus the runner account, $170 for the account's UTC day, or a hosted job failing or past its lease; `watch.log` read `tests/fanout/acctcost.py` every 5 minutes.

| | Run 1966284 (F78's fix) | Run 1966233 (R24, before F78) |
| --- | --- | --- |
| Passes that started runners | 30 `fetch` (App delivery) passes started 30 runners; 5 `scheduled` sweeps started 0 | 24 `fetch`, 0 `scheduled` |
| Queue to start, p50 / p90 / max | **216 / 525 / 625 s** | 261 / 594 / 767 s |
| Last job after dispatch | 1,118 s | 1,262 s |
| Hosted registrations / running Sandboxes at once, max | 20 / 19 | 20 / 19 |
| Stale deletes / refused (`kept_busy`) | 3 (04:40:51Z, 04:42:06Z, 04:46:38Z, each 204, each after its runner's job completed) / 0 | 2 passes deleted a runner holding a job |
| 10 minutes after the last job | 0 running Sandboxes (233 `inactive`), no hosted registration | 0, none |
| Every job `Success` | **49 of 50**: job 1966290 `Failure` (below), not through a delete | 49 of 50, through the delete |

**BF-60 is met at 90%** (`FRAME.md`, "Operator decision, 2026-11-16, 90% gates and a daily spend limit"). At most 20 at once, every job finished, no Sandbox or registration was left, and no runner was deleted while it held a job, so the invariant holds. 49 of 50 jobs succeeded (98%). The one failure is a separate acquire bug, recorded as F79 in `decisions/OPEN-QUESTIONS.md`.

- **Job 1966290 ("build (1, 0)").** Runner `graftyard-hosted-9b3270ab` opened its session at 04:37:13Z and acquired the job at 04:37:17.505Z (`acquirejob` 200, 3,251 ms). The runner logged `##[error]Value cannot be null. (Parameter 'repoFullName')` at "Prepare workflow directory" and completed the job `Failure` at 04:37:20Z (job log via `GET /api/v3/repos/m3org/wp4-bf60/actions/jobs/1966290/logs`). The acquire answer's `github.repository` was empty. On the edge's `run/acquirejob`, `repository_of` reads the job row, the run, the repository's owner, and the owner's login and `Authz`; when any of them fails it returns `None`, and the edge goes on with an empty name and no secrets or variables instead of answering 503 (`workers/edge/src/actions.rs`, since 72a3753b). Which read failed is not logged [measured: no `console_error` on that path].
- **Recurring findings from run 1966233**, from the runner account's Workers Logs events view over 04:35Z to 05:05Z: 1 demand 503 (04:36:46Z; run 1966233: 20). About 1,066 "no container instance that can be provided" (about 1,921), 26 "too many containers per second" (17), and 1 "Container failed to become ready" (2), all from the Sandbox SDK. `access_tokens` answered 201 157 times and never 503 (run 1966233: 1).
- **Teardown.** The driver stopped on its own; service `f78bf60` and the `f78watch` stop watcher were stopped. The runner account listed 0 running Sandboxes (233 `inactive`). m4a's one running shard, `git#0#net:65562` (site a), was stopped at 05:07:27Z through GitShard `/teardown` (200 `torn_down`) from a scratch Worker `graft-m4a-shardkill`, deleted at 05:07:39Z; at 05:08:04Z the Containers API listed 0 running on sites a, b, and c. `/retire` is a `test-faults` member route and m4a's sites are product builds, so there is no test group to retire; the run's groups are m3org's Actions shards and `repo:65562`, which hold the install's own state.
- **Cost** (`tests/fanout/acctcost.py`). Before, at 04:35:27Z: the account $51.18 for the UTC day, graft-m4a $0.34, the runner account $0.00. After, at 05:08:39Z: from 04:30Z, graft-m4a $0.24 (site scripts $0.18, git shard $0.06) and the runner account $0.34 (Sandboxes $0.30): **$0.58** against the $2.50 estimate and the $5 stop; the account $61.83 for the day, under $170. Idle after the teardown, 05:08Z to 05:38Z: graft-m4a $0.04 (about $0.08 an hour, all site scripts) and the runner account $0.00; the account $72.16 for the day at 05:38Z.

## F79 hosted check after edge day 24 (2026-11-17)

Edge day 24 put main `c1d29fc0` (F79's fix) on `graft-m4a-edge` alone, with edge day 23's `FORMATS_ENABLED`, `RUN_SERVICE_PUBLIC`, and `OPS` binding (`tests/m4/raw/wp4/edgeday24.log`, version `5c5d21df`, 07:42:28Z, exit 0). No site script changed, so there was no site day. Smoke reads after: `/user`, wp4-bf60, j4 `/hooks`, `/meta`, and job 1966290's log 200; `/app/hook/config` and `run/acquirejob` 401 without auth.

`tests/m4/wp4/f79.mjs` set a repository secret on a new repository `m3org/wp4-f79` with `gh secret set` (the value lived only in a mode-600 file, deleted after) and dispatched `hosted/f79.yml`: 5 `ubuntu-latest` legs that print `$GITHUB_REPOSITORY` and the secret's SHA-256 prefix and length (`tests/m4/raw/wp4/f79/`). Run 2097152, from 07:44:49Z:

- **Pass, 5 of 5 legs.** Each job log reads `repo m3org/wp4-f79` and the secret's expected hash and length (28), and each job ended `Success`; the last finished 92 s after the dispatch. The edge answered `run/acquirejob` 200 five times and logged no `acquirejob` error. No log holds the value.
- The driver's first verdict was FAIL: its pattern matched the step's echoed script (`printf 'repo %s'`) before the output line. The saved logs were judged again with the output-line pattern now in `f79.mjs`; `result.json` records both.
- **Teardown.** The runner account listed 0 running Sandboxes (243 `inactive`). The check's git shard `git#0#net:65566` (site a) was stopped at 07:49:40Z through GitShard `/teardown` (200) from a scratch Worker `graft-m4a-shardkill`, deleted the same second; at 07:50:18Z the Containers API listed 0 running on sites a, b, and c. No `/retire`: m4a's sites are product builds.
- **Cost** (`tests/fanout/acctcost.py`). Before, at 07:31:16Z: the account $93.77 for the UTC day, graft-m4a $0.72, the runner account $0.34. After, 07:30Z to 07:50:20Z: graft-m4a $0.05 and the runner account $0.01, **$0.06** against the $3 stop; the account $98.94 for the day, under $170. Idle after the teardown, 07:50Z to 08:20Z: graft-m4a $0.03 (about $0.06 an hour, all site scripts) and the runner account $0.00; the account $105.42 for the day at 08:20Z.

## Sandbox placement errors: should `graft-runners` pace its starts? (2026-11-17, F80)

Code-only, no hosted spend. `tests/m4/wp4/sandboxstarts.py` reads the runner account's Workers Logs for each run and groups every Sandbox by when the pass first addressed it (`setSandboxName`). For each Sandbox it reports the SDK's placement errors and how long after the address the last one came. Output: `tests/m4/raw/wp4/f78-bf60/sandbox-starts.json` (run 1966284) and `tests/m4/raw/wp4/r24-bf60/sandbox-starts.json` (run 1966233).

| | Run 1966284: first 20 starts | Run 1966284: later 31 | Run 1966233: first 20 | Run 1966233: later 32 |
| --- | --- | --- | --- | --- |
| Addressed over | 12.9 s | 556 s, one as each slot freed | 25.3 s | 716 s |
| "no container instance" events | 896 | 170 | 1,042 | 877 |
| "too many containers per second" events (Sandboxes) | 25 (13) | 1 (1) | 15 (8) | 2 (2) |
| Placement wait after the address, p50 / max | **77.5 / 155 s** | **1 / 46 s** | 70.5 / 179 s | 10 / 163 s |
| Waits over 30 s | 15 | 1 | 14 | 7 |

- **The error counts are retry ticks, not failed starts.** `@cloudflare/containers` 0.3.7's `doStartContainer` polls every 300 ms, 27 tries for each 8 s attempt, and arms the object's alarm (1 s) on each try. Every try that gets no instance logs one error-level line: from the Sandbox's `alarm` (813 of 1,066 in run 1966284), or else from the RPC that called the start (253). The 1,066 lines come from 34 Sandboxes that waited [measured: code read of `node_modules/@cloudflare/containers/dist/lib/container.js`; matches upstream issue cloudflare/containers#262, unverified].
- **The wait is a cold pool, not a start rate.** In the first burst after the class sat idle, 15 of 20 Sandboxes waited over 30 s for an instance. The later starts, one at a time, waited about a second. All 26 rate errors came in the first 90 s, and 25 were in the burst. That burst addressed 20 Sandboxes at about 1.5 a second, but the SDK's own 300 ms retries made the start requests: each waiting Sandbox retries on its own. Pacing `graft-runners`' calls would spread the 20 addresses over more seconds without changing those retries. Cloudflare's Containers limits page (updated 2026-11-12) publishes no start-rate limit, only concurrent vCPU, memory, and disk [doc]. So pacing has no published limit to pace to, and nothing measured says it would shorten the wait [inference].
- **What does break: a delivery pass outlives its `waitUntil`.** The `fetch` handler runs the pass in `ctx.wait_until`, and the runtime cancels it 30 s after the 202 [doc: Workers context docs, "waitUntil has a 30-second time limit after invocation end"]. A burst start waits longer than that. Workers Logs show "waitUntil() tasks did not complete … cancelled" on 16 delivery passes in run 1966284 and 23 in run 1966233, each on a burst Sandbox. A cancelled pass logs no summary line, so the 30 `fetch` starts in the table above count only passes that finished: 51 Sandboxes were addressed. Its runner is already registered, and its `startProcess` was cut short. If the process never started, the runner never polls and holds a cap slot until `HOSTED_IDLE` (600 s), when a pass deletes it (`workers/runners/src/control.rs`, the start comment). Sandbox `55e4e998`, destroyed 10 min 10 s after its address following "Container failed to become ready" at 04:39:05Z, fits that path. Which cancelled passes left a runner with no process is not logged [inference]. The 5-minute sweep (`scheduled`, 15 min limit) is not cut, which is why earlier sweep-only runs saw 221 to 336 s passes but no cancellation.
- **Alarms as a write loop.** Each Sandbox object re-arms its alarm (one `setAlarm`, one row written) on every start try and on each wake of its keep-alive loop, at most 3 minutes apart. Run 1966284 had 980 Sandbox alarm invocations in 30 minutes. At $1.00 a million rows and $0.15 a million requests, that is well under a cent an hour at 20 runners [inference from the DO pricing page], and it stops when the Sandbox is destroyed. It is noted per the operator's rule, but it is not worth fixing at this scale.

**Not fixed here: recorded as F80** (`decisions/OPEN-QUESTIONS.md`). Pacing is not the fix. The small change, awaiting the start before the 202, would hold webhook deliveries open for up to 2.5 minutes. The real fix moves the start out of the delivery pass's `waitUntil`, either into the Sandbox object (store the JIT config and let its own alarm start the process) or onto a path with a longer limit. That is a design change to `graft-runners`' Shape entry and needs a hosted check of a cold burst, so it waits for a decision.

## F80's fix: the Sandbox's alarm starts the runner (2026-11-17, code only)

Branch `f80-alarm-start` (`decisions/DECISIONS.md`, `f80-alarm-start`). A pass registers the runner and calls the Sandbox's `arm`, which stores the start and schedules it on the Sandbox's own alarm, then returns. `launch.mjs` starts the process from that alarm at most once, retries every 10 s while placement fails, gives up 6 minutes after the arm, and drops the JIT config once the process starts or the runner is destroyed.

- **The alarm covers the wait.** An alarm handler has 15 minutes of wall time and retries a thrown handler up to 6 times [doc: Workers limits, "Durable Object alarm handlers"; Durable Objects alarms]. One start try waits at most the SDK's 150 s transport budget, so the slowest burst placement measured, 179 s, takes two tries.
- **The cap-slot hold, settled.** Runner 1704490 (Sandbox `55e4e998`, run 1966284) never opened a session or made an install request. It was deleted stale at 04:46:38Z, about 610 s after its Sandbox was first called, and its Sandbox was destroyed 0.3 s later. Runs 1966284 and 1966233 opened 50 sessions each for 51 and 52 registrations [measured, the edge's Workers Logs], so in each run one runner never started.
- **Tests.** `workers/runners/tests/launch.test.mjs` (`node --test 'workers/runners/tests/*.test.mjs'`, now a CI job), 4 tests on a fake Sandbox using the burst's measured waits. Plants 60 to 67 in `tests/m4/wp4/plant.py`: 8 of 8 caught. Plant 60 is main's rule, where the pass starts the process itself; under it the burst test's delivery passes are cut at 30 s.
- **`launchRunner` never throws** (`f80-launch-nothrow`). The base class's `alarm()` deletes a scheduled row even when its callback throws, so the start catches every error, a storage read included, and schedules the next try. On a29a57d2 a failed storage read escaped and dropped the start. `a start whose storage read fails is tried again, not dropped with its row` fails on that code, and plant 68 restores it. 5 tests, plants 60 to 68: 9 of 9 caught. `launch.mjs` now logs `graft-runners: launched` once for each process it starts, and `sandboxstarts.py` counts these per Sandbox (`launched_twice`).
- **Hosted cold-burst check: passed** (run 1966335, next section). It passes when no delivery pass logs a `waitUntil` cancellation, every registered runner opens a session, and no Sandbox starts two processes.

## F80 cold-burst check on the alarm start (2026-11-17)

The lead moved the slot from Nov 18 00:00Z to now. `graft-runners` was redeployed from main `a73871ce` at 15:48:07Z (`tests/m4/raw/wp4/runners-init.log`, exit 0; main `fec1d01d` had no change under `workers/runners` or `tests/m4/wp4` since then). Its webhook secret was new, the App's hook config was patched to match, and an unsigned POST answered 401. The image did not change, and the install was not redeployed. No Sandbox had started in the runner account since 07:45Z, so the class had been idle for 8 hours, not the 30 minutes required. Run 1966335 ran `tests/m4/wp4/bf60.mjs` with `DEMAND_EVERY_S=300` from 15:50:19Z (`tests/m4/raw/wp4/f80-burst/`). The stops were $2.50 for graft-m4a plus the runner account, the account's UTC day line ($170, raised to $280 mid-run by the operator), a hosted job failing or sitting past its lease, or any process launched twice. `watch.log` read `tests/fanout/acctcost.py` every 5 minutes; no stop fired.

| | Run 1966335 (F80's fix) | Run 1966284 (before) |
| --- | --- | --- |
| Delivery passes cut at the 30 s `waitUntil` limit | **0** | 16 |
| Registrations (edge `POST hosted/runners` 201) / sessions opened (`POST broker/session`) | **50 / 50** | 51 / 50 |
| `graft-runners: launched` lines / Sandboxes / Sandboxes launched twice | **50 / 50 / 0** | (not logged) |
| Burst of 20: placement wait p50 / max, waits over 30 s | 75.5 / 155 s, 15 | 77.5 / 155 s, 15 |
| Later 30: placement wait p50 / max | 8.5 / 27 s | 1 / 46 s |
| Queue to start, p50 / p90 / max | **186 / 491 / 603 s** | 216 / 525 / 625 s |
| Last job after dispatch | 1,093 s | 1,118 s |
| Registrations / Sandboxes at once, max | 20 / 20 | 20 / 19 |
| Every job `Success` | **50 of 50** | 49 of 50 (F79) |
| 10 minutes after the last job | 0 running Sandboxes (293 `inactive`), no registration | 0, none |

- **All three pass lines hold.** The burst waited on a cold pool for as long as before (15 of 20 Sandboxes over 30 s). But the wait now runs in each Sandbox's alarm, not in the delivery pass, and no pass was cut. Every runner that registered opened a session, and each Sandbox launched exactly one process. The 38 delivery passes that started runners started all 50; the 6 sweeps started 0 (`prompted.jsonl`). The launch lines show the alarm path at work: one Sandbox retried every 10 s after "no container instance" from 15:53:12Z, then launched.
- **BF-60 passes outright** on this run: 50 of 50 jobs succeeded, at most 20 at once, and nothing was left after 10 minutes. No runner was deleted while it held a job. The 3 stale deletes (runner 1704542 twice, 1704543 once, at 15:52:04Z to 15:52:13Z) each came after that runner's `completejob` and session close (edge Workers Logs).
- **Recurring findings.** These are Sandbox SDK log lines from the runner account's Workers Logs, 15:49Z to 16:20Z: 1,177 "no container instance" (run 1966284: about 1,066), 54 "too many containers per second" (26), and 1 "Container failed to become ready" (1). They are retry ticks while placement waits ("Sandbox placement errors" above), and none failed a start. There were no demand 503 lines and no `access_tokens` error lines.
- **`sandboxstarts.py` undercounted busy windows.** One walk back through a window's events ended early after 990 events, and the walk found 14 of the 50 `launched` lines. The script now walks each 30 s slice on its own, and the counts above come from that version. Run 1966284's counts in "Sandbox placement errors" came from the old walk, so they may be low [inference].
- **Teardown.** The driver stopped on its own, and services `f80bf60` and `f80watch` were stopped. The runner account listed 0 running Sandboxes (293 `inactive`). m4a's one running shard, `git#0#net:65562` (site a), was stopped at 16:21:58Z through GitShard `/teardown` (200 `torn_down`) from a scratch Worker `graft-m4a-shardkill`, which was deleted at 16:21:59Z. At 16:22:37Z the Containers API listed 0 running on sites a, b, and c. There was no `/retire`: m4a's sites are product builds.
- **Cost** (`tests/fanout/acctcost.py`, by `PREFIX`; m3fr12 ran on graft-m3f at the same time). Before, from 00:00Z to 15:47:25Z: the account $111.01 for the UTC day, graft-m4a $1.39, and the runner account $0.35. After, from 15:47:25Z to 16:28:46Z: graft-m4a $0.25 and the runner account $0.34, so **$0.59** against the $1.00 budget and the $2.50 stop. The account was at $129.27 for the day, under the $280 line. Idle after the teardown, from 16:22:37Z to 16:52:49Z: graft-m4a $0.03 (about $0.06 an hour) and the runner account $0.00; the account was at $132.29 for the day at 16:52:49Z.

## B0-35 with Actions dispatch on graft-m4a (2026-11-17)

Site day 50 (main `fbe49d59`, the format 0x15.4 reader, `GRAFTYARD_PR_TRIGGER` unset) ran 20:26:48Z to 20:28:16Z, exit 0. Site day 51 (main `c2232435`, with `b035-base-tip`, plus `GRAFTYARD_PR_TRIGGER=1`) ran 20:48:23Z to 20:49:19Z, exit 0 (`tests/m4/raw/wp4/siteday50.log`, `siteday51.log`). The git shard image stayed m4a47; no `containers/` change since day 49. Then `tests/m4/wp4/b035.sh` (`f92d5707`, main plus `dispatchlog.py`) ran on a new repository, `m3org/b035-r1` = repo 65567, with 8 learners (enam 4, weur 2, apac 2). It ran `a1.mjs b0-35 --seconds 600 --workflow --as-admin`: 500 PRs opened (500 of 500), the workflow seeded on `main`, then the 600 s window from 20:58:30Z with 500 agents pushing their PR heads every 120 s, 10 merges a minute, and 500 `mergeable` pollers every 5 s. Run `mwl642gm`; raw files in `tests/m4/raw/wp4/b035-r1/`. k stayed 1: `k.mjs`'s `/bench_seed` is test-faults only.

**Verdict: B0-35 fails on dispatch p99.** Under the 90% rule the line needs at most 11 s, and the fail line is 30 s.

| Line | Value | Source |
| --- | --- | --- |
| Dispatch p50 / p99 / max, push acknowledgment to run listed (driver) | **355 s / 680 s / 680 s** (54 paired) | `a1.mjs` `dispatch` |
| Dispatch, entry to `RunCreated` (site log), window events | p50 320 s, p99 725 s, max 737 s (1,195 runs) | `dispatchlog.py` |
| Same, setup's 446 `opened` events before the window | p50 21.8 s, p99 39.1 s, max 40.6 s | `dispatchlog.py` |
| Agent pushes acknowledged in the window / runs paired / missing / unpaired | 234 / 54 / **180** / 48 | `a1.mjs` `dispatch` |
| Superseded events (head moved on before the run) | 203 of 1,198 events | `dispatchlog.py` |
| `unmerged`, as last logged / on the first try | **0** / 103 (8.6%) | `dispatchlog.py` |
| Conflict | 0 | `dispatchlog.py` |
| Asks per event that ran (0 / 1 / 2 / 3 / 4) | 546 / 566 / 76 / 7 / 0 | `dispatchlog.py` |
| Events logged more than once (a failed round's batch run again) | 278 | `dispatchlog.py` |
| Test-merge CPU (the leader's shard, `standard-4`, 1 vCPU budget) | 64.7 s of merges from 20:54:35Z to 21:10:17Z, about 0.069 vCPU; 1,727 computed, 484 shed (all `late`, past TURN_WAIT_MS), wait max 69 s | leader `/stats` `testMerges.shard` |
| List-field computes | **0** (31,846 list-field asks counted) | same |
| Sandboxes started | 0 | `b035watch.sh` |
| Runs for an entry over 1 h old | 0: the repository was 7 minutes old at the window | |

- **Why dispatch is slow: the group, not the trigger.** The repository leader stayed on site 0 at term 24 throughout. But the load window's commits were what the 500-agent rows have shown before: 234 of 2,490 agent pushes acknowledged (p50 43.8 s, p99 126 s), 3,367 failed peer calls from the leader, `read:mergeable` 24,830 × 503, and side commands mostly 503. `demand`'s reads and commits ride the same group: of the 5,729 `demand` failures the sites logged from 20:58:30Z to 21:10Z (Workers Logs counts), most were "no member answered", a follower's "no answer", "not leader", or the shard's `warming`. A request's events wait for `ActionsTrigger`'s cursor, which moved in bursts. From 21:01Z to 21:09Z the three sites logged 1,182 dispatch lines, 1,057 of them `unmerged`. From 21:10Z, once the load stopped, site a logged 879 lines with 2 `unmerged`, and every event got its run. That is when most of the window's events got their runs, minutes late. The setup's `opened` events, before the window's load, took p50 21.8 s.
- **F84: `unmerged` is 0 in the end, 8.6% on the first try.** A failed round (`did not commit in 10 s`, or a shard `tree`/`blob` read that failed) runs its whole batch again, and that rerun asked again and got the merge. So no event stayed without a run. Each round that ended an event `unmerged` had spent 4 asks on it. That 8.6% is the share a run would have lost if no rerun had followed. It is a load effect: before the window and after it, nearly no event needed more than one ask.
- **Missing runs.** The driver paired 54 of its 234 acknowledged pushes with runs. Of the rest, 180 have no run in the newest 200 the admin route lists, because the route caps its list at 200 and the backlog's runs came after the driver's tail poll ended at 21:10Z. The site log has a run for every event, so `missing` is the driver's view, not lost runs.
- **Close-out.** The driver cancelled 342 of the 345 runs it saw (3 answered 503). The admin route lists only the newest 200, so 737 more were still queued after it: 84 in that list, 401 above it from the backlog that committed after 21:10Z (2 of these cancels failed and went through on the next pass), and 250 of setup's `opened` runs below it. A sweep of the shard's run ids 1,703,500 to 1,707,100 through `GET` and `POST .../cancel` found and cancelled them. A final pass found all 1,195 of the repository's runs `completed` / `cancelled`. A third pass at 22:13Z read the same: 1,195 `completed` / `cancelled`, 0 queued. `teardown.py --sweep --shardkill` tore down 3 shards at 21:13Z and 1 at 21:44Z (`git#0#net:65567`), then listed no shard container running. The product build answers `/retire` 404 (it is a test route), so `retirenets.mjs` left all 12 members joined and `purge.py` refused the network (`b035-r1-retire.log`, `b035-r1-purge.log`). Repo 65567's members stay joined and idle, and its packs stay.
- **Cost** (`tests/fanout/acctcost.py` by `PREFIX`; m3f and LM-1 ran on their own installs at the same time). From 20:24Z (before site day 50) to the last sweep at 21:44:46Z: graft-m4a **$2.41**, both site days included. The runner account spent $0.00 from 20:24Z to 22:14:46Z. Idle reading, 21:44:46Z to 22:14:46Z: graft-m4a $0.30 and the runner account $0.00, with no container `running` on any m4a site; every shard instance listed is `failed` exit 137, the torn-down state. That $0.30 was not idle (attributed by `durableObjectsPeriodicGroups` per object and minute, and Workers Logs): $0.14 from 21:50Z to 21:55Z was this close-out's own cancel sweep (250 cancels, 1,258 edge `GET`s and 440 `POST`s; rows on `repo:65567`'s 12 members and `actions:7100001.11`), and $0.11 from 22:00Z to 22:05:30Z was site a's `*/15` fold audit refolding `repo:65567` for the first time (37 batches, 39 MB, 4,976 entries; its leader `repo:65567#1#f0` wrote 70,107 rows in the minute 22:01Z) plus `repo:65536` (13,978 rows), with the last read pass (`GET`s on run ids 1,703,500 to 1,707,100) ending 22:05:21Z. The audit refolds a chain once a day (`AUDIT_EVERY_MS`, K10), so `repo:65567` staying joined costs about 70,000 rows, $0.07, a day. The real idle reading, 22:05:30Z to 22:31Z, is graft-m4a **$0.01** in 25.5 minutes; `repo:65567`'s members wrote 210 of its 2,289 rows. The account was at $165.92 for the day before and $176.51 at 22:15Z.

**Next.** The trigger did what the design says: 1,195 runs at `refs/pull/N/merge`, 203 superseded events run at the current merge, 0 conflicts, 0 left without a run. Dispatch is bound by the repository group under the 500-agent load, as every other A1 row is. So B0-35's dispatch line moves with LM-1's commit work, not with Actions. Two trigger changes would still shorten it under load. Retry one event's `demand` without rerunning its whole batch. And move the cursor past events whose runs committed, so a failed round does not redo them.

## B0-35 rerun on graft-m4a after site day 54 (2026-11-19, row m54)

Site day 54 (00:52:22Z to 00:53:32Z) and edge day 27 (to 00:56:07Z, version `51a5ec5b`) shipped main `67483ab9` (`76c3e131` plus results-only commits) as product builds, with git shard image `m4a54` (`tests/am1/results/install-m4a.md`). The flags were day 53's (`GRAFTYARD_APP_HOOKS=1`, `GRAFTYARD_PR_TRIGGER=1`), `GRAFTYARD_JOB_TOKENS` was unset, and `FORMATS_ENABLED` was edge day 26's. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m54` = repo 65568, with 8 learners. Run `mwmusr0e`: 500 of 500 PRs opened, then 600 s from 01:15:54.778Z. Raw files: `tests/am1/raw/a1-b0-35-mwmusr0e.jsonl`, `a1-pushes-b0-35-mwmusr0e.jsonl`, `tests/m4/raw/wp4/b035-m54/`, and the voter watch `b035-m54.lead`.

**Verdict: fails on dispatch p99, 120 s** (met at 11 s, fail over 30 s). Both budget lines pass. That is a step from m4a's first run (680 s), because the group now acknowledges the load's pushes.

| Line | Value | Source |
| --- | --- | --- |
| Dispatch p50 / p99 / max, push acknowledgment to run listed (driver) | **45.2 s / 120.2 s / 170 s**, 2,471 of 2,471 paired | `a1.mjs` `dispatch` |
| Dispatch, entry to first `RunCreated` log (site) | p50 44.5 s, p90 78.9 s, p99 144.9 s (2,391 events) | `dispatchsplit.py` |
| Test-merge vCPU (leader's shard, 1 vCPU budget) | **0.54** (198.0 s spent over 367.1 s of merge wall); 5,294 computed, 1,372 shed, all `late`; `inFlightMax` 371, wait max 13.4 s | leader `/stats` `testMerges.shard` |
| List-field computes | **0** of 50,516 list-field asks | same |
| Agent pushes acknowledged | 2,471 of 2,500, p50 2.1 s, p99 12.8 s | `a1.mjs` |
| `mergeable` polls | 3,110 `MERGEABLE`, 54,064 `UNKNOWN`, 2,357 × 503 | `a1.mjs` `pollAnswers` |
| Leader commit p99 | 4,665 ms | leader `/stats` |
| Events: run / no run / `unmerged` on the first try / superseded / logged twice | 2,391 / 53 / 13 (0.53%) / 21 / 416 | `dispatchlog.py` |
| Sandboxes started | 0 | `b035watch.sh` |

`dispatchlog.py`'s first pass lost lines. Its walk stepped back from the oldest line each 100-line slice returned, and it stopped early when a round logged 100 lines in one millisecond: one pass kept 17 of site b's 1,642 lines. The walk now bisects by time and splits a full millisecond by PR number. The rerun read 3,174 lines; Workers Logs counts are site a 1,688 (it read 1,411), b 1,642 (1,587), and c 176 (176). The figures here come from that rerun.

### Where dispatch time goes

The stages, from the push acknowledgment:

1. **Push acknowledgment to entry:** none. The entry is stamped before the push is acknowledged (leader commit p50 0.7 s, p99 4.7 s), so the driver's acknowledgment-to-run (p50 45.2 s) is a little shorter than the site's entry-to-run (p50 44.5 s on first runs). The push acknowledgment itself (p50 2.1 s, p99 12.8 s) is before the driver's clock starts.
2. **Entry to the start of the `ActionsTrigger` round that takes it:** p50 26.2 s, p90 65.2 s, p99 96.9 s (2,251 events whose previous round's end is known). **This stage holds the tail.** The slowest 1% (22 events, 132 s and more) waited 119 s on average for their round and spent 31 s inside it. The slowest 5% (112 events, 92 s and more) waited 88 s and spent 23 s inside.
3. **Inside the round, per event:** p50 14.7 s, p99 52.1 s. The demand loop takes most of it: by asks, 0 asks p50 10.2 s, 1 ask 18.1 s, 2 asks 33.3 s, 3 asks 43.4 s. Each ask is one `demand` call, a `/read`, and a shard `/demand` whose merge waits its turn, plus DEMAND_WAIT_MS (2 s) between asks. Events with no ask still spend 10 s (p50), which is the workflow `tree`/`blob` reads and `CreateRun` on the Actions shard.
4. **`CreateRun` commit, broker match, and `acquirejob`:** the first is inside stage 3. B0-35's label has no runner, so dispatch ends at `RunCreated`, and the broker and `acquirejob` play no part in this line.
5. **Run listed by the driver:** within one 1 s poll.

A round is one batch of up to 200 entries, and its events run at once (`join_all`). The round ends only when its slowest event finishes: rounds took p50 42.2 s and p99 75.0 s. Entries that commit during a round wait for the next one. So an entry waits about one round length for its round to start, then its own demand loop, which is why entry-to-run (p50 44.5 s) is about one round. By entry minute from t0, the worst minutes were 4 and 5 (p99 148.6 s and 168.4 s). Those are 01:19:54 to 01:21:54Z, the minutes of the term changes below, where events entered then p50 69.8 s and p99 163.6 s against p50 40.7 s and p99 92.4 s for the rest. Outside those minutes the p99 is still 92 s, so the round structure, not the leader changes, sets the line.

### The leader changes

The voter watcher read site 1's superseded `repo:65568#1#f1` (generation 1, applied 12, term 2), not the live `#3#f1`, so site 1's row is wrong throughout: the lead settled that `#1#f1` is the dropped first-generation name. Read through f0 and f2, the leaders were:

| Time (Z) | Term | Leader | Cause, as the logs show it |
| --- | --- | --- | --- |
| to 01:21:09 | 2 | f0 (site 0) | — |
| 01:21:09 | 3 | `#3#f1` (site 1) | **suspect withhold**: the sites logged `/suspect` 115 / 115 (b, c) in 01:20 and 804 / 406 / 992 in 01:21, against 0 to 1 a minute before. |
| about 01:21:30 | 4 | not caught (15 s watch) | the same `/suspect` burst. |
| 01:21:41 | 5 | `#3#f1` | the same burst, with 195 / 3 / 168 `/suspect` in 01:22, then none to 01:26. |
| 01:26:10 | 6 | f0 | after the load window ended at 01:25:54. |
| 01:26:25 | 7 | f2 (site 2) | during the tail. Commits stalled at 3076 from 01:26:41 to 01:28:33. Sites a and b each logged 100 or more "Subrequest depth limit exceeded" from 01:26:21 to 01:26:25. |
| 01:31:31 | 8 | f0 | after the load; it held to the end. |

The edge's clients came from DFW and LAX (108,258 and 90,236 requests in the first 10 minutes, about 1,100 from ORD), so a `suspect` from both colos within 10 s meets SUSPECT's two-colo rule. The product build serves no `/elections` or `/trace`, so StepDown cannot be told from a lapse in these logs. The `/suspect` burst at 01:20 to 01:22 is the only cause the logs show for terms 3 to 5, and terms 6 to 8 fall after the window. The `ActionsTrigger` moved from site a to site b at 01:21:55 to 01:22 (site a logged its last full round at 01:22, site b its first at 01:22:07), with 72 `demand` errors naming `#3#f1` "not leader" or "no answer" on the way.

### The `mergeable` answers

The 54,064 `UNKNOWN` against 3,110 `MERGEABLE` come from how GETs reach the shard. A single-PR GraphQL poll by the App asks the shard for a merge (`/demand?cause=get`) after the reply, which never waits. The shard coalesces GETs per PR for GET_COALESCE_MS (30 s) and runs one only on a positive balance that leaves half a second's budget for pushes and dispatch (`MergeBudget::decide`). Of 29,983 GET asks, 3,691 computed. Each PR's head moved every 120 s and the base 80 times in the window (p50 5.1 s apart), and `mergeable` reads only a merge at the current head and base (`prsync::mergeable`). So a computed merge went stale within about one base move. These figures bound the three kinds, but per-poll data does not exist to classify each answer:

- **Never asked:** of the 26,292 GET asks that did not compute, at least 24,920 fell inside a PR's 30 s coalescing slot and were refused without a compute (the rest are at most the 1,372 `late` sheds). No merge was refused by the budget itself: `shed` equals `late`.
- **A compute in flight or shed late:** 1,372 merges shed `late` (TURN_WAIT_MS, 10 s). Waits ran to 13.4 s, with 371 in line at most.
- **Computed but stale:** 3,691 GET computes and 5,294 merges in all. Of the admitted merges' waits, 2,201 were 5 to 10 s and 1,978 were 10 to 20 s (`waitHist`), while the base moved every 5 s or so, so most were stale before a poll could read them.

A poller that read `MERGEABLE` read it within the line's window: 3,110 polls did, so yes, some 5 s pollers saw it. But the row cannot say how many distinct PRs or how soon after a push, because `a1.mjs` records answer counts, not per-poll rows. GHES also answers `null` (GraphQL `UNKNOWN`) until it computes a merge, so `UNKNOWN` is the documented shape, not an error.

### j4 during the site day (asked by the lead)

- **First and last 503:** 00:57:57.585Z (`GET /api/graftyard/v1/repos/m3org/j4/actions/runs`) and 01:05:41.840Z, 31 in all on graft-m4a-edge. Before that, j4 and its hooks answered 200 at 00:57:39 to 00:57:41Z. j4 was leaderless with reads for 7 min 44 s. f0 resumed at 01:08:19Z, and reads after that were 404s, the normal answer.
- **`suspect` and `forgot`:** in 00:56 to 01:09Z the sites stepped one `/suspect` each on a and c, both at 01:01:10Z, and none on b. A `suspect` is sent only when a shed answer names a `forgot` leader (`crates/lookup/src/lib.rs`, `forgot_of`), so `forgot` answers fired at about 01:01:10Z. No site logged a `keep led` probe or campaign. The member reads were 201 / 61 / 69 `/read`s on sites a / b / c, all `ok`.
- **Why f1 sat at 5248:** j4's config at index 5251 is `Degraded` with `lost` = site 1. `quorum::voters` leaves the lost site out (`crates/raft/src/quorum.rs`), and the leader replicates only to `targets()`, its voters and learners (`crates/raft/src/member/leader.rs`). So no leader sends `#2#f1` entries past the config that dropped it, and it stays at its last applied index, 5248 (last 5250, its own `configIndex` 7). It is the same shape as 65568's `#1#f1` at applied 12.

### Close-out and cost

The b035 and b035lead services were stopped. The sweep found no shard container running. 65568 was purged through the route: `DELETE` 204, the first pass 503 (a lost reply from `#1#f1`), then 200 retiring 11 members and deleting 877 objects, then the second pass 200 with 0 objects (`purge35.jsonl`). `b035watch` and the two site-day build services were stopped at 02:20Z.

Cost by `tests/fanout/acctcost.py`, from the site day's start at 00:52:22Z to 02:40Z: graft-m4a **$4.39**, and the runner account $0.00. The idle reading from 02:00Z to 02:40Z was graft-m4a $0.02. The account's UTC day was $9.77 at 02:40Z. The grant's $12 stop has $7.61 left.

### Next: the dispatch tail

The tail is the round, not the group. An entry waits for the round in flight to finish its slowest event, then for its own demand loop. The proposed fix is code-only, in `ActionsTrigger` and the shard's merge turn:

1. **Stream events instead of rounds.** Each scan reads the newly applied entries and starts their events at once, with no wait for the events already running. A bound on events in flight protects the isolate. The cursor still settles below the oldest unfinished event, as carried items do now (`trigger::settle`), so a new leader still rescans what was not done. This removes stage 2 (p50 26.2 s, p99 96.9 s).
2. **Dispatch merges go first in the turn.** The merge turn is one line shared by head pushes, dispatch, and GETs, and up to 371 merges waited in it. `MergeCause` already says dispatch is served before GETs, but the turn does not order by cause. A dispatch merge that takes the next turn waits one merge (tens of ms) instead of 5 to 20 s, which shortens each ask in stage 3 (1 ask p50 18.1 s, 2 asks 33.3 s).
3. **Time each part of an event in its log line:** the demand, the workflow reads, and `CreateRun`. Events with no ask still took 10.2 s (p50), and this row cannot split that time.

With 1 and 2, the expected entry-to-run is the 0-ask in-round time, p50 about 10 s and p99 about 15 s on this row's load. That is inside the fail line, but it is not shown to meet 11 s until 3 attributes those 10 s. B0-35 is not rerun until this change, or LM-1's leader-tail work, lands.

**Built** code-only on branch `dispatch-tail` as approved (`decisions/DECISIONS.md`, "`trigger-stream`"). In memory, a stream and rounds commit the same `CreateRun`s for one entry stream, a slow event holds one slot and the cursor below it, and a dispatch merge behind queued pushes and GETs runs first, at most four in a row while another waits. Plants 99 to 105 are caught. The rerun waits on a site day for it and on LM-1's B0-33 work.

## B0-35 rerun on graft-m4a after site day 57 (2026-11-19, row m57)

Site day 57 (18:32:14Z to 18:33:18Z) shipped main `801c1b5a` as a product build, with git shard image `m4a56` (no `containers/git` change since it was built) and day 56's flags. Edge day 28 stayed: the edge changed only in comments since `e1b99f5b`. `dispatch-tail` and `shed-retry` were on m4a from site day 55; this day added `lock-split`, `sync-stall-q`, and `round-time`. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m57` = repo 65571, with 8 learners. Run `mwnw5a81`: 500 of 500 PRs opened, the learner gate passed 8 of 8 in 1.3 s, then 600 s from 18:41:45.615Z. A first start at 18:33Z stopped at once on two harness causes (no `TOOLS_TOKEN_FILE` in the service's environment, and `b035watch.sh` counting R23's 8 `stopped` RunnerBox instances as running Sandboxes); both were fixed before the row. Raw files: `tests/am1/raw/a1-b0-35-mwnw5a81.jsonl` (not committed), `tests/m4/raw/wp4/b035-m57/`, and the voter watch `b035-m57.lead`, which now reads each site's live voter.

**Verdict: fails on dispatch p99, 184 s** (met at 10 s, fail over 30 s). Before the first term change, dispatch met the line: the 585 events logged before 18:46:35Z had entry-to-run p50 3.9 s and p99 5.3 s. The tail is three leader changes and the shard warm-up after them. Both budget lines, the list-field line, and the memory line pass.

| Line | Value | Source |
| --- | --- | --- |
| Dispatch p50 / p99 / max, push acknowledgment to run listed (driver) | **11.4 s / 184.5 s / 200 s**, 2,420 of 2,420 paired | `a1.mjs` `dispatch` |
| Dispatch, entry to run (site) | p50 32.6 s, p99 203 s (1,565 runs); `splitMs` p50 / p99: wait 19.6 s / 182.3 s, demand 2.0 s / 179.0 s, reads 2.0 s / 19.2 s, create 0.6 s / 4.8 s | `dispatchlog.py` |
| By entry minute from t0 | minutes 0 to 3 p99 4.9 to 6.1 s; minutes 4 to 7 p50 97 to 195 s; minutes 8 to 10 p50 44, 24, 4.5 s | `dispatchsplit.py` |
| Test-merge vCPU (leader's shard, 1 vCPU budget) | **0.46** over merge wall (140.7 s CPU over 307.3 s), 0.23 over the 600 s window; 3,814 computed, 1,586 shed `late`; `inFlightMax` 364, wait max 11.8 s | leader `/stats` `testMerges.shard` |
| List-field computes | **0** of 60,329 list-field asks | same |
| `mergeable` polls | 4,353 `MERGEABLE`, 51,365 `UNKNOWN`, 4,200 × 503 | `a1.mjs` `pollAnswers` |
| Agent pushes acknowledged | 2,420 of 2,500, p50 2.1 s, p99 16.9 s; 73 × 503 shed read, 7 × 503 shard warming | `a1.mjs` |
| Leader commit p50 / p99 | 526 ms / 2,981 ms | leader `/stats` |
| Leader changes | f0 → f1 (term 3) 18:46:40Z, f1 → f0 (term 4) 18:47:06Z, f0 → f1 (term 5) 18:49:37Z | `a1.mjs` `leaderChanges`, `b035-m57.lead` |
| `/suspect` per minute, sites a / b / c | 18:46: 287 / 70 / 0; 18:47: 34 / 500 / 343; 18:49: 418 / 15 / 418; 18:50: 625 / 4 / 625; 18:51: 243 / 3 / 243; 0 in 18:44, 18:45 and 18:48 | Workers Logs |
| Worker outcomes 18:41 to 18:55Z | a 619,514 ok, 1,482 canceled, 57 exception; b 190,646 / 857 / 33; c 170,000 ok; no `exceededMemory` or `exceededCpu` | Workers Logs |
| Sandboxes started | 0 | `b035watch.sh` |

No dispatch line was logged from 18:46:43Z to 18:49:39Z (177 s) while `ActionsTrigger` moved between sites a and b. In that window the shard answered `{"warming":true}`, and the demands and tree reads that named `repo:65571#2#f1` failed with "no answer". Shards rehydrated on site b at 18:48:11Z and on site c at 18:48:50Z, and site a's promotion failed after 94 s at 18:48:36Z with "warming". Events that entered after 18:49:45Z had p50 25.7 s and p99 67.6 s.

Cost by `tests/fanout/acctcost.py`, from the site day's start at 18:32:14Z to 19:10:45Z: graft-m4a **$4.21**, and the runner account $0.00 (stop $8). a1 cancelled the row's 2,635 runs (all 202) by 19:09:04Z. The teardown sweep tore down `git#{0,1,2}#net:65571` (all 200) and found no shard container running; the containers API showed 0 active on all three m4a sites. The M2 canary's comparison excludes 18:32:14Z to 19:23:32Z.

### Deeper analysis of m57

Sources: Workers Logs on the three m4a site Workers, 18:41 to 18:55Z, the voter watch `b035-m57.lead`, and `dispatch-lines.jsonl` / `dispatch-split.json` in `tests/m4/raw/wp4/b035-m57/`.

**1. The three leader changes.**
- **Term 3, 18:46:40Z, f0 → f1:** a `STEP_DOWN` Confirm trip on f0 at 18:46:33.612Z, `{p99_ms: 792, slow: 6, of: 32}`, with no `/suspect` before it. f0's storage p99 was 295/563 ms in that window. P2 (`confirm-storage`, d8dcd513) judges storage time, not the wait for the flush, so it would likely have kept f0 [inference]. `tests/fanout/stepdown.py` cannot replay the window, because a product build writes no storage-time trace. The next row is a test-faults build so that it can.
- **Term 4, 18:47:06Z, f1 → f0:** deposed by `/suspect` reports (site b 500 and site c 343 in 18:47), not by a trip of its own. f1 committed at p50 6,329 ms and p99 7,984 ms while its storage p99 was 85/207 ms, so it was slow for some other reason. The cause is unknown (`decisions/OPEN-QUESTIONS.md`, F102). Nothing here argues against m54's withhold. A follower counts a report only once it has had no AppendEntries for CALL_DEADLINE (1 s), and with commits at p50 6.3 s f1's rounds plausibly left that gap, so the election matches the rule doing its job on a slow leader [inference: a product build logs no `Trace::Reported`, so `withhold.py` cannot check it].
- **Term 5, 18:49:37Z, f0 → f1:** a `STEP_DOWN` LoopLag trip on site a at 18:49:27.485Z, `{clamped: 2}`. Two clamped loop-lag samples in the window, as the isolate drained the backlog of the 177 s gap. P2 does not change LoopLag.

**2. Why dispatch stopped while a git shard warmed.** Dispatch needs this site's shard of the network (`git#<site>#net:65571`, which the lines name `repo:65571#2#f1`) for two steps. One is `ActionsTrigger`'s workflow reads, `/ui/tree` and `/ui/blob` at the pushed commit (`workers/site/src/actions_trigger.rs`, `ui`). The other is the test merge for `pull_request` events, `/demand` (`workers/site/src/prsync.rs`, `demand`). Both call only their own site's shard, and on main the shard ran `caught_up()` before every one of them. Site a's catch-ups failed with "repo:65571: no member answered" at 18:47:29, :49, 18:48:25, :45 and 18:49:14Z (120 such lines in 18:48) while the group committed 2378 → 2787 under f0, so every read answered 503 `warming` or "no answer". Site b's shard, where `ActionsTrigger` ran after term 3, had never caught up. It rehydrated 196 entries until 18:48:11Z, and site c rehydrated until 18:48:50Z. Nothing else could answer: no step falls back to another site's shard.

The fix, built on branch `r27-disk-first` (`decisions/DECISIONS.md`, "`disk-first-reads`"): a read that names only commits answers from the disk when the shard holds a cursor for its container, and waits for a catch-up only when the container answers that a commit is missing. Commits are immutable, so freshness cannot change the answer. That covers site a in term 4. Site b in term 3 had no cursor, which is the previous-holder fallback's case. That fallback would add a new cross-site call on the dispatch path, so it is recorded as `decisions/OPEN-QUESTIONS.md` G29 and not built. Expected effect on 18:46–18:49: the row's dispatch p99 at about 25–35 s instead of 184 s [guess]. What remains is site b's cold shard for about 90 s, and demands whose head was pushed during the gap.

**3. The tail after recovery (p99 67.6 s after 18:49:45Z) is the backlog draining, not a slower steady state.** By entry window: 18:49:45–18:50:45Z p50 43.9 s / p99 73.6 s, the next minute p50 24.3 s / p99 46.7 s, and from 18:51:45Z p50 4.5 s / p99 6.5 s. The last is back at the steady state before 18:46:35Z (p50 3.9 s, p99 5.3 s, 585 events) [measured, `dispatch-lines.jsonl`].

**Ranked code change.** First, `disk-first-reads`: it removes the shard's catch-up from the dispatch path wherever the disk holds the commits, and it is a local rule on an existing route. Second, instrumentation only: the shard isolate now logs its lookup-mark holder's posts and answers (`graftyard_lookup::trace_marks`, F101), so the next row shows why lookups found no member for about 2 min. Neither touches leadership; P2 is already on main for term 3's trip.

## B0-35 on a test-faults build after site day 58 (2026-11-19, row m58)

Site day 58 (21:24:11Z to 21:25:13Z) shipped main `f50ad36b` as a **test-faults** build (`worker-build --release --features test-faults`, `disk-first-reads` and the lookup-mark trace), with image `m4a56` (no `containers/git` change besides a test since it was built) and day 57's flags. Edge day 28 stayed. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m58` = repo 65572, with 8 learners, plus `tests/nr3/tsave.mjs` on the chain every 30 s (`tests/fanout/raw/tsave-m58.jsonl.xz`). Run `mwo29syc`: 500 of 500 PRs opened, the learner gate passed 8 of 8 in 636 ms, then 600 s from 21:33:31.912Z. Raw files: `tests/m4/raw/wp4/b035-m58/` and `b035-m58.{status,lead,watch}`.

**Verdict: passes dispatch** (p99 9.1 s at the driver and 7.5 s at the site, against 10 s). One leader change came during the load and one 28 s before it. Both budget lines, the list-field line and the memory line pass.

| Line | Value | Source |
| --- | --- | --- |
| Dispatch p50 / p99 / max, push acknowledgment to run listed (driver) | **3.0 s / 9.1 s / 17.6 s**, 2,498 of 2,498 paired (m57: 11.4 / 184.5 / 200 s) | `a1.mjs` `dispatch` |
| Dispatch, entry to run (site) | p50 1.6 s, **p99 7.5 s** (2,552 runs, 7 `no run`); `splitMs` p50 / p99: wait 0 / 0.5 s, demand 1.0 / 6.3 s, reads 0.4 / 1.4 s, create 0.4 / 0.8 s | `dispatchlog.py` |
| Split by term 4 (entry time) | before (to t4 − 5 s): p50 1.3 s, p99 2.7 s (1,104); during (t4 − 5 s to t4 + 60 s): p50 2.7 s, **p99 15.1 s** (305; demand p99 14.5 s, reads p99 6.6 s); after: p50 1.8 s, p99 3.9 s (1,166). Largest gap with no dispatch line 8.1 s (21:33:38Z, load start); 2.9 s at the term change (m57: 177 s) | `dispatch-lines.jsonl` |
| `warming` 503s on demand, tree, blob | **0** on all three sites in the load window; "no member answered" 0 (m57: the shard answered `warming` for 177 s, and site a logged "no member answered" 120 times in 18:48) | Workers Logs |
| Test-merge vCPU (term 4 leader's shard, 1 vCPU budget) | **0.62** over merge wall (232.3 s CPU over 376.1 s), 0.39 over the 600 s window; 7,211 computed, 995 shed `late`; `inFlightMax` 305, wait max 10.0 s | leader `/stats?summary=1` `testMerges.shard`, read after the row |
| List-field computes | **0** of 53,372 list-field asks | same |
| `mergeable` polls | **6,869 `MERGEABLE`**, 53,591 `UNKNOWN`, **7 × 503** (m57: 4,353 and 4,200) | `a1.mjs` `pollAnswers` |
| Agent pushes acknowledged | 2,498 of 2,500, p50 1.6 s, p99 4.2 s; 2 `fetch failed` at the driver | `a1.mjs` |
| Leader commit p50 / p99 | term 3 (f1, to the change): 434 / 1,052 ms; term 4 (f0, to the end): 423 / **3,094 ms** | `leader stats` row, leader `/stats` |
| Leader changes | f0 → f1 (term 3) 21:33:04Z, 28 s before load; f1 → f0 (term 4) 21:38:04.8Z, t0 + 273 s | `b035-m58.lead`, `/elections` |
| `STEP_DOWN` alarms | 2, both `CommitErrors` on f0 (21:32:41.682Z, calls 26 and 25, 3 failed each; 21:33:03.527Z, 51 and 51, 3 failed each), transfer to f1. **0 Confirm, 0 LoopLag** | Workers Logs |
| Worker outcomes | 27 "Subrequest depth limit exceeded" on site a at 21:33:01Z (setup, before load), 5 `actions dispatch` and 2 webhook lines among them; no `exceededMemory` | Workers Logs |
| Sandboxes started | 0 | `b035watch.sh` |

**The two leader changes.**
- **Term 3 (21:33:04Z, f0 → f1):** two `CommitErrors` trips on f0, 3 failed calls to each follower in the window, at the 27 "Subrequest depth limit exceeded" errors on site a at 21:33:01Z, which came while setup opened its 500 PRs. Not a Confirm trip, so P2 does not apply. `/tests/fanout/stepdown.py`'s window replay on f0's trace before it (21:32:30 to 21:33:04Z): storage p99 180 ms, whole p99 233 ms, 0 trips under either rule.
- **Term 4 (21:38:04.578Z, f1 → f0): the transfer back to `home`.** The group's `home` is site 0. f0 had stepped down at 21:33:03.527Z, so `RESET_QUARANTINE` (300 s) held it until 21:38:03.527Z, and f1, leading away from home, transferred back (`maybe_hand_off`: home caught up, HOME_CALLS answered, none failed). f0's `/elections` rows show real vote requests (`pre` 0) at 21:38:04.578Z, 1.05 s after its quarantine ended. Only a `TimeoutNow` starts a campaign with no pre-vote (`crates/raft/src/member/transfer.rs`, `on_timeout_now`). f0 led from 21:38:04.800Z. No `STEP_DOWN` fired, and every `/suspect` row on the voters (`Reported`, kind 28) names term 4, so none came before f0's campaign. f1 was healthy before it: every AppendEntries to f0 and f2 in the 15 s before ended ok (81 and 82, largest gaps 383 and 575 ms), confirms p99 106 ms, and no ticker lag of 200 ms or more. So this term change is the design working as written. Under load it still costs a dispatch tail: the term-change minute's p99 was 15.1 s (demand p99 14.5 s), against 2.7 s before it and 3.9 s after. Whether a planned transfer back to home should wait for a quieter moment is a design question, not a fault.
- **Replays (P2):** `stepdown.py` on the trace (storage = the Confirm row's ms, whole = Confirm end less each `Stepped` since the last), at 6 over 500 ms in 10 s: f1 for t0 to t4, storage p99 106 ms, whole p99 106 ms, 0 trips each; f0 for t4 to the end, storage p99 200 ms, whole p99 201 ms, 0 trips each. No Confirm window was logged, so no `stepDownWindow` line exists to compare. P2 was not exercised.
- **LoopLag:** no trip, and no ticker lag of 200 ms or more on any voter in the 20 s before term 4 (B0-02's isolate-clock fix is not judged here).

**The lookup-mark lines around term 4 (F101).** Site a's shard isolate logged 59 lines from 21:38:04.8Z to 21:38:13.8Z and site b's 6. Two holders on site a, t2526 and t2534, walked for 6.4 s and 9.0 s. Each pass posted the cached f0 (`Missed("no answer")` in 2 to 20 ms, f0 being mid-election and then busy), f1 by its new name `#8#f1` (`notLeader`, leader 0), f2 (`notLeader`, leader 0), and the record's bootstrap name `#1#f1`, the first-generation site-1 voter, which answered `notLeader(leader None)` in about 120 ms. Each pass then ended "no leader: repo:65572#1#f1: not leader", forgot the leader and walked again 250 ms later. t2526 found f0 at 21:38:11.174Z and t2534 at 21:38:13.783Z, each once f0 answered (35 ms and 1,566 ms). So the walk finds the leader once it answers, and nothing starved for minutes here. Two things are new: a superseded bootstrap name still answers and ends each pass, and the lookup forgets a leader that its followers name, because that leader missed once. Whether this explains m57's 2 min is not shown: m57 had no such trace, and this row's term change resolved in 9 s.

**Follow-up, code only (branch `r29-home-quiet`, `tests/m0/results/home-quiet.md`).** The transfer back to `home` now waits for a quiet leader (at most 16 commands queued, the entry in flight at most ENTRY_SOFT/2), and goes anyway 10 min after it was first due (`decisions/DECISIONS.md`, "`home-quiet`"). The lookup leaves out a bootstrap name the held config has replaced, and keeps for one more pass a leader that two followers name after its single miss (`lookup-keep`). Replayed on this walk in memory, neither `#1#f1` post happens, and the second pass goes to f0 as the leader.

**Cost** (`tests/fanout/acctcost.py`, from the site day's start at 21:24:11Z to 22:03:51Z): graft-m4a **$5.13**, the runner account $0.00, total $5.13 against the $8 stop; the account $49.69 for the UTC day, under $170. The watcher's last reading was $4.94 at 22:01:56Z; the shards idled while a1 cancelled the row's runs (21:45 to 22:02Z). The teardown sweep tore down `git#0#net:65572` and `git#1#net:65572` (both 200) at 22:03:00Z and found no shard container running; the containers API shows no m4a gitshard instance in a running state. The M2 canary's comparison excludes 21:24:11Z to 22:32:37Z.

## B0-35 on a test-faults build after site day 59 (2026-11-20, row m59)

Site day 59 (2026-11-19 23:42:54Z to 23:43:47Z) shipped main `857c4f02` as a **test-faults** build, with `home-quiet` and `lookup-keep`, and image `m4a56` (no `containers/git` change since it was built), with day 58's flags. Edge day 28 stayed. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m59` = repo 65573, with 8 learners. Run `mwo77i3i`: 500 of 500 PRs opened, the learner gate passed 8 of 8 in 503 ms, then 600 s from 23:51:25.434Z; done at 00:18:46Z. A first start at 23:44:01Z stopped before any repository because the service had no `TOOLS_TOKEN_FILE` (m57's cause again). The trace saver stopped after its first pull: the script reused m58's done file. A second pull at 00:14Z kept each voter's ring, which on site 0 reached back only to 00:10:51Z. Raw files: `tests/m4/raw/wp4/b035-m59/`, `b035-m59.{status,lead,watch}`, `tests/fanout/raw/tsave-m59.jsonl.xz`.

**Verdict: fails dispatch** (p99 18.6 s at the driver, against 10 s; fail over 30 s). Through minute 8 of the load it held p99 2.4 to 3.7 s, the best of any row. The tail is the last 30 s of the load, when f0 raised three held `STEP_DOWN` alarms. Budget, list-field, memory, warming and isolate-error lines pass.

| Line | Value | Source |
| --- | --- | --- |
| Dispatch p50 / p99 / max, push acknowledgment to run listed (driver) | 3.3 s / **18.6 s** / 25.1 s, 2,486 of 2,486 paired (m58: 3.0 / 9.1 / 17.6 s) | `a1.mjs` `dispatch` |
| Dispatch, entry to run (site) | p50 1.8 s, p99 17.3 s (2,983 runs, 9 `no run`); `splitMs` p50 / p99: wait 0 / 0.5 s, demand 1.0 / 6.7 s, reads 0.5 / 1.7 s, create 0.4 / 2.6 s | `dispatchlog.py` |
| By entry time | t0 to t0 + 570 s: p99 3.7 s (2,330; m58 8.1 s); per minute p99 2.4 to 3.7 s through minute 8, 7.8 s in minute 9; entries from 00:00:55Z: p99 24.7 s (182), demand and reads. Around the LoopLag alarm: before p99 4.9 s; from 5 s before to 60 s after, p99 24.7 s (91) | `dispatch-lines.jsonl` |
| `warming` 503s, "no member answered" | **0** and 0 on all three sites | Workers Logs |
| Test-merge vCPU (1 vCPU budget) | **0.59** over merge wall (379 s CPU over 639 s), 0.63 over 600 s; 11,646 computed, 1,781 shed `late`; `inFlightMax` 318, wait max 15.7 s | leader `/stats` `testMerges.shard` |
| List-field computes | **0** of 77,684 asks | same |
| `mergeable` polls | 4,562 `MERGEABLE`, 54,639 `UNKNOWN`, 397 × 503 | `a1.mjs` `pollAnswers` |
| Agent pushes | 2,986 of 3,000 ok, p50 1.6 s, p99 4.3 s; 10 × 503 (access rows shed), 4 `fetch failed` | `a1-pushes-…jsonl` |
| Leader commit p50 / p99 | 503 / **2,829 ms** (round p99 580, confirm p99 279) | leader `/stats` |
| Leader changes | **0**: f0 (home) led at term 2 throughout | `b035-m59.lead` |
| `STEP_DOWN` alarms | 2 on f0, both held: 00:01:18.265Z `LoopLag { clamped: 2 }` (to None); 00:01:42.9Z `CommitErrors` (site 1: 34 calls, 3 failed; site 2: 22, 3), to None. The third, 00:01:58.6Z `CommitErrors` (11/3, 11/3) to Some(1), came from `actions:7100001.9`'s site-a member, not this repository (see "Deeper analysis of m59") | Workers Logs, `stepdown-alarms.jsonl`, `stepdown-windows.json` |
| Transfer home | none; home led, so `home-quiet` was not exercised | |
| Lookup-mark walks | 16 on site a, 00:01:34 to 00:01:55Z, 0.9 to 1.9 s each (m58: 6.4 and 9.0 s); 14 passes "leader kept", each followed by f0 as cached leader; `#1#f1` posted 15 times, but as this group's live site-1 voter (Cached, never replaced); no superseded name posted | `lookup-mark.txt` |
| Isolate errors | "closure invoked recursively or after being dropped" **0**, "Cannot perform I/O on behalf of a different Durable Object" **0**, on site-a, site-b, site-c and the edge (23:42 to 00:06Z) | Workers Logs |
| Other | "Subrequest depth limit exceeded" 70 on site a (62 at 23:49 to 23:51Z in setup, 8 at 00:01Z); no `exceededMemory`; 0 Sandboxes | Workers Logs |

The same two error strings in the m58 window (21:24 to 22:04Z, build `f50ad36b`, disk-first without looplag-clock) were 0 on every m4a service. They were asked for after B0-29 m3fp3 on graft-m3f logged 220,515 and 3,897 on site a.

**Cost** (`tests/fanout/acctcost.py`, 23:42:54Z to 00:22:25Z): graft-m4a **$4.94**, the runner account $0.00, against the $8 stop. `b035.sh`'s sweep failed on shardkill's read-after-upload 10007, twice; a rerun of `teardown.py --sweep --shardkill` tore down 3 shards (200), found no shard container running and deleted shardkill (`b035-m59/sweep-rerun.log`). The M2 canary's comparison window excludes 23:30:00Z to 00:48:46Z.

### Deeper analysis of m59

Code and logs only, no hosted spend. Sources are Workers Logs for `graft-m4a-*` (00:00 to 00:04Z), `tsave-m59.jsonl.xz` (the followers' rings cover the stall; site 0's does not), `b035-m59/dispatch-lines.jsonl`, and the step-down windows, now saved in `b035-m59/stepdown-windows.json`. They were in each alarm event's `source.stepDownWindow`, not its message, so the m58 search for them by message found none.

**1. The stall, 00:00:55 to 00:01:58Z.** Dispatch's tail is one git shard catch-up, not the leader [measured]. The 32 of 42 runs over 10 s that ended in that minute ended at 00:01:55 to 00:01:57Z with `reads` 18.6 to 22.2 s or `demand` 15.9 to 23.4 s.
- Site a's shard (`eaa40cdb`) stored entries 3516 to 3528 one at a time, 200 to 500 ms each, until 00:01:33.2Z. Its next catch-up logged `heads` 1,875 ms and `ingest` 21,223 ms for **one entry, 3529, with 0 packs and 0 bytes**, and ended at 00:01:54.5Z. The next one stored 86 entries in 305 ms.
- Entry 3528 is f0's `Promoted { 2369..2645 }`, committed at 00:01:35.2Z after a 22.6 s promotion on the same shard. The archive's own log lines in this run also show promotions of 18.6 to 22.6 s each about 80 s apart from 23:56Z.
- While that catch-up ran, every read and demand naming a commit after 3528 waited on it (`disk-first-reads` waits when an object is missing). That covered 14 `/ui/tree` (wall up to 19.7 s), 235 `/demand` and 100 `upload-pack` with `wait=15` (wall 14.9 s each), all on `eaa40cdb`. The edge answered 42 of the `upload-pack`s 503.
- The page reads for 3529 reached the members only at 00:01:53.4 to 54.1Z: sites b and c without `local=1`, then f0 as leader. No `local=1` read of 3529 reached f0 in the 19 s before.
- From 00:01:34.9Z to 00:01:55.4Z the shard isolate's lookup mark for `repo:65573` was held by 13 walks in a row (t15103 to t15117), each 0.9 to 1.9 s. Each started at f1 and f2 with no cached leader, kept f0 after their `NotLeader`, and found f0. In that window the same isolate (`x-graft-sent-by`) sent f0 262 `/commit`s (the shard's `/demand` `Sync` commits, `commit_as`), 4 `/read`s and 1 non-local `/entries`.
- Where the catch-up's 19 s went between `heads` and its page read is **not traced**. [hypothesis] It waited behind that chain of walks: `GroupLog::page` falls back to `call`, which waits on the mark while another request's lookup runs. What drops the cached leader before each walk is also unknown: `commit_as` forgetting it on `done.notLeader`, or `learn_leader` on a reply with no `meta.term` (which sets `leader = None`). Recorded as F104.
- **Counterfactual** [inference, `dispatch-lines.jsonl`]: take 00:01:32.4 to 00:01:54.5Z out of every run that overlapped it, and the site's dispatch p99 is 7.1 s (p99.9 12.8 s, max 13.7 s), against 17.3 s measured.

What f0 did in that minute:
- **LoopLag at 00:01:18.265Z was real isolate lag** [measured, its window]. 31 confirms. Two that ended at 00:01:16.329Z took 1,768 and 1,816 ms from step to sync on the isolate clock, while their storage took 1,133 and 1,190 ms. Two more read whole ticks: 600 ms (sync 5132, storage 308) and 400 ms (sync 5133, storage 42). The rest were 305 ms or less. No `Trace::Lag` row survives: site 0's ring covers only to 23:48:53Z and from 00:10:51Z.
- **f0's work per `/raft` call grew through the load** [measured]. CPU per call went from 5.2 ms (23:51) to 8.0 (23:55), 11.3 (00:00) and 13.6 ms (00:01:00 to 00:01:45). Calls per minute fell from about 1,770 to 833, wall p99 rose from 128 to 1,118 ms, and f0's CPU reached 26 s a minute at 00:00. The 8 learners send almost all of them, and their share grew. `/raft` calls over 30 ms CPU were 62 (2.7 s) at 23:52 and 86 (11.0 s) at 00:00, all from learners. One `/commit` took 772 ms CPU at 00:00:24Z. Calls over 100 ms CPU were 1 in 23:52 and 49 to 57 a minute from 23:58. Only 70 of the 271 since 23:55:45Z fell inside a promotion, so promotion does not explain them. m57's f0 grew the same way (6.0 to 15.6 ms over 18:41 to 18:46). m58's f0, leader for about 6 min, stayed at 4.8 to 6.0 ms. What grows is **unknown**. The m58 voter-only minute had no learner `/raft` over 30 ms, so learner `ReadIndex` handling (`read_resolved` → `append_for_read`, held answers in `crate::callers`) is the place to look [hypothesis].
- **CommitErrors at 00:01:42.882Z: deadline misses, not errors** [measured]. The leader's `peers` counters for the run list every failed call to sites 1 and 2 as `deadline` (14 and 20), with none failed otherwise. Attributing the window's 3 and 3 to deadlines is [inference]. On the followers, the leader's AppendEntries took 0.6 to 1.0 s from its `x-graft-sent` to arrival in 00:01:36 to 42 (2.7 s at 00:01:35.2Z, 1.4 s at 00:01:42.95Z), and each was handled in 30 ms or less. That fits a lagging sender. The window's storage max was 697 ms, with isolate samples of 966 and 957 ms.
- **The 8 "Subrequest depth" at 00:01:56Z were not f0's.** They were 6 and 2 on `actions:7100001.9`'s site-a member (`b336ddfa`), on two `/read`s. Neither alarm lines up with them.
- **The third alarm (`to: Some(1)`) was that actions group, and it did transfer.** It tripped at 00:01:58.625Z on 11/3 and 11/3 calls. Its window's 9 confirms were all 59 ms or less, so storage was not the cause. From 00:01:58.6Z `b336ddfa` takes `/raft` calls from site 1, and `runsvc` moved to site b's member (`dee2a6f8`, 249 calls 00:02 to 00:05Z). The m59 table above has been corrected.

**2. Shared isolates.** Not checkable for m59. The saver ran without `ISOLATES`, so `tsave-m59` has no `/isolate` rows. `b035tsave.sh` now sets `ISOLATES=1`, so the next row has them.

**3. The 70 "Subrequest depth limit exceeded" on site a.**
- **62 in setup (23:49 to 23:51Z)**, on 11 invocations of f0 (`203ba6bc`): 9 `/read` and 2 `/entries?…&local=1`. Every one was sent by the git shard's isolate (`x-graft-sent-by` isolate 2603326811: `/read` as Leader, `/entries` as Other). Inside them f0's own cursor consumers logged the errors, for example "actions dispatch 65573 #259 at 651", "activity 65573", "webhooks 65573: site 0", "webhooks store site 0". The shard's `/demand` invocation `d283ce5c` failed the same way.
- **Chain** [inference from the code and these headers]. The runtime charges an object's outgoing calls to its newest incoming request (`workers/site/src/callers.rs`). f0's Actions dispatch calls the shard (`prsync::demand` → `/demand`, and `/ui/tree`). The shard's catch-up and demand call f0 back (`GroupLog::page`, `/read`). f0's consumers' next calls then leave charged to that request, one loop deeper each time. That is the class `prsync::in_band` and `archive::in_band` already escape through the `SITE` binding.
- **8 at 00:01:56Z**, on the actions group's two `/read`s, sent by another isolate (3432527746).

**4. Harness fixes.** These are on this branch (`b035tsave.sh`, `env-m4a.sh`, `teardown.py`).

**Built after the analysis (branch `r31-catchup-local`, code only, `decisions/DECISIONS.md`, `catchup-local`).** The catch-up's `ranges` reads go by read-index to this site's member first, and pages keep going there first with `local=1`; neither takes the lookup mark, and each falls back to the lookup as before. A wait on another request's mark now logs its time and holders, and a traced post to the cached leader that is not a reply logs its answer. For m59's own case, site a's member was the leader f0, and main already sent the page read there first; no `local=1` read of 3529 reached f0 in those 19 s [measured], so the next row's lines must show whether that post missed (F104). Plants 150 to 157, 8 of 8 caught.

**Ranked change.** Make the git shard's catch-up independent of the isolate's lookup walk. Read the log's `ranges` and pages from this site's member first, as `GroupLog::page` already does for `/entries`, and fall back to the leader only on a miss. Log the wait when a fallback queues behind another request's walk. Expected effect [inference, the counterfactual above]: dispatch p99 about 7 s against 18.6 s. That holds only if the 19 s really was that wait, which the new log line would show on the next row. F104 asks what drops the cached leader. Second, not ranked: run actions dispatch's shard calls in a fresh invocation (`in_band`), which would remove the setup depth errors.

## B0-35 on a test-faults build after site day 60 (2026-11-20, row m60)

Site day 60 (05:05:21Z to 05:06:27Z) shipped main `96a91d58` as a **test-faults** build, with `catchup-local`, `midx-background` and `chain-open`, and git shard image `m4a60`, rebuilt because `containers/git` changed in b034-midx (`gitimage-m4a60.log`). Day 59's flags; edge day 28 stayed. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m60` = repo 65574, with 8 learners, and `b035tsave.sh` with `ISOLATES=1`. Run `mwoipaxb`: 500 of 500 PRs opened, the learner gate passed 8 of 8 in 482 ms, then 600 s of load (agent pushes 05:10:08Z to 05:23:34Z); done at 05:40:24Z. Raw files: `tests/m4/raw/wp4/b035-m60/`, `b035-m60.{status,lead,watch}`, `siteday60.log`, `tests/fanout/raw/tsave-m60.jsonl.xz`.

**Verdict: fails dispatch** (p99 26.2 s at the driver, against 10 s; fail over 30 s). Up to 05:23:20Z the site's p99 was 5.4 s, and from 05:10:30Z 3.2 s (per minute 1.8 to 3.7 s). The tail is one event at the end of the load, 05:23:20 to 05:23:50Z on site a. Site a's posts to its cached leader f0 failed at once. A chain of lookup-mark walks followed, then f0's `CommitErrors` trip moved the leader to f2. Budget, list-field, memory and isolate-error lines pass.

| Line | Value | Source |
| --- | --- | --- |
| Dispatch p50 / p99 / max, push acknowledgment to run listed (driver) | 3.0 s / **26.2 s** / 70.6 s, 2,485 of 2,485 paired (m59: 3.3 / 18.6 / 25.1 s) | `a1.mjs` `dispatch` |
| Dispatch, entry to run (site) | p50 1.5 s, p99 66.3 s (2,991 runs, 16 `no run`); `splitMs` p50 / p99: wait 0 / 3.8 s, demand 1.0 / 52.0 s, reads 0.5 / 1.8 s, create 0.3 / 9.7 s | `dispatchlog.py` |
| By entry time | to 05:23:20Z: p99 5.4 s (2,966); 05:10:30 to 05:23:20Z: p99 3.2 s (2,916); entries 05:23:20 to 05:23:50Z: p99 88.4 s (74), demand p99 74 s. No entries after term 3 or term 4: the load had ended | `dispatch-lines.jsonl` |
| Leader changes | **2**. Term 3 at about 05:23:32Z, f0 to f2, on `StepDown { CommitErrors { (1, 26, 3), (2, 27, 3) }, to: Some(2) }` at 05:23:33.683Z, the row's only alarm. Term 4 by 05:28:29Z, f2 to f0, no alarm: the transfer back to `home` about 5 min (RESET_QUARANTINE) after f0's step-down. The load was over, so it was not deferred [inference; `home-quiet` logs no line] | `b035-m60.lead`, `stepdown-alarms.jsonl` |
| Cached-leader post misses | **125**, all `0/repo:65574#1#f0: Missed("no answer")` on site a, 05:23:25.285 to 05:24:50Z, each in **2 to 98 ms**: the call threw at once, not a CALL timeout. Site a logged "Network connection lost" at 05:23:23.591Z (activity 65574) and three at 05:23:29.586Z | `lookup-mark.txt`, Workers Logs |
| Mark waits | 130 (site a 106, site c 24), p50 5.8 s, max 19.3 s; 123 with "cached leader tried by it: false". The holders were a chain of 17 or more walks (t11608, t11612, t11627, …), each 1 to 2 s. Then 193 "gave up" at LOOKUP_FOR (20 s) on site a, 05:23:45 to 05:24:22Z. The walks' own posts to f0 answered (`Reply` in 0.3 to 1.7 s) | `lookup-mark.txt` |
| Local catch-up reads not taken | 1: site a 05:23:25.794Z, "local /read at 0/…#f0: no answer in 2000 ms, to the leader" (site a's member was f0) | `lookup-mark.txt` |
| `chain_open` / `pages_ms` | `chain_open` 0 lines (not exercised). `pages_ms` 3,288 lines: site a ingest p50 / p99 22 / 226 ms, room p99 85 ms, catch-up p50 / p99 183 / 683 ms; sites b and c one rehydrate each at 05:24:30Z with ingest about 34 s, after term 3 | Workers Logs |
| Shared isolates | none: 11 members, 12 isolates | `isolates.py` on `tsave-m60` |
| Isolate errors | "closure invoked recursively or after being dropped" **0**, "Cannot perform I/O on behalf of a different Durable Object" **0**, on sites a, b and c (05:05 to 05:42Z) | Workers Logs |
| `warming` 503s | **15** (m58, m59: 0): 14 `shard tree` on site c at 05:24:07Z, during its warm after term 3; 1 `demand` on site a at 05:23:45Z. "no member answered" 0 | Workers Logs |
| Test-merge vCPU (1 vCPU budget) | **0.60** (370 s CPU over 616 s wall); 11,781 computed, 1,360 shed `late`; `inFlightMax` 305 | leader `/stats` `testMerges.shard` |
| List-field computes | **0** of 77,162 asks | same |
| `mergeable` polls | 3,586 `MERGEABLE`, 55,875 `UNKNOWN`, **796 × 503** (m59: 397) | `a1.mjs` |
| Agent pushes | 2,485 of 2,500 ok, p50 1.6 s, p99 3.8 s; 15 × 503 (access rows shed) | `a1-pushes-…jsonl` |
| Leader commit p50 / p99 | 441 / **1,690 ms** (round p99 553, confirm p99 210); isolate lag max 1,777 ms. Read at the end, from f2 | leader `/stats` |
| Other | "Subrequest depth limit exceeded" 15, all on site c at 05:24:13Z (site a 0; m59 70). No `exceededMemory`. Site a's member of `actions:7100001.1` reset at 05:24:57.756Z on "fold storage lost … Network connection lost". 0 Sandboxes | Workers Logs |

**F104.** The hypothesis holds: every walk started with the request's own post to its cached leader missing, and none started with a dropped cache. This time the miss was a call that threw within milliseconds on site a's isolate, beside "Network connection lost" there, while the walks' posts to the same member answered. Why the cached post fails and the walk's does not is in `decisions/OPEN-QUESTIONS.md`, F104.

**Cost** (`tests/fanout/acctcost.py`, 05:05:21Z to 05:48:05Z): graft-m4a **$5.43** (account $5.47), the runner account $0.00, against the $8 stop. Teardown: 3 shards 200, no shard container running, shardkill deleted; the b035 services stopped. The M2 canary's comparison window excludes 04:53:00Z to 06:10:24Z.

## B0-35 on a test-faults build after site day 61 (2026-11-20, row m61)

Site day 61 (09:34:49Z to 09:35:47Z) shipped main `9d6fdb71` as a **test-faults** build, with `step-agree` and `alarm-steps`. Day 60's flags; edge day 28 stayed. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m61` = repo 65575, with 8 learners, and `b035tsave.sh` with `ISOLATES=1`. Run `mwoscm62`: load 09:44:21.8Z to about 09:54:22Z; done at 10:14:54Z, exit 0. Raw files: `tests/m4/raw/wp4/b035-m61/`, `b035-m61.{status,lead,watch}`, `siteday61.log`, `tests/fanout/raw/tsave-m61.jsonl.xz`.

**Verdict: fails dispatch** (driver p99 90.2 s, p50 10.3 s, 2,285 of 2,285 paired; site entry-to-run p50 7.2 s, p99 107 s). The verdict lines went to the lead within 30 minutes of the done line. In short:
- Leader commit p99 10.9 s, almost all of it queueing.
- Term 3 at 09:52:49.1Z was a transfer (TimeoutNow, no pre-vote), after `step-agree`'s hold window lapsed and the group was no longer hot. 302 `suspect` reports started no pre-vote.
- Term 4 at about 09:57:30Z was the transfer home.
- f0 logged 29 alarm-steps lines. `activity` (15.7 s in all) and `load` (8.3 s) are mostly waiting, not CPU.
- Cost: graft-m4a $4.68 to 10:23:39Z, the runner account $0; teardown clean.

**Correction to the verdict.** The interim line said f0's `/raft` burned "20 to 34 CPU-s per 10 s". That came from a two-key groupBy in the Workers calculations view, which double-counts, and it is wrong. GraphQL `durableObjectsPeriodicGroups` for f0's object gives CPU-s per minute: 09:44 19.3, 09:45 29.7, 09:46 37.1, 09:47 34.3, 09:48 35.8, 09:49 34.3, 09:50 36.2, 09:51 35.8, 09:52 24.0. So f0 used **about 0.6 vCPU sustained** against 60 s of active time a minute, for the whole object and not `/raft` alone [measured].

### f0's `/raft` CPU on m61, by call kind

Raw Workers events for f0, read in 1 s slices, which are near complete (the calculations view sampled about a quarter of the `/raft` calls), 09:48:00 to 09:49:39Z, joined to the learners' and the leader's trace rows (`tsave-m61`). Of 45.5 s of f0 CPU in that window [measured]:

| Kind | Calls | CPU | CPU a call | Bytes a call | Source |
| --- | --- | --- | --- | --- | --- |
| Learner `/raft` carrying a `ReadIndex` | 7,970 | **22.9 s (50%)** | 2.87 ms | request under 100 B. The response holds the `ReadIndexRes` and any held appends: 4,520 had none, 4,107 had one and 1,120 two or more, at 64 to 101 KB each | learner `CallSent` + `IndexSent`, leader `ResSent`/`AppendTaken` |
| Learner `/raft` with no `ReadIndex` (`AppendRes`, answers) | 2,947 | 4.6 s (10%) | 1.58 ms | under 100 B in, the same held appends out | same |
| AppendEntries to the voters (f0's own calls) and their answers in f0's handlers | 523 inbound voter `/raft` | 1.3 s (3%) inbound. The encode and send run in f0's `/commit` and alarm turns, counted below | 2.5 ms | 83 to 87 KB a call: 1,373 calls to f1 and 1,421 to f2 over the load, 113 and 124 MB | leader `AppendQueued` |
| Votes and heartbeats | 0 votes in the window; empty rounds ride the voter row | ~0 | | | |
| `/commit` (admission, cut, apply, the voter appends' encode) | 3,032 | 7.1 s (16%) | 2.3 ms | about 90 KB pack | raw events |
| `/read` (leader reads) | 1,112 | 3.9 s (9%) | 3.5 ms | | raw events |
| `/entries` | 139 | 1.6 s (4%) | 11.3 ms | | raw events |
| alarm | 210 | 1.2 s (3%) | 5.7 ms | | raw events |
| other (`/outcome`, `/status`, `/stats`) | | 0.9 s+ | | | raw events |

Learner `/raft` was 27.5 s (60%) of f0's CPU in the window. 31 of those calls took 100 ms or more of CPU, 10.4 s together, and the rest took 1.57 ms each. The heaviest, 2,015 ms, was a learner call at 09:48:53.9Z whose span held one leader step sending 754 KB of appends. It covered a 2,056 ms ticker stall.

**Duplicates.** Over the whole load f0 received 46,250 learner read-index calls. 9,535 of them, 21%, were copies of batches already in flight: 7,089 `HEDGE` copies and 2,446 `CALL_DEADLINE` resends [measured, learner traces]. By minute, the share of copies (HEDGE copies plus calls past 1 s, each resent) against the mean admission queue:

| Minute of load | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Copies, % of read-index calls | 7.2 | 13.6 | 35.0 | 28.4 | 31.4 | 40.7 | 38.1 | 40.1 | 37.7 |
| Mean queued commands | 9 | 18 | 91 | 50 | 39 | 159 | 23 | 44 | 83 |

**Does the read-index rate grow with the queue?** Not the base rate: learner batches to the leader ran 39/s, then 65 to 88/s for all 8 learners, and their correlation with the queue is −0.32. The copies do grow, from 1.4/s to 40/s at the peak, correlation +0.51. Queued commands make the leader slow to answer, and every batch slower than `HEDGE` is copied, so the retries add calls exactly when the leader is slowest. Learner ReadIndex call time, 09:46 to 09:52Z: p50 261 ms, p90 1,000 ms, p99 2,427 ms [measured].

**Entry sizes (the lead's question after B0-02's m3fgt1 finding).** f0's cut trace (`Trace::Cut`, `tests/m0/cuttrace/cuts.py`) over the load shows 1,296 entries. Commands per entry: p50 7, p90 20, p99 48, max 80; 1 entry over 64, none at 100 or more. Bytes per entry: p50 75.7 KB, p90 102 KB, p99 116 KB, max 147 KB. 969 cuts ended `Soft` and 327 `Empty`. Every command was classed Human in the cut columns, so an Agent-only cap binds on none of them. The 34 f0 `/raft` turns with 100 ms or more of CPU in the window had at most 24 commands in any entry cut within their span, and no cut of 40 or more commands fell in any of them [measured]. So on m61 **f0's CPU-heavy turns do not line up with big entries**. m61's entries are 2 to 4 times smaller than m3fgt1's 160 and 191, and its CPU is many small learner calls, not a few large applies. B0-02's `AGENT_CMDS_ENTRY` cap and this change are complementary. File ownership in `crates/raft` was agreed with B0-02: this branch touches `install.rs` and `tests/leadership.rs` only, and admission, the cut, and `lib.rs` are B0-02's.

### `read-newest`: the change

The dominant kind is learner read-index calls, and about a fifth of them were copies of batches in flight. Design first (`design/DESIGN.md`, Shape `fresh`, "Follower read", the Scale learners row; `decisions/DECISIONS.md`, "`read-newest`"):
- an answer to batch b at index i grants b's reads and those of every batch sent before b, never a later one (`crates/raft/src/member/install.rs`, `on_read_index_res`). The earlier reads arrived before b left, so before the leader took it and confirmed i;
- `tick_remote` resends only the newest batch in flight once it is `CALL_DEADLINE` old;
- the site host copies a learner's `ReadIndex` call at `HEDGE` only while its batch is the newest it sent (`workers/site/src/host.rs`, `hedge_due`). Other learner calls are hedged as before.

**Replay** (`tests/fanout/readnewest.py tests/fanout/raw/tsave-m61.jsonl.xz 1795167860000 510`, every call's traced time held, so the leader's lighter load is not credited) [inference]:
- f0's learner read-index calls fall from 46,250 to 36,467 (21% fewer). 6,751 of the 7,089 copies and 2,440 of the 2,446 resends are dropped, and 592 copies of dropped resends with them.
- Batch waits fall from p50 / p90 / p99 222 / 796 / 2,256 ms to 218 / 713 / 1,778 ms, since a slow batch is now granted by a later answer.
- At 2.87 ms a call, that is about 28 s of f0's CPU over the 510 s load, 0.055 vCPU or about 9% of its 0.6 vCPU. The fix takes away the retry loop that feeds on the leader's slowness, not most of its CPU. The rest of the learner call cost, 1.6 to 2.9 ms each at 110 to 140 calls/s, is per call, not per byte, and the next lever would be fewer learner calls per read.

**Expected effect on dispatch** [guess]: small on its own. The m61 tail is queueing behind one entry in flight at about 0.6 vCPU. This change removes about a fifth of the learner calls at the moments of highest queue, which helps the leader's input queue, and does not change the commit path.

**Results.**
- `a_later_read_index_answer_grants_earlier_batches_and_only_the_newest_is_resent` (`crates/raft/tests/leadership.rs`) fails on main (main resends all three batches) and passes on the branch. It checks that every read is granted at or above a write acknowledged before the answer.
- `each_read_index_answer_grants_only_its_own_batch` keeps every floor check. Reversed, it now also sees the first read granted by the second answer.
- `a_learner_hedges_only_its_newest_read_index_batch` (`workers/site/src/host.rs`).
- Plants: `tests/m0carried/plant63.py`, 6 of 6 caught (the oldest batch resent; no earlier batch granted; later batches granted; the resend ignoring `CALL_DEADLINE`; every read-index call hedged; none hedged). `tests/nr3/plant.py`'s read-index and floor mutants (0, 28 to 32): 6 of 6 caught. Its `--check` names one residue, mutant 7 in `workers/site/src/family.rs`, and that residue is also on main `9a6db022`: the mutant's original line now appears there twice, which is not a planted file.
- Long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, release), on the branch: harsh 0 step-downs and 8,206 holds, longest stuck stretch 14,707 ms (seed 10439); gray 2,133 and 10,374, 28,501 ms (seed 11893); live 1,747 and 10,251, 27,362 ms (seed 12553); passed in 949 s. Main `9a6db022`, same command in a worktree: harsh and gray identical to the branch; live 1,782 step-downs and 10,443 holds, 25,191 ms (seed 13897); passed in 999 s. Harsh and gray are identical, counter for counter. In live the branch completes more reads: 3,800,585 against main's 3,602,538 (+5.5%), because a late answer now grants the batches behind it. Porcupine (`tests/m0/porcupine`, built, `-seg 2000`) on the branch's histories, each exit 0: `hist.jsonl` Ok, 146,836 operations; `.gray` Ok, 2,336,111; `.live` Ok, 5,476,993 (12,000 segments).
- Checks, each by its exit code, all 0: `cargo fmt --check`; workspace clippy `-D warnings`; wasm32 clippy for site and edge with and without `test-faults`, and for front and runners; `cargo test --workspace` 1,306 passed, 0 failed; `node --test workers/runners/tests/*.test.mjs` 11 of 11; `go test ./...` in `containers/git/server`, `containers/ssh/server` and `tests/m0/porcupine`; `check_design.py`, `check_names.py`, `check_links.py`.

## B0-35 on a test-faults build after site day 62 (2026-11-20, row m62)

Site day 62 (14:14:19Z to 14:15:44Z, exit 0, 6 versions) shipped main `99f614ee` as a **test-faults** build, with `read-newest` on top of `agent-cap`; image `m4a60` (`sha256:e55ab2ad…`) and edge day 28 stayed. `tests/m4/wp4/b035.sh` (driver commit `90c01b5c`) ran on a new repository, `m3org/b035-m62` = repo 65576, with 8 learners, and `b035tsave.sh` with `ISOLATES=1`. Run `mwp2c2od`: load 14:25:12.929Z to 14:35:13Z; done at 14:52:53Z, exit 0. Raw files: `tests/m4/raw/wp4/b035-m62/` (`dispatchlog-whole.json`, `dispatch-lines.jsonl`, `stepdown-alarms.txt`, `alarm-steps-f0.txt`), `b035-m62.{status,lead,watch}`, `siteday62.log`, `tests/fanout/raw/tsave-m62.jsonl.xz`.

**Verdict: fails dispatch** (driver p99 38.6 s against 10 s, past the 30 s fail line; m61 was 90.2 s). The verdict lines went to the lead at about 14:55Z, inside 30 minutes of the done line.

| Line | m62 | m61 |
| --- | --- | --- |
| Dispatch, driver | p50 5.5 s, **p99 38.6 s**, 2,415 of 2,423 paired | p50 10.3 s, p99 90.2 s |
| Dispatch, site entry-to-run / demand | p99 32.9 s / 30.6 s | p99 107 s |
| Dispatch p99 by minute of load | 4.1 to 5.4 s through minute 4; 18.2 s (5), 20.9 s (7), 45.8 s (8), 44.0 s (9) | |
| Leader commit | p50 1.0 s, p99 18.0 s | p99 10.9 s |
| Leader queue (slot) | p99 17.8 s (17.6 s) | |
| Round / confirm / deps | p99 880 ms / 898 ms / 1.4 s | |
| f0's vCPU (GraphQL CPU-s per minute from 14:25) | 23.4, 23.6, 26.3, 27.1, 31.2, 31.0, 33.5, 40.0, 31.0, 27.1: mean 0.49, peak 0.67 | about 0.6 sustained |
| Learner read-index calls to f0 | 44,515 sent, plus 1,904 `HEDGE` copies (**4.1%**); 2,543 past `CALL_DEADLINE` | 46,250, 15.3% `HEDGE` copies, 21% copies in all |
| Leader changes | **0**. Holds: 14:32:43 CommitErrors, 14:33:07 LoopLag, 14:35:19 CommitErrors (each `to: None`); no step-down | 2 (a transfer and the transfer home) |
| `suspect` | each follower logged 2,000 or more reports from 14:33:35 to 14:35:00; no pre-vote | 302, no pre-vote |
| f0 alarm-steps lines | 17: `activity` 8 lines and 8,363 ms; `load` 4 and 4,075 ms; `prsync` 3 and 2,028 ms; `actions_trigger` 4 and 1,088 ms; `split` 157 ms; `hookseq` 111 ms | 29 |
| Pushes | 2,423 of 2,500 ok, 76 answered 503; `mergeable` 503s 5,089 | |
| Shared isolates (`isolates.py`) | none | |
| Spend | graft-m4a **$4.41** (14:14:19Z to 14:54:51Z, `acctcost.py`), runner account $0 | $4.68 |
| Teardown | 1 shard 200, no container running, shardkill deleted (14:53:20Z) | clean |

`read-newest` did what its replay said: `HEDGE` copies fell from 15.3% to 4.1% of learner read-index calls, and f0 averaged 0.49 vCPU against m61's 0.6. Dispatch improved from 90.2 to 38.6 s, but the tail is still queueing behind one entry in flight, and the step-agree rule held three times with no leader change.

### What the queue waits behind at about 0.5 vCPU

From f0's cut and append trace (`Trace::Cut` and the leader rows in `tsave-m62`, `tests/fanout/entrycycle.py tests/fanout/raw/tsave-m62.jsonl.xz 1795184712929 600 0`) [measured]:
- **Round time does not track commands.** Round p50 is 206 to 245 ms across command bands, and it rises only from 198 to 257 ms between entries under 40 KB and entries over 130 KB.
- **The cut is byte-bound.** Cuts that ended `Soft` rose to 158 of 159 by minute 9, and commands left queued after a cut rose from 33 to 350.
- **Apply does not block the next cut.** The append-to-append cycle is p50 273 ms: the voter round is 80% of it, apply about 1%, and apply-to-next-append 19%.
- **The voter round is wire and queues, not the follower's handler.** Follower handler p50 is 24 ms. Send-to-answer less the handler is p50 235 ms, with p90 rising to about 1 s in minutes 7 and 8.
- **Ceiling.** One entry of about 100 KB a round is about 200 KB a second, against about 209 KB a second offered, so the queue grows whenever the round slips.

The lever this points to is bytes per round while the queue is deep, not more entries in flight, which AGENTS.md rules out.

**End-of-load `/commit` storm: not edge receipt re-sends** (analysis only, from Workers Logs events for f0's object, every `/commit` 14:24:30 to 14:37:00Z, read in slices of at most 4 s so no page saturated; `x-graft-sent-by` decoded as isolate × 16 + step) [measured]:
- f0 received 9,763 `/commit`s. 2,793 had no `x-graft-sent-by`: the edge's, whose day-28 build is not test-faults and sends none. 6,312 came from **one site-a isolate** with step `Leader`, which is `commit_as`'s post to the cached leader, not `commit_hedged`'s `Send`. The remaining 658 came from other site isolates. No `/commit` carried `Send` (4), so none came from the edge's receipt path.
- By minute (that isolate / edge / other): 14:30 621 / 297 / 36; 14:31 415 / 256 / 31; 14:32 436 / 243 / 27; **14:33 1,273 / 298 / 22; 14:34 1,605 / 319 / 30**; 14:35 377 / 130 / 77. The edge's rate stayed flat at about 5 a second through the storm, while that one isolate went from about 7 to 27 a second (peak 97 in one second).
- The isolate is site a's git shard for `net:65576` (`GitShard` object `5e83e4c2…`) [inference: `x-graft-sent-by` names an isolate, not an object; every cached-leader `lookup mark` line in the window came from that object, and only a site isolate sends the header]. Its `lookup mark` lines in the same window come from its `/upload-pack`, `/demand`, `/ask`, `/catchup` and `/ui/*` handlers, and its `/demand` calls carry `cause=get`: on-demand test merges for single-PR reads, which commit `RepoCmd::Sync` through `commit_as` (`workers/site/src/shard.rs`, the `/merge` and `/demand` arm). Each `commit_as` re-posts the same request id every 200 ms while the leader answers `admitted` (held past `COMMIT_HOLD`, 1 s). In 14:33 to 14:34, 59% of its `/commit`s ran 1.0 s or longer at f0 (p50 1,044 ms, p90 1,837 ms), against 18% from 14:25 to 14:32: the posts were waiting out `COMMIT_HOLD` behind the queue and then posting again.
- So the storm is the shard's `mergeable` test merges for the B0-35 driver's PR reads, plus their `admitted` re-posts while f0's queue was 17 s deep. It is not the edge, and not the receipt rule. Site a's 1,755 "Durable Object is overloaded" throws were these posts. The row's 5,089 `mergeable` 503s are the other side of the same load.
- Not built here. If it recurs after `soft-grow` shortens the queue, the lever is in `commit_as` (wait for the ticket by request id, as the edge does, rather than re-posting the command every 200 ms) or in the shard's `/demand` (shed `cause=get` merges while the group is hot). That needs its own decision.

### `soft-grow`: the change

The lead's decision after m62: raise bytes per round while the queue is deep, keeping one entry in flight. Design first (`design/DESIGN.md`, Shape `EntryBody` and the write path, the Scale entries row, CF-ACT-4; `decisions/DECISIONS.md`, "`soft-grow`"). File split agreed with B0-02, who owns admission and the cut's agent rules: this branch changes only the cut's soft bound in `crates/raft/src/member/leader.rs`, plus `ENTRY_SOFT_DEEP` and `DEEP_QUEUE` in `crates/raft/src/lib.rs`.
- At each cut the leader sums the stored bytes of the queued Human and MergeQueue commands. Over `DEEP_QUEUE` (2) times the soft bound, the cut fills to `ENTRY_SOFT_DEEP` (150 KB); otherwise to `ENTRY_SOFT` (100 KB), so the bound drops back at the first cut that finds the queue shallow.
- Admission capacity and the agent bucket stay sized from `ENTRY_SOFT`. `ENTRY_MAX`, `AGENT_INLINE_ENTRY` and `AGENT_CMDS_ENTRY` are unchanged.
- Why 150 KB: a far voter's AppendEntries first missed `CALL_DEADLINE` at 150 KB (`APPEND_BYTES_MAX`). B0-02's m3fst1 (first row with `Trace::Sync`) had leader confirm wall p50 51 ms, p99 243 ms, max 1,449 ms. Its three confirms of 1 s or more, each labelled `storage`, were 8.1, 75.5 and 175.5 KB, and one of 191 KB took 579 ms, so confirm time did not track size in that range.

**Replay of m62's queue** (`python3 tests/fanout/softgrow.py tests/fanout/raw/tsave-m62.jsonl.xz 1795184712929 600 0`) [inference]. Arrivals come from each cut's taken and left Human counts, with sizes in FIFO order. Each entry's cycle is the row's own append-to-append time at that moment, moved by a least-squares fit over m62's 1,760 entries: 306 ms plus 0.49 ms per KB, so 355 ms at 100 KB and 379 ms at 150 KB. Offered: 14,511 commands, 183 KB/s mean.

| Rule | Entries | Queue wait p50 / p99 | Peak queue | Back under one entry after the peak |
| --- | --- | --- | --- | --- |
| Flat `ENTRY_SOFT` (main) | 1,677 | 0.7 s / **18.0 s** | 357 commands (at 496 s) | 49.1 s |
| `soft-grow` | 1,668 | 0.6 s / **5.6 s** | 134 commands (at 513 s) | 1.5 s |

The flat replay's p99 of 18.0 s matches m62's measured leader queue p99 of 17.8 s, which supports the model. On m61's trace the same replay gives flat p99 15.7 s and soft-grow p99 7.3 s. **Expected effect on dispatch** [guess]: if the queue tail falls from about 18 s to about 6 s, dispatch p99 should fall by about the same 12 s, from 38.6 s toward 25 s. That is still over 10 s, since dispatch also carries the demand, reads and create stages. The replay does not credit a leader that is lighter because it cuts fewer entries.

**Results.**
- `a_deep_human_queue_fills_entries_to_entry_soft_deep_and_drains` (`crates/raft/tests/admission.rs`) fails on main and passes on the branch. On main the first deep cut takes 9 pushes, not 14. Its load half offers 40 pushes/s of 10 KiB for 20 s at a round of about 350 ms. Main reaches commit p50 6,090 ms, p99 11,470 ms, at most 286 queued after a cut; the branch reaches 1,205 ms, 1,800 ms and 44.
- `each_cut_records_what_ended_it` now queues 15 pushes (154 KB, not deep) for its `ENTRY_SOFT` case. `a_backlog_splits_into_entries_within_entry_soft_in_order` checks each entry against the bound for the queue behind it: 300 KB queued splits 154, 102 and 56 KB.
- Plants: `tests/m0carried/plant65.py`, **6 of 6 caught**. The mutants: the bound never grows; it grows on any queue; it grows to `ENTRY_MAX`; it never drops back; depth counts commands, not bytes; `DEEP_QUEUE` of 8.
- Long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, release), branch and main `4ec26547` side by side. **Identical counter for counter**: harsh 0 step-downs and 8,206 holds, longest stuck stretch 14,707 ms (seed 10439); gray 2,133 and 10,374, 28,501 ms (seed 11893); live 1,747 and 10,251, 27,362 ms (seed 12553). Both exit 0, in 1,002 s and 1,046 s. The sim's commands are short key-value writes of tens of bytes, so a queue past `DEEP_QUEUE` entries' worth (200 KB) would need thousands queued, and the identical counters say no cut grew [inference]. The sweep shows that the change leaves the fault paths as they were; the admission tests above exercise the grown bound.
- Porcupine (`tests/m0/porcupine`, built, `-seg 2000`) on the branch's histories, each exit 0: `hist.jsonl` Ok, 146,836 operations; `.gray` Ok, 2,336,111; `.live` Ok, 5,476,993 (12,000 segments).
- Checks, each by its exit code, all 0: `cargo fmt --check`; workspace clippy `-D warnings`; wasm32 clippy for site and edge with and without `test-faults`, and for front and runners; `cargo test --workspace` 1,312 passed, 0 failed (on the merge with main `b8e555df`); `node --test workers/runners/tests/*.test.mjs`; `go test ./...` in `containers/git/server`, `containers/ssh/server` and `tests/m0/porcupine`; `check_design.py`, `check_names.py`, `check_links.py`.

## B0-35 on a test-faults build after site day 63 (2026-11-20, rows m63, m64 and m65)

Site day 63 (17:02:58Z to 17:04:00Z, exit 0) shipped main `63e82533` as a **test-faults** build, with `soft-grow` on top of `read-newest` and `agent-cap`. Image `m4a60` (`sha256:e55ab2ad…`) and edge day 28 stayed. `tests/m4/wp4/b035.sh` ran on a new repository, `m3org/b035-m63` = repo 65577, with 8 learners, and `b035tsave.sh` ran with `ISOLATES=1`. Run `mwp8i0vt`: load 17:16:11.086Z to 17:26:11Z; done at 17:44:03Z, exit 0. Raw files: `tests/m4/raw/wp4/b035-m63/` (`dispatchlog-whole.json`, `dispatch-lines.jsonl`, `stepdown-alarms.txt`, `alarm-steps-f0.txt`, `grown.txt`, `commitsenders.txt`, `f0cpu.txt`), `b035-m63.{status,lead,watch}`, `siteday63.log`, `tests/fanout/raw/tsave-m63.jsonl.xz`.

**Verdict: passes dispatch on m63** (driver p99 6.6 s against 10 s; m62 was 38.6 s). The verdict lines went to the lead at about 17:47Z, inside 30 minutes of the done line. The lead accepted m63 as B0-35's first pass and asked for two more rows on the same site day, gated on the median of the three driver p99s (below).

| Line | m63 | m62 |
| --- | --- | --- |
| Dispatch, driver | p50 3.8 s, **p99 6.6 s**, max 10.4 s, 2,500 of 2,500 paired | p50 5.5 s, p99 38.6 s |
| Dispatch, site entry-to-run / demand | p99 4.7 s / 3.2 s | p99 32.9 s / 30.6 s |
| Dispatch p99 by minute of load | 3.4 to 4.9 s, except minute 8 at 8.1 s | 18.2 to 45.8 s from minute 5 |
| Leader commit | p50 631 ms, p99 1,487 ms | p50 1.0 s, p99 18.0 s |
| Leader queue (slot) | p99 1,175 ms (1,103 ms) | p99 17.8 s (17.6 s) |
| Round / confirm / deps | p99 477 ms / 292 ms / 578 ms | p99 880 ms / 898 ms / 1.4 s |
| f0's vCPU (`tests/fanout/docpumin.py`, CPU-s per minute from 17:16) | 17.8, 18.3, 18.0, 19.3, 19.8, 19.4, 20.4, 20.0, 18.9, 14.6: mean 0.31, peak 0.34 | mean 0.49, peak 0.67 |
| Learner read-index calls to f0 | 50,537 sent, plus 1,033 `HEDGE` copies (**2.0%**); 170 past `CALL_DEADLINE` | 44,515, 4.1% `HEDGE`; 2,543 past `CALL_DEADLINE` |
| Leader changes | **0**, term 2 throughout. No hold in the load. One setup-time trip (below) | 0; three holds |
| `suspect` | 10 lines in the load window; no pre-vote | 2,000 or more per follower |
| f0 alarm-steps lines | 2, both `load`: 1,408 ms (17:16:41) and 2,047 ms (17:16:43). f1 logged `snap` 1,648 and 7,530 ms at 17:08, in setup | 17 |
| Pushes | 2,500 of 2,500 agent ok; human 318 ok, 7 `stale`, 29 `stale:race`; `mergeable` 503s 2; api 503s 5 | 2,423 of 2,500 ok; `mergeable` 503s 5,089 |
| Shared isolates (`isolates.py`) | none (11 members, 11 isolates) | none |
| Spend | graft-m4a **$4.03** (17:02:58Z to 17:45Z, `acctcost.py` prefix; $4.87 for the whole account in that window), runner account $0 | $4.41 |
| Teardown | 1 shard 200, no container running, shardkill deleted (17:44:25Z) | clean |

**The setup-time trip.** At 17:14:21.741Z, before load, f0 logged `StepDown { why: CommitErrors { calls: [(SiteId(1), 20, 3), (SiteId(2), 19, 3)] }, to: Some(SiteId(1)) }`. `step_down_to` logs `to: Some` only after `start_transfer` succeeds, so a transfer to f1 began. The term stayed 2 through 17:14:50 and the whole row, so the transfer was abandoned within its LEASE [inference]. No line says why: the host logs no `abandon_transfer`, and tsave traces no `TimeoutNow`. Recorded as a logging gap (`decisions/OPEN-QUESTIONS.md`, F107), to be fixed only if it recurs in a load window.

**Cuts** (`tests/fanout/grown.py tests/fanout/raw/tsave-m63.jsonl.xz 1795194971086 600 0`) [measured]: 1,932 cuts, 693 ended `Soft` and 1,239 `Empty`. Of the `Soft` cuts, 556 ended at 100 KB or less and 137 grew past it, to at most 150 KB. Soft bytes p50 60 KB, p99 145 KB. Commands left queued after a cut: p99 17, max 25 (m62: 350).

**Queue wait against the replay.** Measured leader queue p99 1.18 s, against the 5.6 s the m62 replay predicted. `softgrow.py` on m63's own trace gives flat 1.9 s and `soft-grow` 1.2 s, with a cycle fit of 310 ms plus 0.00 ms per KB. So m63's offered load ran under the m62 model, and the grown entries did not lengthen the cycle [measured].

**The `soft-grow` flip line, grown against 100 KB or less** [measured]:
- Leader confirm wall: grown n 183, p50 57 ms, p99 476 ms, max 530 ms, none of 1 s or more. 100 KB or less: n 2,722, p99 356 ms, max 2,037 ms, 2 of 1 s or more.
- Followers: grown p99 77 ms; 100 KB or less p99 99 ms.
- Leader sync by transaction bytes: over 100 KB p99 359 ms; 100 KB or less p99 357 ms. The driver's `confirmSplit` gives sync p99 282 ms under 100 KB and 359 ms from 100 KB to 1 MB.
- Grown entries' confirm p99 is above the smaller entries' on the same row, but no grown entry took 1 s, and both are under m62's flat 966 ms (24 of 1 s or more). One row does not meet the flip; the lead asked for the comparison on each of three rows.

**Far voter.** No AppendEntries failed in the trace. The driver counted `CALL_DEADLINE` misses of 3 to f1, 4 to f2, and 1 and 2 to learners 198 and 199. f2's call p99 was 665 ms, and 660 ms for its 141 calls that carried a grown entry. f1's was 515 ms.

**The end-of-load `/commit` storm did not recur** (`tests/fanout/commitsenders.py`, f0's object, 17:15:30 to 17:28Z, no saturated slice) [measured]. f0 received 6,212 `/commit`s: 2,501 from the edge, 3,711 with step `Leader` (`commit_as`), of which 2,809 came from the top isolate, and none with step `Send`. The top isolate peaked at 287 a minute and 27 a second (m62: 1,605 and 123). Calls of 1 s or more: edge 24%, top isolate 14%. Nothing is built. If the storm returns, the lead's preferred fix is `commit_as` asking by request id.

### Row m64 (same site day, no redeploy)

Granted by the lead with a $10 stop on graft-m4a plus the runner account from m64's start, later raised to $12 for m65. `b035.sh` at `67700476` (driver unchanged since m63) ran on `m3org/b035-m64` = repo 65578, with 8 learners and `ISOLATES=1`. Run `mwpb99mz`: started 18:26:07Z, load 18:32:45.269Z to 18:42:45Z, done 19:03:41Z, exit 0; the sweep tore down shard 65578 and the canary burst's leftover shard 65569 (both 200), no container running, shardkill deleted (19:04:05Z). Raw files: `tests/m4/raw/wp4/b035-m64/` (as m63's, plus `softgrow.txt`, `readnewest.txt`, `isolates.txt`, `hedge.txt`, `cost.txt`, `alarm-steps-all.txt`), `b035-m64.{status,lead,watch}`, `tests/fanout/raw/tsave-m64.jsonl.xz`. Verdict lines went to the lead at about 19:12Z, inside 30 minutes of the done line.

| Line | m64 |
| --- | --- |
| Dispatch, driver | p50 5.4 s, **p99 9.9 s**, max 12.2 s, 2,500 of 2,500 paired |
| Dispatch, site entry-to-run / demand | p99 7.4 s / 5.6 s |
| Dispatch p99 by minute of load | 4.5 to 5.0 s through minute 4, then 6.2, 5.6, 7.7, 5.6, 8.4 s |
| Leader commit | p50 621 ms, p99 2,675 ms |
| Leader queue (slot) | p99 2,459 ms (2,375 ms) |
| Round / confirm / deps | p99 776 ms / 373 ms / 695 ms |
| f0's vCPU (CPU-s per minute from 18:33) | 23.5, 24.8, 27.7, 29.2, 30.6, 30.8, 31.7, 33.0, 33.5, 25.4: mean 0.48, peak 0.56 |
| Learner read-index calls to f0 | 49,522 sent, plus 2,145 `HEDGE` copies (4.3%); 964 past `CALL_DEADLINE` |
| Leader changes in the load | **0**, term 2. Holds: 18:40:27 LoopLag (clamped 2), 18:42:14 CommitErrors [(f1, 25, 3), (f2, 21, 3)], each `to: None` |
| After the load (observed, outside the gate window) | During the driver's run cancellation, 19:02:27 CommitErrors [(f1, 29, 3), (f2, 24, 3)] with `to: Some(SiteId(2))`, and the transfer completed: f2 led term 3 from 19:02:24Z (the lead watcher's sample), and the transfer home put f0 at term 4 by 19:07:22Z. The first completed step-down transfer on these rows; no action (lead, 2026-11-20) |
| `suspect` | none in the load window |
| f0 alarm-steps lines | in the load, `actions_trigger` 389 ms (18:42:00); after it, during the cancel, `load` 1,556, 3,026, 2,796, 2,649 and 3,305 ms (19:01:53 to 19:03:16) |
| Pushes | 2,500 of 2,500 agent ok; human 333 ok, 24 `stale:race`, 29 skipped at the cap; `mergeable` 503s 30; api 503s 61; merges 66 × 200, 13 × 409 |
| Cuts | 1,974: 655 `Soft` (503 at 100 KB or less, 152 grown, max 150 KB), 1,319 `Empty` |
| Queue replay on its own trace | flat p99 3.4 s, `soft-grow` p99 2.3 s; measured 2.46 s; cycle fit 260 ms + 0.72 ms per KB |
| `/commit` at f0 | 6,182: edge 2,586, step `Leader` 3,596 (top isolate 2,832), `Send` 0; top isolate peak 341 a minute and 28 a second. Not recurred |
| Shared isolates | none (11 members, 11 isolates) |
| Spend | graft-m4a $5.08 (18:26:07Z to 19:05Z, prefix; $7.72 for the whole account), runner account $0 |

**The watcher's dollar stop never fired** (found starting m65). Since `f88f1a48`, `b035watch.sh` reassigned `STOP` to the stop file's path after reading it as dollars, so each pass's comparison raised a `NameError` and wrote no stop file; the sandbox-running stop still worked. The watchers for rows m54 to m64 therefore logged spend but would not have stopped a row on cost. No row reached its stop: the largest watcher reading was m64's $5.20 against $10, and m54 and m57 to m63 read $4.04 to $4.94 against $8. Fixed in `ffc28b71`: the stop file has its own variable; m65's watcher has run with it since 19:16:45Z.

## Repository purge route, hosted check on graft-m4a (2026-11-18)

The route has no gate row, so this is a hosted check of `POST /api/graftyard/v1/admin/repos/{id}/purge` (`decisions/DECISIONS.md`, "`repo-purge`"), granted to stop the fold audit's daily refolds of 35 leftover test repositories (65537 to 65567, 589824 to 589827). Site day 52 and edge day 25 shipped main `703a9642` as product builds (`tests/am1/results/install-m4a.md`). Driver: `tests/m4/wp4/purge35.mjs`; every reply is in `tests/m4/raw/wp4/purge35.jsonl`.

**Result: the first run stopped before any DELETE or purge. The gate refused 27 of the 35 chains for a reason that is not timing, so nothing was retired or deleted. The 8 clean chains were purged next (below).**

- **Who.** The route answered 404 to m3-admin: on graft-m4a no seeded user administered the install account (`authz:1`), and `/api/graftyard/v1/agents/apps` answered 403. `SetMember { user: 7100003, role: Admin }` was committed as the operator on `authz:1` at 05:29Z (index 106), as `tests/m3/hosted/rows.ts` does on other installs, and `agents/apps` answered 200 after. m3-member still gets 404 from the route [measured].
- **The refusal.** `NoTombstone` on a site-1 member that is no longer in the group: a voter replaced by a later generation (`repo:N#1#f1`, joined in its own view at an old config, often applied index 1), which can never apply the tombstone because no leader sends it entries. The route's `plan` asks every record's bootstrap member, so it always reaches that member. 26 chains hold one: 22 on site a and all 4 wp2eu chains. 13 chains (12 on site a, and 589826) also have a registry row naming a replacement that never got a config (`config: null`, applied 0, so `changing` is true). For 65551 that member is the only blocker [measured, `/status` of every member `plan` reads].
- **No such member.** The 8 live repositories 65537, 65548, 65552, 65554, 65556, 65559, 65561 and 65563 have only their three first-generation voters, all in the newest config. They answered `NotDeleted`, as they should before a DELETE [measured].
- **Retries.** 65543 and 589824 were each asked 3 times, 20 s apart, with the same member named each time.
- **Cost.** 05:19Z to 05:45Z, by `tests/fanout/acctcost.py` with `PREFIX=graft-m4a`: $0.02. The account was at $7.35 for the UTC day. No idle reading was taken: nothing changed, so the daily refolds still run (about $1.50 a day).

`decisions/OPEN-QUESTIONS.md` R26 and R27 carry what this found.

### The 8 clean repositories, purged (2026-11-18, 05:53Z to 06:13Z)

The lead granted a purge of the 8 live repositories that hold no such member, the route's first real hosted pass. Same builds, same driver and log.

| Repository | `DELETE` | First pass | Retired | Objects | Second pass |
| --- | --- | --- | --- | --- | --- |
| 65537 `m3org/bf57b` | 204 | 503, then 200 | 0 (all 3 already gone) | 16 | 0 |
| 65548 `m3org/nh3` | 204 | 200 | 3 | 12 | 0 |
| 65552 `m3org/wp3-live` | 204 | 200 | 3 | 22 | 0 (after one 503) |
| 65554 `m3org/wp3-bf59` | 204 | 503, then 200 | 2 | 16 | 0 |
| 65556 `m3org/wp3-bf37` | 204 | 503, then 200 | 2 | 13 | 0 |
| 65559 `m3org/wp3-bf55` | 204 | 200 | 3 | 12 | 0 |
| 65561 `m3org/wp4-smoke` | 204 | 200 | 3 | 15 | 0 |
| 65563 `m3org/wp4-bm4` | 204 | 200 | 3 | 15 | 0 |

- **No refusal.** Every first call passed the gate at once: `early`, and the tombstone already applied on every member within the 5 s after the `DELETE` [measured].
- **The 503s are the lost reply the design expects.** In 65537, 65554 and 65556 the first call answered "`repo:N#1#f<s>`: purge_retire answered None; retry": the member aborts as it answers, the reply is lost, and its `/status` was still read before the abort had finished. The next call found those members not joined, so it retired only the rest (65537: none left, the first call had retired all three) and purged the bucket. No member was left half-retired [measured]. The one second-pass 503 was the install's `Authz` read, retried 40 s later.
- **Second pass.** 7 of 8 after `secondPassAt` 05:55Z, and 65563 after its own 06:10Z: each found 0 objects.
- **Verify.** Every member that `plan` reads answers `{"joined": false}` for all 8 (`/status` of each first-generation voter and registry row). `tests/fanout/purge.py --dry-run` on the 8 networks lists 0 objects on sites a, b, and c [measured].
- **Cost.** 05:53Z to 06:13Z: $0.02 on m4a. Site day 52 to here (05:19Z to 06:13Z): $0.05.

### The other 27, purged under `purge-outside` (2026-11-18, 07:48Z to 08:51Z)

Site day 53 (07:48:21Z to 07:49:13Z, exit 0) and edge day 26 (to 07:51:33Z, exit 0) shipped main `94fdfb48`, which added `purge-outside` (`decisions/DECISIONS.md`; R27). Same driver and log.

**Result: all 27 purged, with no refusal that was not a lost reply or timing.**
- **Delete.** 20 repositories answered `DELETE` 204. The other 7 (65543 to 65545, 589824 to 589827) were already deleted.
- **First pass.** Every repository answered 200. The passes retired 71 members, the outside ones first, and deleted 832 objects (65567: 464).
- **Second pass.** Each ran after its own `secondPassAt` (08:35Z to 08:40Z), and all 27 found 0 objects.
- **Verify.**
  - Only `repo:0` and `repo:65536` (j4) are still joined, in a scan of 65535 to 65568 and 589823 to 589828. That scan read each site's registry row and each first-generation voter.
  - The 35 purged repositories' `repo-meta` members all answer not joined.
  - `purge.py --dry-run` on the 27 networks lists 0 objects on sites a, b, and c [measured].

What each call answered, besides the 200s, all asked again by the driver:

| Answer | Count | Cause |
| --- | --- | --- |
| 503 "purge_retire answered None; retry" | 22 | The lost reply the design expects: the member aborts as it answers |
| 409 `NoTombstone` on an in-config member | 15 | Timing: the follower had not yet learned the delete's commit. 65540 and 65547 caught up within 1 min. 65567's group had no leader (term 26, every member a follower, leader lease long past), so its followers stayed one entry behind. One `/campaign` to `#1#f0` resumed it (`verdict: resumed`), its next append carried the commit, and the purge passed |
| 409 `Unanswered` on a `repo-meta` member | 2 | Timing: 65558 and 65565, both answered on the next try |
| 409 "A member refused its purge", `changing` | 1 | 65546's `#1#f0` was in its daily near check (`near_check` marker) when `/purge_retire` re-checked it. That is the member's own refusal, which the design keeps. The rerun 2 min later passed. The driver counted this as non-timing and stopped once; it now asks again on `changing` too |

- **The orb's disk filled at 08:18Z** during a build of the R28 branch. The driver stopped mid-call on 65565 and left a partial log line, which was dropped. The rerun found 65565 with nothing joined and 0 objects: the cut call had finished both.
- **Cost** (`tests/fanout/acctcost.py`, `PREFIX=graft-m4a`): from site day 53's start to the end of the idle reading (07:48:21Z to 09:30Z), **$0.11** on graft-m4a, against the $3 stop. The account's UTC day was $10.81 at 09:30Z. **Idle**, 09:00Z to 09:30Z across the 09:15 and 09:30 crons: **$0.02** on graft-m4a (site a $0.01, no container). That matches the true idle the lead accepted before ($0.01 in 25.5 min). The leak was a daily refold per chain, so a half-hour window cannot show it stop. A 24-hour reading would: before the purge, m4a ran about $1.50 a day


## R23: release M1 and the `RunnerBox` cold-burst check on the runner account (2026-11-19)

Granted by the lead with a $2 stop on the runner account: M1 from main `911f59e2`, then one cold burst of 8 runners on a test version only (`decisions/DECISIONS.md`, "`r23-runnerbox`").

**M1: done.** `graftyard runners init` ran at 10:00:04Z from `911f59e2` (bundle by `wrangler deploy --dry-run`, same image digest `d0652a89…`), exit 0 (`tests/m4/raw/wp4/runners-init.log`).
- `graft-runners` went from migration tag `v1` to `v2`.
- `graft-runners-runnerbox` was created on the `durable_object` policy, with no `max_instances` and an empty configuration; its id is its namespace's.
- The two Sandbox applications are unchanged.
- The new `WEBHOOK_SECRET` was patched into the App's hook config (200). An unsigned POST answered 401.
- Version `e05903aa` serves 100%, with `RUNNER_BOX_ON` unset.

**The test version.** `RUNNER_BOX_ON=1` plus temporary logging around `ctx.container.start` and `monitor()`, uploaded directly (`POST /workers/scripts/graft-runners/versions`, no Wrangler) as `c1d38f31` and deployed at 100% from 10:07:59Z to 10:16:18Z. Before the rename, the first upload was refused: `Binding name 'RUNNER_BOX' already in use` (code 10053), because the variable and the Durable Object namespace had the same name. The variable is now `RUNNER_BOX_ON`, and a CLI test checks that binding names are unique; it fails on `911f59e2`.

**The burst.** Run 1712951 on `m3org/wp4-r23` (`tests/m4/wp4/r23.mjs`, `tests/m4/wp4/hosted/r23.yml`: 6 `ubuntu-22.04` and 2 `ubuntu-24.04-4core` jobs).
- The first try, run 1712942, put the label in `runs-on: ${{ matrix.label }}`. The install queued those jobs as `self-hosted`, so no hosted demand was raised; the run was cancelled. `r23.yml` now names each label statically.
- From 10:15:44Z the passes registered and armed 8 runners, 6 `standard-3` and 2 `standard-4` (the 10:15:55Z sweep logged `started 8`).
- Every one of the 8 alarms called `ctx.container.start` without a throw. Each `monitor()` rejected 0.66 to 0.76 s later with `Error: Image not found. Please, make sure you have pushed the image, you have set a digest on it, and have created an application with it`. `box.mjs` logged `runner exited within 3 s … image <digest>` for each, and closed it.
- No container placed: `graft-runners-runnerbox` listed 0 instances throughout (`tests/m4/raw/wp4/r23/workers-logs.txt`, `run.log`).
- Stopped at 10:16:07Z as the grant asked. Run 1712951 was cancelled, and M1's version `e05903aa` was redeployed at 100%. The 10:20:44Z sweep, on M1's code, deleted the 8 registrations as stale and destroyed each on `RunnerBox` (`destroyed 8`). Demand then read empty.

**The four unverified items.**
1. **A version upload needs image preparation: yes.** An image map naming a pushed, digest-pinned image is accepted by the version upload but does not run: every start's monitor rejects with "Image not found" until the image is prepared. Wrangler calls `POST /containers/image-preparations {image}` and polls it to `READY` before it uploads a version with an `images` map (`wrangler-dist/cli.js`, `waitForImagePreparation`); `graftyard runners upgrade` does not, and neither does `graftyard sites` for `GitShard`. Per the grant, this was not worked around.
2. **How a failed start surfaces:** not a throw from `start()`. `start()` returned, and the error came from `monitor()`, under a second. A refused placement for want of an instance was not reached, because no start got that far; its shape is still [unverified].
3. **`run.sh` as PID 1:** not reached; no container ran.
4. **Beta policy behaviour:** the `durable_object` application was created and bound by namespace id, and `ctx.container.images.runner` returned the digest from the version's metadata. Nothing else was reached.

**`box.mjs` in this failure.** It treated "Image not found" as the runner's exit: one start per runner, no retry loop, one `exited within` line per runner naming the image. That is the bad-canary signal the design names, with `by_image` counting it. Once a new version's image is prepared, an unprepared image is a release error, not a runtime one.

**Cost** (`tests/fanout/acctcost.py`, 09:51Z to 10:22:35Z): runner account **$0.00** (no container ran, and its Durable Object and Workers use rounds to zero); graft-m4a $0.06. The stop was $2.

### The rerun with the image prepared (same grant)

The operator CLI now prepares every image a version names before it uploads (`cli/src/cf.rs`, `prepare_image`; `decisions/DECISIONS.md`, "`r23-runnerbox`").
- **Preparation.** `POST /containers/image-preparations` for the runner image answered `pending` ("image preparation scheduled", then "snapshot artifact build already dispatched") at 10:32:19Z and `ready` at 10:33:22Z, about 63 s for this 2.04 GB image. Later calls answer `ready` at once.
- **Upload.** The test version `00cab528` (the same bundle and metadata as `c1d38f31`) went through `Cf::upload_version` itself, from an ignored test in `cli/src/cf.rs` (`r23_upload_test_version`): 1,206 ms, with the image already prepared. Deployed at 100% from 10:36:02Z.
- **The burst.** Run 1712968, 8 jobs. No `workflow_job` webhook reached `graft-runners` for it, so the jobs waited for the 10:40:52Z sweep (the 202 at 10:15Z on try two had come from that path too; the delivery side is not part of this check). That one sweep pass registered and armed all 8 (`started 8`).
  - Each alarm launched its container 3 s after the arm (`launched 3 s after arm, image <digest>`, 8 of 8), with no refusal, no retry, and no "exited within" line.
  - Containers at `standard-3` (2 vCPU, 8 GiB, 16 GB; 6) and `standard-4` (4 vCPU, 12 GiB, 20 GB; 2), in `ewr15` and `iad15`.
  - All 8 jobs `Success`. Each printed PID 1 as `/bin/bash /home/runner/run.sh` with comm `run.sh`, 0 Sandbox SDK server processes, and 2 or 4 vCPU.
  - Each `monitor()` resolved 43.9 to 53.2 s after its start, as the runner exited after its one job. All 8 instances were then `stopped`, exit code 0, by 10:41:41Z: the container ended with the runner, with no destroy.
- **Shards on graft-m4a.** Creating `m3org/wp4-r23` and pushing its workflow started its network's git shard (`git#0#net:65569` on site a) on both tries. The first ran from 10:08:39Z until torn down at 10:35:17Z; the rerun's from 10:36:21Z until 10:43:57Z. `tests/m4/wp4/r23.mjs` now runs `teardown.py <repo> <repo> --shardkill` on every exit, a crash or signal included.
- **teardown.py's 500 at 10:35Z.** The first teardown call through the new `graft-m4a-shardkill` Worker answered 500, and a direct call to the same route 8 s later answered 200 and tore the shard down. Workers Logs show the site's `/teardown` invoked once, at 10:35:18Z, outcome `ok`, from the direct call; nothing from the first call reached the shard. So the 500 came from a just-deployed workers.dev script before its Durable Object bindings answered, not from the shard, and the script now retries a 5xx, as it already retried a 404, and prints its body. The rerun's teardown through the fixed script answered 200 on the first call.

**The four items, settled or not.**
1. **Image preparation:** needed, and now in the CLI. With it, all 8 starts placed and ran.
2. **How a failed start surfaces:** through `monitor()` rejecting, not a throw from `start()` (the first try). Refused placement for want of capacity was not reached on either try, and stays [unverified]. It was not forced.
3. **`run.sh` as PID 1:** yes, `/bin/bash /home/runner/run.sh` on 8 of 8 under the entrypoint override on the `cloudflare/sandbox` base, with the Sandbox server not running; the container stopped with exit 0 when the runner exited.
4. **Beta policy behaviour:** one `durable_object` application served both instance types chosen per start, placed each start in about 3 s, and stopped each container with its main process. Nothing unexpected.

**An unplanned third run.** The driver ran as an orb service, which restarted it after it exited, and it dispatched run 1712985 at 10:52:24Z. The service was stopped at 10:52:47Z, and its exit hook tore the shard down (200). `graft-runners` went back to M1's version `e05903aa` at 100%. The run's cancel answered 503 (the install shed its tier reads) until 10:56:05Z, so the 10:55:56Z sweep, now on M1's Sandbox path, started its 8 jobs on `SandboxStd3` (6) and `SandboxStd4` (2). All 8 logged "launched runner": seven 2 s after the arm, and `6349d772` 34 s after it, having logged "There is no container instance that can be provided" 37 times from 10:55:54Z to 10:56:25Z. That is a placement refusal that retried until it placed, seen on the legacy Sandbox path, not on `RunnerBox`. Its runner went `stale` after the cancel, and the 11:00:47Z sweep destroyed it (`sandbox.destroy success`). Lesson: one-shot drivers do not run as orb services. This run's `run.log` overwrote the rerun's; the rerun's facts are in `result.json`, `samples.jsonl`, and `workers-logs.txt`, and this run's are under `try3/`.

**Close-out.** At 11:03Z the runner account listed 0 running on all three applications (`runnerbox` 8 `stopped`, `sandboxstd3` 299 and `sandboxstd4` 2 `inactive`), hosted demand was empty, and graft-m4a's three gitshard applications listed 0 running. Cost from 09:51Z to 11:03:41Z (`tests/fanout/acctcost.py`): the runner account $0.02, graft-m4a $0.16 (the leaked shard included), **$0.18 of the $2 stop** for both tries, the rerun, and the third run.

### M2 canary reads, chunked (2026-11-20)

The M2 canary check (13193b05 at 10%) read the runner account's Workers Logs for the whole window since 2026-11-19 11:34:56Z, 26 hours and more, in one pass of 30 s slices with no retry. Both wakes that day, at 09:11Z and 13:14Z, failed on one TLS handshake timeout to the observability API and gave no verdict. The lead asked for a harness-only fix:
- `tests/m4/wp4/runnerlogs.py` reads the window in chunks of at most 1 h.
- Each chunk has its own deadline (900 s), and each query in it has its own timeout (30 s) and up to 3 tries, with 5 s then 20 s backoff.
- A span that answers fewer than 100 events is complete. A span that answers 100 splits down to 30 s slices, and a 30 s slice is walked back by timestamp, as before.
- A chunk that never answers is named by its bounds in a `chunks` line (`chunks`, `failed`, `complete`), and the exit is 3. The rest of the window is still read.
- `sandboxstarts.py` uses the same fetch and ends its summary with the same three fields.
- The canary's schedule now promotes only when both reads report `complete` true. It reads the runner account's credentials first, since `env-m4a.sh` also sets `CLOUDFLARE_ACCOUNT_ID` to graft-m4a.

Measured on the runner account: a quiet hour is one query in about 1 s. The site day 56 hour (17:35Z) is 183 events, 23 queries and 15 s. The m57 hour (18:35Z) is 11,156 events, 181 queries and 140 s. The old one-pass read made about 3,100 queries for the same window, and a single failed call ended it. A first version retried a whole chunk and gave it 300 s, which the m57 hour could not meet. Retries are now per query, so one failed call costs one retry, not the hour's work so far.

Tests: `python3 -m unittest tests/m4/wp4/test_runnerlogs.py`, 7 tests on a fake API. They cover:
- hour chunks, with a quiet hour costing one query;
- a query retried twice with backoff;
- a chunk that never answers named while the rest is read;
- a busy hour split with no event lost;
- a failure deep in a busy hour retrying that query, not the hour;
- a chunk past its deadline named;
- a busy 30 s slice walked back.

Seven planted bugs, each caught: one chunk for the whole window; no retry; a failed chunk failing the whole check; no split; no walk-back; no backoff; no chunk deadline.

## Spend against the $25 stop

| Account | Window | Metered |
| --- | --- | --- |
| graft-m4a | 12:27Z to 22:02Z | $4.13 (containers $2.58: git shards and site days 42 to 45) |
| Runner account, `graft-runners` | 15:00Z to 22:02Z | $0.83 (Sandboxes $0.76: smoke, BF-60 twice, BM-4) |
| Runner account, BF-54 scratch | 20:30Z to 21:47Z | $2.72 (16,334 vCPU-s and 913,634 GiB-s on `standard-4`), from `containersUsageAdaptiveGroups` by application id. `APPS=graft-runners-` no longer finds the deleted application |
| Workers Paid | one month | $5.00 |
| **Total, 2026-11-15** | | **$12.68 of $25**, inside the approved $12.30 to $18.76 |
| graft-m4a | 22:02Z to 00:26Z (2026-11-16) | $0.29, building image `m4a46`, then site day 46 |
| graft-m4a | 00:26Z to 01:30Z | $0.94, BF-54 run 2's polls and job commits (containers $0.27: the three git shards) |
| Runner account, BF-54 run 2 scratch | 00:26Z to 01:40Z | $1.76 (13,744 vCPU-s and 568,468 GiB-s on `standard-4`), from `containersUsageAdaptiveGroups` for application `a032924c-…` |
| **Total** | | **$15.67 of $25** |
| graft-m4a | 01:30Z to 05:20Z | $1.15: image `m4a47`, site day 47, edge day 21, and BF-54 run 3's polls and job commits (containers $0.43: the four git shards) |
| Runner account, BF-54 run 3 scratch | 04:10Z to 05:05Z | $1.50 (13,130 vCPU-s, 471,337 GiB-s, 785,561 GB-s on `standard-4`), from `containersUsageAdaptiveGroups` for application `a03fbdb5-…` |
| **Total** | | **$18.32 of $25** |
| graft-m4a | 05:20Z to 18:50Z | $0.41: idle, then site day 48 and edge day 22 |
| Runner account, `graft-runners` | 05:05Z to 18:50Z | $0.00 |
| graft-m4a | 18:50Z to 19:50Z | $0.42: run 1966233's commits and its git shards (containers $0.22) |
| Runner account, `graft-runners` | 18:50Z to 19:50Z | $0.34: run 1966233's Sandboxes (containers $0.31) |
| **Total** | | **$19.49 of $25** |

BF-54 cost $2.72, under its $4.63 to $10.03 estimate. It needed 1.3 hours of 25 boxes, not one at full price, because the boxes idled at low CPU. Run 2 cost $1.76 in the runner account and about $1.23 on m4a, $2.99 in all, inside its forecast of about $5. Run 3, with its site and edge day, cost $2.65 ($1.50 and $1.15), inside its forecast of about $3.

## Plants: `tests/m4/wp4/plant.py`

Plants 25 to 30 (F77, branch `f77-acquire`): 6 of 6 caught. 21 of 21 caught after the rebase onto main 4833094d (plant 12 is BF-55's "never rematch an ephemeral runner" rule, which holds for hosted runners because they are ephemeral and `Owner`-scoped; plants 18 to 20 are `graftyard runners upgrade`'s). The first run missed two. "Start ignores the cap's room" got through because every test queued only one row; `hosted::tests::rows_share_the_caps_room` now queues two. "A runner whose job completed is not stale" got through because every runner that had held a job was also past `HOSTED_IDLE`; the demand test now queues jobs while those runners are stale. One planned plant was dropped: "the cap counts only the fold" cannot be written without reaching into the scratch's private fields.

| # | Plant | Caught by |
| --- | --- | --- |
| 0 | `decide` never checks a hosted registration | bf60, the cap test |
| 1 | No cap | bf60, the cap test |
| 2 | The cap lets one more through | bf60, the cap test |
| 3 | A hosted runner may be repository-scoped | the cap test |
| 4 | A busy hosted runner may be deleted | `decide_refuses_to_delete_a_runner_holding_a_job_on_its_shard` (since F78, the edge's reads catch it first in the stale-runner test) |
| 5 | Idle runners are not counted against the queue | the stale-runner test, two `hosted` tests |
| 6 | Start ignores the cap's room | `rows_share_the_caps_room` |
| 7 | Start ignores runners already registered | the demand test |
| 8 | Idle runners beyond the queue are never stale | the stale-runner test, the newest-first test |
| 9 | A runner whose job completed is not stale | the demand test |
| 10 | An idle runner is never stale by age | the demand test |
| 11 | A busy runner is listed stale | the demand test |
| 12 | A broker matches a hosted runner again after its job (BF-55's rule) | bf60, the broker's ephemeral-runner test |
| 13 | A refused delete still destroys the Sandbox | the stale-runner test, the pass test |
| 14 | A stale runner's Sandbox is destroyed without deleting it | bf60, the stale-runner test, the pass test |
| 15 | The vCPU stop is ignored | the vCPU test |
| 16 | Registered runners are not counted toward the vCPU stop | the vCPU test |
| 17 | Every label runs on `standard-3` | the pass test, the vCPU test |
| 18 | `runners upgrade` deploys a release whose signature does not verify | the upgrade refusal test |
| 19 | `runners upgrade` goes to 100% before the canary day | the canary test |
| 20 | `runners upgrade` rolls the image with the 10% canary | the canary test |
| 21 | The broker's stored `Seen` drops each runner's first sight (the smoke run's bug) | `seen_ages_runners_across_stored_reads` |
| 22 | A `Repo`-scope poll never resends a lost match (main's rule, F76) | `a_repo_scope_runner_whose_match_answer_was_lost_gets_the_job_on_a_later_poll` |
| 23 | A `Repo`-scope poll matches a runner that holds a job a second job | the same |
| 24 | A summary forgets when a job was matched, so nothing is ever resent | the same, and `bf55` |
| 25 | A holder's repeated `acquirejob` is refused again (main's rule, F77) | `a_runners_repeated_acquire_of_its_own_job_is_answered_and_anothers_is_refused` |
| 26 | A repeated acquire re-arms the lease | the same |
| 27 | Any runner may acquire an `InProgress` job | the same |
| 28 | The `actions` rows are answered as a whole-tier dump (main's read, F77) | `the_acquire_secrets_read_is_never_shed_for_a_dump_rebuild` |
| 29 | The `actions` rows leave out the variables | the same |
| 30 | A follower sends the `actions` rows to the leader | the same |
| 49 | The delete reads no shard first (F78) | `a_stale_delete_reads_before_and_after_it_commits`, the F78 bf60 test |
| 50 | The delete never reads after it commits | `a_stale_delete_reads_before_and_after_it_commits`, the stale-runner test |
| 51 | The delete reads the registration shard only (main's rule) | `a_delete_reads_every_shard_its_owners_jobs_can_be_on`, the F78 bf60 test |
| 52 | `Seen` counts a mark a later read left (main's rule) | `a_mark_from_an_overlapping_later_read_never_makes_a_runner_stale`, the F78 bf60 test |
| 53 | `Seen` moves a mark to the newest read | `a_job_mark_keeps_its_first_sighting` |
| 54 | The owner's deleted repositories are not read | `a_delete_reads_every_shard_its_owners_jobs_can_be_on` |
| 55 | A failed repository-name read acquires with an empty name (main's rule, F79) | `an_acquire_whose_repository_read_fails_is_refused_never_sent_without_a_name_or_secrets` |
| 56 | A failed owner read acquires with an empty name (main's rule) | the same |
| 57 | A failed run read acquires with an empty name (main's rule) | the same |
| 58 | A failed secrets read acquires without the secrets | the same |
| 59 | A job the shard does not hold is retried as a failed read (503, not 404) | the same |
| 91 | `purge-outside`: an outside member is held to the tombstone too (main's rule, R27) | `a_voter_a_replacement_removed_does_not_block_the_purge`, `a_voter_a_replacement_removed_is_retired_unchecked_and_first`, `a_registry_member_that_never_received_a_config_is_retired_unchecked` |
| 92 | An in-config member is retired unchecked | the same, and `a_purge_cut_after_the_outside_step_passes_when_called_again` |
| 93 | No member of the newest config needs to answer | `every_member_of_the_newest_config_must_answer` |
| 94 | A `Degraded` config's lost voter counts as in-config | `a_degraded_config_s_lost_voter_is_outside_and_an_in_config_change_refuses` |
| 95 | In-config members are retired first | `a_voter_a_replacement_removed_is_retired_unchecked_and_first`, the lifecycle test |
| 96 | The member's own rule ignores `I` | `a_member_is_retired_unchecked_only_while_it_is_still_outside` |
| 97 | `C` is the oldest config, not the newest | the lifecycle test and two gate tests |
| 143 | A read of named commits catches up first on a disk that holds them (main's order) | both disk-first tests (`crates/git/tests/disk_validate.rs`) |
| 144 | A demand catches up first on a disk that holds base and head (main's order) | `a_demand_on_commits_on_the_disk_computes_while_the_catch_up_fails` |
| 145 | A read that finds a commit missing answers without a catch-up | `a_missing_commit_still_waits_for_a_catch_up_and_answers_warming` |
| 146 | A 404 for a path at a present commit reads as a missing commit | the same, and the tree-or-blob test |
| 147 | A rehydrating shard reads from its disk | the missing-commit test |
| 148 | A merge still missing a commit after the catch-up answers shed | the same |
| 149 | The lookup-mark trace logs none of the holder's posts | `a_traced_lookup_logs_its_walk_and_answers_the_same` |
| 150 | A catch-up read never takes its site member's answer (main's `ranges` read, `catchup-local`) | `a_catch_up_read_answers_from_its_site_while_another_walk_holds_the_mark` |
| 151 | A local `ranges` read takes a shed or `busy` answer | `a_local_catch_up_read_is_taken_only_at_or_above_what_was_asked`, the catch-up test |
| 152 | A local `ranges` read takes an answer with no value | `a_local_catch_up_read_is_taken_only_at_or_above_what_was_asked` |
| 153 | A local page that starts past the index asked for is taken | the same |
| 154 | A wait on another request's mark is not logged (F104) | `a_wait_on_another_requests_mark_is_logged_with_its_holder`, the catch-up test |
| 155 | A traced cached leader's miss is not logged (F104) | `a_wait_on_another_requests_mark_is_logged_with_its_holder` |
| 156 | A follower answers a shard's `ranges` read `NotLeader` (main's rule) | `a_follower_serves_only_the_designs_reads` |
| 157 | A follower serves any read that names `ranges` | the same |

## Cost estimate for the hosted runs

`python3 tests/m4/wp4/estimate.py`. Rates come from `research/prices.json` at overage, with allowances ignored as in `tests/m4/results/m4-wp2.md`: memory $0.0000025/GiB-s and disk $0.00000007/GB-s while an instance runs, and CPU $0.00002/vCPU-s active [doc]. Instance types are from the Containers limits page (2026-11-12): `standard-3` has 2 vCPU, 8 GiB, and 16 GB; `standard-4` has 4 vCPU, 12 GiB, and 20 GB [doc]. An hour of `standard-3` costs $0.090 at 10% CPU and $0.220 at 100%. An hour of `standard-4` costs $0.185 at 25% CPU and $0.401 at 100%.

| Item | What runs | Estimate |
| --- | --- | --- |
| BF-60 | 50 jobs of 1 to 8 min (mean 4.5) on `standard-3`. Each Sandbox adds 1.5 min to start and exit [guess]. Up to 10 spares live 10 min until the stale delete [guess; the in-memory run had none]. 6.7 instance-hours | $0.60 to $1.47 |
| BM-4 | A 2 × 4 matrix of 5-min jobs, with each job waiting out a 5-min site failover [guess]. 1.5 instance-hours | $0.14 to $0.34 |
| BF-54 | 2,500 unmodified listeners, 25 `standard-4` for 1 hour (below) | $4.63 to $10.03 |
| Install | One site day and one edge day for M4-WP4's routes, plus BM-4's two site deploys: 3 × M4-WP3's measured $0.53 window. BF-54's polls add about 0.75 M requests | $1.93 |
| Runner account | Workers Paid, one month | $5.00 |
| **Total** | | **$12.30 to $18.76, with a $25 stop** |

### BF-54: 2,500 held polls with unmodified `actions/runner`

- **Instance type: `standard-4`.** It is the largest type, at 12 GiB [doc]. A listener holds about 85 MB idle and a job's worker about 95 MB more [measured, lead]. Memory binds, not vCPU.
- **100 listeners per instance.** 100 × 85 MB plus 512 MiB for the Sandbox server and OS [guess] is 9,012 of 12,288 MiB. The remaining 3.2 GiB holds 34 concurrent workers per instance before memory runs out, or 850 jobs at once across the run.
- **25 instances**, 100 vCPU in all. That is far under the account's limit of 15,000 concurrent vCPU (Containers limits page, read 2026-11-15).
- **Run length: about 36 min, budgeted at 60.**
  - 3 min for the image pull and start [guess].
  - 8.3 min for 2,500 registrations plus 2,500 session opens at 10 a second.
  - 10 min holding 2,500 polls, while the 500 past `BROKER_HELD_MAX` get 503 and re-poll.
  - 10 min of queued jobs.
  - 5 min of teardown.
- **Registration needs rate limiting: yes, about 10 a second, on the driver's side.** All 2,500 runners share one broker scope, so they register on one Actions shard, one Raft group. Each registration and each session open is one commit. One group acked 14.5 to 25 writes a second in earlier runs (`tests/m1/results/m1-wp3.md`, `tests/m0/results/m0-wp12.md`) [measured]. 5,000 commits unpaced would queue for minutes and time out client calls. `RUNNERS_PER_SCOPE_MAX` (10,000) is not reached. The listeners on one instance should also start in batches, because 100 .NET runners starting at once on 4 vCPU is the CPU peak [guess]. These are self-hosted-style runners, not hosted ones, so `HOSTED_CAP` does not apply.
- **Cost:** 25 instance-hours of `standard-4`, $4.63 at 25% CPU and $10.03 at 100%.

### What would move the total

- BF-54 kept past an hour: $4.6 to $10.0 for each further hour.
- A `standard-4` that fits fewer listeners than 100 once measured. At 80 per instance it takes 32 instances and adds about $1.3 to $2.8.
- Containers bill memory and disk while an instance runs, so a Sandbox that outlives its runner costs $0.09 an hour at least. `keepAlive` means only `destroy` or the runner's own exit stops one. The stop line covers a missed teardown of all 25 BF-54 instances for one more hour.

## Checks

All of these pass on the branch:

- `cargo fmt --check`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- wasm32 clippy for `workers/site` and `workers/edge`, each with and without `test-faults`, and for `workers/front` and `workers/runners`. CI now runs the last one too.
- `cargo test --workspace`: 992 tests in 90 binaries, 0 failed.
- `go test ./...` in `tests/m0/porcupine` and `containers/git/server`.
- `scripts/check_design.py`, `check_names.py`, and `check_links.py`.
