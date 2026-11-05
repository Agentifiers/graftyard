# Site-day fixes: shard images and the 401 window

Run 2026-11-05 on the second test install `graft-m2b-*` (`tests/m2/results/install-m2b.md`), branch `site-day-fixes` from `main` at `d4025be`. `graft-m1-*` was not touched. Raw data is in `raw/sdf/` (gitignored).

Two reports came out of the `graft-m1` site day in `tests/m2/results/m2-edge.md`:

1. After a site day that changed the git image, `/archive` answered Go's "404 page not found" until the test-only `/_bed/kill` tore the shard's container down.
2. One client run, `final3/`, got 401 "Bad credentials" on every request 7 minutes after the site day.

## 1. Shards keep their old image after a site day

**Cause.** A new site-script version restarts no running container. Cloudflare's rollouts do not cover containers under the `durable_object` policy: "Durable Object-managed Container instances do not participate in application-wide rollouts; application code selects their image when it calls `ctx.container.start()`" (https://developers.cloudflare.com/containers/configuration/rollouts/). So `--containers-rollout=immediate` cannot help. `ShardHost::start_once` starts a container only when none is running, so a warm shard keeps its startup image until its container stops. Its alarm renews the container's timeout until 24 h after the last request. This is the design's intended behaviour. DESIGN, "Git cache shards (derived)", says "A new version restarts no running container [measured, `tests/m0/results/container-survival.json`], so a shard changes image only when its site's `ContainerDay` step covers it". `ContainerDay` (M5-WP4) is not built, so test installs had no way to move warm shards.

**First fix, reverted.** `46b52bf` made every shard restart a running container whose recorded image differed from the version's `images.git`, on its next request or alarm. It worked on `graft-m2b`: after a site day from `5be873a0` to `aa3038ea`, all six warm shards reported `startedImage` `aa3038ea` and served `/archive` (`raw/sdf/shards-fix2.txt`). `6288a7d` reverts it, because it skips what the design's `ContainerDay` provides: the 10%, 50%, and 100% steps, the conformance gate on each step, and `Rollback { target: Containers(site) }` to `git_prev` ("Failure modes", git caches, bad-deploy).

**Fix.** `workers/site/deploy.mjs --roll-shards <edge url>` (`319a4cb`) is for test installs only. After the deploys, it lists each site's `<script>-gitshard` instances through the Containers API (`GET .../containers/applications/<id>/instances`, which reports each instance's object name and image). For every running instance whose image is not `--image`, it calls `/_bed/kill` then `/_bed/catchup` through the test edge. The product request path is unchanged. `tests/m2/sitedays/instances.sh` lists the instances and their images.

**Evidence.** Two site days ran with `--roll-shards` and main's site build. Before them, six shard containers (`smoke` and `bf24` on sites 0, 1, and 2) were warm. `shards.sh` pushes fresh branches and asks each site's shard for an uncached archive.

| Site day | Image before | Image after (`instances.sh`) | Rolled | `/archive` per shard |
| --- | --- | --- | --- | --- |
| `roll1`, 20:27Z | `aa3038ea` on all six | `5be873a0` on all six, started 20:28 to 20:29Z | 6, each kill 200 and catchup 200 | 503 "git cache archive: 404" on all six, which is correct for an image without `/archive` |
| `roll2`, 20:31Z | `5be873a0` on all six | `aa3038ea` on all six, started 20:32 to 20:33Z | 6, each kill 200 and catchup 200 | 200 on all six |

Files: `deploy-site-roll{1,2}.log`, `instances-before-roll.txt`, `instances-after-roll{1,2}.txt`, `shards-roll{1,2}.txt`.

**The old code on `graft-m2b`, without a roll.** On the first old-code site day (18:08Z, `5be873a0` to `aa3038ea`), the warm `smoke` and `bf24` shards on site 0 served `/archive` 200 right after the deploy (`status-after-base.txt`, `shards-A.txt`). So their containers had been replaced during that site day. Why is not known [unverified]. It contradicts `container-survival.json`, and it did not happen again. On the next old-code site day (18:47Z, `aa3038ea` to `5be873a0`), all six warm shards kept `aa3038ea` and still served `/archive` 200 (`shards-base-back.txt`, `shards-base-C1.txt`). The deploy changed their image, so this is the reported bug, in its harmless direction. The `fix1` day (19:07Z) deployed `5be873a0` over containers that had started on it, and every shard answered 503 "git cache archive: 404", as that image should (`shards-fix1.txt`).

## 2. The 401 window (F49)

**Cause.** The client sent no token. In `raw/m2-edge/final3/` (on the `edge-wiring` worktree), all 24 requests carry an empty `Authorization:` header. `final`, `final2`, and `final4` carry `Authorization: token …` on all 41, 13, and 41 requests. The git credential helper in `final3`'s work directory (`/tmp/tmp.HVr4kGHaIz/home/.gitconfig`) has an empty password, where `final2`'s and `final4`'s have 40 characters. `edge-drive.sh` reads the token with `sed -n "${LINE}p"` and did not check it, so an empty read ran the whole journey anonymously. The edge's 401 "Bad credentials" was correct for a request with no credential. Why the read came back empty that once is not recorded [unverified].

Reproduced on `graft-m2b` with the old driver and a token line that does not exist (`raw/sdf/old-driver-empty-token/`): 10 requests with an empty `Authorization`, 7 replies 401, every command rc 1. That is the `final3` signature.

**Fix.** `edge-drive.sh` exits 2 with "no token on line N of FILE" when the token is empty (`ee08088`). The same call that reproduced `final3` on the old driver now stops before any request.

**No server 401 during or after a site day.** `tests/m2/sitedays/poll-auth.sh` polled `graft-m2b-edge` once a second with the writer's token, on GraphQL and on git `info/refs?service=git-receive-pack`. Every tenth second it also sent a well-formed token that no account holds. It ran through seven site days: `base` at 18:08Z, `B1` at 18:47Z, `C1` at 18:57Z, `fix1` at 19:07Z, `fix2` at 19:12Z, `roll1` at 20:27Z, and `roll2` at 20:31Z. Each site day was a new version of all three site scripts, deployed twice, which resets every member.

| Probe | Requests | Status |
| --- | --- | --- |
| valid token, GraphQL | 4,375 | 4,374 × 200, 1 client timeout at 60 s (20:37:21Z) |
| valid token, git `info/refs` | 4,375 | 4,374 × 200, 1 client timeout at 60 s (18:18:39Z) |
| unknown token, GraphQL | 439 | 439 × 401 |

So 0 of 8,750 valid-token requests got 401 (`poll-base.log`, `poll-fixed.log`, `poll-roll.log`).

**F49's answer.** Yes, a credential read that cannot get an answer must not be 401, and the code already does that. A member answers a read only after `Host::new` has run `restore` synchronously in its constructor. A member whose restore failed answers 500 (`ready_or`), and `call_opt` counts that as an answer that is not "no group", retries for `LOOKUP_FOR`, then returns `Err`. `authenticate` turns `Err` into an error. `api::serve` renders that as 503 with `Retry-After: 5` for REST, GraphQL, and LFS. The git smart HTTP path renders it as 500 (`fetch`, the last `match r`), which is still not 401 but is not retryable either. That is a small follow-up. `call_opt` returns `Ok(None)` ("no group", read as no row) only when every bootstrap member answers 409 "not joined" on the first pass. A joined member never answers that after a reset, because its stored join rebuilds its host. A restored `Tokens` fold that disagrees with its log fails the restore check (`RESTORE_CHECK_MAX`) and answers 500, not an empty row [inference from `workers/site/src/host.rs` and `crates/lookup/src/lib.rs`, consistent with the 8,750 probes]. No code change was made, so no test fails on old server code. The test that fails on the old code is the driver check above. F49 can close with this evidence.

## Deployed on `graft-m2b`

- `graft-m2b-site-a/b/c`: main's `test-faults` site build (`d4025be`, wasm sha256 `fe5581bc…`), versions `adca5173`, `92f2eda8`, and `31229766` (`roll2`), with image `graft-m1-site-gitshard-git@sha256:aa3038ea61c5637d6cd499100363864f16c389a0da2754be7c4433bcdf3e751d`. All six warm shards run it.
- `graft-m2b-edge`: this branch's `test-faults` edge build, version `4bfeb94d`. Its product source is identical to `main`'s. It has `FORMATS_ENABLED=32.1,32.5` and `LOGINS` for `m2b`, `m2b-writer`, and `m2b-reader`. Before this run it was `115bfa6a`, an edge without the M2 wiring, so `api/graphql` answered 404.

## Reproduce

```
export BED_TOKEN_FILE=~/.graft-m2b/bed.token
tests/m2/sitedays/poll-auth.sh https://graft-m2b-edge.<workers-subdomain>.workers.dev m2b/smoke ~/.graft-m2b/tokens.txt 2 1500 > poll.log &
node workers/site/deploy.mjs ~/.graft-m2b/sites.json --test-faults --anchor ~/.graft-m2b/install.toml \
  --image <digest> --roll-shards https://graft-m2b-edge.<workers-subdomain>.workers.dev
tests/m2/sitedays/instances.sh ~/.graft-m2b/sites.json
tests/m2/sitedays/shards.sh https://graft-m2b-edge.<workers-subdomain>.workers.dev m2b ~/.graft-m2b/tokens.txt smoke:65537 bf24:65538
```
