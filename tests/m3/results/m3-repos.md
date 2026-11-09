# Repository create, update, and delete over REST, and the edge read gate, on graft-m2b

This round builds the creation half of `rest:repos` (`design/SHARED.md`, "Surfaces, tiers, and tests", the `rest:repos` row), so BF-34's `TestAccGithubRepositoryCollaborator` runs as written. It also adds the `PATCH` and `DELETE /repos/{o}/{r}` that the Terraform resource lifecycle needs. Branch `m3-repos`, cut from main `b767933`; `origin/main` `5d18dda` (the edge read gate `1c47d28`, and AM1-WP4's leases) was merged in as `3160687`. Everything ran on the **graft-m2b** test install. Nothing touched `graft-m1-*`.

## Summary

| Check | Result |
| --- | --- |
| `TestAccGithubRepositoryCollaborator`, as written (terraform-provider-github `1b9c5c1`, Terraform 1.17.1) | **6 of 6 runs pass**: 12 of 12 subtest passes (`creates invitations without error`, `creates invitations when repository contains the org name`). The third subtest skips without `GH_TEST_COLLABORATOR`, as written. All 12 `tf-acc-test-repo-collab-*` repositories the runs created answer 404 afterwards |
| `gh repo create` (gh 2.102.1) | **20 of 20**: 5 each of org `--private` (GraphQL `createRepository`), org `--public --add-readme` (`POST /orgs/m3org/repos`), user `--private` (GraphQL), and user `--public --add-readme` (`POST /user/repos`). Each one had the right URL, a `GET` of 200 with the visibility asked for, `main` present exactly when `--add-readme` was given, and a `DELETE` of 204 |
| REST smoke (`tests/m3/repos/smoke.mjs`) | 23 of 23 checks |
| Replies against the GHES 3.22 description | 14 hosted REST replies plus 9 REST and 1 GraphQL written by `crates/api/tests/repos.rs`: 0 failed. BF-34's driver, rerun behind the gate: 51 REST replies, 0 failed |
| Edge read gate (12 rounds per case) | Every case gives the expected status 12 of 12; see "Edge read gate" |
| Planted bugs | 14 of 14 caught (`tests/m3/repos/plant.sh`) |

## What was deployed on graft-m2b

Both deployments are from the merge commit **`3160687`**. The site day ran first, then the edge.

| Script | Version | When |
| --- | --- | --- |
| `graft-m2b-site-a` | `14a74047-d1b3-4bde-8192-1bf404ac0ade` | site day 00:22:08Z to 00:28:57Z |
| `graft-m2b-site-b` | `d053ae58-fe52-4b02-b27c-b73dbff5a70c` | same site day |
| `graft-m2b-site-c` | `acab4d3e-9ef4-4870-bfea-bc21fe23b718` | same site day |
| `graft-m2b-edge` | `03bc76dd-cedf-4a3e-a93b-de81f392c77b` | 00:29:27Z |

[measured, `wrangler deployments list`, each at 100%; logs `tests/m3/raw/repos/siteday-3160687.log` and `edge-3160687.log`]

- Each site day ran `tests/m3/rows/siteday.sh`: `workers/site/deploy.mjs --test-faults --anchor … --image …8fa9abd4… --roll-shards <edge>`, two passes per site. The image did not change, so `--roll-shards` restarted no shard. `GRAFTYARD_AUDIT` was not set, so no `Audit` consumer runs.
- The edge ran `tests/m3/rows/deploy-edge.sh`: `FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2`. That is the install's earlier set plus **32.13**. 32.14 (AM1-WP4's leases) is not enabled on m2b.
- Earlier in this round, a site day and an edge deploy from `75da33f` gave sites a `1ceba909`, b `4b586c11`, c `306dfb77` and edge `8c3f21cf`. That day's first attempt stopped on a wrangler `fetch failed` after site-a's upload (`siteday-1-fetchfail.log`), and the rerun completed. The smoke run on that build found the `auto_init` bug below.

## Formats and tags

- **Repository format 32.13** (`REPO_FORMAT_REPOS`) adds `IssueOp::InitRepo(RepoSettings)` and `IssueOp::DeleteRepo { retain_until }`. It also adds `RepoEvent::Created { actor }` (tag **33**) and `RepoEvent::Deleted { retain_until, actor }` (tag **34**), and the core tables `created` and `deleted`. Main's AM1-WP4 took 32.14 (tags 35 to 38) and already reserved 33 and 34 for this package. In the merge, `created` and `deleted` sit after AM1-WP4's lease tables in the CORE schema.
- No Authz or other format was taken.

## What was built

- **`POST /orgs/{org}/repos`, `POST /user/repos`, and gh's GraphQL `createRepository`** (`workers/edge/src/repos.rs`) commit in this order:
  1. `CreateRepo` on the owner's `Authz` chain, under the caller's credential. This is the commit point: `Taken` gives 422, and a caller who does not administer the owner gets 403.
  2. `RepoOwner` on the repository's `Names` shard.
  3. With `auto_init`, the README commit (`graftyard_git::init::readme_commit`, a 3-object pack that git accepts with `index-pack --strict` and `fsck --strict`), pushed as an ordinary `Push` of `main`.
  4. `InitRepo` with the body's settings, whose leader renders the 201 `full-repository` from `&Acked`.
- **`GET /repos/{o}/{r}`** renders `full-repository` with an ETag. **`PATCH`** is `RenameRepo` on the owner's chain when `name` changes, then `Settings`. **`DELETE`** is the chain's `DeleteRepo` (the `Deleted` tombstone, kept 90 days, after which the chain refuses every push and issue command), then `DeleteRepo` on the owner's `Authz` chain, which turns the name `Deleted` and holds it. A second `DELETE` is 404, and re-creating the name is 422 while it is held.
- **Fixed in this round (`cb98c9d`).** On the first hosted build, a create with `auto_init` answered default settings and `created_at` `1970-01-01T00:00:00Z`. `push_init` sent the README push under the create request's own id, so the leader answered the following `InitRepo`, sent under the same id, from the push's stored row, and `InitRepo` never ran. The README push now has its own request id. After the fix, `created_at` is the entry's time and the description and visibility are stored [measured, `smoke-1.jsonl` before, `smoke-2.jsonl` after].
- **Behind the read gate (merge `3160687`).** `GET`, `PATCH`, and `DELETE /repos/{o}/{r}` arrive through `api::rest_repo`, whose `read` runs `authorize_read` (`Gate::Readers`) before any route sees the repository. The route's own visibility check was removed, because the gate does that job. The create's read of the new chain (`api::read_new`) also passes `Gate::Readers`. The creator administers the new repository, so the gate allows it, and every create above went through it.

## Terraform: `TestAccGithubRepositoryCollaborator`

`tests/m3/repos/tf.sh m3r-tf-ext 6`, run as the orb service `m3repos-tf2`, 00:59Z to 01:04Z. Environment: `TF_ACC=1`, `GH_TEST_AUTH_MODE=organization`, `GITHUB_OWNER=m3org`, `GITHUB_BASE_URL=<edge>/`, `GH_TEST_EXTERNAL_USER1=m3r-tf-ext` (7159001), and m3-admin's PAT in `GITHUB_TOKEN`, set in the script's environment only.

| Run | rc | PASS | FAIL | SKIP | The run's repositories afterwards |
| --- | --- | --- | --- | --- | --- |
| 1 | 0 | 3 | 0 | 1 | `edv6f` 404, `jfemt` 404 |
| 2 | 0 | 3 | 0 | 1 | `4dwjy` 404, `o7y8v` 404 |
| 3 | 0 | 3 | 0 | 1 | `f1vqp` 404, `tkoru` 404 |
| 4 | 0 | 3 | 0 | 1 | `nqxpw` 404, `se8qi` 404 |
| 5 | 0 | 3 | 0 | 1 | `aaysu` 404, `dvavq` 404 |
| 6 | 0 | 3 | 0 | 1 | `0n7mv` 404, `w9peo` 404 |

Each PASS count of 3 is the parent test plus its two subtests, each taking about 46 s. The SKIP is `can delete collaborators from archived repositories without error`, which needs `GH_TEST_COLLABORATOR` [measured, `tests/m3/raw/repos/tf/summary.tsv`, `run-*.log`]. A seventh run, before `tf.sh` learned to find the repository names, also passed 3, 0 failed, 1 skipped (`tf-first/`).

Each subtest's lifecycle on the edge is:
1. `POST /orgs/m3org/repos` with `auto_init`.
2. `PATCH` from the provider's `resourceGithubRepositoryUpdate`.
3. `GET`.
4. `PUT .../collaborators/m3r-tf-ext` with `triage`, which answers 204, a pending invitation, as GHES 3.22 documents.
5. A refresh and plan.
6. The collaborator's destroy, which withdraws the invitation.
7. `DELETE /repos/m3org/<name>`.

## Edge read gate

`tests/m3/repos/gate.mjs 12`, run as the orb service `m3repos-gate2`, 00:39Z to 00:44Z, on edge `03bc76dd`. Setup:
- m3-admin made `m3org/gate-priv-mw8j5wx8` (private) and `m3org/gate-pub-mw8j5wx8` (public), both with `auto_init`.
- Two fresh users were made: an outsider (7171131) with no access, and a collaborator (7171132) invited with `pull` (204) who accepted (204).

| Case | What was sent | Runs | Status codes |
| --- | --- | --- | --- |
| Anonymous `git clone` of the private repository | stock git, no credential | 12 | 12 of 12: `info/refs` **401** with `WWW-Authenticate: Basic realm="GitHub"`, and git exits 128 |
| Anonymous `git clone` of the public repository | stock git, no credential | 12 | 12 of 12: clone succeeds (rc 0) |
| Outsider `git clone` of the private repository | the outsider's PAT | 12 | 12 of 12: `info/refs` **404** "Repository not found.", and git exits 128 |
| Outsider `gh api repos/m3org/gate-priv-…` | gh with its own config | 12 | 12 of 12: **HTTP 404**, gh exits 1 |
| Collaborator `GET /repos/m3org/gate-priv-…` and `git clone` | the collaborator's PAT | 12 | 12 of 12: **200**, and the clone succeeds (rc 0) |
| Revoked token | a new PAT for the collaborator: read, then `DELETE /applications/{gh}/token`, then a read 11 s later | 12 | 12 of 12: before **200**, revoke **204**, git `info/refs` after **401**, REST `GET /repos` after **404** (see below) |
| Outsider LFS batch | `POST …/info/lfs/objects/batch` (download) | 12 | 12 of 12: **404** |
| Outsider codeload | `GET /_codeload/m3org/gate-priv-…/legacy.tar.gz/<sha>` and `GET /repos/…/tarball/main` | 12 | 12 of 12: **404** and **404** |
| One leader read `{"q":{"rows":{"access":true}}}` | through the bed route, on `repo:131113` at index 12 | 1 | 200. Core rows: `settings` 1 and `collaborator` 1; `meta` null; `complete` true. A full `{"rows":true}` read of the same chain at the same index returns core `ids`, `ref`, `collaborator`, `settings`, `created` and metadata `ids`, `retention`. So the access read leaves out every row the gate does not read |

[measured, `tests/m3/raw/repos/gate.jsonl` and `gate.log`. The first attempt, `gate-setupfail.*`, stopped in setup, because the driver looked up the invitation without the `?repo=o/r` that the edge's user-invitation routes need (m3-rows, BF-34). The gate was not involved]

**The revoked token's REST read is 404, not 401.** After revocation, `authenticate` finds the `Tokens` row revoked and returns no principal (`workers/edge/src/lib.rs`, `authenticate`). A REST read then runs as anonymous, and the gate hides a private repository from anonymous callers with 404. Git's routes answer an anonymous caller 401, so git got 401 in 12 of 12 runs. The token is refused in both cases, and nothing in the repository is served. But the check asked for a 401, and REST does not give one. GitHub answers a REST call with a bad token 401 "Bad credentials". So does this edge, but only on the routes that require a caller (`api.rs`, the `who?` check before `Bad credentials`); `GET /repos/{o}/{r}` is not one of them. An unknown well-formed token gives the same 404 on the private repository, and the public repository answers 200 anonymously. I did not change the gate or `authenticate` to work around this. It is reported here as found.

### The existing drivers, behind the gate

| Driver | Who | Runs | Result |
| --- | --- | --- | --- |
| `tests/m3/repos/seeded.sh bf24 10`, the seeded roles on the private `m2b/bf24` (`tests/m1/integ/seed.ts`) | owner `m2b` | 10 | 10 of 10: `gh api` 200 with admin, push, pull; `gh issue list` rc 0; REST `commits/main/statuses` 200; clone rc 0; push accepted |
| | writer `m2b-writer` | 10 | 10 of 10: `gh api` 200 with push and pull but not admin; `gh issue list` rc 0; REST 200; clone rc 0; push accepted |
| | reader `m2b-reader` | 10 | 10 of 10: `gh api` 200 with pull only; `gh issue list` rc 0; REST 200; clone rc 0; push refused **403** |
| | the seed's revoked token | 10 | 10 of 10: `info/refs` **401**, and clone exits 128 |
| `tests/m3/rows/bf34.mjs 3 g` (collaborators, invitations, teams, org invitations on `m3org/j4`) | m3-admin, m3-member, fresh users | 3 | 3 of 3 pass; 51 replies valid |
| `tests/m3/rows/bf35.mjs 10` (`POST`/`GET /user/keys`, `gh ssh-key list`) | m3-member | 10 | 10 of 10 |
| `tests/m3/repos/smoke.mjs` and `ghcreate.mjs` (above) | m3-admin, m3-member | 1 and 5 | 23 of 23 and 20 of 20 |

[measured, `tests/m3/raw/repos/seeded-bf24.tsv`, `bf34.jsonl`, `bf35.jsonl`]

- No hosted run broke on the gate.
- `m2b/bf20` (65536) answers 503 "repo:65536#1#f2: not leader" to every caller, owner included, so the seeded run used `bf24`. That is the group not answering, not the gate (the gate's own failure is a 503 that names the access rows or `Authz`).
- The driver's first REST probe, `GET /repos/m2b/bf24/commits/main`, was 404 for the owner too. The edge serves no `commits/{ref}` or `branches` route at all (`api.rs`, the repository route table), so the probe moved to `commits/main/statuses` (`seeded-bf24-commits404.tsv` kept).
- `tests/m2/edge-drive.sh` was not reused. It hardcodes `graft-m1-edge` and puts the token on `git config`'s argv.

## gh repo create

`tests/m3/repos/ghcreate.mjs 5`, 00:46Z to 00:56Z, 20 of 20 pass [measured, `tests/m3/raw/repos/ghcreate.jsonl`]. gh resolves an org owner with `GET /users/m3org` and sends `createRepository` with that `ownerId`. With no owner, it sends no `ownerId` and the viewer owns the repository. `--add-readme` makes gh use `POST /orgs/{org}/repos` or `POST /user/repos` with `auto_init`. Each created repository was deleted (204).

## Planted bugs

`tests/m3/repos/plant.sh` applies each plant alone, runs `graftyard-git --lib --test issues --test transfer` and `graftyard-api --test repos`, and reverts the plant.

| Plant | Caught by |
| --- | --- |
| `init-without-admin` | `init_repo_writes_the_first_settings_once_and_needs_an_admin` |
| `init-not-once` | the same |
| `init-any-owner` | the same, and `a_created_then_deleted_repository_refuses_every_write` |
| `created-overwritten` | `init_repo_writes_the_first_settings_once_and_needs_an_admin`, after `85f626e` added a second `Created` entry to it. The first pass missed this plant |
| `delete-without-admin` | `delete_repo_tombstones_the_chain_and_refuses_every_later_write` |
| `delete-already-due` | the same |
| `deleted-takes-writes` | the same, and `a_created_then_deleted_repository_refuses_every_write` |
| `deleted-takes-pushes` | `a_created_then_deleted_repository_refuses_every_write` |
| `private-over-visibility` | `create_and_update_bodies_map_to_settings_and_refuse_what_ghes_refuses` |
| `user-internal` | the same |
| `no-merge-method` | the same |
| `created-at-epoch` | `a_created_repository_renders_ghes_full_repository_with_its_settings` |
| `readme-tree-as-blob` | `init::tests::git_reads_the_readme_commit` |
| `readme-wrong-blob-id` | the same |

[measured, `tests/m3/raw/repos/plants.log`, and `PLANTS_ONLY=created-overwritten` after `85f626e`]

The `auto_init` request-id bug has no plant: the in-process raft harness (`crates/git/tests/transfer.rs`) does not replay by request id the way the site's leader does, and a probe test there passed with the shared id. The hosted smoke check `GET is 200 with the same id` (description and settings) is its check.

## Checks

On `85f626e`:
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 `cargo clippy -- -D warnings`: clean for `workers/edge` and `workers/site`, each with and without `test-faults`, and for `workers/ops`.
- `cargo test --workspace` (on `3160687`): 566 passed, 0 failed.
- `scripts/check_design.py`, `check_names.py`, and `check_links.py`: OK.

## Secrets

- No token was on argv, in a URL, or in a log. gh read tokens from 0600 `hosts.yml` files in its own `GH_CONFIG_DIR`. git read them from a credential helper that reads a 0600 file. curl read them from stdin (`-H @-`). Node sent them in headers only.
- `tests/m3/rows/lib.mjs`'s `scan` refuses any log row that holds a token or a token-shaped string.
- A scan of every file in `tests/m3/raw/repos/` and `/tmp/repos-hosted/` (99 files) checked for each m2b and m3r token, the bed token, the tools token, and the token, key, JWT, and PEM shapes: 0 hits.
- The worktree's `tests/m0/.m0-token` is a symlink to the main checkout's gitignored file, which `lib.mjs` reads. It is not committed.

## Design notes and contradictions (not edited)

- `design/SHARED.md`, "Multi-chain operations", says: "Every multi-chain operation names its commit point, its finisher, and its abort path." It lists transfer, enterprise change, fork, signup, credentials, and workflow run, but not a repository's create or delete. The create here is `CreateRepo` (the commit point), then `RepoOwner`, the README push, and `InitRepo`, all in band. No finisher owes the rest: `Audit`'s `authz_owed` owes only `Reserved` names and enterprise changes. So an edge crash between `CreateRepo` and `RepoOwner` strands a `Live` name whose group cannot be built [inference]. The delete has the same gap: an edge crash between the chain's `Deleted` and the name's `Deleted` leaves the name resolving to a chain that answers `NotFound`.
- `design/SHARED.md`'s `RepoEvent` list has no creation event and no creation time. GHES's `full-repository` requires `created_at`, `updated_at`, and `pushed_at`. 32.13's `Created` writes `created_at`. `updated_at` and `pushed_at` render as the creation time [inference], since nothing records them yet.
- `design/SHARED.md`, `RepoEvent`: "`Deleted { retain_until: Timestamp, actor: Actor }`". The code's `Deleted` carries `actor: Option<AccountId>`, the shape the other 32.x events use. SHARED, "Repository repair, tombstones, and retention": "Deleting a repository appends `Deleted { retain_until }` to its chain, 90 days by default, and turns its name entry `Deleted`." That is what is built, in that order.
- The read gate answers a REST read made with a revoked or unknown token as anonymous: 404 on a private repository, 200 on a public one. GitHub answers 401 "Bad credentials" (see "Edge read gate").

## Not done

- `GET /orgs/{org}/repos`, `GET /user/repos`, `commits/{ref}`, and `branches` are not served. Neither the Terraform test nor `gh repo create` needs them.
- The archived-repository subtest was not run. It needs `GH_TEST_COLLABORATOR`, and it skips by design without it.
- Nothing finishes a create or delete that an edge crash cut short (see above).
- 32.14 is not enabled on m2b, so its lease routes were not exercised here.

## Commits on `m3-repos`

| Commit | What |
| --- | --- |
| `5066a6f` | Repository format 32.13 (`InitRepo`, `DeleteRepo`, `Created`, `Deleted`), the README commit, `graftyard_api::repos`, tests |
| `75da33f` | The edge's `rest:repos` module, member rendering, site reads, and the siteday and deploy fixes |
| `cb98c9d` | `auto_init`'s README push takes its own request id; the smoke driver and the plants |
| `3160687` | Merge of `origin/main` `5d18dda` (the read gate, AM1-WP4); the `rest:repos` routes behind the gate |
| `85f626e` | The read-gate, seeded-role, `gh repo create`, and Terraform drivers; the `created-overwritten` assertion |
| `c141f88` | This file |
| `e2a69da` | `tf.sh` reads repository names from the kept working directories |

Nothing is pushed or merged.
