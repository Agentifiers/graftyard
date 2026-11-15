# M4-WP4, hosted runners on Sandboxes: results

Branch `m4-runners`, from main f6db1765. The choices are in `decisions/DECISIONS.md`, "M4-WP4: how hosted runners register, count against the cap, and go stale, 2026-11-15". Nothing was deployed, no runner account exists, and nothing was spent on Sandboxes or Cloudflare: the hosted rows wait for the operator's approval of the estimate below.

## Status

| Row | In memory | Hosted |
| --- | --- | --- |
| BF-60 | **Pass** (`workers/runners/tests/bf60.rs`) | Not run: waits for the estimate's approval |
| BM-4 | (M4-WP3's matrix with an unmodified runner) | Not run: waits for the estimate's approval |
| BF-54 | (M4-WP3's broker cap) | Not run: waits for the estimate's approval |

## What is built

- **The install side** (`crates/actions/src/hosted.rs`, additive calls in `crates/actions/src/shard.rs`):
  - The hosted label table, which is SHARED's.
  - `decide`'s rule for a hosted `PutRunner` (`admit`): the runner is ephemeral, `Owner`-scoped, and carries exactly one row's labels. Past `HOSTED_CAP` (20) hosted runners it is refused `TooMany`, and the count includes the entry's earlier commands.
  - The rule for a hosted `DeleteRunner` (`may_delete`): a delete by anyone but the runner itself is refused while the runner holds a job.
  - `ShardSummary` and `Broker::demand` (`demand`, `HostedDemand`, `LabelDemand`), with the broker's memory in `Seen`.
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
- **The runner image** (`containers/runner/Dockerfile`): the `cloudflare/sandbox` 0.12.14 base plus `actions/runner` 2.338.2, checked against its release SHA-256. Not built, because the orb cannot reach the Docker daemon. The first hosted run builds it.
- **Not built:** `graftyard runners init` and `graftyard runners upgrade`. `HOSTED_OWNERS` (owner to installation id) stands in for listing the App's installations, which the install does not serve yet.

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

## Plants: `tests/m4/wp4/plant.py`

17 of 17 caught. The first run missed two. "Start ignores the cap's room" got through because every test queued only one row; `hosted::tests::rows_share_the_caps_room` now queues two. "A runner whose job completed is not stale" got through because every runner that had held a job was also past `HOSTED_IDLE`; the demand test now queues jobs while those runners are stale. One planned plant was dropped: "the cap counts only the fold" cannot be written without reaching into the scratch's private fields.

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
| 12 | A refused delete still destroys the Sandbox | the stale-runner test, the pass test |
| 13 | A stale runner's Sandbox is destroyed without deleting it | bf60, the stale-runner test, the pass test |
| 14 | The vCPU stop is ignored | the vCPU test |
| 15 | Registered runners are not counted toward the vCPU stop | the vCPU test |
| 16 | Every label runs on `standard-3` | the pass test, the vCPU test |

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
- **25 instances**, 100 vCPU in all. That is far under the account's concurrent vCPU limit, which the limits page now gives as 15,000 (the design and `research/agents.md` give 1,500).
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
- `cargo test --workspace`: 966 tests in 88 binaries, 0 failed.
- `go test ./...` in `tests/m0/porcupine` and `containers/git/server`.
- `scripts/check_design.py`, `check_names.py`, and `check_links.py`.
