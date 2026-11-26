# The `graft-m3d` test install

A second shared test install, so threads stop queuing on `graft-m3c`. It is built the way `install-m3c.md` built `graft-m3c` (`graftyard init`, `sites add` three times with a test-faults bundle, `genesis --offline-keys`, `sign`, an edge with the install's anchor, `install --genesis`, the tools edge, `tests/m3/hosted/setup.ts`), with three differences: everything is built from main `d7ee927`; it has `graft-m3d-ops` from the start, bound to the edge as `OPS`; and its sites run hot-shards-2's git shard image. Nothing here touches `graft-m1-*`, `graft-m2b-*`, or `graft-m3c-*`. Built 2026-11-12 from 23:08Z (branch `install-m3d`, from `d7ee927`).

## Resources

All resources are in account `<cf-account-id>`. The work directory is `~/.graft-m3d` on the building orb (mode 700, files 0600); it holds the install's private keys and tokens. Threads get a copy as a tarball (below).

| Resource | Name or id | Made by |
| --- | --- | --- |
| Install | `01M790R3NR0HCX61DZB27XW2G9`, zone `git.m3d.example`, not an evaluation install | `graftyard init` |
| Site 0 script | `graft-m3d-site-a`, `enam` | `graftyard sites add`, then `workers/site/deploy.mjs --test-faults --anchor --image --skip-build` |
| Site 1 script | `graft-m3d-site-b`, `weur` | the same |
| Site 2 script | `graft-m3d-site-c`, `apac` | the same |
| R2 buckets | `graft-m3d-site-a-packs` (ENAM), `graft-m3d-site-b-packs` (WEUR), `graft-m3d-site-c-packs` (APAC) | `sites add` |
| Container apps | `graft-m3d-site-a-gitshard` (`24a795d2…`), `graft-m3d-site-b-gitshard` (`b71e1d52…`), `graft-m3d-site-c-gitshard` (`e9688b94…`), class `GitShard` | `sites add` |
| Git shard image | `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5`, hot-shards-2's image (`origin/hs2-deploy`, `tests/am1/hotshards/siteday.sh`); reused, not rebuilt. It lives in `graft-m1`'s repository | |
| Cron Trigger | `*/15 * * * *` on each site script | `sites add` |
| Queues | `graft-m3d-prsync-a` (`d54caecb…`), `graft-m3d-prsync-b` (`9976b996…`), `graft-m3d-prsync-c` (`34e1661e…`), each the `PRSYNC` producer and consumer of its site | `workers/site/deploy.mjs` |
| Genesis | digest `e45c47d56548…`, committed at `Release` index 3. Every system chain is `Tri` on sites 0, 1, and 2, with home 0. `--offline-keys`: the account's Secrets Store count is at its limit, so the drain and executor keys were not stored | `genesis --offline-keys`, `sign`, `install --genesis` |
| Tokens shards | All 3,072 `Tokens` shard groups. `install --genesis` creates them one range at a time, about 25 groups a minute, so after it had made `tokens:0` to about `tokens:490` it was stopped at 23:51Z and `R20_GENESIS=~/.graft-m3d/genesis.signed node tests/m3/hosted/precreate.mjs <out> 8` (the same product route, `POST /api/graftyard/v1/release/tokens`, eight 16-group ranges at once; the script now defaults to 8-group ranges, which do not time out) continued from 23:45Z. A range answers 503 when a group's placement misses `NEAR_RTT` (site 1 measured 104 to 111 ms from site 0, over 100 ms) or its config change is still in flight; a rerun retries it. At 23:55Z about 800 groups existed and the run was continuing; check with `tests/am1/hosted/lib.mjs` `statuses("tokens:<n>")` (a group not created yet answers `joined: false` on every site). | `install --genesis`, `tests/m3/hosted/precreate.mjs` |
| Git edge | `graft-m3d-edge`, `https://graft-m3d-edge.<workers-subdomain>.workers.dev`, a test-faults build with the install's anchor, `OPS = graft-m3d-ops`, `FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18`, no `LOGINS`; secret `BED_TOKEN` (`~/.graft-m3d/bed.token`) | `tests/m1/integ/deploy-edge.mjs --test-faults --anchor --ops --var FORMATS_ENABLED=…` |
| Raw `/member` test edge | `graft-m3d-tools-edge`, `https://graft-m3d-tools-edge.<workers-subdomain>.workers.dev`, secret `TOKEN` (`~/.graft-m3d/tools.token`) | `tests/m1/wp7/deploy-edge.mjs` with `SITES`, `GENESIS`, and `EDGES` |
| Ops worker | `graft-m3d-ops`, no public route, `MINT_WRAP_KEY` bound from the Secrets Store | `workers/ops/deploy.mjs graft-m3d-ops <store> graft-m3d-mint-wrap-key` |
| Mint wrap key | Secrets Store secret `graft-m3d-mint-wrap-key` (id `20a3c4cee28043a8930be2441440fa73`, scope `workers`) in the account's one store `<store>`, kid 1, from `tests/m3/hosted/tools` `mintkey 1`, created by an API POST whose body was read from the key file (the value never reached argv). `PutVerifyingKey` kid 1 committed on `tokens:0` at index 9 through the tools edge's `/commit`. The key file was then removed with `shred -u`; only the public command remains (`~/.graft-m3d/ops/putvk-kid1.json`) | |
| Seed | org `m3org` 7100001, `m3-member` 7100002, `m3-admin` 7100003, App `m3-ci` 131072 (client `Iv1.0000000000020000`) installed on `m3org` as 7190001, a PAT each for member and admin; repository `m3org/j4` = `repo:65536` | `tests/m3/hosted/setup.ts --out ~/.graft-m3d/m3` |

The agent formats are enabled on the edge (32.8, 32.12, 32.14, 18.3) and so is 32.18 (`IssueOp::CreateCheck`); the site scripts read every format their build knows. `tests/am1/rest/apps.mjs` (the three agent Apps) was not run; a thread that needs them runs it.

Measured member round trips at `sites add`: 0 to 1 135 ms, 0 to 2 252 ms, 2 to 0 213 ms, 2 to 1 152 ms.

Files in `~/.graft-m3d`: `install.toml`, `customer.key` and `release.key` with their `.pub` files, `genesis.unsigned`, `genesis.signed`, `genesis-records.json` (the placements the tools edge reads), `sites.json`, `bed.token`, `tools.token`, `t0` (the build's start, Unix seconds), `ops/putvk-kid1.json`, and `m3/` (`m3.json`, `m3-tokens.txt`, `app.pem`, `app.secret`).

## Deployed versions

| Script | Version | Build |
| --- | --- | --- |
| `graft-m3d-site-a` / `-b` / `-c` | `a57e71f9` / `e58f542f` / `8fdaed2e` (site day 1, 23:19Z to 23:21Z, via phase 1 `121b851d` / `7a4ce7d4` / `8f34d952`; `sites add` made the versions before) | `d7ee927`, `--features test-faults`, `--anchor`, the image above |
| `graft-m3d-edge` | `d36b87e5` (no `OPS`), then the `BED_TOKEN` secret, then `39a78805` (23:30Z, `--ops graft-m3d-ops`) | `d7ee927`, test-faults |
| `graft-m3d-tools-edge` | `a2403c09`, then the `TOKEN` secret | `tests/m1/wp7/edge.ts` at `d7ee927` |
| `graft-m3d-ops` | `bef3a6d1` | `workers/ops` at `d7ee927` |

## Point the drivers at it

Fetch the state: `download_thread_file` of `.amp/xfer/graft-m3d.tgz` from the building thread, then `tar -C $HOME -xzpf graft-m3d.tgz` (it unpacks `~/.graft-m3d` with its modes). Never print its files.

- `source tests/am1/lf13/env-m3d.sh` sets `GRAFT_EDGE`, `GRAFT_TOOLS_EDGE`, `BED_TOKEN_FILE`, `M3_STATE=~/.graft-m3d/m3`, and `INSTALL=graft-m3d` (for `tests/fanout/m2bcost.py`, `containers.py`, and `teardown.py`).
- `tests/am1/hosted/lib.mjs` and `tests/m3/hosted/setup.ts` read the tools token from `tests/m0/.m0-token` in the checkout `GRAFT_MAIN_CHECKOUT` names (default `/home/user/workspace/repo`): `ln -sf ~/.graft-m3d/tools.token tests/m0/.m0-token` (git-ignored), and remove the link after.
- The Member class a bench edge binds is `Member` in each site script: `MEMBER_0` → `graft-m3d-site-a`, `MEMBER_1` → `graft-m3d-site-b`, `MEMBER_2` → `graft-m3d-site-c`, each with `script_name`. `Registry` and `GitShard` follow the same pattern (`REGISTRY_<n>`, `SHARD_<n>`).

### Site day

Every new site-script version resets the site's members, so this is a site day (`AGENTS.md`). From the checkout to deploy, with the Rust and `worker-build` toolchains:

```
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
node workers/site/deploy.mjs ~/.graft-m3d/sites.json --test-faults --anchor ~/.graft-m3d/install.toml --image "$IMG"
```

**Shard size:** the operator approved `standard-4` shard containers on test installs (2026-11-13). graft-m3d was built without `SHARD_INSTANCE`, so its shards run the default `standard-2`. From its next site day it runs `standard-4`: add `--vars <file>` to the command above, where the file holds `{"SHARD_INSTANCE": "standard-4"}`, and tear the running shards down after so they restart at the new size.

With a new image, add `--roll-shards https://graft-m3d-edge.<workers-subdomain>.workers.dev` (needs `BED_TOKEN_FILE`), or tear the running shards down after (`INSTALL=graft-m3d python3 tests/fanout/teardown.py --all-nets --pools`).

### Edge day

The edge's `OPS` binding and `FORMATS_ENABLED` are kept only if every deploy passes them again:

```
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
node tests/m1/integ/deploy-edge.mjs ~/.graft-m3d/sites.json graft-m3d-edge --test-faults \
  --anchor ~/.graft-m3d/install.toml --ops graft-m3d-ops --var FORMATS_ENABLED=$FORMATS
```

`BED_TOKEN` survives a redeploy. A new edge name needs it put once: `cd tests/m1/integ/.gen && npx --prefix ../../../m0 wrangler secret put BED_TOKEN -c <name>.jsonc < ~/.graft-m3d/bed.token`.

Cost: `INSTALL=graft-m3d python3 tests/fanout/m2bcost.py <from ISO> <to ISO>`.

## Proof it works

- `install` committed the genesis at `Release` index 3, and the `Tokens` groups it had created by 23:45Z (sampled `tokens:1` to `tokens:480`) each had a member on all three sites.
- m3-ci's installation token: `POST /api/v3/app/installations/7190001/access_tokens` with the App's JWT (`ghs({})` in `tests/am1/hosted/lib.mjs`) answered **201**, a `ghs_` token, permissions `contents: write, metadata: read`, through `graft-m3d-edge` → `OPS` → `graft-m3d-ops`.
- `m3-member` acquired and released a Shared lease on `m3org/j4` through the edge (201, 204). `repo:65536` had a leader on site 0 and followers on 1 and 2 (term 2), and a follower's `/feed` answered `hold` with a cursor.
- `m3-admin` cloned `m3org/j4` and pushed a branch through the git cache shard on the image above; `m3-member` cloned it after. That started shard `git#0#net:65536`, which `INSTALL=graft-m3d tests/fanout/teardown.py --all-nets --pools` tore down (200). No shard container is left running.
- `tokens:0`'s members reported applied 9 (site 0) and 8 (sites 1 and 2) a few minutes after the `PutVerifyingKey` commit at index 9, with no leader: an idle group has none until a commit wakes it.

## Teardown

Run these in order. Each step is safe to repeat.

1. Delete the scripts `graft-m3d-edge`, `graft-m3d-tools-edge`, `graft-m3d-ops`, `graft-m3d-site-a`, `graft-m3d-site-b`, and `graft-m3d-site-c` (`npx --prefix tests/m0 wrangler delete --name <script>`). Delete the edge before `graft-m3d-ops`, which it binds. Deleting a site script deletes its Durable Objects and their storage.
2. Delete the container applications `graft-m3d-site-{a,b,c}-gitshard` (`wrangler containers list`, then `wrangler containers delete <id>`).
3. Empty and delete the buckets `graft-m3d-site-{a,b,c}-packs` (`wrangler r2 bucket delete <name>` after removing their objects).
4. Delete the queues `graft-m3d-prsync-{a,b,c}` (`wrangler queues delete <name>`).
5. Delete the secret `graft-m3d-mint-wrap-key` (id `20a3c4ce…`) from store `<store>`. Do not delete the store: it holds other installs' secrets.
6. Remove `~/.graft-m3d` and every staged copy of it. It holds the install's private keys and tokens.

The git shard image lives in `graft-m1`'s registry repository. Do not delete it.
