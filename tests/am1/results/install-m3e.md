# The `graft-m3e` test install

A shared test install for narrow-reads-2 ([thread](https://ampcode.com/threads/T-01a1ce58-d840-723b-aa5a-a57d8d049708)). It is built the way `install-m3d.md` built `graft-m3d`, with two differences: it is built from main `20147db`, and its shards run on `standard-4` from the first site day (`SHARD_INSTANCE=standard-4`, which the operator approved for test installs on 2026-11-13). `graft-m3e` and `graft-m3f` were built at the same time from one worktree. Nothing here touches another install. Built 2026-11-13 from 03:07Z (branch `install-m3ef`, from `20147db`).

## Resources

All resources are in account `<cf-account-id>`. The work directory is `~/.graft-m3e` on the building orb (mode 700, files 0600); it holds the install's private keys and tokens. Threads get a copy as a tarball (below).

| Resource | Name or id | Made by |
| --- | --- | --- |
| Install | `01M79DGV5MK312RMVCDTGZ7XHH`, zone `git.m3e.example`, not an evaluation install | `graftyard init` |
| Site 0 script | `graft-m3e-site-a`, `enam` | `graftyard sites add`, then `workers/site/deploy.mjs --test-faults --anchor --image --skip-build --vars` |
| Site 1 script | `graft-m3e-site-b`, `weur` | the same |
| Site 2 script | `graft-m3e-site-c`, `apac` | the same |
| Site variables | `SHARD_INSTANCE=standard-4` (`~/.graft-m3e/site-vars.json`, passed with `--vars`) | `workers/site/deploy.mjs --vars` |
| R2 buckets | `graft-m3e-site-a-packs` (ENAM), `graft-m3e-site-b-packs` (WEUR), `graft-m3e-site-c-packs` (APAC) | `sites add` |
| Container apps | `graft-m3e-site-a-gitshard` (`ef5f7367…`), `graft-m3e-site-b-gitshard` (`a38d2767…`), `graft-m3e-site-c-gitshard` (`0f513ff2…`), class `GitShard` | `sites add` |
| Git shard image | `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5`, hot-shards-2's image, the same one `graft-m3d` runs; reused, not rebuilt. It lives in `graft-m1`'s registry repository | |
| Cron Trigger | `*/15 * * * *` on each site script | `sites add` |
| Queues | `graft-m3e-prsync-a` (`f91bf4de…`), `graft-m3e-prsync-b` (`3a52bd95…`), `graft-m3e-prsync-c` (`bf7400c8…`), each the `PRSYNC` producer and consumer of its site | `workers/site/deploy.mjs` |
| Genesis | digest `8e2e6eaced35…`, committed at `Release` index 5. Every system chain is `Tri` on sites 0, 1, and 2, with home 0. `--offline-keys`: the account's Secrets Store count is at its limit, so the drain and executor keys were not stored | `genesis --offline-keys`, `sign`, `install --genesis` |
| Tokens shards | All 3,072 `Tokens` shard groups, made by `GRAFT_EDGE=<edge> R20_GENESIS=~/.graft-m3e/genesis.signed node tests/m3/hosted/precreate.mjs <out> 8` (the product route `POST /api/graftyard/v1/release/tokens`, eight ranges at once; 16 groups each when this was built, 8 now). `install --genesis` was stopped once it printed the genesis commit, before its slow one-range precreation. **All 3,072 exist** (finished 04:29Z): every 8-group start from 0 to 3,064 answered 200 in some pass, and sampled groups (`tokens:0`, 63, 143, 511, 1279, 1411, 1516, 2304, 2923, 3071, several of them once-failed) each have a member on all three sites. About 470 range requests over 03:15Z to 04:29Z, 41 or 42 not 200. Two kinds of failure, both retried: a 16-group range that outlasts the edge's 300 s answers `fetch failed` (about a third of 16-group ranges did while two installs precreated at once; 8-group ranges, `count=8`, did not), and a 503 whose `failed` names one group's `NotOneChange` or "the committed config is not Normal", a placement change still in flight, which a later single retry cleared. The extra passes used ranges of 8 from a helper; `precreate.mjs` now defaults to 8 (`PRECREATE_RANGE`) and retries listed starts with `ONLY=`. | `tests/m3/hosted/precreate.mjs` |
| Git edge | `graft-m3e-edge`, `https://graft-m3e-edge.<workers-subdomain>.workers.dev`, a test-faults build with the install's anchor, `OPS = graft-m3e-ops`, `FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18`, no `LOGINS`; secret `BED_TOKEN` (`~/.graft-m3e/bed.token`) | `tests/m1/integ/deploy-edge.mjs --test-faults --anchor --ops --var FORMATS_ENABLED=…` |
| Raw `/member` test edge | `graft-m3e-tools-edge`, `https://graft-m3e-tools-edge.<workers-subdomain>.workers.dev`, secret `TOKEN` (`~/.graft-m3e/tools.token`) | `tests/m1/wp7/deploy-edge.mjs` with `SITES`, `GENESIS`, and `EDGES` |
| Ops worker | `graft-m3e-ops`, no public route, `MINT_WRAP_KEY` bound from the Secrets Store | `workers/ops/deploy.mjs graft-m3e-ops bfaa0c1573de4775b1b92a12fb89acb4 graft-m3e-mint-wrap-key` |
| Mint wrap key | Secrets Store secret `graft-m3e-mint-wrap-key` (id `6742c5a215e541978fad05e0413b8268`, scope `workers`) in the account's one store `bfaa0c1573de4775b1b92a12fb89acb4`, kid 1, from `tests/m3/hosted/tools` `mintkey 1`, created by an API POST whose body was read from the key file (the value never reached argv). `PutVerifyingKey` kid 1 committed on `tokens:0` at index 7 through the tools edge's `/commit`. The key file was then removed with `shred -u`; only the public command remains (`~/.graft-m3e/ops/putvk-kid1.json`) | |
| Seed | org `m3org` 7100001, `m3-member` 7100002, `m3-admin` 7100003, App `m3-ci` 131072 (client `Iv1.0000000000020000`) installed on `m3org` as 7190001, a PAT each for member and admin; repository `m3org/j4` = `repo:65536` | `tests/m3/hosted/setup.ts --out ~/.graft-m3e/m3` |

The agent formats are enabled on the edge (32.8, 32.12, 32.14, 18.3), and so is 32.18; the site scripts read every format their build knows. `tests/am1/rest/apps.mjs` (the three agent Apps) was not run; a thread that needs them runs it.

Measured member round trips at `sites add`: 0 to 1 110 ms, 0 to 2 252 ms, 2 to 0 207 ms, 2 to 1 165 ms.

Files in `~/.graft-m3e`: `install.toml`, `customer.key` and `release.key` with their `.pub` files, `genesis.unsigned`, `genesis.signed`, `genesis-records.json`, `sites.json`, `site-vars.json`, `bed.token`, `tools.token`, `t0` (the build's start, Unix seconds), `ops/putvk-kid1.json`, and `m3/` (`m3.json`, `m3-tokens.txt`, `app.pem`, `app.secret`).

## Deployed versions

| Script | Version | Build |
| --- | --- | --- |
| `graft-m3e-site-a` / `-b` / `-c` | `11e870b4` / `4c697b63` / `c7359636` (site day 1, 03:08Z to 03:10Z, via phase 1 `1571f000` / `c8df6e19` / `950ce569`) | `20147db`, `--features test-faults`, `--anchor`, the image above, `SHARD_INSTANCE=standard-4` |
| `graft-m3e-edge` | `0534a614` (no `OPS`), then the `BED_TOKEN` secret, then `5a8aa9da` (03:24Z, `--ops graft-m3e-ops`) | `20147db`, test-faults |
| `graft-m3e-tools-edge` | `78a3cb57`, with the `TOKEN` secret | `tests/m1/wp7/edge.ts` at `20147db` |
| `graft-m3e-ops` | `f946f9f8` | `workers/ops` at `20147db` |

## Point the drivers at it

Fetch the state: `download_thread_file` of `.amp/xfer/graft-m3e.tgz` from the building thread ([thread](https://ampcode.com/threads/T-01a1c666-2b83-7522-8afd-45a508a3cd74), workspace `/home/user/workspace/repo`), then `tar -C $HOME -xzpf graft-m3e.tgz` (it unpacks `~/.graft-m3e` with its modes). Never print its files.

- `source tests/am1/lf13/env-m3e.sh` sets `GRAFT_EDGE`, `GRAFT_TOOLS_EDGE`, `BED_TOKEN_FILE`, `M3_STATE=~/.graft-m3e/m3`, and `INSTALL=graft-m3e` (for `tests/fanout/m2bcost.py`, `containers.py`, and `teardown.py`).
- `tests/am1/hosted/lib.mjs` and `tests/m3/hosted/setup.ts` read the tools token from `tests/m0/.m0-token` in the checkout `GRAFT_MAIN_CHECKOUT` names (default `/home/user/workspace/repo`): `ln -sf ~/.graft-m3e/tools.token tests/m0/.m0-token` (git-ignored), and remove the link after.
- The Member class a bench edge binds is `Member` in each site script: `MEMBER_0` → `graft-m3e-site-a`, `MEMBER_1` → `graft-m3e-site-b`, `MEMBER_2` → `graft-m3e-site-c`, each with `script_name`. `Registry` and `GitShard` follow the same pattern (`REGISTRY_<n>`, `SHARD_<n>`).

### Site day

Every new site-script version resets the site's members, so this is a site day (`AGENTS.md`). Pass `--vars` every time, or the shards fall back to `standard-2`:

```
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
node workers/site/deploy.mjs ~/.graft-m3e/sites.json --test-faults --anchor ~/.graft-m3e/install.toml --image "$IMG" \
  --vars ~/.graft-m3e/site-vars.json
```

With a new image or instance size, add `--roll-shards https://graft-m3e-edge.<workers-subdomain>.workers.dev` (needs `BED_TOKEN_FILE`), or tear the running shards down after (`INSTALL=graft-m3e python3 tests/fanout/teardown.py --all-nets --pools`).

### Edge day

The edge's `OPS` binding and `FORMATS_ENABLED` are kept only if every deploy passes them again:

```
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
node tests/m1/integ/deploy-edge.mjs ~/.graft-m3e/sites.json graft-m3e-edge --test-faults \
  --anchor ~/.graft-m3e/install.toml --ops graft-m3e-ops --var FORMATS_ENABLED=$FORMATS
```

`BED_TOKEN` survives a redeploy. A new edge name needs it put once: `cd tests/m1/integ/.gen && npx --prefix ../../../m0 wrangler secret put BED_TOKEN -c <name>.jsonc < ~/.graft-m3e/bed.token`.

Cost: `INSTALL=graft-m3e python3 tests/fanout/m2bcost.py <from ISO> <to ISO>`.

## Proof it works

- `install` committed the genesis at `Release` index 5. All 3,072 `Tokens` groups exist (row above).
- m3-ci's installation token: `POST /api/v3/app/installations/7190001/access_tokens` with the App's JWT (`ghs({})` in `tests/am1/hosted/lib.mjs`) answered **201**, a `ghs_` token, permissions `contents: write, metadata: read`, through `graft-m3e-edge` → `OPS` → `graft-m3e-ops`.
- `m3-member` acquired and released a Shared lease on `m3org/j4` (201, 204); `repo:65536` had a leader on site 0 and followers on 1 and 2, and a follower's `/feed` answered `hold`. `m3-admin` cloned `m3org/j4` and pushed a branch through the git cache shard, and `m3-member` cloned it after. The shard, `git#0#net:65536`, ran at 4 vCPU, 12,288 MiB, 20,000 MB (`standard-4`, from the Containers API) and was torn down with `teardown.py --all-nets --pools` (200). No shard container is left running.

## Teardown

Run these in order. Each step is safe to repeat.

1. Delete the scripts `graft-m3e-edge`, `graft-m3e-tools-edge`, `graft-m3e-ops`, `graft-m3e-site-a`, `graft-m3e-site-b`, and `graft-m3e-site-c` (`npx --prefix tests/m0 wrangler delete --name <script>`). Delete the edge before `graft-m3e-ops`, which it binds. Deleting a site script deletes its Durable Objects and their storage.
2. Delete the container applications `graft-m3e-site-{a,b,c}-gitshard` (`wrangler containers list`, then `wrangler containers delete <id>`).
3. Empty and delete the buckets `graft-m3e-site-{a,b,c}-packs` (`wrangler r2 bucket delete <name>` after removing their objects).
4. Delete the queues `graft-m3e-prsync-{a,b,c}` (`wrangler queues delete <name>`).
5. Delete the secret `graft-m3e-mint-wrap-key` (id `6742c5a2…`) from store `bfaa0c1573de4775b1b92a12fb89acb4`. Do not delete the store: it holds other installs' secrets.
6. Remove `~/.graft-m3e` and every staged copy of it. It holds the install's private keys and tokens.

The git shard image lives in `graft-m1`'s registry repository. Do not delete it.
