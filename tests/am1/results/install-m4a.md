# The `graft-m4a` install

A shared install for Milestone 4. The M4 web thread ([thread](https://ampcode.com/threads/T-01a1d2ef-821f-7572-ad62-26039a8ea35a)) and the M4 Actions thread ([thread](https://ampcode.com/threads/T-01a1d2ef-89bf-77de-94b0-1c2734c66b69)) share it. Its site scripts are **product builds (no `test-faults`)**, so M4's UI and Actions work runs on the code that ships. Its edge is also a product build. Otherwise it is built the way `install-m3e.md` and `install-m3f.md` built `graft-m3e` and `graft-m3f`: from main `20147db`, with `SHARD_INSTANCE=standard-4` (approved for test installs on 2026-11-13), its own `graft-m4a-ops`, the `m3org` seed, and all 3,072 `Tokens` groups precreated through the product route. Nothing here touches another install. Built 2026-11-13 from 03:33Z (branch `install-m3ef`).

## Who deploys what

- **Edge days** belong to the M4 web thread, which needs a stable edge for the UI. Nobody else redeploys `graft-m4a-edge`.
- **Site days** belong to the M4 Actions thread. Before one, it adds a row to the site-day log below with a start time at least 10 minutes ahead, commits and pushes it on `main`, and messages the web thread. When it finishes, it marks the row `done` with the new versions.

## Site-day log

| Announced (UTC) | Start (UTC) | Thread | What | Site a / b / c, edge | State |
| --- | --- | --- | --- | --- | --- |
| 2026-11-13 03:33Z | 03:34Z | build (`install-m3ef`) | site day 1 (product, `standard-4`), then the product edge with `--ops` | `bbe9dfae` / `20c39df1` / `48417c20`, edge `9fb34188` | done |
| 2026-11-13 08:42Z | 08:54Z | M4 Actions | site day 2 (product, `standard-4`): `m4-actions` `aa1ac0d` (the `HookSequencer` class, migration v3, `HOOKSEQ_n` and `OPS` bindings, repository 32.16 read) merged with `m4-web` `8c57a4f` (`/ui/*` routes), image `sha256:099a022c…`, `--vars` site-vars.json, `--ops graft-m4a-ops`, `--var GRAFTYARD_HOST=graft-m4a-edge.<workers-subdomain>.workers.dev`. Every member resets once per site; no site is down for longer than its own deploy. Also `graft-m4a-ops` `ba230a3d` (`/unwrap`, `SECRETS_WRAP_KEY_A` = Secrets Store `graft-m4a-secrets-wrap-key-a` `91947aef…`) | `61050b5d` / `7ffa44fc` / `a7b298a5`, edge unchanged | done 08:56Z |
| 2026-11-13 09:01Z | 09:01Z | M4 web | edge day 2 (product): built from `m4-actions` `aa1ac0d` (contains `m4-web` `8c57a4f`; `hooks.rs` sequencer routes), `deploy-edge.mjs` from `aa1ac0d` (adds `HOOKSEQ_n` → `HookSequencer`), `--anchor`, `--ops graft-m4a-ops`, `FORMATS_ENABLED` plus 32.16. Checked: `GET /repos/m3org/j4`, `/hooks`, and `/user` answer 200 | sites unchanged, edge `06a0aa62` | done |
| 2026-11-13 09:15Z | 09:26Z | M4 Actions | site day 3 (product): `m4-actions` `83afa6d` (a `HookSequencer` fix) merged with `m4-web` `8c57a4f`, same image, flags, and variables as site day 2. Every member resets once per site | `b8353a13` / `2ea98d1c` / `f3fca206`, edge unchanged | done 09:27Z |
| 2026-11-13 11:54Z | 12:05Z | M5-WP1 ([thread](https://ampcode.com/threads/T-01a1d3a3-4afd-70e6-b6e0-3f2cd759f4b5)) | site day 4 (product): `m5-wp1-m4a` `e6377c3` (`m5-wp1` cherry-picked onto `m4a-deploy` `23f14e1`: every `Release` entry, Release formats 0x30.1 and 0x30.2, the `release_at` table), same image, flags, and variables as site day 3. No migration, class, or binding change. Every member resets once per site. Then a separate test edge `graft-m4a-m5-edge` (deleted after) carries BF-42 and BF-45's entries; `graft-m4a-edge` is not redeployed. After this day the `Release` group holds entries past the genesis that only an `m5-wp1` build decodes, so every later site day must include `m5-wp1-m4a`. Checked after the deploy: `GET /api/v3/repos/m3org/j4`, `/api/v3/user`, and `j4.git/info/refs` answer 200 on `graft-m4a-edge`; the `Release` group then committed `Keys` generation 1 (executor and drain keys held in `~/.graft-m4a/m5`), one `ReleaseUploaded`, and a `SiteDay` intent left `IntentDone`, last digest `8b2f4278…` at index 68, nothing current (`tests/m5/results/m5-wp1.md`). `graft-m4a-m5-edge` deleted 12:09Z | `8b3debe1` / `06555a76` / `d0d31e7e`, edge unchanged | done 12:09Z |
| 2026-11-13 ~12:20Z | ~12:20Z | M4 web | edge day 3 (product): `m4-web` `863dd9d` merged with `m4-actions` `83afa6d`, `deploy-edge.mjs --web` (binds the web app's build as `ASSETS`, edge first), same anchor, `--ops`, and `FORMATS_ENABLED` as edge day 2. Serves the web app: the shell, `/_t/ui/*`, and persisted queries. Checked: `/api/v3/repos/m3org/j4`, `/hooks`, `/user`, `/meta`, and git `info/refs` answer 200 | sites unchanged, edge `a57a7a87` | done |
| 2026-11-13 12:42Z | 12:43Z | M4 web | edge day 4 (product): `m4-web` `a3315fa` (favicon from `ASSETS`, a missing asset answers 404) merged as above | sites unchanged, edge `f588de76` | done |
| 2026-11-13 13:08Z | 13:08Z | M4 web | edge day 5 (product): `m4-web` `3a88585` (the shell's document `GET` seeds a private repository from the session cookie) | sites unchanged, edge `d9956ab8` | done |
| 2026-11-13 13:23Z | 13:23Z | M4 web | edge day 6 (product): `m4-web` `91464e1` (web build: review threads without a listed review, the agent card's missing fields). BF-38 ran against this edge | sites unchanged, edge `3911b234` | done |
| 2026-11-13 13:44Z | 13:56Z | M4 Actions | site day 5 (product): `m4a-deploy` `994231a` = `m4-actions` `1be2f12` (a `HookSequencer` fix for BF-57: delivery bodies in their own table, only pending ones in memory, a report chains its queue's next attempt) merged onto `m5-wp1-m4a` `830029f` and `m4-web` `8c57a4f`; same image, flags, and variables as site day 4. No migration, class, or binding change. Every member resets once per site | `a43c4407` / `9fc0a9eb` / `a53f57eb`, edge unchanged | done 13:58Z |
| 2026-11-13 14:09Z | 14:20Z | M4 Actions | site day 6 (product): `m4a-deploy` `15c61f6` = `m4-actions` `cdee31b` (the `HookSequencer`'s restore streams its rows and keeps no done delivery's body; site day 5's store still failed in `save` reading runs 1 and 2's rows) merged as site day 5; same image, flags, and variables. No migration, class, or binding change. Every member resets once per site | `c5e2b5b9` / `7f2886eb` / `7e7616d0`, edge unchanged | done 14:21Z |
| 2026-11-13 14:36Z | 14:47Z | M4 Actions | site day 7 (product): `m4a-deploy` `afa2596` = `m4-actions` `108c1b2` (two `Webhooks` consumer fixes from BF-57 run 4: a consumer back at a store continues past what it holds, and a stale directory refreshes beside the rounds) merged as site day 6; same image, flags, and variables. No migration, class, or binding change. Every member resets once per site | | announced |

## Resources

All resources are in account `<cf-account-id>`. The work directory is `~/.graft-m4a` on the building orb (mode 700, files 0600); it holds the install's private keys and tokens. Threads get a copy as a tarball (below).

| Resource | Name or id | Made by |
| --- | --- | --- |
| Install | `01M79F6BJFJSQCH68C772935XB`, zone `git.m4a.example`, not an evaluation install | `graftyard init` |
| Site 0 script | `graft-m4a-site-a`, `enam` | `graftyard sites add`, then `workers/site/deploy.mjs --anchor --image --skip-build --vars` (no `--test-faults`) |
| Site 1 script | `graft-m4a-site-b`, `weur` | the same |
| Site 2 script | `graft-m4a-site-c`, `apac` | the same |
| Site variables | `SHARD_INSTANCE=standard-4` (`~/.graft-m4a/site-vars.json`, passed with `--vars`) | `workers/site/deploy.mjs --vars` |
| R2 buckets | `graft-m4a-site-a-packs` (ENAM), `graft-m4a-site-b-packs` (WEUR), `graft-m4a-site-c-packs` (APAC) | `sites add` |
| Container apps | `graft-m4a-site-a-gitshard` (`7609860e…`), `graft-m4a-site-b-gitshard` (`4fa2d37a…`), `graft-m4a-site-c-gitshard` (`3e9ace8e…`), class `GitShard` | `sites add` |
| Git shard image | `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5`, the same one `graft-m3d`, `m3e`, and `m3f` run. It lives in `graft-m1`'s registry repository | |
| Cron Trigger | `*/15 * * * *` on each site script | `sites add` |
| Queues | `graft-m4a-prsync-a` (`72fe1fc2…`), `graft-m4a-prsync-b` (`e646e148…`), `graft-m4a-prsync-c` (`e4742ccb…`) | `workers/site/deploy.mjs` |
| Genesis | digest `b40c10ed84bd…`, committed at `Release` index 3. Every system chain is `Tri` on sites 0, 1, and 2, with home 0. `--offline-keys` (the account's Secrets Store count is at its limit) | `genesis --offline-keys`, `sign`, `install --genesis` |
| Tokens shards | **All 3,072 exist** (finished 04:41Z): every 8-group start from 0 to 3,064 answered 200 through the product route `POST /api/graftyard/v1/release/tokens` (the signed genesis as the body), and sampled groups (`tokens:0`, 16, 88, 360, 1000, 1672, 1712, 2400, 3046, 3047, 3071, several once-failed) each have a member on all three sites. The first pass used 16-group ranges and about a third timed out at the edge's 300 s (`fetch failed`), so it was restarted at `tokens:368` with 8-group ranges, up to 18 at once across runs; a few 503s (`NotOneChange`, a placement change in flight, including runs that raced on one range) cleared on a single retry. 03:39Z to 04:41Z, alongside m3e's and m3f's | `tests/m3/hosted/precreate.mjs`'s route |
| Git edge | `graft-m4a-edge`, `https://graft-m4a-edge.<workers-subdomain>.workers.dev`, a **product build** with the install's anchor, `OPS = graft-m4a-ops`, `FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18`, no `LOGINS`. It still holds the secret `BED_TOKEN` from its first, test-faults deploy, which the product build does not read | `tests/m1/integ/deploy-edge.mjs --anchor --ops --var FORMATS_ENABLED=…` |
| Raw `/member` test edge | `graft-m4a-tools-edge`, `https://graft-m4a-tools-edge.<workers-subdomain>.workers.dev`, secret `TOKEN` (`~/.graft-m4a/tools.token`). A TypeScript test Worker that calls the members' raft routes directly; those routes are in the product site build too | `tests/m1/wp7/deploy-edge.mjs` with `SITES`, `GENESIS`, and `EDGES` |
| Ops worker | `graft-m4a-ops`, no public route, `MINT_WRAP_KEY` bound from the Secrets Store | `workers/ops/deploy.mjs graft-m4a-ops bfaa0c1573de4775b1b92a12fb89acb4 graft-m4a-mint-wrap-key` |
| Mint wrap key | Secrets Store secret `graft-m4a-mint-wrap-key` (id `a14276c78b88453388513830f2670a9e`, scope `workers`) in store `bfaa0c1573de4775b1b92a12fb89acb4`, kid 1 from `mintkey 1`, created by an API POST whose body was read from the key file. `PutVerifyingKey` kid 1 committed on `tokens:0` at index 5. The key file was then removed with `shred -u` | |
| Seed | org `m3org` 7100001, `m3-member` 7100002, `m3-admin` 7100003, App `m3-ci` 131072 (client `Iv1.0000000000020000`) installed on `m3org` as 7190001, a PAT each for member and admin; repository `m3org/j4` = `repo:65536` | `tests/m3/hosted/setup.ts --out ~/.graft-m4a/m3` |

Measured member round trips at `sites add`: 0 to 1 97 ms, 0 to 2 206 ms, 2 to 0 196 ms, 2 to 1 183 ms.

Files in `~/.graft-m4a`: `install.toml`, `customer.key` and `release.key` with their `.pub` files, `genesis.unsigned`, `genesis.signed`, `genesis-records.json`, `sites.json`, `site-vars.json`, `bed.token`, `tools.token`, `t0`, `ops/putvk-kid1.json`, and `m3/` (`m3.json`, `m3-tokens.txt`, `app.pem`, `app.secret`).

## What the product build leaves out

A product build compiles out everything under `#[cfg(feature = "test-faults")]`. For M4 this means:

- **Not served on the edge:** every `/_bed/...` route, behind the bed token. These are `identity`, `kill`, `kill_pool`, `precreate`, `status`, `log`, `push`, `heads`, `archive`, `fsck`, `catchup`, `shard_status`, `fault`, `sweep`, and the row routes. Also missing: the `x-graft-kill`, `x-graft-shard`, and `FORCE_FORMAT` hooks, the `x-graft-outcome-index` reply header, and the `GRAFTYARD_GENESIS` stand-in. The install does not need that stand-in, because it reads its system records from the `Release` group.
- **Not served on the members:** the fault and diagnostic routes, which are `/log`, `/audit`, `/faults`, `/deps_log`, `/isolate`, `/arrivals`, `/record`, `/probe_vote`, `/sever`, `/depth_left`, `/depth_hop`, `/read_bench`, and `/reset`. Also missing: the `kill`, `down_ms`, `receipt`, `no_marker`, `hold_ms`, and `hold_deps_ms` command fields, and `/place`'s `near_ms` override. `/status`, `/commit`, `/read`, `/entries`, `/create`, and the rest of the raft and placement routes are in the product build, and the tools edge uses only those.

As far as `20147db` goes, nothing M4 needs is test-faults only. The UI's REST and GraphQL routes, webhooks (`hooks.rs`), the device flow, and the test consent pages (`GET|POST /login/oauth/authorize` and `POST /login/oauth/device/approve`) are all served by the product edge, and formats are enabled through `FORMATS_ENABLED`. What a driver loses is listed below:

- `tests/m3/hosted/setup.ts` seeds `identity:` chains through `/_bed/identity`, so the seed ran while the edge was still its first, test-faults build. Seeding more users or orgs needs a test-faults edge for the duration (an edge day), or a product path such as sign-up.
- `tests/fanout/teardown.py` stops a shard through `/_bed/kill`, so on this install a shard container stops only through its own 6 h inactivity timeout or a site day. A `standard-4` shard bills about 4 vCPU, 12 GiB, and 20 GB while it runs. The Containers API refused a direct instance delete (`NOT_ENABLED`).
- `precreate.mjs`'s `/_bed/precreate` path is gone, but its `R20_GENESIS` mode uses the product route: `GRAFT_EDGE=https://graft-m4a-edge.<workers-subdomain>.workers.dev R20_GENESIS=~/.graft-m4a/genesis.signed node tests/m3/hosted/precreate.mjs <out.jsonl> 8` (8-group ranges by default).

## Point the drivers at it

Fetch the state: `download_thread_file` of `.amp/xfer/graft-m4a.tgz` from the building thread ([thread](https://ampcode.com/threads/T-01a1c666-2b83-7522-8afd-45a508a3cd74), workspace `/home/user/workspace/repo`), then `tar -C $HOME -xzpf graft-m4a.tgz`. Never print its files.

- `source tests/am1/lf13/env-m4a.sh` sets `GRAFT_EDGE`, `GRAFT_TOOLS_EDGE`, `BED_TOKEN_FILE`, `M3_STATE=~/.graft-m4a/m3`, and `INSTALL=graft-m4a`.
- `tests/am1/hosted/lib.mjs` reads the tools token from `tests/m0/.m0-token` in `GRAFT_MAIN_CHECKOUT`: `ln -sf ~/.graft-m4a/tools.token tests/m0/.m0-token` (git-ignored), and remove the link after. Its `bed()` helper calls `/_bed/identity`, which answers 404 here.
- A bench edge binds `Member` in each site script: `MEMBER_0` → `graft-m4a-site-a`, `MEMBER_1` → `graft-m4a-site-b`, `MEMBER_2` → `graft-m4a-site-c`, each with `script_name`. `Registry` and `GitShard` follow the same pattern.

### Site day (M4 Actions thread)

Announce the site day in the log first, at least 10 minutes ahead. Pass `--vars` every time, and never pass `--test-faults`:

```
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
node workers/site/deploy.mjs ~/.graft-m4a/sites.json --anchor ~/.graft-m4a/install.toml --image "$IMG" \
  --vars ~/.graft-m4a/site-vars.json
```

A new site format reaches the edge only after a site day, and then through the web thread's next edge day with the format added to `FORMATS_ENABLED`.

### Edge day (M4 web thread)

The `OPS` binding and `FORMATS_ENABLED` are kept only if every deploy passes them again. Leave out `--test-faults`, because the product build goes to `/tmp/edge-prod-graft-m4a-edge`:

```
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18,32.16
node tests/m1/integ/deploy-edge.mjs ~/.graft-m4a/sites.json graft-m4a-edge \
  --anchor ~/.graft-m4a/install.toml --ops graft-m4a-ops --web /tmp/webprod/client --var FORMATS_ENABLED=$FORMATS
```

Build from `m4-web` merged with `m4-actions` (its `hooks.rs` and the `HOOKSEQ_n` bindings in `deploy-edge.mjs`) and with main through 3b18ded (M5-WP1's `release.rs`). `--web` takes the web app's product build: in `web/`, `VITE_MOCK_API=false npx vite build`, then copy `dist/client` without `mockServiceWorker.js`.

Cost: `INSTALL=graft-m4a python3 tests/fanout/m2bcost.py <from ISO> <to ISO>`.

## Proof it works

- `install` committed the genesis at `Release` index 3, through the edge's first, test-faults build. All 3,072 `Tokens` groups exist (row above).
- On the **product** edge (`9fb34188`), m3-ci's installation token (`ghs({})`, `POST /api/v3/app/installations/7190001/access_tokens`) answered **201**, a `ghs_` token, `contents: write, metadata: read`, through `OPS` → `graft-m4a-ops`. `POST /_bed/status` answered 404, as a product build should.
- `m3-member` acquired and released a Shared lease on `m3org/j4` (201, 204). `repo:65536` had a leader on site 0 and followers on 1 and 2, and a follower's `/feed` answered `hold` with a cursor. `m3-admin` cloned `m3org/j4` and pushed a branch, and `m3-member` cloned it after.
- That push started shard `git#0#net:65536` on site a. The Containers API reported it at 4 vCPU, 12,288 MiB, and 20,000 MB, which is `standard-4`. It could not be torn down here, because the product edge has no `/_bed/kill` and the API answered `NOT_ENABLED`. So it runs until its 6 h inactivity timeout or the next site day.

## Teardown

Run these steps in order. Each one is safe to repeat.

1. Delete the scripts `graft-m4a-edge`, `graft-m4a-tools-edge`, `graft-m4a-ops`, `graft-m4a-site-a`, `graft-m4a-site-b`, and `graft-m4a-site-c` (`npx --prefix tests/m0 wrangler delete --name <script>`). Delete the edge before `graft-m4a-ops`, because the edge binds it.
2. Delete the container applications `graft-m4a-site-{a,b,c}-gitshard`.
3. Empty and delete the buckets `graft-m4a-site-{a,b,c}-packs`.
4. Delete the queues `graft-m4a-prsync-{a,b,c}`.
5. Delete the secret `graft-m4a-mint-wrap-key` (id `a14276c7…`) from store `bfaa0c1573de4775b1b92a12fb89acb4`, but not the store itself.
6. Remove `~/.graft-m4a` and every staged copy of it.
