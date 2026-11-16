# M4-WP4, hosted runners on Sandboxes: results

The hosted rows ran on 2026-11-15 on graft-m4a (site days 42 to 45, edge day 20) and in the runner account, from main `aa4a9d9c` and then `c7970488`. The status table below gives the results. The next paragraph is the record from before the merge.

Branch `m4-runners`, rebased onto main 4833094d. The choices are in `decisions/DECISIONS.md`, "M4-WP4: how hosted runners register, count against the cap, and go stale, 2026-11-15". The operator approved the estimate below with a $25 stop. The runner account exists: "Graftyard Runners", `<cf-runner-account-id>`, on Workers Paid, workers.dev subdomain `graftyard-runners`. Its account-owned token is the project secret `CLOUDFLARE_RUNNER_API_TOKEN`. On 2026-11-15 it verified at `/accounts/{id}/tokens/verify` and read Workers scripts, Durable Object namespaces, Containers, the registry, and Secrets Store. Its policy grants Workers Scripts, Containers (Cloudchamber), Secrets Store, Workers Observability, Workers Routes, and Account Settings Write. The account then held no script, no container application, and no store. Nothing is deployed yet: the hosted rows wait for the lead's merge to main.

## Status

| Row | In memory | Hosted |
| --- | --- | --- |
| BF-60 | **Pass** (`workers/runners/tests/bf60.rs`) | **Pass**, run 1966182 on site day 43, sweep only, demand read at the sweep's cadence |
| BM-4 | (M4-WP3's matrix with an unmodified runner) | **Pass**, run 1900546, site a down mid-run (site days 44 and 45) |
| BF-54 | (M4-WP3's broker cap) | **Fail**, 2,500 listeners on 25 Sandboxes: 503s, re-polls, and sessions pass; job start fails (a lost `Repo`-scope match waits out its lease, F76) |

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

## Spend against the $25 stop

| Account | Window | Metered |
| --- | --- | --- |
| graft-m4a | 12:27Z to 22:02Z | $4.13 (containers $2.58: git shards and site days 42 to 45) |
| Runner account, `graft-runners` | 15:00Z to 22:02Z | $0.83 (Sandboxes $0.76: smoke, BF-60 twice, BM-4) |
| Runner account, BF-54 scratch | 20:30Z to 21:47Z | $2.72 (16,334 vCPU-s and 913,634 GiB-s on `standard-4`), from `containersUsageAdaptiveGroups` by application id. `APPS=graft-runners-` no longer finds the deleted application |
| Workers Paid | one month | $5.00 |
| **Total** | | **$12.68 of $25**, inside the approved $12.30 to $18.76 |

BF-54 cost $2.72, under its $4.63 to $10.03 estimate. It needed 1.3 hours of 25 boxes, not one at full price, because the boxes idled at low CPU.

## Plants: `tests/m4/wp4/plant.py`

21 of 21 caught after the rebase onto main 4833094d (plant 12 is BF-55's "never rematch an ephemeral runner" rule, which holds for hosted runners because they are ephemeral and `Owner`-scoped; plants 18 to 20 are `graftyard runners upgrade`'s). The first run missed two. "Start ignores the cap's room" got through because every test queued only one row; `hosted::tests::rows_share_the_caps_room` now queues two. "A runner whose job completed is not stale" got through because every runner that had held a job was also past `HOSTED_IDLE`; the demand test now queues jobs while those runners are stale. One planned plant was dropped: "the cap counts only the fold" cannot be written without reaching into the scratch's private fields.

| # | Plant | Caught by |
| --- | --- | --- |
| 0 | `decide` never checks a hosted registration | bf60, the cap test |
| 1 | No cap | bf60, the cap test |
| 2 | The cap lets one more through | bf60, the cap test |
| 3 | A hosted runner may be repository-scoped | the cap test |
| 4 | A busy hosted runner may be deleted | the stale-runner test |
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
