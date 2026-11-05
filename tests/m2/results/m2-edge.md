# M2 edge wiring: M2-WP2's and M2-WP4's handlers on `graft-m1-edge`

Run on 2026-11-05 against the test install. The edge `graft-m1-edge` ran version `1c3e665a` (test-faults build of branch `edge-wiring`). The sites `graft-m1-site-a/b/c` ran what the M2-WP5 work had deployed there. This run did not deploy any site script. The repository was `wp6/edge1` (repository 196630, owner account 7000001), seeded for this run with `tests/m1/integ/seed.ts`. Its `main` and `f` branches were pushed through this edge. The clients used the install's owner test token. No GitHub credential was used, and no request went to github.com.

## Summary

| Area | Result |
| --- | --- |
| Edge-only surfaces (`/status`, `/api/v3/`, `/meta`, `/api/v3/status`, `/rate_limit`, GraphQL `__type` probes, admission rules, token checks) | **Served.** Every reply validates against the pinned GHES 3.22 schemas |
| LFS transfer `PUT` and `GET` to two buckets | **Served.** A 200,000-byte object went through `BlobStore::place`. All three site buckets hold `blob/196630/<oid>` with equal bytes, and `GET` reads it back |
| git smart HTTP after the routing change | **Unchanged.** `git ls-remote`, `git clone`, and `git push` through the edge succeed |
| `gh issue`, `gh pr`, `gh release`, git-lfs batch and locks | **Blocked by the deployed site.** Each command gets the edge's 503 "the repository's members do not serve tier rows yet (site release)". The deployed members predate the `rows` read and the issue command (see "What the site deploy needs") |
| Issue, release, and LFS writes | **Gated off.** `FORMATS_ENABLED` is `32.1`, so the edge sends no 32.4 command and answers 503 (see "FormatsEnable") |

Only the edge-only rows in the table could pass on the current site. Every repository-scoped read and write needs the site release this branch commits.

## What is wired

The edge routes each request through `graftyard_wire::Host::endpoint`. `Rest`, `Graphql`, `Uploads`, `Lfs`, `Objects`, `Codeload`, and `Status` go to `workers/edge/src/api.rs`. `Repo` paths still go to git smart HTTP. Each reply gets its cross-cutting headers from `graftyard_wire::finish`.

- **Authentication.** `authenticate` reads the token's row through `fresh` on its `Tokens` shard, as git does.
- **Reads.** The edge sends one `{"q": {"rows": true}}` read to the repository's leader through `graftyard_lookup::call_opt`. The read returns both tiers at one index (`graftyard_api::hosted::TierRows`). The edge loads them into `MemStore`s and calls `crates/api`'s handlers through `view::Repo`. A repository with more than `ROWS_MAX` (50,000 [guess]) rows answers 503. A paged read would mix indexes, and `Repo` reads one index. This is a scale gap for large repositories.
- **Directory.** `hosted::HostedDir` holds:
  - the owner's `Authz` fold, read as a `dep` through `fresh`, for permissions and `author_association`;
  - the owner's login, from the path;
  - the viewer, from the token.

  `Names` has no account-to-login row, so other logins render as `ghost`. Test installs can name them in `LOGINS`.
- **Writes.** An issue, release, or LFS command crosses the binding as `IssueCall`, record format 32.4 (`crates/git/src/issue_ops/call.rs`). It carries a `Render` request. The leader renders the client's reply from the command's `&Acked` over its own tiers (`hosted::render_ok` in `Family::out_json`). The edge only frames it, so no 2xx exists before the quorum. The edge commits with `commit_hedged`, M1's write path.
- **Ref delete.** `DELETE /git/refs/{ref}` commits a `Push`.
- **Blobs.**
  - Upload and LFS transfer bytes go through `BlobStore::place` to the placement's buckets.
  - The asset upload and LFS `verify` send the `BlobRef`. Before the member admits the command, it rebuilds the `DurableBlob` with `BlobStore::confirm` (`Family::admit`). `decide` refuses `Unavailable` unless two buckets in two regions hold the bytes. So `verify`'s 200 and the upload's 201 render only after that check and the quorum (BF-25).
  - After `Acked`, the leader calls `mark_blob` (`Family::after_ok`).
- **Codeload.** `graftyard_git::blob::archive` reads from the warm site's `ArchiveCache`. On a miss it runs `git archive` on the git cache through a new shard and container route, `/archive`.
- **Not wired.** `mergePullRequest` (M2-WP3), fork heads (`Directory::ref_of`), the git cache's compare counts, and `raw-content`.

## Real clients on Cloudflare

Each command ran once with `tests/m2/edge-drive.sh`, gh 2.102.1 (`/usr/bin/gh`), git 2.56.0, and git-lfs 3.8.0. `GH_DEBUG=api` logged every request. The logs, with tokens redacted, are in `raw/m2-edge/clients/`, and `raw/m2-edge/requests.txt` lists each request and its status.

| # | Command | Exit | What happened |
| --- | --- | --- | --- |
| 01 | `gh issue list -R wp6/edge1` | 1 | `IssueList` GraphQL: 503 (no tier rows) |
| 02 | `gh issue create -t .. -b ..` | 1 | `/meta` 200, `Issue_fields` `__type` probe 200, `IssueRepositoryInfo`: 503 |
| 03 | `gh issue view 1` | 1 | `/meta` 200, probe 200, `IssueByNumber`: 503 |
| 04 | `gh issue comment 1 -b ..` | 1 | `IssueByNumber`: 503 |
| 05 | `gh issue view 1 --comments` | 1 | `/meta` 200, probe 200, `IssueByNumber`: 503 |
| 06 | `gh pr list` | 1 | `PullRequestList`: 503 |
| 07 | `gh pr create -H f -B main --draft` | 1 | `RepositoryInfo`: 503 |
| 08 | `gh pr view 1` | 1 | `/meta` 200, `PullRequestByNumber`: 503 |
| 09 | `gh pr checks 1` | 1 | `PullRequestByNumber`: 503 |
| 10 | `git lfs install` | 0 | Local |
| 11 | `git clone https://graft-m1-edge.../wp6/edge1.git` | 0 | git smart HTTP through the new routing |
| 12 | `git lfs track '*.bin'` | 0 | Local |
| 13 | `git push origin HEAD:refs/heads/lfs-edge` (one 300,000-byte LFS object) | 1 | `locks/verify` and `objects/batch` 503, so git-lfs's pre-push hook stopped the push |
| 14 | `git clone -b lfs-edge` | 128 | The branch was never pushed (13) |
| 15 | `git lfs pull` | 1 | No clone (14) |
| 16 | `git lfs locks` | 2 | `GET locks`: 503 |
| 17 | `gh release create v-edge1 --notes x --target main` | 1 | `POST /repos/wp6/edge1/releases`: 503 |
| 18 | `gh release upload v-edge1 app.tgz` | 1 | `RepositoryReleaseByTag`: 503 |
| 19 | `gh release view v-edge1` | 1 | `RepositoryReleaseByTag`: 503 |
| 20 | `gh release download v-edge1` | 1 | `RepositoryReleaseByTag`: 503 |
| 21 | `gh release download v-edge1 --archive=tar.gz` | 1 | `GET /releases/tags/v-edge1` and GraphQL: 503 |

Every 503 above has the same cause. The deployed members answer `{"q": {"rows": true, "ranges": true}}` with their `ranges` reply, because they predate the `rows` read. The edge then refuses the request rather than serve an empty repository. An earlier edge build sent `{"q": {"rows": true}}` alone. An old member answers that with every ref of the repository. On `wp6/main` that read took up to 8.2 s, so the edge now also asks for `ranges`.

The LFS transfer itself was driven with curl, because git-lfs reaches the transfer only through a batch reply (`raw/m2-edge/probes.txt`, `raw/m2-edge/lfs-transfer-buckets.txt`):

| Request | Status | Check |
| --- | --- | --- |
| `PUT /_lfs/196630/<oid>`, 200,000 bytes, owner token | 200 in 2.7 s | `wrangler r2 object get --remote` on `graft-m1-packs-a`, `-b`, and `-c`: all three hold `blob/196630/<oid>`, bytes equal |
| The same `PUT` again | 200 | Create-only put of the same bytes |
| `PUT` with other bytes under the oid | 422 | `BlobError::Mismatch`, nothing put |
| `PUT` with no credential | 401 | |
| `GET /_lfs/196630/<oid>` | 200 | Bytes equal the `PUT` body |
| `GET /_lfs/196630/<unknown oid>` | 404 | |

No `LfsObject` entry was committed, because `verify` is gated (see "FormatsEnable"). So nothing called `mark_blob` on Cloudflare, and the blob has no `Committed` marker.

Edge-only replies (`raw/m2-edge/probes.txt`):

| Request | Status |
| --- | --- |
| `GET /status`, `/api/v3/`, `/api/v3/meta`, `/api/v3/status` | 200 |
| `GET /api/v3/rate_limit` | 404 "Rate limiting is not enabled.", as GHES answers by default |
| `GET /api/v3/meta` with no `User-Agent` | 403 |
| `GET /api/v3/meta` with `X-GitHub-Api-Version: 1999-01-01` | 400 |
| `POST /api/graphql` `__type` probe, owner token | 200 |
| `POST /api/graphql`, revoked token | 401 |
| LFS `objects/batch` with no credential | 401 with the Basic challenge |
| `GET /api/v3/repos/wp6/nope/releases/tags/v1` | 404 |

## Schema validation of hosted replies

`crates/api/gh/validate.py` checked every 200 reply the edge served in the final run, against the pinned `ghes-3.22.json` and `schema.docs-enterprise.graphql`:

- the three `Issue_fields` `__type` replies gh received;
- the four `/meta` replies gh received;
- one more of each taken with curl, plus `/api/v3/` and `/rate_limit`.

```
4 GraphQL replies, 7 REST replies, and 0 LFS replies checked, 0 failed
```

The replies are in `raw/m2-edge/replies/` and the output is in `raw/m2-edge/validate.txt`.

The oracle catches a bad reply. With one `/meta` body changed to an integer `installed_version`, it reported `FAIL ... 3 is not of type 'string'`. No repository-scoped reply could be validated, because none was served.

Those handlers' replies are validated offline: 93 replies in M2-WP4 and the M2-WP2 set (`crates/api/gh/README.md`). This branch adds `a_hosted_mutation_renders_on_the_member_as_in_process` (`crates/api/tests/surfaces.rs`). In that test:

- the tiers cross as `TierRows` JSON;
- the edge plans the mutation;
- the render request crosses as JSON;
- the member renders from `&Acked`.

The reply equals the in-process render.

## FormatsEnable

Event tags 17 to 20 need `FormatsEnable` before any member writes them, and so does the command that writes them.

- SHARED, "Record formats and the two-release rule", covers "entry bodies, event kinds, ... commands and read requests from the edge". Rule 2 says "Release N+1 starts writing it only after the format is recorded as enabled".
- DESIGN, Shape: "A caller sends a new command, message kind, or field only after `FormatsEnable` has enabled its format".

Only an issue command writes the four events. So the format to enable is the issue command's, 32.4 (`REPO_FORMAT_ISSUES`). An entry that carries one is tagged 32.4, and a follower whose release does not read 32.4 refuses that entry.

What the test install does:

- **Rule 1.** The site release on this branch reads 32.1 and 32.4 (`Family::reads`).
- **Rule 2.** The edge sends an `IssueCall` only when `FORMATS_ENABLED` contains `32.4`. `FORMATS_ENABLED` is the edge's existing stand-in for the `Release` view's enabled set. Without 32.4, every issue, release, and LFS write answers 503 "issue writes need record format 32.4, which FormatsEnable has not enabled on this install". `FORMATS_ENABLED` is `32.1` on `graft-m1-edge` now.
- **Rule 3.** The test install has no `Release` group, and `FormatsEnable` is not built. So enabling 32.4 means this:
  1. Every site runs a release that reads 32.4.
  2. The operator then redeploys the edge with `--var FORMATS_ENABLED=32.1,32.4`. That is an edge change only, not a site day.
- **Rule 4.** No swap-back guard exists on the test install.

## What the site deploy needs

This branch changes `workers/site` and `containers/git/server`. None of it is deployed. The `graft-m1-site-*` scripts belong to the M2-WP5 work, and the parent merges both branches.

1. **Merge with M2-WP5 first.** M2-WP5 made the Repo family's command `RepoCmd { Push, Sync }` in format 32.3. This branch makes it `RepoCall { Push, Issue }` in format 32.4. The merged command needs all three variants, and `reads()` needs 32.1, 32.3, and 32.4. `out_json`, `err_json`, `after_ok`, and `unreadable` each need the third arm.
2. **Deploy all three site scripts** from the merged branch: the `rows` read, issue commands, `Family::admit`'s `BlobStore::confirm`, `mark_blob` after `Acked`, and the shard's `/archive` route.
3. **Rebuild and push the git container image.** It gains `/archive`, which codeload needs. The archive route was not run on Cloudflare.
4. **Then redeploy `graft-m1-edge`** with `--var FORMATS_ENABLED=32.1,32.4`. Keep `--var LOGINS=...` if replies should name the test writer and reader.
5. Run `tests/m2/edge-drive.sh <out> wp6/edge1 1 issue pr lfs release` again and validate the replies.

## Commits

On `edge-wiring`, from `005bc62`:

- `fd32823` wire: refuse install hosts by whole label, so notgithub.com is allowed.
- `081f694` git: key ArchiveCache by commit and prefix digest, as DESIGN's R2Key does.
- `144a869` git, api: an issue command that crosses the binding and renders on its leader.
- `18c332b` site: repository members commit issue commands and serve tier rows and archives.
- `b0212d4` edge: route REST, GraphQL, LFS, uploads, and codeload to crates/api.
- The commit holding this file, the product-build grep change, and the edge read fixes found on Cloudflare.
