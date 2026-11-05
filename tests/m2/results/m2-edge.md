# M2 edge wiring: M2-WP2's and M2-WP4's handlers on `graft-m1-edge`

These results are from a run on 2026-11-05 against the test install `graft-m1-site-a/b/c` (enam, weur, apac) and `graft-m1-edge`, on branch `edge-wiring` after it merged `main` at `c7a3b3b` (M2-WP5, F47, and M2-WP3).

- The repository was `wp6/edge1` (repository 196630, owner account 7000001).
- The clients used the install's owner test token. No GitHub credential was used, and no request went to github.com.
- The raw data is in `raw/m2-edge/`. It is gitignored and exists on disk only.

An earlier pass ran against the M2-WP5 site release, before the site day. Every repository-scoped command got a 503 then, because those members did not serve the `rows` read. That pass is summarized at the end.

## Summary

| Area | Result |
| --- | --- |
| `gh issue list/create/view/comment` | **Pass.** All 5 commands exit 0 |
| `gh pr list/create --draft/view` | **Pass.** All 3 commands exit 0 |
| `gh pr checks` | Exit 1, "no checks reported on the 'pr-e9' branch". That is gh's answer for a pull request with no checks, and every request got a 200 |
| git-lfs push, clone, pull, lock, locks, unlock | **Pass.** All exit 0, and the pulled bytes equal the pushed bytes |
| `gh release create/upload/view/download`, `download --archive=tar.gz` | **Pass.** All 5 commands exit 0. The asset round-trips byte for byte, and the archive comes from `git archive` on the git cache |
| LFS `verify` and asset upload after two buckets | **Pass.** Each commits after `BlobStore::confirm` on the member. Each blob has its `Committed` marker from `mark_blob` in all three buckets |
| Schema validation | **0 failures.** 26 GraphQL, 12 REST, and 5 LFS replies checked in the final run |
| BF-24 `PrSync` after the site day | **Pass.** One clean head push got its `TestMerged`, and `mergeable=true`, 6.3 s after the push on the leader's clock |

## What is wired

The edge routes each request through `graftyard_wire::Host::endpoint`.

- `Rest`, `Graphql`, `Uploads`, `Lfs`, `Objects`, `Codeload`, and `Status` go to `workers/edge/src/api.rs`.
- `Repo` paths still go to git smart HTTP.
- Each reply gets its cross-cutting headers from `graftyard_wire::finish`.

These parts are unchanged since the first pass:

- **Authentication.** `authenticate` reads the token's row through `fresh` on its `Tokens` shard.
- **Reads.** The edge sends one `{"q": {"rows": true}}` read for `TierRows` to the repository's leader. It loads the rows into `MemStore`s and calls `crates/api` through `view::Repo`.
- **Directory.** `hosted::HostedDir` carries:
  - the owner's `Authz` fold, read as a `dep`;
  - the owner's login;
  - the viewer.
- **Writes.** An `IssueCall` commits through `commit_hedged`. The leader renders the reply from `&Acked` (`hosted::render_ok` in `Family::out_json`).
- **Blobs.** The member rebuilds the `DurableBlob` with `BlobStore::confirm` before it admits the command (`Family::admit`). The leader calls `mark_blob` after `Acked` (`Family::after_ok`).
- **Codeload.** Codeload reads the `ArchiveCache`, or runs `git archive` on the shard's `/archive`.

### What changed in this pass

**One repository command.** M2-WP5's `RepoCmd { Push, Sync }` and this branch's `RepoCall { Push, Issue }` are now one `RepoCmd { Push, Sync, Issue }` in `crates/git/src/repo.rs`. `RepoOut` and `RepoErr` gained the matching variants.

- WP5's `Push` and `Sync` arms are unchanged.
- `Sync` keeps its `{"prSync": ..}` JSON.
- `Issue` is `{"issue": ..}`, and its format is checked before the rest of the body is read.

**`IssueCall` is format 32.5, not 32.4.** M2-WP3 took repository format 32.4 for the merge commands (`crate::merge::REPO_FORMAT_V4`). So `REPO_FORMAT_ISSUES` is now `0x20` version 5, and a test asserts the two differ.

- The Repo family reads 32.1, 32.3, and 32.5.
- No site member hosts `MergeCmd` yet, so the family does not read 32.4.

**Three bugs, found only on Cloudflare.** Each one is fixed in code, with a test where one can catch it.

1. **`eba06c7`: every hosted write failed with 503 "JSON serialization error".** `HostedDir` holds `AuthzFold`. Its `grants` map is keyed by `(RepoId, UserId)`, and serde_json cannot write a map with tuple keys. The offline test built `HostedDir` with no `Authz`, so it never hit this. The fold now crosses as hex of `graftyard_raft::codec`. The hosted-mutation test round-trips a `HostedDir` that has a grant, and it fails on the old code with "key must be a string".
2. **`6685801`: every LFS `verify` answered 404, and every asset upload 503.** `IssueCall::confirm` and `cmd` compared the edge's `BlobRef` to the member's confirmed one as whole structs, sites included.
   - The edge's `verify` names no sites.
   - An upload names only the two buckets `place` waited for, while `confirm` finds all three.

   So nothing ever matched, and `decide` refused `Unavailable`. A blob now matches by oid and size, and the sites the member found decide. The test `a_confirmed_blob_is_matched_by_its_bytes_not_its_sites` fails on the old code. The edge now logs an issue command's refusal cause, and the site logs a failed `confirm`. The first log is how the cause was found.
3. **`82f04c0`: `mark_blob` marked the edge's `BlobRef`.** For a `verify` that meant no buckets, and for an upload only two. `after_ok` now marks the member's confirmed `DurableBlob`, so every bucket that holds the bytes gets its marker. No unit test covers this, because `after_ok` needs worker R2 bindings. The marker checks below show the fix on Cloudflare.

**The git container serves both `/merge` (M2-WP5) and `/archive` (this branch).** The shard routes both.

## The site day

All of it is on `graft-m1-site-*` and `graft-m1-edge` only. `graft-m1-lin-*` and `graft-m2b-*` were not touched.

| Step | What | Result |
| --- | --- | --- |
| Image | `docker build containers/git`, then `wrangler containers push graft-m1-site-gitshard-git:edge` (the first push hit "blob unknown to registry" and was retried) | `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:aa3038ea61c5637d6cd499100363864f16c389a0da2754be7c4433bcdf3e751d`. Its `shard-server` has both `/merge` and `/archive` |
| Sites | `node workers/site/deploy.mjs tests/m1/wp1/sites.json --genesis tests/m1/wp7/genesis.json --test-faults --image <the digest above>`, 4 times as fixes landed (`raw/m2-edge/sd/deploy-site*.log`) | Last deploy, build of `82f04c0`: a `4079ddd2`, b `9a84c900`, c `25231068`. Site wasm sha256 `dbbd8b6b…2c` |
| Edge | `node tests/m1/integ/deploy-edge.mjs tests/m1/wp1/sites.json graft-m1-edge --test-faults --genesis tests/m1/wp7/genesis.json --var FORMATS_ENABLED=32.1,32.5 --var 'LOGINS={…}'`, 4 times | Last deploy: `06ed3ac3`, the build of `6685801`'s edge, with edge wasm sha256 `02f45a52…85`. `82f04c0` changes only the site |

The edge does not need 32.3. Only the site's `PrSync` consumer sends `Sync`.

**The container image was not rolled out to running instances.** After the site deploys, `/archive` on `wp6/edge1`'s shard still answered Go's "404 page not found". That reply means the shard's running container was the old image. The Containers application API showed no rollout for `graft-m1-site-*-gitshard`. Its `updated_at` was still 2026-11-01, and `configuration` was `{}`. Tearing the shard's container down with the test-only `/_bed/kill?repo=196630` (and `&shard=1`, `&shard=2`) started a new one, and `/archive` then answered 200.

So a site day that changes the container image may leave running containers on the old image until they stop. This was not investigated further, and the deploy script does not pass `--containers-rollout`.

## Real clients on Cloudflare

The final run is `raw/m2-edge/final4/`. Each command ran once with `tests/m2/edge-drive.sh <out> wp6/edge1 1 issue pr lfs release`, with `RUN=e10 PR_HEAD=pr-e9`.

- The clients were gh 2.102.1 (`/usr/bin/gh`), git 2.56.0, and git-lfs 3.8.0.
- `GH_DEBUG=api` logged every gh request.
- `pr-e9` is a branch with one commit over `main`, pushed through the edge first.

| # | Command | Exit | Requests and statuses |
| --- | --- | --- | --- |
| 01 | `gh issue list -R wp6/edge1` | 0 | `IssueList` 200. Lists issues 6, 4, 2, 1 |
| 02 | `gh issue create -t "edge issue" -b ..` | 0 | `/meta` 200, `Issue_fields` 200, `IssueRepositoryInfo` 200, `IssueCreate` 200. Printed `.../issues/6` |
| 03 | `gh issue view 6` | 0 | `/meta`, `Issue_fields`, `IssueByNumber`, and `IssueProjectItems`, all 200 |
| 04 | `gh issue comment 6 -b "hello from the edge"` | 0 | `IssueByNumber` 200, `CommentCreate` 200. Printed `.../issues/6#issuecomment-…` |
| 05 | `gh issue view 6 --comments` | 0 | All 200. The comment is shown |
| 06 | `gh pr list` | 0 | `PullRequestList` 200 |
| 07 | `gh pr create -H pr-e9 -B main --draft` | 0 | `/meta`, `Issue_fields`, `PullRequestForBranch`, and `PullRequestCreate`, all 200. Printed `.../pull/7` |
| 08 | `gh pr view 7` | 0 | `/meta`, `PullRequestByNumber`, and `PullRequestProjectItems`, all 200. State `DRAFT` |
| 09 | `gh pr checks 7` | 1 | `PullRequestByNumber`, two `__type` probes, and `PullRequestStatusChecks`, all 200. gh: "no checks reported on the 'pr-e9' branch", its exit for an empty rollup (`pkg/cmd/pr/checks/checks.go`) |
| 10 | `git lfs install` | 0 | Local |
| 11 | `git clone https://graft-m1-edge…/wp6/edge1.git` | 0 | git smart HTTP |
| 12 | `git lfs track '*.bin'` | 0 | Local |
| 13 | `git push origin HEAD:refs/heads/lfs-e10` (one 300,000-byte LFS object) | 0 | `locks/verify` 200, `objects/batch` 200, transfer `PUT` 200, `objects/verify` 200, then the git push |
| 14 | `git clone -b lfs-e10` | 0 | Smudge downloaded the object (`batch` 200, transfer `GET` 200) |
| 15 | `git lfs pull` | 0 | `cmp` of the pulled and pushed files: equal |
| 16 | `git lfs locks` | 0 | `GET locks` 200 |
| 17 | `gh release create v-e10 --notes x --target main` | 0 | `POST /releases` 201 |
| 18 | `gh release upload v-e10 app.tgz` | 0 | `RepositoryReleaseByTag` 200, `GET /releases/tags/v-e10` 200, `POST /api/uploads/.../assets?name=app.tgz` 201 |
| 19 | `gh release view v-e10` | 0 | 200s. Shows `asset: app.tgz` |
| 20 | `gh release download v-e10` | 0 | `GET /releases/assets/262152` 200. The bytes equal the uploaded file |
| 21 | `gh release download v-e10 --archive=tar.gz` | 0 | `GET /tarball/v-e10` 302 to `/_codeload/wp6/edge1/legacy.tar.gz/refs/tags/v-e10`, then 200. The file is `wp6-edge1-v-e10.tar.gz` |

More git-lfs commands ran with `GIT_TRACE=1 GIT_CURL_VERBOSE=1`, in `raw/m2-edge/final4-lfs/`:

- a second push of a 120,000-byte object;
- `git lfs lock c.bin` (201);
- `git lfs locks`;
- `git lfs unlock c.bin`;
- a clone and pull of that branch.

All exited 0, and the bytes matched.

Two earlier full runs give the same result: `final/` (`RUN=e7`) and `final2/` (`RUN=e8`, lfs and release only).

One run, `final3/`, began at 12:52:57Z, 7 minutes after the last site deploy finished and 4 minutes after the BF-24 check. In that run, every gh request got 401 "Bad credentials" and every git request got "Authentication failed". The same token worked at 12:57Z and in the next full run, `final4/`. The edge answers 401 when `authenticate` finds no token row. A `fresh` read on a `Tokens` shard whose members had just restored after the site day probably answered with no row. This was not investigated. It is reported as a possible gap: a token read during a group's restore should answer 503, not 401.

### Blob durability and markers

Each LFS `verify` and asset upload was checked against all three site buckets (`raw/m2-edge/markers-final2.txt`, `markers-final4.txt`):

| Blob | `blob/196630/<oid>` | `committed/blob/196630/<oid>` |
| --- | --- | --- |
| LFS `952eb8be…` (final2, 300,000 bytes) | a, b, c | a, b, c, `index` 78 |
| LFS `979efb6b…` (final4, 300,000 bytes) | a, b, c | a, b, c, `index` 93 |
| LFS `e8fb6ce2…` (final4-lfs, 120,000 bytes) | a, b, c | a, b, c, `index` 99 |
| Asset `98ea6e4f…` (3 bytes, uploaded in several runs) | a, b, c | a and b from an upload at `index` 58, before `82f04c0`. c from `index` 81, after it |

The asset row shows bug 3 before and after its fix. Before `82f04c0`, an upload marked only the two buckets `place` had waited for. After it, the third bucket got its marker too.

The 201 for an upload and the 200 for a `verify` come only from the member's render of a committed command. That command's `decide` refuses `Unavailable` unless `confirm` found the blob in two buckets in two regions. Before bug 2 was fixed, every `verify` got 404 and every upload 503. So no 2xx was served without that check.

## Schema validation of hosted replies

`tests/m2/gh-replies.py` (new) turns each drive's logs into `crates/api/gh/validate.py` cases.

- From gh's `GH_DEBUG=api` logs, it writes every 2xx GraphQL reply with the document gh sent, and every 2xx REST reply with its OpenAPI path. That includes the `/api/uploads` asset upload.
- From git-lfs's `GIT_CURL_VERBOSE` traces, it writes every batch, lock, locks, and lock-verify reply with git-lfs 3.8.0's schema for it.
- gh sometimes sends two requests at once and logs the headers of both before either reply. The tool pairs each GraphQL reply with the pending document whose fields or aliases name its `data` keys.

The cases were validated against the pinned `ghes-3.22.json` and `schema.docs-enterprise.graphql`:

```
final4 + final4-lfs: 26 GraphQL replies, 12 REST replies, and 5 LFS replies checked, 0 failed
final  + final-lfs:  24 GraphQL replies, 12 REST replies, and 4 LFS replies checked, 0 failed
```

`final4` had 40 HTTP replies, and 38 are cases. The other two are:

- the asset download, whose body is not JSON;
- the codeload tarball, whose path is not in the description.

The 302 to codeload is a case, with its `location`. The cases are in `raw/m2-edge/final4-replies/`, and the output is in `final4-validate.txt`.

The oracle catches a bad reply. Two planted changes in a copy of the cases were each reported:

- a string `number` in a `PullRequestByNumber` reply;
- a string `id` in the `POST /releases` 201.

## FormatsEnable

The finding from the first pass stands, with the format number changed. Tags 17 to 20 (`Release`, `ReleaseAsset`, `LfsObject`, `LfsLock`) are written only by `IssueCall`, so their format is `IssueCall`'s, now 32.5.

- SHARED, "Record formats and the two-release rule": "Release N+1 starts writing it only after the format is recorded as enabled".
- DESIGN, Shape: "A caller sends a new command, message kind, or field only after `FormatsEnable` has enabled its format".

What the test install did:

1. Every site now runs a release that reads 32.5.
2. The edge was then redeployed with `FORMATS_ENABLED=32.1,32.5`, the test install's stand-in for the `Release` view's enabled set.

Before the edge redeploy, every issue, release, and LFS write answered 503.

## BF-24's PrSync after the site day

`python3 tests/m2/bf24/bf24.py edgesd 1` ran one trial: a clean head push to pull request #1 of `wp6/bf24`, through the edge. The output is in `raw/m2-edge/bf24-edgesd.jsonl` and `.log`. The work tree was copied from WP5's `dist.work` and removed after.

| Measure | Value |
| --- | --- |
| Push entry | index 356 at the leader (site 1) |
| `TestMerged` | index 359, clean, 6,322 ms after the push entry on the leader's clock |
| `refs/pull/1/head` | the new head, in the same entry 359 |
| `refs/pull/1/merge` | `6b42cde8`, parents (base `a1996acb`, head `0c4c8461`) |
| `mergeable` on the leader | `true` |
| Client-side latency from the push's return | 6.99 s |

That is within WP5's `dist` distribution, and `PrSync` works on the merged build.

## Things seen and not fixed

- **`member restore failed: raft_state: codec version 0, this release reads 1`.** It appears in Workers Logs on all three sites: 378 events over 99 objects since 2026-11-04 13:46Z, at about the same hourly rate before and after this site day. It began under M2-WP5's versions, so this branch did not cause it.
- **Container image rollout.** See "The site day" above.
- **The 401 window after the site day.** See `final3/` above.
- **Not wired, as before.**
  - `mergePullRequest`: M2-WP3's `MergeCmd` is not in the site's `RepoCmd`, and the edge answers the GraphQL error.
  - Fork heads.
  - The git cache's compare counts. A pull request shows `additions: 0`.
  - `raw-content`.

## Design sentences the code contradicts

None new. These are carried from the first pass:

- **`Names` has no account-to-login row.** So logins other than the owner's render from the test-only `LOGINS`. Without `LOGINS`, they render as `ghost`.
- **`ROWS_MAX` (50,000 [guess]).** A repository larger than this cannot be read on the hosted path, because a paged read would mix indexes.

The 32.5 number does not contradict DESIGN. DESIGN names no repository format versions.

## Commits

On `edge-wiring`, after the first pass's `fd32823` through `aae0448`:

- `410291b` Merge `main` into `edge-wiring`, with one `RepoCmd { Push, Sync, Issue }`.
- `eba06c7` api: `HostedDir` carries the owner's `Authz` fold in the raft codec.
- `6685801` git: a confirmed blob matches the command's by oid and size, not by sites.
- `82f04c0` site: mark the blob the member confirmed, in every bucket that holds it.
- The commit that holds this file: `tests/m2/gh-replies.py`, and `edge-drive.sh` reading the created issue and pull request numbers (`RUN` and `PR_HEAD`).

## Reproduce

```
export PATH=$HOME/.cargo/bin:$PATH
docker build -t graft-m1-site-gitshard-git:edge containers/git
(cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:edge)
node workers/site/deploy.mjs tests/m1/wp1/sites.json --genesis tests/m1/wp7/genesis.json --test-faults --image <digest>
node tests/m1/integ/deploy-edge.mjs tests/m1/wp1/sites.json graft-m1-edge --test-faults --genesis tests/m1/wp7/genesis.json \
  --var FORMATS_ENABLED=32.1,32.5 --var 'LOGINS={"7000001":"wp6","7000002":"wp6-writer","7000003":"wp6-reader"}'
RUN=e10 PR_HEAD=pr-e9 tests/m2/edge-drive.sh $PWD/tests/m2/results/raw/m2-edge/final4 wp6/edge1 1 issue pr lfs release
python3 tests/m2/gh-replies.py tests/m2/results/raw/m2-edge/final4 tests/m2/results/raw/m2-edge/final4-replies ghes-3.22.json
python3 crates/api/gh/validate.py ghes-3.22.json schema.docs-enterprise.graphql tests/m2/results/raw/m2-edge/final4-replies
```

## The first pass, before the site day

The edge ran version `1c3e665a`, and the sites ran M2-WP5's release. Every repository-scoped gh and git-lfs command got the edge's 503 "the repository's members do not serve tier rows yet (site release)". Only edge-only replies were served: `/status`, `/api/v3/`, `/meta`, `/rate_limit`, `__type` probes, and the admission and token checks. 11 replies validated with 0 failures. The data is in `raw/m2-edge/clients/`, `probes.txt`, and `validate.txt`.
