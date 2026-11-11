# The `graft-m3c` test install

LF-13's own install, so the row stops queuing behind the 500-agent load rows on `graft-m2b`. It is built the way `tests/m2/results/install-m2b.md` built `graft-m2b` and `tests/m1/integ/throwaway.sh` builds a fresh install: `graftyard init`, `sites add` three times with a test-faults bundle, `genesis`, `sign`, an edge with the install's anchor, and `install --genesis`. The operator approved it. Nothing here touches `graft-m1-*` or `graft-m2b-*`. Built 2026-11-11 from 01:11Z (branch `lf13-m3c`, from `422bc02`).

## Resources

All resources are in account `<cf-account-id>`. The work directory is `~/.graft-m3c` on the orb (mode 700, files 0600), and it holds the install's private keys and tokens.

| Resource | Name or id | Made by |
| --- | --- | --- |
| Install | `01M742HEA2SBCYP1VW26624KMW`, zone `git.m3c.example`, not an evaluation install | `graftyard init` |
| Site 0 script | `graft-m3c-site-a`, `enam` | `graftyard sites add`, then `workers/site/deploy.mjs --test-faults --anchor --image` |
| Site 1 script | `graft-m3c-site-b`, `weur` | the same |
| Site 2 script | `graft-m3c-site-c`, `apac` | the same |
| R2 buckets | `graft-m3c-site-a-packs` (ENAM), `graft-m3c-site-b-packs` (WEUR), `graft-m3c-site-c-packs` (APAC) | `sites add` |
| Container apps | `graft-m3c-site-a-gitshard` (`88b9ff85…`), `graft-m3c-site-b-gitshard` (`e63f087f…`), `graft-m3c-site-c-gitshard` (`76c68b90…`) | `sites add` |
| Git shard image | `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9`, the image `graft-m1` runs; reused, not rebuilt (`containers/` has no commit since `e46da45`) | |
| Cron Trigger | `*/15 * * * *` on each site script | `sites add` |
| Queues | `graft-m3c-prsync-a`, `graft-m3c-prsync-b`, `graft-m3c-prsync-c`. `sites add` created none, so the day-1 site scripts had no `PRSYNC` binding; site day 2 (`workers/site/deploy.mjs`) created them | `workers/site/deploy.mjs` |
| Secrets Store | none. `graftyard genesis` failed with `maximum_stores_exceeded` (the account's one store is `graft-m2b`'s), so it ran with `--offline-keys`: the drain and executor keys were not stored | `graftyard genesis --offline-keys` |
| Genesis | digest `0ec9dc705015…`, committed at `Release` index 3. Every system chain is `Tri` on sites 0, 1, and 2, with home 0. All 3,072 `Tokens` shard groups created (the first `install` hit its 30-min `timeout` in the precreation; the rerun answered "already installed" and finished it) | `genesis`, `sign`, `install --genesis` |
| Git edge | `graft-m3c-edge`, `https://graft-m3c-edge.<workers-subdomain>.workers.dev`, a test-faults build with the install's anchor, `FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3`, no `OPS` binding, no `LOGINS`; secret `BED_TOKEN` (`~/.graft-m3c/bed.token`) | `tests/m1/integ/deploy-edge.mjs --test-faults --anchor --var FORMATS_ENABLED=…` |
| Raw `/member` test edge | `graft-m3c-tools-edge`, `https://graft-m3c-tools-edge.<workers-subdomain>.workers.dev`, secret `TOKEN` (`~/.graft-m3c/tools.token`) | `tests/m1/wp7/deploy-edge.mjs` with `SITES`, `GENESIS`, and `EDGES` |
| Seed | org `m3org` 7100001, `m3-member` 7100002, `m3-admin` 7100003, App `m3-ci` 131072 installed on `m3org` as 7190001, a PAT each for member and admin; repository `m3org/j4` = `repo:65536` | `tests/m3/hosted/setup.ts --out ~/.graft-m3c/m3` |
| LF-13 repositories | `m3org/lf13-g` = `repo:65537` (gap), `m3org/lf13-i` = `repo:65538` (install) | `tests/am1/runs/mkrepos.mjs lf13-g lf13-i` |

No `graft-m3c-ops`: LF-13's lease commits are `m3-member`'s Shared leases under its PAT, which need no task and so no installation token. Without a Secrets Store there is nowhere to put a `MintWrapKey` without touching `graft-m2b`'s store, so `POST /app/installations/{id}/access_tokens` on this install answers 5xx (no `OPS` binding). `tests/am1/rest/apps.mjs` and `app-perms.mjs` were not run, for the same reason.

The agent formats are enabled on the edge (32.8, 32.12, 32.14, 18.3); the site scripts read every format their build knows.

Measured member round trips at `sites add`: 0 to 1 95 ms, 0 to 2 235 ms, 2 to 0 222 ms, 2 to 1 160 ms.

Files in `~/.graft-m3c`: `install.toml`, `customer.key` and `release.key` with their `.pub` files, `genesis.unsigned`, `genesis.signed`, `genesis-records.json` (the placements the tools edge reads), `sites.json`, `bed.token`, `tools.token`, `t0` (the build's start, Unix seconds), and `m3/` (`m3.json`, `m3-tokens.txt`, `app.pem`, `app.secret`, `am1-runs-repos.txt`).

## Site versions

| Day | When | Site a / b / c | Build |
| --- | --- | --- | --- |
| 1 (`sites add`) | 01:19Z to 01:20Z | `39b7c668` / `9f05f217` / `9c8f33da` | `422bc02` plus `429eede` and `3597c6f` (then uncommitted), test-faults |
| 2 | 03:22Z to 03:29Z | `4592b9dd` / `92faf215` / `a4ef986c` | `7972504` (the ring's install skip), test-faults, `--anchor`, the image above. A first pass at 03:24Z made `aa42ac6d` / `24a4b785` / `2e2ac440` (phase 1, binding only its own classes) on the way. It created the queues `graft-m3c-prsync-{a,b,c}` |
| 3 | 03:44Z to 03:51Z | `987311dc` / `96b8d31e` / `6ed07eb5` | `606b403` (the install skip keeps the snapshot's leases), test-faults, `--anchor`, the image above; via phase 1 `d2aea902` / `2ce5e7ce` / `5a84d5e4`. LF-13's batch 3 ran on these |
| 4 | 04:11Z to 04:19Z | `13bd72fd` / `9418e14d` / `34c1fd4b` | `9d59283` (the ring restarts from the loaded tables in `finish_install`), test-faults, `--anchor`, the image above; via phase 1 `1beed8af` / `f62ec979` / `494ae79b`. The recorded LF-13 batch ran on these versions |

Edge `graft-m3c-edge`: `0543a640-ccee-403f-8a60-5e592c864a72` (the build), then `ba54c862-bbb8-4b85-a793-17b9e26955e7` (the `BED_TOKEN` secret). Tools edge: `17adeb1a-3e51-43ad-a2c3-cbf6f5a2521e`, then `a3b633da-604f-46d0-b542-005e63e1c691` (`TOKEN`). Neither changed after.

## Point the drivers at it

`source tests/am1/lf13/env-m3c.sh` sets `GRAFT_EDGE`, `GRAFT_TOOLS_EDGE`, `BED_TOKEN_FILE`, `M3_STATE=~/.graft-m3c/m3`, `INSTALL=graft-m3c` (for `tests/fanout/m2bcost.py` and `containers.py`), and `HELD=999`. `tests/m0/.m0-token` was a symlink to `~/.graft-m3c/tools.token` while seeding (it is git-ignored) and was removed after.

- `LF13_ENV=tests/am1/lf13/env-m3c.sh bash tests/fanout/lf13.sh` runs LF-13, both modes, 3 runs each.
- Redeploy the sites with `node workers/site/deploy.mjs ~/.graft-m3c/sites.json --test-faults --anchor ~/.graft-m3c/install.toml --image <the image above>`. Every new version resets the members: a site day.
- Cost: `INSTALL=graft-m3c python3 tests/fanout/m2bcost.py <from> <to>`, from `~/.graft-m3c/t0`.

## Proof it works

- `install` answered "created all 3072 Tokens shard groups"; `tokens:0` to `tokens:2600` each had a leader or follower on site 0.
- `m3-member` acquired and released a Shared lease on `m3org/lf13-g` through the edge (201, 204), and a follower's `/feed` answered `hold` with a cursor.
- `/sever {"for_ms": 3000}` on a follower answered `{now, until}`, and `{"for_ms": 0}` lifted it (`until: 0`).
- LF-13 ran 12 hosted runs here (`tests/am1/results/lf13-m3c.md`).

## Teardown

Run these in order. Each step is safe to repeat.

1. Delete the scripts `graft-m3c-edge`, `graft-m3c-tools-edge`, `graft-m3c-site-a`, `graft-m3c-site-b`, and `graft-m3c-site-c` (`npx --prefix tests/m0 wrangler delete --name <script>`). Deleting a site script deletes its Durable Objects and their storage.
2. Delete the container applications `graft-m3c-site-{a,b,c}-gitshard` (`wrangler containers list`, then `wrangler containers delete <id>`).
3. Empty and delete the buckets `graft-m3c-site-{a,b,c}-packs` (`wrangler r2 bucket delete <name>` after removing their objects).
4. Delete the queues `graft-m3c-prsync-{a,b,c}` (`wrangler queues delete <name>`).
5. Remove `~/.graft-m3c`. It holds the install's private keys and tokens.

The git shard image belongs to the `graft-m1` install. Do not delete it. There is no Secrets Store to delete.
