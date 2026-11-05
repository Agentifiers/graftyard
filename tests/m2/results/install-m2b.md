# The `graft-m2b` test install

A second test install, so hosted test work stops queuing behind `graft-m1`. It is built the way `tests/m1/integ/throwaway.sh` builds a fresh install, with the test-faults flags of `tests/m2/results/m2-wp1.md` and `tests/m1/results/m1-wp8.md`. Nothing here touches `graft-m1-site-*`, `graft-m1-edge`, or `graft-m1-lin-*`. Built 2026-11-05.

## Resources

All resources are in account `<cf-account-id>`. The work directory is `~/.graft-m2b` on the orb, and it holds the install's private keys and tokens.

| Resource | Name or id | Made by |
| --- | --- | --- |
| Install | `01M6MNQWC88HGQ669NCJTJ4JA2`, zone `git.m2b.example`, not an evaluation install | `graftyard init` |
| Site 0 script | `graft-m2b-site-a`, `enam` | `graftyard sites add`, then `workers/site/deploy.mjs --test-faults --anchor --image` |
| Site 1 script | `graft-m2b-site-b`, `weur` | the same |
| Site 2 script | `graft-m2b-site-c`, `apac` | the same |
| R2 buckets | `graft-m2b-site-a-packs` (ENAM), `graft-m2b-site-b-packs` (WEUR), `graft-m2b-site-c-packs` (APAC) | `sites add` |
| Container apps | `graft-m2b-site-a-gitshard`, `graft-m2b-site-b-gitshard`, `graft-m2b-site-c-gitshard` | `sites add` |
| Git shard image | `registry.cloudflare.com/<cf-account-id>/graft-m1-wp6-bed-gitshard-git@sha256:b7bbc1f6644c428a7f27495fe676fa3d6772b02c73030ffcab3039b56e46b43e`, the image the M1 test install runs; reused, not rebuilt | |
| Cron Trigger | `*/15 * * * *` on each site script, for the pack sweeper and the fold audit | `sites add` |
| Secrets Store | `graft-ops-01m6mnqwc88hgq669ncjtj4ja2` (id `bfaa0c1573de4775b1b92a12fb89acb4`), holding `graftyard-drain-0` and `graftyard-executor-0` | `graftyard genesis` |
| Genesis | digest `b21e46c4ddaebca2b9b5570f22bcef3489e251bb4e5c8905f047f9e977465b8f`, committed at `Release` index 3. Every system chain is `Tri` on sites 0, 1, and 2, with home 0 | `genesis`, `sign`, `install --genesis` |
| Git edge | `graft-m2b-edge`, `https://graft-m2b-edge.<workers-subdomain>.workers.dev`, a test-faults build with the install's anchor and secret `BED_TOKEN` (`~/.graft-m2b/bed.token`) | `tests/m1/integ/deploy-edge.mjs --test-faults --anchor` |
| Raw `/member` test edge | `graft-m2b-tools-edge`, `https://graft-m2b-tools-edge.<workers-subdomain>.workers.dev`, secret `TOKEN` (the M0 token, `tests/m0/.m0-token`) | `tests/m1/wp7/deploy-edge.mjs` with `SITES`, `GENESIS`, and `EDGES` |
| Seed | owner `m2b` 7000001, writer 7000002, reader 7000003, revoked 7000004; repository `bf20` = `repo:65536`, `smoke` = `repo:65537` | `tests/m1/integ/seed.ts --login m2b --repos bf20,smoke --out ~/.graft-m2b` |
| BF-20 tier | `repo:65536` seeded to 1.33 M metadata rows, 3.99 GB on each member | `tests/m2/bf20/seed.py` |

Measured member round trips at `sites add`: 2 to 0 230 ms, 2 to 1 157 ms, 0 to 1 133 ms, 0 to 2 147 ms.

**Deviation.** The buckets are named `graft-m2b-site-{a,b,c}-packs`, not `graft-m2b-packs-{a,b,c}`. `graftyard sites add` names each site's bucket `<script>-packs`, and the install was built with the CLI unchanged.

Files in `~/.graft-m2b`: `install.toml`, `customer.key` and `release.key` with their `.pub` files, `genesis.unsigned`, `genesis.signed`, `genesis-records.json` (the placements the tools edge reads), `sites.json`, `tokens.txt`, `repos.json`, and `bed.token`.

## Point the drivers at it

Each driver keeps its `graft-m1` default and reads the install from the environment (commit `bc976f6`):

```sh
export GRAFT_TOOLS_EDGE=https://graft-m2b-tools-edge.<workers-subdomain>.workers.dev   # seed.ts, observe.ts, tests/m2/bf20/*
export GRAFT_EDGE=https://graft-m2b-edge.<workers-subdomain>.workers.dev               # tests/m2/bf20/push.sh
export BF20_HOME=$HOME/.graft-m2b   # tokens.txt and repos.json
export BF20_DIR=/tmp/m2b/bf20       # the drivers' work directory and logs
export BED_TOKEN_FILE=$HOME/.graft-m2b/bed.token
export REPO=65536                   # the BF-20 repository; CH=repo:65536 for m.sh
export WRITER=7000002
```

- `node tests/m1/wp8/observe.ts --stats FILE --chains repo:65537` reads each member's stats through `GRAFT_TOOLS_EDGE`.
- `tests/m1/integ/seed.ts` seeds through `GRAFT_TOOLS_EDGE`.
- `tests/m1/wp7/deploy-edge.mjs` redeploys the tools edge with `SITES=~/.graft-m2b/sites.json GENESIS=~/.graft-m2b/genesis-records.json EDGES=graft-m2b-tools-edge`.
- `tests/m2/bf20/bf20c.sh` and `bf20d.sh` run BF-20 with the variables above.
- Redeploy the sites with `node workers/site/deploy.mjs ~/.graft-m2b/sites.json --test-faults --anchor ~/.graft-m2b/install.toml --image <the image above>`. Every new version resets the members.

## Proof it works

- `git push https://x:<writer>@graft-m2b-edge.<workers-subdomain>.workers.dev/m2b/smoke.git HEAD:refs/heads/main` succeeded, and `git ls-remote` through the same edge listed `03cc126d…` on `refs/heads/main`.
- `observe.ts --stats` with `GRAFT_TOOLS_EDGE` set read every member of `repo:65537` and `ids`. The `repo:65537` leader is site 0, with commit 4,266 ms for the first push, and its appends to sites 1 and 2 ended `ok`.
- BF-20's drivers ran there at 300 commands per second (`tests/m2/results/f47.md`).

The first push failed with `no such table: c_counter`, because a fresh member's `tier::reshape` did not create a table a release adds. Commit `3abfca5` cherry-picks the fix from `m2-wp5` (`5f43a65`).

## Teardown

Run these in order. Each step is safe to repeat.

1. Delete the scripts `graft-m2b-edge`, `graft-m2b-tools-edge`, `graft-m2b-site-a`, `graft-m2b-site-b`, and `graft-m2b-site-c` (`npx --prefix tests/m0 wrangler delete --name <script>`). Deleting a site script deletes its Durable Objects and their storage.
2. Delete the container applications `graft-m2b-site-{a,b,c}-gitshard` (`wrangler containers list`, then `wrangler containers delete <id>`).
3. Empty and delete the buckets `graft-m2b-site-{a,b,c}-packs` (`wrangler r2 bucket delete <name>` after removing their objects).
4. Delete the Secrets Store `bfaa0c1573de4775b1b92a12fb89acb4` (`wrangler secrets-store store delete bfaa0c1573de4775b1b92a12fb89acb4 --remote`).
5. Remove `~/.graft-m2b`. It holds the install's private keys and tokens.

The git shard image belongs to the M1 test install. Do not delete it.
