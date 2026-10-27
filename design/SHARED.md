# Graftyard shared specification

This document is the platform-independent part of Graftyard: what unmodified GitHub clients see, the chain event and command types, tokens, the issue and pull request model, folds, Actions, event delivery, the two-release format rule, repository repair and retention, release signing, public service rules, and the web app. It holds no storage or fleet mechanics. `design/DESIGN.md` maps every mechanism here onto Raft groups of Durable Objects on three sites. Where the two disagree, `design/DESIGN.md` wins.

## Read this first

- Evidence tags carry over from the source: [measured], [doc], [inference], [guess], [unverified]. A claim with no tag and no citation is a design decision.
- A chain is one gap-free log of entries: a repository, a fork network, an owner's `Authz` or `Identity` chain, and the other chains that `design/DESIGN.md` lists in `ChainId`. Each chain is one group there, and a chain position is an `Index`.
- `commit`, `Command`, `decide`, `Acked`, `fresh`, `Fresh`, `Stale`, `ReadView`, `PackRef`, `DurablePack`, and `PackStore` are defined in `design/DESIGN.md`, "Shape". This document uses them and does not redefine them.
- `ObjectId` here and `Oid` in `design/DESIGN.md` name the same git object id [inference].
- "The blob store" means the pack buckets and the Actions blob bucket that `design/DESIGN.md` defines in "Shape" and "Stack".
- "The git cache" means the git cache shards of `design/DESIGN.md`, "Shape", which run upstream git.
- `design/DESIGN.md` changes some reused parts. It adds `provenance`, `protected`, and `changed` to `Push`, the `parent_tip` rule to the merge queue, lazy test merges, agent admission, and agent refusals in `decide`. It also defines `Names`, `Tokens`, `Actions`, and `Ids` entries, `IdentityEvent::Record`, the recorded stall `ExtendLeases`, the `DbId` layout, and the `Release` group's rollback barrier. Each section below points to those changes where they apply.

## Wire compatibility

Graftyard targets GitHub Enterprise Server (GHES) 3.22. gh classifies a host as GHES because it is not `github.com`, `*.ghe.com`, or `github.localhost` (`research/clients.md#1`). It derives `/api/v3/` and `/api/graphql` (`#3`) and reads `installed_version` from `/api/v3/meta` (`#9`). Nothing else is configured.

An "Octokit contract run" means Schemathesis cases generated from the pinned `ghes-3.22` OpenAPI file for that category, executed through unmodified `@octokit/rest` with `octokit.paginate`, under both API versions `2022-11-28` and `2026-03-10`, with responses validated against the schema (`research/clients.md` conformance table, octokit row). It is always paired with a named client run where one exists.

### Surfaces, tiers, and oracles

In the Approach column, a "row" is a row in a SQLite fold table, derived from chain entries. Tiers are T0 (launch) to T3, and Out means not served.

| Surface | Tier | Oracle | Approach |
| --- | --- | --- | --- |
| `git-http` | T0 | `git clone`, `git push --atomic`, `git fetch --filter=blob:none` with git 2.39 and 2.51, plus `GCM_TRACE=1 git clone` with no provider override | Smart HTTP only, dumb HTTP refused. Graftyard's own receive-pack, upstream upload-pack on the git cache, refs from the fresh core tier. |
| `git-ssh` | T0 | `ssh -T git@HOST` prints the GitHub-style greeting and exits 1, then `git clone git@HOST:o/r.git` and push; registering a public key that another account already holds returns 422 | One service host key set. The client key's service-wide unique fingerprint resolves to its account, the repository path comes from the exec command, and the request is authorized and committed as HTTPS is (`design/DESIGN.md`, journey 4, supplies the SSH path). |
| `git-protocol-v2` | T0 | `GIT_TRACE_PACKET=1 git ls-remote https://HOST/o/r` shows `ls-refs=unborn` and `fetch=shallow wait-for-done filter` | Advertise exactly github.com's capability set (`research/wire-census.md#41`). `bundle-uri` and `packfile-uris` stay off until T2. |
| `lfs` | T0 | `git lfs track '*.bin' && git push && git lfs fetch --all && git lfs locks` over HTTPS and SSH | Batch API with presigned URLs to `R2Key::Blob` objects, `verify` action required, `LfsObject` entries for verified objects, and locks as `LfsLock` entries on the repository chain. `verify` returns only after the object is a `DurableBlob`, as durable as a stored pack [inference: the source's rule was a second-Region copy before `verify`]. |
| `rest:actions` | T0 | `gh run list`, `gh run view --log`, `gh workflow run`, `gh secret set`, plus Octokit contract run | Registration, runners, runs, jobs, secrets (sealed box), variables at T0. Remaining operations T1. |
| `rest:activity` | T1 | `gh status`, `gh api notifications`, `gh api user/starred`, Octokit contract run | Stars, watches, subscriptions, follows, and notification thread state are `IdentityEvent::Record` entries on the actor's `Identity` chain, off the repository chain. The `Counts` consumer writes coalesced star and watcher counts, and a derived watcher index of stars, watches, and thread subscriptions, to each repository's metadata tier. `/stargazers`, `/subscribers`, and `Notify` read that index. Notifications, feeds, `/issues`, and `/user/repos` come from a derived per-account `Inbox` that the `Notify` consumer fills from chain cursors (`design/DESIGN.md`, "Shape", Identity), and they lag ("Known deviations from GitHub"). |
| `rest:agent-automations` | Out | none | dotcom-only Copilot agent surface, absent from GHES 3.22. |
| `rest:agent-tasks` | Out | none | dotcom-only, absent from GHES 3.22. |
| `rest:agents` | Out | none | dotcom-only, absent from GHES 3.22. |
| `rest:announcement-banners` | T2 | Octokit contract run | Rows per enterprise and org. |
| `rest:apps` | T0 | Renovate in GitHub App mode, ARC with app credentials, `@octokit/app` installation token flow | App JWT auth, installations, stateless `ghs_` tokens. |
| `rest:billing` | T2 | Octokit contract run | Metering rows. |
| `rest:branches` | T0 | `TF_ACC=1 go test ./github -run TestAccGithubBranchProtectionV3`, Octokit contract run | `Rule` entries on the repository chain, enforced in `Push::decide` and `Merge::decide`. |
| `rest:campaigns` | Out | none | dotcom-only. |
| `rest:checks` | T0 | `gh pr checks`, Jenkins GitHub Branch Source posting checks | `CheckRun` and `CheckSuite` entries in the metadata tier, kept `check_retention_days`; required conclusions for open pull request heads, queue refs, and protected tips also as `RequiredCheck` entries in the core tier. |
| `rest:classroom` | Out | none | dotcom-only. |
| `rest:code-quality` | Out | none | dotcom-only. |
| `rest:code-scanning` | T2 | `github/codeql-action/upload-sarif` on a pinned runner | SARIF in the blob store, alerts as rows. |
| `rest:code-security` | T2 | Octokit contract run | Configuration rows. |
| `rest:codes-of-conduct` | T3 | Octokit contract run | Static data. |
| `rest:codespaces` | Out | none | dotcom-only, and Codespaces is a separate product. |
| `rest:collaborators` | T0 | `go test ./github -run TestAccGithubRepositoryCollaborator`, Octokit contract run | Outside collaborators as `Collaborator` entries on the repository chain, org members and teams as `Grant` entries on the owner's `Authz` chain, never both ("Who owns which fact"), and repository `Invitation` entries with accept and decline. |
| `rest:commits` | T0 | Jenkins multibranch commit status, `gh api repos/o/r/commits`, Octokit contract run | Commit data from the git cache, statuses as metadata-tier `Status` entries, and statuses a rule requires also as `RequiredCheck` entries. |
| `rest:copilot` | Out | none | dotcom-only. |
| `rest:copilot-spaces` | Out | none | dotcom-only. |
| `rest:credentials` | Out | none | dotcom-only. |
| `rest:dependabot` | T2 | `dependabot update -f job.yml` with `source.hostname=HOST` | Dependabot runs as Actions jobs on labelled runners (`research/clients.md#41`). |
| `rest:dependency-graph` | T2 | `actions/dependency-review-action` on a pinned runner | Manifests parsed by a chain consumer. |
| `rest:deploy-keys` | T0 | `gh repo deploy-key add`, then `git clone` with that key | `ssh_key` rows scoped to one repository. |
| `rest:deployments` | T1 | A workflow with `environment:` protection and `gh api` deployment listing | Environments and deployments rows, approvals gate job queueing. |
| `rest:emojis` | T3 | Octokit contract run | Static data. |
| `rest:enterprise-admin` | T1 | Octokit contract run over the GHES-only operations, `gh api` admin scripts | User and org administration first. Appliance console operations map to the install's own admin API, and in managed mode to vendor staff tooling. |
| `rest:enterprise-teams` | T2 | Octokit contract run | Team rows at enterprise scope. |
| `rest:gists` | T2 | `gh gist create`, `gh gist view` | Gists are repositories in the same storage. |
| `rest:git` | T0 | Renovate with `platformCommit=enabled`, Octokit contract run | Objects written by the git cache as a small pack, refs through `commit`. |
| `rest:gitignore` | T3 | Octokit contract run | Static templates. |
| `rest:interactions` | Out | none | dotcom-only. |
| `rest:issues` | T0 | `gh issue create/list/view/comment/close` in cli/cli acceptance tests | `IssueOpened` and related entries, numbers from the fold at decide, lists from the metadata tier. |
| `rest:licenses` | T2 | Octokit contract run | License detection by a chain consumer. |
| `rest:markdown` | T1 | `gh api markdown -f text=...`, VS Code Pull Requests preview | Rust renderer matched against GitHub's output corpus. |
| `rest:meta` | T0 | GCM provider probe, `gh auth status`, Desktop sign-in | `installed_version: "3.22.0"`, `verifiable_password_authentication: false`. |
| `rest:metrics` | T2 | Octokit contract run | Derived from consumer counters. |
| `rest:migrations` | T1 | Octokit contract run with an export and import round trip | Archives in the blob store, imports as chunked entries. `graftyard import ghes <archive>` maps a `ghe-migrator` archive: users, orgs, teams, and memberships into name claims and `Authz` and `Identity` chains, repositories as git bundles with LFS objects, then issues, pull requests, reviews, comments, labels, milestones, releases with assets, branch protections, and webhooks. Every entry an import writes is an `EntryBody::Imported(ImportBatch)` (`design/DESIGN.md`, Shape, "Chains"). `Webhooks`, `ActionsTrigger`, `Notify`, `RunOwner`, `PrSync`, `MergeQueue`, and `Insights` skip it, and only `Search`, `Counts`, and `Audit` read it, so an import fires no hook, run, or notification. The import Workflow writes it through an import-only command set (`ImportIssue`, `ImportPull`, `ImportComment`, `ImportReview`, `ImportLabel`, `ImportRelease`), which `decide` accepts only from that Workflow while the repository's `Settings` say `importing`. Each item keeps the archive's issue number, `created_at`, and author, mapped to the imported login or to `ghost` when none was imported. `decide` refuses a number already used and moves the number counter past the largest, so `#N` links in imported bodies still resolve. A merged pull request is `ImportPull` with `merged: (merge commit, time, actor)`. `apply` records the merged state and commit with no `Merged` event, because the imported refs already hold the merge commit, so no tree-diff check or merge engine is recorded. A final `Settings` entry clears `importing`. Every mode assigns new ids. A `legacy_id(kind, legacy, id)` table maps archive ids to new ones: issue, comment, review, and release ids in that repository's metadata tier, and account, team, and repository ids in the importing owner's `Authz` core tier. SSH and GPG public keys carry over. PATs, OAuth tokens, webhook secrets, Actions secrets, and runner registrations do not, so each user gets a re-issue notice, owners re-enter secrets, and runners register again [inference: the archive carries no usable secrets]. Instance SAML or OIDC maps each identity to its imported login. |
| `rest:oauth-authorizations` | T3 | Octokit contract run | Deprecated GHES-only API, read paths only. |
| `rest:orgs` | T0 | `go test ./github -run TestAccGithubOrganization`, `gh org list`, Octokit contract run | `Authz` and `Identity` entries, org `Invitation` entries on the `Authz` chain, the login claim in the `Names` groups, deletes as `Deleted` tombstones kept 90 days. |
| `rest:packages` | T2 | `gh api` package listing after a container push | Metadata over the package tag records. |
| `rest:pages` | T2 | `gh api repos/o/r/pages` after a Pages deploy | Site metadata rows. |
| `rest:private-registries` | T2 | Octokit contract run | Sealed-box secrets as rows. |
| `rest:projects` | T2 | `gh project list`, Octokit contract run | Project rows. |
| `rest:pulls` | T0 | cli/cli acceptance `pr` tests, Renovate onboarding and update PRs | `PrOpened` and related entries on the base chain, merge as one `Merged` entry. |
| `rest:rate-limit` | T0 | VS Code Pull Requests `HEAD /api/v3/rate_limit`, Jenkins hub4j endpoint check | One `RateCounter` object per key (user, installation, `GITHUB_TOKEN` repository, minted agent token, or IP) holds each window's total (`design/DESIGN.md`, Shape, Identity), with GitHub's documented numbers in managed mode and the error bound in "Public service concerns". `/rate_limit` reads the same objects and does not count against them. |
| `rest:reactions` | T1 | Octokit contract run | Reaction rows. |
| `rest:releases` | T0 | `gh release create --generate-notes`, `gh release upload`, `gh release download` | `Release` and `ReleaseAsset` entries in the metadata tier, asset bytes as `R2Key::Blob` objects in two buckets. |
| `rest:repos` | T0 | `go test ./github -run TestAccGithubRepository`, `gh repo create/view/clone`, `gh repo fork`, `gh repo rename`, Octokit contract run | `RepoName` entries on the owner's `Authz` chain, contents through the git cache, renames as `Alias` entries, deletes as `Deleted` tombstones kept 90 days, forks in their network. |
| `rest:search` | T1 | `gh search issues`, `gh search prs`, `gh search repos` | FTS5 tables in `SearchShard` objects, 16 per residency domain by repository hash [guess] (`design/DESIGN.md`, Shape, Identity), fed by the `Search` consumer. A query fans out to the shards of the `Default` domain and of the viewer's orgs' domains, and filters by public visibility or the viewer's reach from `Authz` folds. A shard that misses a 5 s deadline [guess] is left out and the reply sets `incomplete_results`. Search inside one repository uses that repository's own metadata-tier FTS5 tables ("Folds"). Code search T2. |
| `rest:secret-scanning` | T2 | Octokit contract run with a seeded secret push | Scanner as a push consumer. |
| `rest:security-advisories` | T2 | Octokit contract run | Advisory rows. |
| `rest:teams` | T0 | `go test ./github -run TestAccGithubTeam` | `Team` entries on the org's `Authz` chain, CODEOWNERS uses them. |
| `rest:users` | T0 | `gh api user`, cli/cli acceptance tests | `Identity` entries and SSH and GPG keys. SSH fingerprints are unique across the service. |
| `graphql` | T0 | Static validation of VS Code PR's 4 `.gql` files, JetBrains' 86 `.graphql` files, Renovate's queries, and gh's raw strings against introspection, then the cli/cli acceptance run and a `@vscode/test-electron` list-review-merge run | Schema generated from GHES 3.22 SDL, so `__type` probes match. Resolvers for the harvested operation set at T0, the rest T1. Unimplemented fields return `undefinedField` errors. |
| `webhooks` | T0 | Argo CD refresh on push, Jenkins multibranch discovery, go-playground/webhooks parsing recorded payloads for all 272 event-action pairs | Chain consumer. Core events (push, pull_request, issues, workflow_job, check_run) T0, the rest T1. |
| `oauth-web-flow` | T0 | `gh auth login --web`, GitHub Desktop Playwright sign-in | Authorization code flow with seeded first-party apps. |
| `oauth-device-flow` | T0 | `gh auth login`, VS Code `getSession('github-enterprise', ['repo'])`, `GCM_GITHUB_AUTHMODES=device` | Device flow with `slow_down` and form or JSON responses. |
| `github-apps` | T0 | Renovate app mode, ARC app auth, Argo CD `--github-app-enterprise-base-url` | Manifests, private keys, JWT auth, installation tokens. |
| `tokens` | T0 | Renovate token checks (`ghp_`, `github_pat_`, `ghs_`), gh `X-Oauth-Scopes` handling | GitHub prefixes and CRC32, stateless `ghs_`. |
| `actions-runner-protocol` | T0 | Pinned unmodified `actions/runner` `config.sh` then a job, and ARC `gha-runner-scale-set` scaling from zero (ARC at T1) | `use_v2_flow: true`: broker, run service, launch, results. |
| `actions-workflow-syntax` | T0 | A corpus of public workflows parsed differentially against `@actions/workflow-parser`, then executed by the pinned runner | Rust port of the runner's `WorkflowParser` and `MatrixBuilder`. |
| `actions-cache` | T0 | `actions/cache@v4` and `@v5` save and restore on a GHES-classified host | Cache v1 at T0, twirp v2 with the Azure Blob dialect shim at T1. |
| `actions-artifacts` | T0 | `upload-artifact@v3.2.2` and `download-artifact@v3`, then `upload-artifact@v4` through the pin map (T1) | Legacy pipelines artifacts at T0. Twirp v4 plus block shim at T1, reachable through vendor forks ("Actions"). |
| `actions-oidc` | T0 | `aws-actions/configure-aws-credentials` assuming a role through an IAM OIDC provider | Issuer `https://HOST/_services/token` (managed: `https://graftyard.dev/_services/token`), JWKS with every current and previous signing key epoch, legacy `sub` format. |
| `actions-logs-results` | T0 | `gh run view --log` after a pinned runner job, live log streaming in the web UI | Results twirp, `comp=appendblock` into the blob store. |
| `packages-container` | T1 | `docker login HOST && docker push HOST/o/img:tag && docker pull` | OCI distribution over blob-store blobs, manifests by digest, tags as conditional records added at T1 [inference]. |
| `packages-npm` | T2 | `npm publish --registry https://HOST/_registry/npm/` | Same blob store. |
| `packages-maven` | T2 | `mvn deploy` to the GHES Maven path | Same blob store. |
| `packages-nuget` | T2 | `dotnet nuget push` | Same blob store. |
| `packages-rubygems` | T2 | `gem push --host` | Same blob store. |
| `pages` | T2 | `actions/deploy-pages` through the pin map, then `curl` the site | Static files in the blob store behind the edge. |
| `codeload-archives` | T0 | `gh release download --archive=tar.gz`, runner action tarball download | `git archive` on the git cache. The result is a derived `R2Key::ArchiveCache` object in the warm site's bucket beside the clone cache, named by no entry and rebuilt on a miss. |
| `raw-content` | T0 | `curl -L https://HOST/o/r/raw/main/README.md` | Served from the git cache with a sandbox CSP, `nosniff`, and `text/plain`. |
| `uploads-release-assets` | T0 | `gh release upload v1 ./dist/app.tgz` | `/api/uploads` streams to two buckets as a `DurableBlob`, then commits `ReleaseAsset`. |
| `meta-endpoint` | T0 | GCM HEAD probe for `X-GitHub-Request-Id`, gh feature detection, Desktop header read | GHES `/meta` shape, version from the build. |

### Cross-cutting conventions

These come from `research/wire-census.md` section B. They live in one shared wire layer, so no handler can skip them.

- Every response, git smart HTTP included, carries `X-GitHub-Request-Id` in GitHub's colon-separated hex shape, with the trace id inside it, and `X-GitHub-Enterprise-Version: 3.22.0`.
- REST responses carry `x-github-media-type: github.v3; format=json` and `x-github-api-version-selected`.
- `X-GitHub-Api-Version` selects `2022-11-28` (the default) or `2026-03-10`. An unknown value returns 400. The `2026-03-10` breaking changes (`#13`) are per-version response shapes generated from the two versioned OpenAPI files.
- ETags are weak and derive from the index of the last entry that changed the resource. A `304` costs one fresh read and does not count against the rate limit (`#17`).
- Pagination emits `link` headers with `/repositories/{id}/...` URLs, clamps `per_page` to 100 silently, and uses keyset scans behind `page`, `since`, `before`, and `after`.
- Error bodies use `basic-error` and `validation-error` with a string `status`.
- The 404 rule (`#21`) is the `Decision::Hidden` variant. A handler cannot render 403 for a private resource, because `authorize` never returns `Forbidden` there.
- Requests without `User-Agent` are rejected.
- A renamed repository resolves through its `Alias` entry. The API returns 301 to `/repositories/{id}`, and git is served transparently (`#23`).
- GHES rate limits are off by default (`#19`) in BYOC and self-hosted, and on in managed.

### Hostnames and endpoints

These rules follow `research/clients.md` and `research/actions-protocol.md` section H.

- The install host is validated at install time. It may not end in `github.com`, `ghe.com`, or `localhost`, so every client classifies Graftyard as GHES. Managed is one host, `graftyard.dev`.
- `/api/v3`, `/api/graphql`, `/api/uploads`, `/login/oauth/*`, and `/login/device/code` live on one bare host on port 443, because gh rejects ports and the runner drops them (`research/clients.md#4`, `research/actions-protocol.md#4`).
- `GET /status` returns 200 with `X-GitHub-Request-Id` for Dependabot. `GET /api/v3/status` returns a body that starts `GitHub lives!` for VS Code Pull Requests. `GET /api/v3/` returns the root document with `rate_limit_url` for Jenkins (`research/clients.md` implication 4).
- Self-hosters are told that a `github.` label, for example `github.example.com`, lets GCM and Desktop skip their probe.
- Subdomain isolation (`raw.`, `codeload.`, `uploads.`, `objects.`) is on in managed, where the service certificate covers `graftyard.dev` and `*.graftyard.dev` (`research/wire-census.md#33`). It is optional elsewhere.
- `RESERVED` keeps every first path segment the host serves out of the login namespace ("Names").

### Client ids and object ids

- The five first-party OAuth client ids in `FirstPartyClient` ("Identity and tokens") are seeded once, when the managed service or the install is created, and cannot be deleted.
- Every numeric id is `i64` in storage and JSON, and allocation stops below 2^53. GraphQL exposes `databaseId: Int` only where the value fits, and `fullDatabaseId: BigInt` as a string everywhere (`research/wire-census.md#28`).
- Node ids use GitHub's new MessagePack format, with the repository id first for repository-scoped types. They accept the legacy base64 form as input and honor `X-Github-Next-Global-ID: 1` (`#26`, `#27`).

### Known deviations from GitHub

These are the places where an unmodified client can observe a difference. Each one has an oracle that proves the documented behavior. `design/DESIGN.md` adds its own: the push size cap by Cloudflare plan ("Scale" and "Compared with the AWS design") and `mergeable: null` past the test-merge budget ("Shape").

| Deviation | GitHub | Graftyard | Oracle |
| --- | --- | --- | --- |
| Atomic push size | No limit found in the research [unverified] | At most 10,000 ref updates per `--atomic` push [guess at GitHub's own limit], rejected whole with a message that names the limit. Non-atomic pushes are not capped. | An atomic push of 10,001 tags is rejected with the message, and one of 10,000 tags succeeds |
| Pull request refs | Updated after the push [inference] | `refs/pull/N/head` and `refs/pull/N/merge` move within seconds through `PrSync` | After a push to a PR branch, `git ls-remote origin refs/pull/N/head` shows the new SHA within 10 s |
| Automatic webhook retries | Never; failed deliveries are redelivered manually for 3 days (`research/wire-census.md#37`) | One automatic attempt by default, as on GitHub. A hook may opt in to automatic retries with backoff for 3 days under the same `X-GitHub-Delivery` GUID | A default hook answering 500 receives exactly one delivery, and an opted-in hook receives a retry with the same GUID |
| `upload-artifact@v4` and later pinned by SHA | Works | Fails as on GHES unless an org admin approves the signed pin map | A SHA-pinned v4 upload fails with the pin-map annotation, then passes after approval |
| `@actions/attest` provenance | Works | Unsupported, because the toolkit accepts only `github.com` and `*.ghe.com` issuers (`research/actions-protocol.md` section H) | A workflow using `actions/attest-build-provenance` fails with the toolkit's host error |
| Token revocation | Takes effect at once [inference] | A revoked token is refused for writes at once and for reads within 10 s, the read cache bound (`design/DESIGN.md`, "Shape", `READ_CACHE`, and journey 4) | A token revoked, then used for `git push` at once, is refused, and for `gh api user` 11 s later gets 401 |
| Notifications, feeds, `/issues`, and `/user/repos` | Current at once [inference] | Served from a per-account `Inbox` that the `Notify` consumer fills from chain cursors, so they lag commits by seconds (`design/DESIGN.md`, "Shape", Identity) | A comment on a watched issue appears in `gh api notifications` within 10 s, and the same `Inbox` rebuilt from cursors keeps read state |
| Reads during signed stale mode | No such mode [inference] | Fetches and reads serve each group's applied state for the flag's limit, so a read may miss an acknowledged write, and writes get 503. The mode starts only by a signed flag (`design/DESIGN.md`, "Shape", the read path) | During a stale drill, a push acknowledged just before the flag may be missing from a fetch, and it is present once the mode ends |
| Pull request from a resident org's detached fork to an outside upstream | Allowed for ordinary orgs | 422 with a message that names the residency rule ("Public service concerns") | `gh pr create` from such a fork returns that 422 |
| Transfer across a residency boundary | Allowed [inference] | 422 that names the residency rule when a transfer would move a resident network's owner outside its residency or bring an outside network into a resident org | `gh api -X POST repos/bob/tool/transfer -f new_owner=beta`, for a US network and an EU-resident `beta`, returns that 422 |

## Chain events and commands

Every acknowledged change is one chain entry, and every event inside it exists if and only if that entry committed. Refs, pull request state, reviews, rules, required check conclusions, and the event stream of a repository are entries on its one chain, so a merge decides on every input committed before it (`history/arena/round4/SYNTHESIS.md`).

### Event types

```rust
pub enum Event { Repo(RepoEvent), Network(NetworkEvent), Authz(AuthzEvent), Identity(IdentityEvent) }
pub enum RepoEvent {
    Genesis { owner: AccountId, network: NetworkId, name: RepoName, fork_of: Option<(RepoId, Index)> },
    Refs { push: PushId, actor: Actor, updates: Vec<RefUpdate>, pack: Option<PackRef> },   // one atomic push or bot batch
    PrOpened { n: IssueNumber, head: HeadSpec, base: RefName, capture: PrCapture },
    Merged { n: IssueNumber, base: RefUpdate, head: ObjectId, method: MergeMethod, also: Vec<RefUpdate>,
             pack: Option<PackRef>, verified: TreeDiffDigest, engine: EngineVersion },
    PrClosed { n: IssueNumber, final_head: ObjectId }, IssueOpened { n: IssueNumber, capture: IssueCapture },
    IssueChanged { n: IssueNumber, change: IssueChange, actor: Actor },
    Comment(CommentEvent), Review(ReviewEvent), Labels(LabelEvent), Queue(QueueEvent), Rule(BranchRule),
    Settings(RepoSettings), Collaborator { who: AccountId, permission: Option<RepoPermission> },
    DeployKey(DeployKeyEvent), LfsLock(LfsLockEvent), Takedown(TakedownEvent),
    /// An invitation to become an outside collaborator ("Who owns which fact"). Accepting it appends `Collaborator` in
    /// the same entry.
    Invitation(InvitationEvent),
    /// Graft 1, bounded: a required conclusion, from a check run or a commit status, for the head of an open pull request,
    /// a merge-queue ref, or a protected branch tip. `apply` prunes its row when the pull request closes or its head moves.
    /// `conclusion: None` is a queued or in-progress run or a pending status, so a re-run blocks a merge again.
    RequiredCheck { sha: ObjectId, name: Box<str>, source: CheckSource, version: u64, conclusion: Option<CheckConclusion> },
    /// Metadata tier. Every check run, check suite, and commit status is an entry, required or not. `apply` deletes a
    /// row once an entry's `at` passes the row's time plus `RepoSettings.check_retention_days`, so every member prunes
    /// the same rows. The archive keeps the entries.
    CheckRun(CheckRunUpdate), CheckSuite(CheckSuiteUpdate), Status(StatusUpdate),
    /// Core tier. One lazy test merge of pull request `n` (`design/DESIGN.md`, Shape, `TestMerge`). `apply` records
    /// `Mergeability { base, head, result }` on the `pull_head` row and moves `refs/pull/N/merge` to the merge commit, or
    /// deletes it on a conflict. `pack` holds the merge commit's objects. Whoever ran the merge commits it: `PrSync`
    /// after a head push, `ActionsTrigger` on dispatch, or the edge on a single pull request GET.
    TestMerged { n: IssueNumber, base: ObjectId, head: ObjectId, result: MergeabilityResult, pack: Option<PackRef> },
    /// Metadata tier, plus Core when `tag` is set: publishing a release whose tag does not exist creates the tag in the
    /// same entry. `record: None` deletes the release.
    Release { id: RepoReleaseId, record: Option<ReleaseRecord>, tag: Option<RefUpdate>, actor: Actor },
    /// Metadata tier. Committed by the upload after the bytes are a `DurableBlob`. `asset: None` deletes it.
    ReleaseAsset { release: RepoReleaseId, id: AssetId, asset: Option<AssetRecord>, actor: Actor },
    /// Metadata tier. Committed by the LFS `verify` action after the bytes are a `DurableBlob`, so `verify` answers only
    /// once the object is as durable as a stored pack. The batch API answers a download from these rows.
    LfsObject { blob: BlobRef, actor: Actor },
    /// Core tier. A repository webhook ("Event fan-out", `HookRecord`). `record: None` deletes it.
    Hook { id: HookId, record: Option<HookRecord>, actor: Actor },
    /// Core tier. A repository or environment secret or variable ("Actions", Secrets and variables). `None` deletes it.
    ActionsSecret { scope: SecretScope, name: ActionsName, record: Option<SecretRecord>, actor: Actor },
    ActionsVariable { scope: SecretScope, name: ActionsName, record: Option<VariableRecord>, actor: Actor },
    /// Graft 2: the named refs return to their targets at `to`; every other ref and the metadata tier stay.
    RestoredTo { to: Index, refs: Vec<RefName>, actor: Actor, reason: Box<str> },
    /// A separate admin intent: the whole fold becomes its state at `to`; entries between stay as history.
    ResetTo { to: Index, actor: Actor, reason: Box<str> },
    Deleted { retain_until: Timestamp, actor: Actor }, Undeleted { actor: Actor },   // retained 90 days by default
}
pub enum CheckSource { Run { app: AppId, run: CheckRunId }, Status { creator: AccountId } }
pub enum NetworkEvent { ForkAdded(RepoId), Compacted { consumed: Vec<PackId>, produced: Vec<PackRef> },
                        Takedown(TakedownEvent) }
/// What gates requests. An org's `Opened` names its enterprise, whose own `Authz` chain is a third dependency.
pub enum AuthzEvent { Opened { kind: AccountKind, enterprise: Option<AccountId> },
    Member { user: AccountId, role: Option<OrgRole> }, Team(TeamEvent),
    /// Org members and teams only. An outside collaborator is a repository `Collaborator` ("Who owns which fact").
    Grant { repo: RepoId, grantee: Grantee, permission: Option<RepoPermission> },
    RepoName { key: RepoNameKey, entry: Option<RepoEntry> },   // Reserved | Live | Alias | Deleted ("Names")
    Installation { id: InstallationId, state: InstallState, not_before: Timestamp }, Policy(OrgPolicy) /* incl. rulesets */,
    /// An invitation to join the org. Accepting it appends `Member`, and `Team` entries for its teams, in the same entry.
    Invitation(InvitationEvent),
    /// The Apps and OAuth apps this account owns ("Identity and tokens"). `None` deletes the registration.
    App { app: AppId, record: Option<AppRecord> }, OAuthApp { client: ClientId, record: Option<OAuthAppRecord> },
    /// Only on the install's own `Authz` chain, the account that `Genesis` names (`design/DESIGN.md`, Shape, "Release
    /// group"). Written by a `SiteAdmin` and read fresh by every sign-in.
    InstanceAuth(InstanceAuth),
    Deleted { retain_until: Timestamp }, Undeleted }
/// Never gates a request. `Record` holds stars, watches, follows, and notification state (`UserRecord`, `design/DESIGN.md`).
pub enum IdentityEvent { Created(AccountCapture), Credential(CredentialEvent), Sso(SsoEvent), Audit(AuditEvent),
                         Record(UserRecord) }
```

The source design also carried `Sealed` and `Unsealed` events on every chain kind. They fenced moves of chains between cells. `design/DESIGN.md` has no such move, so they are not part of this specification. Its `ChainId` also adds `Tokens`, `Names`, `Actions`, `Ids`, and `Release` chains, whose entries it defines, and `Event` gains `Names`, `Tokens`, and `Actions` variants there.

Every entry's tier tag sits outside its encoded body. An entry in a format not yet enabled is refused, by followers in `verify` and by the lifeboat, never skipped. The tag only lets the core tier apply an entry without decoding its metadata body ("Folds"). `tier_of` assigns the tags.

### Refs and pushes

```rust
pub struct ObjectId([u8; 20]);        // SHA-1 at launch; SHA-256 is a new variant, not a flag
pub struct RefName(Box<[u8]>);        // git check-ref-format valid; at most 900 bytes [guess]
/// `expect: None` creates. `new: None` deletes. Checked against the fresh core tier inside `decide`.
pub struct RefUpdate { pub name: RefName, pub expect: Option<ObjectId>, pub new: Option<ObjectId> }
pub enum Atomicity { AllOrNothing, PerRef }
/// impl Command. The last three fields come from the git cache's `validate`; their types are in `design/DESIGN.md`, "Shape".
/// `credential()` comes from `actor.credential`, and `request` is the edge's `RequestId` for the client request.
pub struct Push { pub updates: BoundedVec<RefUpdate, 10_000>, pub atomicity: Atomicity, pub pack: Option<DurablePack>,
                  pub actor: Principal, pub options: PushOptions, pub request: RequestId,
                  pub provenance: SmallVec<[Provenance; 4]>, pub protected: SmallVec<[ProtectedHits; 1]>,
                  pub changed: SmallVec<[ChangedPaths; 1]> }
pub struct PackId(Ulid);              // fresh for every placement; never derived from the pack's content
pub enum RefOutcome { Ok, Rejected(RejectReason) }
pub enum RejectReason { Stale, NonFastForward, Protected(RuleViolation), TooManyRefs, Hidden }
```

`report_status` takes `&Acked`, so git's `ok` cannot precede the commit. A push's `Acked` carries one `RefOutcome` per ref [inference: the source's `Acked` held `per_ref`, and `design/DESIGN.md` makes `Acked` generic over the command's output].

**Push, up to 10,000 refs atomically.** Receive-pack quarantines and indexes the pack, checks connectivity against the fresh core tier, and places it (`PackStore::place`, `design/DESIGN.md`). `commit` then runs `Push::decide`:

1. Each update's expected old value must equal the fold. If it does not, the ref gets `Stale`, which git shows as `fetch first`.
2. Rules come from the fold's `Rule` rows and the owner's rulesets.
3. `AllOrNothing` rejects every update when one fails. `PerRef` marks only the failing ref.

Admission caps a push at 10,000 refs [guess at GitHub's own limit]. A pack with zero objects names nothing.

**Size.** One command's encoding stays under `COMMAND_MAX` (`design/DESIGN.md`, "Shape", the write path). The edge encodes the push with its updates first. If an inline pack would take it over the cap, the edge stores the pack through `PackStore::place` instead. If the ref updates alone exceed the cap, the edge refuses the whole push before `commit`: every ref gets `ng <ref> push has too many ref updates, split it into smaller pushes` and a `remote:` line names the limit [guess at GitHub-style wording]. GitHub documents an optional per-repository cap on branch and tag updates in one push, off by default (https://docs.github.com/en/enterprise-server@3.15/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/managing-the-push-policy-for-your-repository).

Invariants:

- **Linearizable per chain, atomic per push.** A multi-ref push is one entry, so readers see all of it or none of it. Pushes to one repository serialize on its chain with group commit, which approximates GitHub's 2026 per-reference ordering (`research/git-storage.md#6`).
- **Idempotent retries.** The edge mints one `RequestId` per client request and re-sends with it after `NotLeader`. The entry maps the request id to its events, and the group's `request` table answers a duplicate from them. Git re-sends expected old values, so a push that git itself retries over a new connection gets `Stale` instead of applying twice.
- **Reachability.** Every ref target is reachable from the network's live packs, because the entry names its pack.
- **Compaction never drops an object.** It follows git's cruft-pack model. Upstream git writes reachable objects into a geometric pack with a bitmap and unreachable ones into a cruft pack with per-object times. It places both, then appends `Compacted`, so a push that lands during compaction stays live. `consumed` names stored packs only, including the packs that `Promoted` entries made from inline ones, and never an inline pack.
- **Inline packs are promoted before archive.** Before a chain archives a range, the objects of every inline pack the range names are placed as one stored pack, and a `Promoted` entry records the range and that pack. A git cache rebuilds a network from its stored packs plus the inline packs after each member chain's last `Promoted` (`design/DESIGN.md`, "Shape", Packs).
- **GC is the only deleter of objects.** Its roots are every member chain's fresh fold refs plus its `ref_log` table, which keeps every ref value of the last 35 days. It deletes an object only 14 days after the object leaves that set, at a capped rate. A deleted repository's fold keeps its refs until `retain_until`, so GC keeps its objects that long.
- **Orphan sweep.** After `Acked` the committing leader writes a create-only marker for each stored pack and blob its entry names. A sweeper deletes a pack or blob object that has no marker after 24 hours, once the chain named in the object's metadata confirms that its request never committed. That removes objects placed for a commit that failed, without scanning any chain. A commit is refused for a pack placed more than an hour earlier [guess], so that answer is final.
- **Write-once objects.** Every pack, snapshot, and archive object is written create-only under a key that is a ULID or a content digest named by an entry, never a chain position. Racing writers and restored timelines never collide on one key (`research/uptime.md#31`).
- **Maintenance cadence.** Compaction is scheduled by pack count and bytes, with geometric factor 2, and a full repack with bitmaps runs weekly above 1 GiB.

### Multi-chain operations

Every multi-chain operation names its commit point, its finisher, and its abort path.

- **Transfer.** The commit point is a `Reserved` `RepoName` on the destination owner's `Authz` chain, which `decide` treats as taken. `Settings` on the repository chain, carrying the new `owner` and its `enterprise`, the `Live` name, the old owner's `Alias`, and the repository's `RepoOwner` row on `Names` follow, and none of them can be refused. When the destination owner's placement record differs, the repository's group, and the network's group for a root repository, move to it through `Joint` before `Live` commits, so a route through either owner reaches a member that knows the config. `design/DESIGN.md` runs transfers as Workflows ("Stack").
- **Enterprise change.** The commit point is the org's `Authz` entry that names its new enterprise. A Workflow then appends `Settings` with that `enterprise` to each of the org's repository chains, and until it lands a repository's `deps` still name the old enterprise.
- **Fork.** The commit point is its `RepoName` on the fork owner's `Authz` chain. Whoever first touches the fork appends `ForkAdded` to the network chain, then `Genesis`, both idempotent by repository id. If `ForkAdded` meets a taken-down network, the finisher tombstones the fork's name instead.
- **Signup.** The commit point is the login claim, which carries the account record. The owner's two chains begin from it on first touch. `design/DESIGN.md` places login claims on its `Names` groups.
- **Credentials.** The commit point of a new PAT, OAuth token, or SSH key is its `Tokens` entry, which is also where an SSH fingerprint is unique. The edge appends the `Identity` side right after that commit.
- **Workflow run.** The commit point is `RunCreated` on the run's Actions shard, which `ActionsTrigger` appends at most once per (chain, index, workflow path) ("Actions runs and jobs"). `RunOwner` is the finisher. From its cursor on the shard it appends to the repository chain the check suite, one `CheckRun` per job, and `RequiredCheck { conclusion: None }` for each job whose check a rule requires. For each job that reaches `Completed` it appends the conclusion as a `CheckRun` and, when required, a `RequiredCheck`. Every repository entry is keyed by (job, attempt), so a replay after a crash appends nothing twice. `completejob` commits `Finished` on the shard first, then appends the repository entries itself before it answers, and `RunOwner` appends any that it did not.
  - The abort path is `Cancel`. When the repository chain refuses the run's entries, because the repository was deleted or taken down, `RunOwner` cancels every unfinished job. A cancel by a user or a concurrency group takes the same path. A cancelled job gets its conclusion entries wherever the repository still accepts them, and a refused repository has no merge left to wait.
  - No crash leaves a `RequiredCheck` with no conclusion. `RunOwner` writes `conclusion: None` only for a job that a committed `RunCreated` holds. Every such job reaches `Completed`, through `Finished`, `LeaseExpired`, `QueueExpired`, `NeedsFailed`, or `Cancel`. `RunOwner`'s cursor passes a `Completed` transition only after the repository chain acknowledges or refuses its conclusion entries.
- **Finisher.** The `Audit` consumer finishes any other chain operation that nobody touched within a minute, such as a credential whose edge died between the two commits.

### Names

```rust
/// 1..=39 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen [unverified as a rule].
pub struct Login(Box<str>); pub struct LoginKey(Box<str>);   // LoginKey: ASCII lowercase, the only form compared
pub struct Shortcode(ArrayString<8>);                         // an enterprise's managed-user suffix
pub enum OwnerName { Public(Login), Managed { base: Login, shortcode: Shortcode } }   // "<base>_<shortcode>"
/// 1..=100 of [A-Za-z0-9._-], not "." or "..", a trailing ".git" stripped [unverified exact GitHub limits].
pub struct RepoName(Box<str>); pub struct RepoNameKey(Box<str>);
/// Every first path segment the host serves, so no login can shadow a route. Generated from the router; CI fails on drift.
pub const RESERVED: &[&str] = &["api", "login", "logout", "session", "settings", "orgs", "organizations", "users", "repositories",
    "notifications", "search", "status", "site", "raw", "gist", "codeload", "uploads", "_services", "_t", "_codeload", "_registry" /* .. */];
pub struct AccountId(DbId); pub enum AccountKind { User, Org, Enterprise }
pub struct RepoId(DbId); pub struct NetworkId(DbId);   // a network id is its root repository's id
/// Bumped by the `Authz` entry that changes which names resolve to a repository: rename, transfer, alias overwrite.
pub struct NameEpoch(u32);
/// `Reserved` is a transfer's commit point and counts as taken. `Deleted` keeps the name until `retain_until`.
pub enum RepoEntry { Reserved { repo: RepoId, from: AccountId }, Live { repo: RepoId, epoch: NameEpoch },
                     Alias { repo: RepoId, epoch: NameEpoch }, Deleted { repo: RepoId, retain_until: Timestamp } }
```

- **One namespace, one key.** Users, orgs, and enterprises share one login per `LoginKey`. Logins are ASCII, so lowercasing is exact.
- **Repository names are `Authz` entries.** Creating, renaming, transferring, or deleting a repository is a `RepoName` entry on the owner's `Authz` chain. Its `decide` rejects a taken or reserved key with 422 `already_exists` (`research/wire-census.md#20`).
- **Repository renames.** A rename leaves `Alias` at the old key, which the API answers with 301 and git with 200 (`research/wire-census.md#23`).
- **Owner renames.** An owner rename keeps the old login pointing at the account as a former name, because GitHub frees an old username and keeps old repository links working (https://docs.github.com/en/account-and-profile/reference/username-reference). A new claim of the old login ends the redirect.
- **Retirement.** When an owner renames or is deleted, each public repository with more than 100 clones or Actions uses in the prior week, or with a Marketplace action, is retired under the old login before the old login can be claimed. Later claims of that owner and repository pair fail with GitHub's text (same page). A retired pair is kept forever.
- **Ids.** `DbId` is `i64` with 0 < id < 2^53 (`research/wire-census.md#28`), and no id repeats. `design/DESIGN.md` packs a cell, a block, and an offset into it and leases blocks to groups ("Shape", `DbId`), so a managed front routes an id by its cell. Issue numbers stay per repository.
- **Fork networks.** GitHub opens pull requests only within one network (https://docs.github.com/en/rest/pulls/pulls). Every fork joins its network, so a pull request from any fork touches chains of one network, and a fork merge touches one base chain.

## Identity and tokens

```rust
pub enum TokenKind { Pat, FineGrained, OAuth, UserToServer, Installation, Refresh }
// prefixes:          ghp_  github_pat_  gho_   ghu_          ghs_          ghr_
/// Stored tokens: 30 base62 chars plus a CRC32 in 6 (research/wire-census.md#29), about 177 random bits.
pub struct OpaqueToken { kind: TokenKind, secret: [u8; 30], crc: [u8; 6] }
pub struct UserId(AccountId); pub struct OrgId(AccountId); pub struct TeamId(DbId);
pub enum OrgRole { Member, Admin, BillingManager }
pub enum Grantee { User(UserId), Team(TeamId) }
/// Installation tokens, including the Actions GITHUB_TOKEN: "ghs_" + compact JWS. `kid` names the signing key epoch.
/// `task` is set only when the token request's additive `graftyard_task` field named a live Graftyard-run `AgentTask`,
/// the token's App is the task's App or the install's `graftyard-workspace`, and every repository in `repos` is the
/// task's repository or its workspace fork (`design/DESIGN.md`, Shape, agent block).
pub struct InstallationClaims { pub app: AppId, pub installation: InstallationId, pub repos: RepoScope,
    pub permissions: PermissionSet, pub task: Option<SessionId>, pub jti: Ulid, pub iat: Timestamp, pub exp: Timestamp,
    pub kid: Kid }
/// A web sign-in session on the `Identity` chain. Not the agent layer's `SessionId`.
pub struct WebSessionId(Ulid);
pub enum Credential { Stored([u8; 32]), Stateless(InstallationClaims), Ssh(Fingerprint), Session(WebSessionId),
                      RunnerJwt(RunnerId), RuntimeToken(JobId) }
pub enum Actor { User(UserId), App { app: AppId, installation: InstallationId }, Job { run: RunId, job: JobId },
                 Runner(RunnerId), Anonymous, SiteAdmin(UserId) }   // SiteAdmin: installs only, audited in the same entry
/// The authenticated caller.
pub struct Principal { pub actor: Actor, pub scopes: ScopeSet, pub credential: Credential,
                       pub sso_orgs: SmallVec<[OrgId; 2]> }   // from the token or the session
/// Install scope only, chosen at launch as GHES offers: built-in accounts, SAML, or OIDC, and LDAP at T1. An
/// `AuthzEvent::InstanceAuth` entry on the install's own `Authz` chain; secrets inside the configs are ciphertext.
pub enum InstanceAuth { Builtin, Saml(SamlConfig), Oidc(OidcConfig), Ldap(LdapConfig) }
/// An OAuth client id, 20 characters as GitHub's. GitHub Apps have one too.
pub struct ClientId(ArrayString<20>);
/// SHA-256 of a client secret. The secret is shown once and never stored. Two allow a rotation [guess].
pub struct SecretHash([u8; 32]);
/// Core tier of the owner's `Authz` chain. The edge reaches it through the `Names` route from the App id, slug, or
/// client id to the owner (`design/DESIGN.md`, Shape, Identity). The edge verifies an App JWT against `keys` before it
/// mints, and an installation's permissions never exceed `permissions`. The `Webhooks` consumer delivers `events` to
/// `webhook`, signed with `webhook_secret`, which is ciphertext as org webhook secrets are.
pub struct AppRecord { pub id: AppId, pub slug: Box<str>, pub name: Box<str>, pub client: ClientId,
                       pub keys: SmallVec<[AppKey; 2]>, pub permissions: PermissionSet, pub events: SmallVec<[EventKind; 8]>,
                       pub webhook: Option<Url>, pub webhook_secret: Option<Box<[u8]>>,
                       pub client_secrets: SmallVec<[SecretHash; 2]>, pub callbacks: SmallVec<[Url; 2]>, pub public: bool }
/// An App's RS256 public key. GitHub keeps the private half only with the App's owner.
pub struct AppKey { pub id: DbId, pub public: RsaPublic, pub created: Timestamp }
/// Core tier of the owner's `Authz` chain. `FirstPartyClient` ids have no record: their embedded secrets are ignored.
pub struct OAuthAppRecord { pub client: ClientId, pub name: Box<str>, pub callbacks: SmallVec<[Url; 1]>,
                            pub client_secrets: SmallVec<[SecretHash; 2]>, pub device_flow: bool }
pub struct InvitationId(DbId);
/// On the chain that owns the fact: the repository chain for an outside collaborator, the org's `Authz` chain for a
/// member. A pending invitation lapses at `expires`, 7 days after `Sent` [unverified for GitHub's repository
/// invitations], with no command. `Notify` puts pending invitations in the invitee's `Inbox` for
/// `/user/repository_invitations` and `/user/memberships/orgs`.
pub enum InvitationEvent {
    Sent { id: InvitationId, invitee: Invitee, grant: InvitationGrant, inviter: Actor, expires: Timestamp },
    Accepted { id: InvitationId }, Declined { id: InvitationId }, Cancelled { id: InvitationId, actor: Actor } }
pub enum Invitee { User(UserId), Email(Digest) /* org invitations only; the user who verifies the email accepts */ }
pub enum InvitationGrant { Repo(RepoPermission), Org { role: OrgRole, teams: SmallVec<[TeamId; 2]> } }
/// First-party GitHub clients, seeded once with their real client ids.
pub enum FirstPartyClient { GhCli, Gcm, VsCode, DesktopProd, DesktopDev }
impl FirstPartyClient {
    pub const fn client_id(self) -> &'static str { match self {
        Self::GhCli => "178c6fc778ccc68e1d6a", Self::Gcm => "0120e057bd645470c1ed",
        Self::VsCode => "01ab8ac9400c4e429b23", Self::DesktopProd => "de0e3c7e9973e1c4dd77",
        Self::DesktopDev => "3a723b10ac5575cc5bb9" } }
}
/// Node ids in GitHub's new format: type prefix + "_" + base64url(msgpack([0, ids..])); repo id first where scoped.
pub struct NodeId { pub ty: NodeType, pub ids: SmallVec<[DbId; 3]> }
/// Pure over the repository core tier and the owner's `Authz` fold, plus the enterprise's for an enterprise-owned org:
/// membership, teams, grants, base permission, SSO enforcement, installations, collaborators, visibility. Denial on a
/// private resource returns Hidden, rendered as 404 (research/wire-census.md#21). An anonymous git request for a
/// repository that is not visibly public gets 401 at once.
pub fn authorize(p: &Principal, a: ReadAction, authz: &[ReadView], repo: &ReadView) -> Decision;
pub fn authorize_write(p: &Principal, a: WriteAction, authz: &[Fresh], repo: &Fresh) -> Decision;   // never `Stale`
pub enum Decision { Allow, Forbidden, Hidden }
```

`design/DESIGN.md` adds `ActorKind::Agent` and the agent refusals in `decide` ("Shape", agent load and agent safety).

**Who owns which fact.**

- The `Authz` chain holds memberships, teams, repository grants, repository names, org policies, apps' installations with their repository selections, and the installation `not_before`. Org policies are base permission, SSO enforcement, residency, custom roles, rulesets, webhooks, and Actions secrets as ciphertext. It also holds the owner's App and OAuth app registrations (`AppRecord`, `OAuthAppRecord`) with their client secret hashes, org invitations, and, on the install's own `Authz` chain only, `InstanceAuth`.
- The `Identity` chain holds the account record, password hashes, second factors, sessions, SSO authorizations, and the identity audit log. It holds no client secret. Sign-in churn never touches what gates requests, and an org's authorization and its sign-ins never share a chain.
- Logins, emails, the routes from an App id, App slug, or client id to the owning account, installation owners, and each account's `Account` row (`suspended`, `sessions_not_before`) live in the `Names` groups. Token hashes, SSH key fingerprints, verifying keys, and the short-lived OAuth and sign-in codes live in the `Tokens` groups. An edge resolves all of them without an owner's chain (`design/DESIGN.md`, "Shape", Identity, and "Not needed here").
- **Collaborators and grants never overlap.** A user's direct permission on an org-owned repository is a `Grant` on the org's `Authz` chain while the user is an org member, and a `Collaborator` entry on the repository chain while the user is not. Every collaborator on a user-owned repository is a `Collaborator`. The edge picks the chain from the owner's `Authz` fold, read fresh, and a repository `decide` refuses a `Collaborator` write for a current member, because it reads the owner's `Authz` as a dependency. `authorize` takes the higher of a member's `Grant` and a `Collaborator` row left from before the user joined, so joining keeps the access the user had. Removing such a member's access writes both. Adding an outside collaborator sends a repository `Invitation`, and adding an org member sends an `Authz` `Invitation`. Each acceptance appends the grant in the same entry as `Accepted`.

**Authentication and sign-in.** The edge checks a token's prefix and CRC offline, then resolves it. Sign-in checks the password hash and second factor in the `Identity` fold, appends a session entry there, and signs a session cookie that the edge checks against the account's `sessions_not_before`. Suspension and sign-out-everywhere live on the `Account` row, which every write-class request reads fresh.

**Revocation.** Membership, team, grant, ruleset, and installation changes are `Authz` entries, and visibility and collaborator changes are repository entries. They bind every command decided after the acknowledgment. App uninstall bumps `not_before` on the `Authz` chain, so its stateless tokens fail at once. `design/DESIGN.md` sets the token revocation bounds: every write command decided after the revocation is refused, because the leader reads each command's credential shard once per entry, and reads honor a revocation within 10 s. Stateless installation tokens are revoked by `jti` ("Shape", Identity and write path, and journey 4).

**Stored tokens** (`ghp_`, `github_pat_`, `gho_`, `ghu_`, `ghr_`). Creating a PAT or finishing an OAuth flow stores the token's hash, never the token. A first use at any edge finds it at once. `design/DESIGN.md` shards them in `Tokens` groups.

**Installation tokens are stateless**, matching github.com since 2026-10-02 (`research/wire-census.md#30`).

- Each is "ghs_" plus a compact JWS. Signing keys are Ed25519.
- Tokens are about 400 characters and match `ghs_[A-Za-z0-9\.\-_]{36,}`, which Renovate accepts (`research/clients.md#38`).
- `authorize` intersects the claims with the installation's current selection in the `Authz` fold.
- Each job's `GITHUB_TOKEN` `jti` is recorded on the job and denied when the job ends.
- A new verifying key is published at least one token lifetime before it signs, and the old one retires after the last token's `exp`.
- `design/DESIGN.md` signs every installation token in `graft-ops` ("Shape", Identity, and "Install operations").

**SSO** attaches to orgs and enterprises, with SCIM provisioning at T1. The SAML or OIDC endpoint appends an `Sso` entry to the member's `Identity` chain and records the org in the token's or the session's `sso_orgs`. `authorize` checks the principal's `sso_orgs` when the owner's `Authz` policy enforces SSO. Break-glass owners are local accounts with hardware keys and no IdP dependency (`research/uptime.md` implication 25).

**Instance sign-in in installs.** An install chooses built-in accounts, SAML, or OIDC at launch, with LDAP at T1 (`InstanceAuth`). The choice is an `AuthzEvent::InstanceAuth` entry on the install's own `Authz` chain, the account that `Genesis` names, and only a `SiteAdmin` writes it. A `SiteAdmin` actor bypasses org and repository permissions, and every bypass is audited in the same entry as the action. `graftyard admin bootstrap` creates the first site admin with a one-time sign-in link that lives 15 minutes [guess]. The link is a `SignInLink` code row on a `Tokens` shard. `design/DESIGN.md`, "Install operations", says who mints it.

**OAuth.** Authorization code and device flow run on the bare host at `/login/oauth/*` and `/login/device/code`. First-party apps are seeded once with GHES-era callbacks: `http://localhost/` for gh and GCM, `http://127.0.0.1/` also accepted, and `x-github-desktop-auth://` for Desktop [unverified exact path]. They are public clients, so their embedded secrets are accepted and ignored (`research/clients.md#44`-`#47`). Every other client is an `OAuthAppRecord` or an `AppRecord`, found through its client id's `Names` route, and its secret is checked against `client_secrets`. Device codes, user codes, and authorization codes are TTL rows on `Tokens` shards keyed by the code's hash (`design/DESIGN.md`, "Shape", Identity). A device code expires after 900 s and an authorization code after 10 minutes, as on GitHub [unverified for GHES 3.22]. An exchange marks the code used before it stores the new token's hash, so one code never yields two tokens.

## Issues and pull requests

```rust
pub struct IssueNumber(u32);
pub enum IssueKind { Issue, PullRequest }
pub struct HeadSpec { pub repo: RepoId, pub branch: RefName }   // same network as the base repository
pub enum PrState { Open { draft: bool }, Closed { at: Timestamp, final_head: ObjectId },
                   Merged { at: Timestamp, by: UserId, merge_commit: ObjectId, final_head: ObjectId } }
/// Keyed by the SHA pair, so a stale result is visibly stale instead of needing invalidation.
pub struct Mergeability { pub base: ObjectId, pub head: ObjectId, pub result: MergeabilityResult }
pub enum MergeabilityResult { Clean { merge: ObjectId }, Conflict { paths: SmallVec<[Box<str>; 4]> } }
pub enum MergeMethod { Merge, Squash, Rebase }
/// Produced by the git cache. The tree-diff check has already passed when this exists (research/uptime.md#49).
pub struct MergePlan { pub base_old: ObjectId, pub head: ObjectId, pub commit: ObjectId, pub pack: DurablePack, pub method: MergeMethod }
/// impl Command with tiers() = Core, because review verdicts live in the core tier. `credential()` comes from
/// `actor.credential`.
pub struct Merge { pub n: IssueNumber, pub plan: MergePlan, pub expected_head: Option<ObjectId>, pub actor: Principal }
/// Pure. Reads the rule, `RequiredCheck`, and review-verdict rows from the core tier.
pub fn check_protection(rule: &BranchRule, pr: &PrView, required: &[RequiredCheckView], reviews: &[ReviewView]) -> Result<(), RuleViolation>;
pub enum MergeError { HeadModified, BaseModified, Protection(RuleViolation), NotOpen, Store(StoreError) }
// Every variant but Store renders GitHub's text and is never retried into success.
/// Graft 1, bounded. Commits a `CheckRun` entry. When a rule requires (name, app) and the fresh core tier shows the SHA
/// as the head of an open pull request, a merge-queue ref, or a protected branch tip, the same entry carries a
/// `RequiredCheck` keyed by (run, version), so a repeat changes nothing. `record_status` does the same for a `Status`.
pub async fn record_check(repo: RepoId, run: CheckRunUpdate, arrived: Instant) -> Result<(), CheckError>;
pub async fn record_status(repo: RepoId, status: StatusUpdate, arrived: Instant) -> Result<(), CheckError>;

// Stored formats of the `RepoEvent` payloads in "Event types". Each follows "Record formats and the two-release rule".
/// Captured at `IssueOpened`, and inside `PrCapture` at `PrOpened`. A title is at most 256 characters and a body at
/// most 65,536 [unverified GitHub limits].
pub struct IssueCapture { pub id: DbId, pub author: Actor, pub title: Box<str>, pub body: Box<str>,
                          pub labels: SmallVec<[LabelId; 4]>, pub assignees: SmallVec<[AccountId; 2]> }
/// Metadata tier, plus Core when it changes a pull request's `State` or `Draft`. `State` reopens a closed pull
/// request; closing one is `PrClosed`.
pub enum IssueChange { Title(Box<str>), Body(Box<str>), State { open: bool, reason: Option<StateReason> },
                       Draft(bool), Locked(Option<LockReason>) }
pub enum StateReason { Completed, NotPlanned, Reopened }
pub struct CommentId(DbId);
/// Metadata tier. Issue, review, and commit comments, with bodies of at most 65,536 characters [unverified]. A review's
/// comments ride in the entry of its `ReviewEvent`. `Resolved` also updates the core count of unresolved threads per
/// pull request, which `check_protection` reads when a rule requires conversation resolution.
pub enum CommentEvent {
    Created { id: CommentId, on: CommentTarget, author: Actor, body: Box<str>, reply_to: Option<CommentId> },
    Edited { id: CommentId, body: Box<str>, actor: Actor }, Deleted { id: CommentId, actor: Actor },
    Resolved { thread: CommentId, resolved: bool, actor: Actor } }
pub enum CommentTarget { Issue(IssueNumber), Review { n: IssueNumber, review: Option<ReviewId>, at: LineAnchor },
                         Commit { sha: ObjectId, at: Option<LineAnchor> } }
/// Where a review comment was written. Reads re-anchor it lazily against the current head.
pub struct LineAnchor { pub commit: ObjectId, pub path: Box<str>, pub line: u32, pub start_line: Option<u32>,
                        pub side: DiffSide }
pub enum DiffSide { Left, Right }
pub struct ReviewId(DbId);
/// `Submitted` touches both tiers: the core `review_verdict` row keeps (reviewer, state, commit), and the metadata
/// tier keeps the body. A `Pending` review is visible only to its author and is never a verdict.
pub enum ReviewEvent {
    Submitted { id: ReviewId, n: IssueNumber, reviewer: Actor, state: ReviewState, commit: ObjectId, body: Box<str> },
    Dismissed { id: ReviewId, message: Box<str>, actor: Actor },
    Requested { n: IssueNumber, reviewers: SmallVec<[Grantee; 2]>, removed: bool, actor: Actor } }
pub enum ReviewState { Pending, Approved, ChangesRequested, Commented }
/// A name is at most 50 characters [unverified] and compares case-insensitively within a repository.
pub struct LabelId(DbId); pub struct LabelName(Box<str>);
/// Metadata tier. `Defined` creates a label or renames and edits it by id.
pub enum LabelEvent {
    Defined { id: LabelId, name: LabelName, color: [u8; 3], description: Option<Box<str>> /* <= 100 chars */ },
    Deleted { id: LabelId },
    Applied { n: IssueNumber, add: SmallVec<[LabelId; 4]>, remove: SmallVec<[LabelId; 4]>, actor: Actor } }
/// Core tier, `Rule` entries, in the shape of GitHub's branch protection API. `pattern` matches branch names as
/// GitHub's fnmatch patterns do. A repository ruleset's branch rules compile to the same struct, and an org ruleset is
/// an `OrgPolicy` row on `Authz`. `protection: None` deletes the rule.
pub struct BranchRule { pub id: DbId, pub pattern: Box<str>, pub protection: Option<Protection> }
pub struct Protection { pub checks: Option<RequiredChecks>, pub reviews: Option<RequiredReviews>,
                        pub push: Option<SmallVec<[Pusher; 4]>> /* who may push; None is anyone with write */,
                        pub enforce_admins: bool, pub linear_history: bool, pub force_pushes: bool, pub deletions: bool,
                        pub block_creations: bool, pub conversation_resolution: bool, pub lock_branch: bool,
                        pub signatures: bool, pub merge_queue: Option<MergeQueueSettings> }
/// `strict` requires the head to contain the base tip. `app: None` accepts any source that reports the name.
/// Success, Neutral, and Skipped satisfy a required check, as on GitHub [unverified for GHES 3.22].
pub struct RequiredChecks { pub strict: bool, pub checks: SmallVec<[(Box<str>, Option<AppId>); 4]> }
pub struct RequiredReviews { pub count: u8 /* 0..=6 */, pub dismiss_stale: bool, pub code_owners: bool,
                             pub last_push: bool, pub dismissers: SmallVec<[Pusher; 2]>, pub bypass: SmallVec<[Pusher; 2]> }
pub enum Pusher { User(UserId), Team(TeamId), App(AppId) }
/// The `merge_queue` rule's parameters (https://docs.github.com/en/enterprise-server@3.22/rest/repos/rules):
/// `check_response_timeout_minutes`, `grouping_strategy`, `max_entries_to_build`, `min_entries_to_merge`,
/// `max_entries_to_merge`, `min_entries_to_merge_wait_minutes`, and `merge_method`.
pub struct MergeQueueSettings { pub check_timeout_minutes: u16, pub grouping: Grouping, pub max_build: u8,
                                pub min_merge: u8, pub max_merge: u8, pub min_wait_minutes: u16, pub method: MergeMethod }
pub enum Grouping { AllGreen, HeadGreen }
/// Core tier. Every `Settings` entry carries the whole record. `owner` and `enterprise` change only by a transfer or an
/// enterprise change ("Multi-chain operations"), and `Command::deps` reads them.
pub struct RepoSettings { pub owner: AccountId, pub enterprise: Option<AccountId>, pub visibility: Visibility,
                          pub default_branch: RefName, pub archived: bool, pub description: Option<Box<str>>,
                          pub homepage: Option<Url>, pub topics: SmallVec<[Box<str>; 4]>, pub has_issues: bool,
                          pub has_wiki: bool, pub has_projects: bool, pub merge_methods: SmallVec<[MergeMethod; 3]>,
                          pub delete_branch_on_merge: bool, pub allow_auto_merge: bool, pub allow_update_branch: bool,
                          pub check_retention_days: u16 /* 1..=400, default 90, as GHES 3.22 */ }
pub enum Visibility { Public, Private, Internal }
pub struct CheckRunId(DbId); pub struct CheckSuiteId(DbId); pub struct StatusId(DbId);
pub enum CheckStatus { Queued, InProgress, Completed, Waiting, Requested, Pending }
/// Check run and suite conclusions. An Actions job's `Conclusion` maps to the variant of the same name.
pub enum CheckConclusion { ActionRequired, Cancelled, Failure, Neutral, Success, Skipped, Stale, TimedOut,
                           StartupFailure }
/// The run's whole state after one create or update. `version` counts updates, and the highest version wins.
pub struct CheckRunUpdate { pub run: CheckRunId, pub suite: CheckSuiteId, pub app: AppId, pub head_sha: ObjectId,
                            pub name: Box<str>, pub version: u64, pub status: CheckStatus,
                            pub conclusion: Option<CheckConclusion>, pub started: Option<Timestamp>,
                            pub completed: Option<Timestamp>, pub external_id: Option<Box<str>>,
                            pub details_url: Option<Url>, pub output: Option<CheckOutput> }
/// `summary` and `text` are at most 65,535 characters [unverified]. Annotations append, at most 50 per update, as
/// GitHub's API takes them.
pub struct CheckOutput { pub title: Box<str>, pub summary: Box<str>, pub text: Option<Box<str>>,
                         pub annotations: Vec<CheckAnnotation> }
pub struct CheckAnnotation { pub path: Box<str>, pub start_line: u32, pub end_line: u32, pub level: AnnotationLevel,
                             pub message: Box<str>, pub title: Option<Box<str>> }
pub enum AnnotationLevel { Notice, Warning, Failure }
pub struct CheckSuiteUpdate { pub suite: CheckSuiteId, pub app: AppId, pub head_sha: ObjectId,
                              pub head_branch: Option<RefName>, pub version: u64, pub status: CheckStatus,
                              pub conclusion: Option<CheckConclusion> }
/// The latest status per (sha, context) is that context's state. A required one maps Success to Success, Error and
/// Failure to Failure, and Pending to `None` in its `RequiredCheck`.
pub struct StatusUpdate { pub id: StatusId, pub sha: ObjectId, pub context: Box<str>, pub state: StatusState,
                          pub target_url: Option<Url>, pub description: Option<Box<str>>, pub creator: AccountId }
pub enum StatusState { Error, Failure, Pending, Success }
/// SHA-256 of the bytes, stored under `R2Key::Blob { repo, oid }` (`design/DESIGN.md`, Shape, "Packs"). LFS objects
/// and release assets use it. GC deletes a blob RESTORE_WINDOW after the last row that names it is deleted.
pub struct BlobRef { pub oid: Digest, pub bytes: u64, pub sites: SiteSet }
/// Built only when the bytes are in the buckets of two sites in two regions, each put create-only, as `DurablePack`.
pub struct DurableBlob { blob: BlobRef, _sealed: () }
pub struct RepoReleaseId(DbId); pub struct AssetId(DbId);
pub struct ReleaseRecord { pub tag: RefName, pub target: ObjectId, pub name: Box<str>, pub body: Box<str>,
                           pub draft: bool, pub prerelease: bool, pub latest: bool, pub author: Actor }
pub struct AssetRecord { pub name: Box<str>, pub label: Option<Box<str>>, pub content_type: Box<str>, pub blob: BlobRef }
```

`design/DESIGN.md` adds `MergeQueueEntry` with the `parent_tip` rule, the `QueueEvent` variants, the `QueueOp` and `QueueMerge` commands, and the `MergeQueue` consumer ("Shape", "Merge queue"), and lazy `TestMerge` under a budget ("Shape", agent block).

**Same-repository merge.**

1. The plan reads the fresh fold: the pull request, head ref, base ref, reviews, rule, and required checks.
2. The git cache runs `merge-tree` and `commit-tree`, and checks that the result equals the base plus the pull request's own diff (`research/uptime.md#49`, implication 19).
3. `Merge::decide` re-checks on the fold it is given:
   - Head moved: `Head branch was modified. Review and try the merge again.`
   - Base moved: `Base branch was modified. Review and try the merge again.`
   - A new review, rule, or required conclusion: the protection error.
4. It emits one `Merged` entry, with head-branch deletion in `also`.

A 409 is never retried into success, because a rerun of `decide` re-runs protection (`history/arena/round4/candidate-3/DESIGN.md`, "Commit and read paths"). Each `Merged` entry records `engine` and the pre-merge base.

**Required checks are race-free and bounded** (`history/arena/round4/SYNTHESIS.md`, graft 1).

- A conclusion for a check that a branch rule requires is also a `RequiredCheck` entry on the repository chain. A merge's `decide` sees every required conclusion committed before it, and one committed after it is a check of a merged pull request.
- Commit statuses that a rule requires take the same path, so a Jenkins-style status cannot race a merge either.
- Only conclusions for SHAs that are heads of open pull requests, merge-queue refs, or protected branch tips are appended. `apply` prunes the row when the pull request closes or its head moves, so the core tier stays small. Every check run and status is also a metadata-tier `CheckRun` or `Status` entry, which no merge reads.
- If a rule change newly requires a check whose conclusion already exists, the merge planner appends that conclusion as a `RequiredCheck` entry before it plans, idempotent by run and version. A merge never waits on a check that already ran [inference: GitHub counts existing runs].
- Pinning check versions at plan time instead would reopen the race between a failing required check and a merge.

**Fork merge.** The head is the fork's chain, in the same network. The plan proves it fresh and pins the head SHA, and GitHub's `sha` parameter, when given, must match. The `Merged` entry goes on the base chain alone. A push to the fork between plan and commit is not merged, and the pull request closes merged at the reviewed head. That is a valid ordering of the merge before that push (`history/review/round4-judge.md`, Correctness).

**Merge queue.** `Queue` events on the base chain record each enqueue, each group built with its temporary `gh-readonly-queue/*` refs, each ejection, and each landing. When a group's required checks pass on its queue refs, one `QueueMerge` entry carries a `Merged` per pull request and the base fast-forward, each tree diff verified separately, so a group lands whole or not at all. The base rule's `MergeQueueSettings` set the group sizes, the wait, and the check timeout. `design/DESIGN.md` adds that each queue entry's `parent_tip` must equal the previous entry's `group_tip`, checked in `decide`, so queue tips only fast-forward, and it names the `MergeQueue` consumer that drives the queue ("Shape", "Merge queue").

**Branch-rule changes.** A repository rule is a `Rule` entry on the same chain as pushes, so a push decided before it is refused and re-decides under it. An org ruleset is an `Authz` entry, and every push reads the owner's `Authz` chain fresh as a dependency of its `decide`.

**Pull request state never diverges from refs in any way that decides anything.**

- An open pull request's head is the head branch's ref in the fork's fresh core tier.
- A merge sets the base ref and `Merged { final_head }` in one entry.
- `refs/pull/N/head` and `refs/pull/N/merge` are derived refs. The `PrSync` consumer moves `refs/pull/N/head` within seconds in a coalesced bot `Refs` entry, ordered below user commands in the entry. A `TestMerged` entry moves `refs/pull/N/merge` each time a lazy test merge runs.
- Issues and pull requests share one number space. An issue's number comes from the fold that `IssueOpened`'s `decide` saw, so numbering and labels commit together.
- Comments, reviews, and timeline events are entries. Review comments anchor to `(commit, path, line)` and re-anchor lazily on read.
- Comments and reactions share the chain under a per-repository cap of 50 entries a second, so a viral issue cannot starve pushes. Stars stay off the repository chain. Check runs, check suites, and statuses are metadata-tier entries that `apply` prunes after `RepoSettings.check_retention_days`.

## Folds

A fold is a SQLite database derived from one chain's entries. It is the query index for every list, filter, and sort. `design/DESIGN.md` keeps both tiers in each member's SQLite ("Shape").

```rust
/// Graft 5. Each repository chain has two folds. `Authz`, `Identity`, and network chains have only a core tier.
pub enum Tier { Core, Metadata }
/// Core: ref, rule, collaborator, deploy_key, pull_head(n, head, base, state, agent_app), review_verdict(n, reviewer, state,
/// commit), required_check(sha, name, conclusion) pruned when its pull request closes or its head moves, settings, and the
/// agent rows that `decide` reads: agent_task(session, kind, state, prefix, requester, app), lease, agent_gate, and
/// agent_policy (`design/DESIGN.md`, Shape, agent block). Metadata: issue, issue_label, issue_assignee, issue_mention, pull,
/// review bodies, comment, timeline, reaction, and the agent rows nothing gates on: a task's prompt, summary, and pr,
/// path_set, provenance, and the activity feed's ring. The entry's `tiers` tag says which tiers must decode it.
pub fn tier_of(e: &RepoEvent) -> TierSet;   // Merged, PrOpened, and Review touch both tiers; Refs only Core; Comment only Metadata
/// The only writer. Refuses an entry whose index is not applied + 1 or whose `prev` digest is wrong. A tier applies the
/// events it owns and advances `applied` past every entry, so both tiers check the same digest chain.
pub fn apply(fold: &mut Fold, e: &LogEntry) -> Result<(), FoldError>;
/// Refolds sampled chains from genesis on each release and daily, compares `fold_digest`, and alarms on any mismatch.
pub async fn fold_audit(sample: &[ChainId]) -> Result<AuditReport, FoldError>;
```

**Two tiers keep git off the metadata path.** The source design's top flaw, found by its judge, was that every repository-scoped list, pull request page, and authorization needed one warm fold, so a giant repository's git and authorization could wait on a 2 GB load or a refold storm (`history/review/round4-judge.md`, "Most serious flaw"). Graft 5 splits the fold.

- The core tier holds what git, merges, and `authorize` read: refs, rules, collaborators, deploy keys, pull request heads, review verdicts (reviewer, state, and commit), required checks, and settings. On a repository with agents it also holds the agent rows that pushes and merges check: each `AgentTask`'s core part, live leases, `AgentGate`, and `AgentPolicy`. An `AgentApp` flag is an `Authz` row, which has only a core tier. It has one row per ref, rule, collaborator, open pull request, current verdict, open task, and live lease [inference: tens of MB for the largest repositories, measured in Milestone 0]. It loads first and alone gates fetches, pushes, merges, and authorization.
- The metadata tier holds issues, comments, review bodies, labels, and the timeline. It loads lazily the first time a list or pull request page needs it.
- A core request, a merge included, never waits for a metadata load. A metadata schema change never touches git.

**Snapshots and archive.** Each tier is snapshotted at one index, and a new reader loads the newest snapshot and then the tail. One follower, chosen by the snapshot's index, writes each snapshot as a stream of row chunks while it keeps applying entries, after a fixed number of entries or log bytes and at least daily on a day the chain commits. A member whose next entry has been archived installs the newest snapshot and the archives after it before it follows again. Entries older than 30 days move to immutable archive objects, so every fold can be rebuilt from genesis. Snapshots carry their schema, release, and fold digest. A release marked bad has its snapshots ignored, and its folds are refolded from the archive. `design/DESIGN.md` supplies the writer, the cadence, the frame format, `InstallSnapshot`, snapshot placement, the archive keys, and `LOG_MAX_BYTES` ("Shape", Packs, and "Scale").

**List endpoints.**

- The metadata tier has tables `issue(number, kind, state, author, milestone, created, updated, comments, locked)`, `issue_label`, `issue_assignee`, `issue_mention`, `pull(number, head, base, draft, ...)`, `review`, and `comment`, each with indexes per GitHub filter.
- `GET /repos/{o}/{r}/issues` maps `milestone`, `state`, `assignee`, `creator`, `mentioned`, `labels` (all must match), `since`, `sort` (created, updated, comments), and `direction` to one indexed SQLite query.
- Pulls add `head`, `base`, and `sort=popularity|long-running`.
- GraphQL `filterBy` and `orderBy` use the same queries with keyset cursors.
- The metadata tier also holds SQLite FTS5 tables over issue and pull request titles, bodies, and comments (`design/DESIGN.md`, "Stack"). `apply` updates them with the other metadata tables, so repository issue and pull request search has no lag behind the fold.
- Cross-repository lists (`/issues`, `/user/repos`, notifications) come from the per-account `Inbox` and, like cross-repository search, lag by seconds, as GitHub's search does [inference] (`history/arena/round4/candidate-3/DESIGN.md`, "List endpoints").

**Schema change is a refold, never a migration.** No table schema exists to migrate. A fold schema change ships in a release that reads both schemas and refolds lazily per chain. The old schema's file keeps serving until the new one catches up [inference]. A core-tier schema change ships only in a release dedicated to it, because every fetch, push, merge, and authorization reads the core tier, and the core tier reads both schemas until each chain's refold is done. `design/DESIGN.md` rolls refolds one site at a time ("Downtime").

## Actions runs and jobs

```rust
pub struct RunId(DbId); pub struct JobId(DbId); pub struct RunnerId(DbId);
pub enum JobState {
    Pending { needs: SmallVec<[JobId; 4]> },
    Queued { labels: LabelSet, since: Timestamp },
    Assigned { runner: RunnerId, lease_until: Timestamp },
    InProgress { runner: RunnerId, lease_until: Timestamp },
    Completed { conclusion: Conclusion, at: Timestamp },
}
pub enum Conclusion { Success, Failure, Cancelled, Skipped, TimedOut, StartupFailure }
pub enum JobEvent { NeedsMet, NeedsFailed, Matched(RunnerId), Acquired, Renewed, Finished(Conclusion), LeaseExpired,
                    QueueExpired, Cancel }
/// Pure transition table. Each transition commits against the job's current version.
pub fn transition(s: &JobState, e: &JobEvent, now: Timestamp) -> Result<JobState, IllegalTransition>;
/// Run status is derived from job states by the run's single owner (the `RunOwner` consumer), never written by jobs.
/// `RunOwner` records each change as a status flip on the run's Actions shard (`design/DESIGN.md`, `ActionsEvent`).
pub enum RunStatus { Queued, InProgress, Completed(Conclusion) }
pub fn derive_run_status(jobs: &[JobState]) -> RunStatus;
pub struct WorkflowRun { pub id: RunId, pub repo: RepoId, pub workflow: WorkflowRef, pub head_sha: ObjectId,
                         pub trigger: (ChainId, Index), pub attempt: u16, pub concurrency: Option<ConcurrencyKey> }
/// One job as run creation records it, before any state change. `needs` names jobs of the same run. `matrix` is this
/// job's row of the expanded matrix, empty for a job with no matrix. `labels` are the lowercased `runs-on` labels.
/// `environment` names the deployment environment whose secrets the job reads after approval. A job with no `needs`
/// starts `Queued`, and every other job starts `Pending`.
pub struct JobSpec { pub id: JobId, pub name: Box<str>, pub labels: LabelSet, pub needs: SmallVec<[JobId; 4]>,
                     pub matrix: SmallVec<[(Box<str>, MatrixValue); 4]>, pub environment: Option<EnvName> }
/// Who may take a job: runners registered to one repository, or to the whole owner, an org or a user. GitHub has no
/// user-level runners, so a user's `Owner` scope holds only hosted runners ("Actions", Hosted runners).
pub enum RunnerScope { Owner, Repo(RepoId) }
/// Written by registration or a JIT config. `key` verifies the runner's JWT assertions (`Credential::RunnerJwt`).
/// `hosted` marks a runner that the hosted-runner service started. An ephemeral runner's record is deleted after its job.
pub struct RunnerRecord { pub scope: RunnerScope, pub name: Box<str>, pub labels: LabelSet, pub key: RsaPublic,
                          pub ephemeral: bool, pub hosted: bool, pub version: RunnerVersion }
/// Opened by the runner's `POST {broker}/session` and closed by its `DELETE`. Both are committed entries, so any broker
/// object accepts a session that another one opened.
pub struct RunnerSessionId(Ulid);
/// Wire type, private to the Actions service. Built from job state, never stored.
struct AgentJobRequestMessage { /* plan, timeline, steps as template tokens, context data, variables, SystemVssConnection */ }
pub trait Broker {
    async fn poll(&self, session: RunnerSessionId, wait: Duration) -> Option<RunnerMessage>;
    async fn acquire(&self, runner: RunnerId, job: JobId) -> Result<AgentJobRequestMessage, AcquireError>;
    async fn renew(&self, job: JobId) -> Result<Timestamp, RenewError>;
    async fn complete(&self, job: JobId, result: JobResult) -> Result<(), CompleteError>;
    /// Hosted-runner demand, answered by the owner's `Owner`-scope broker ("Actions", Hosted runners).
    async fn demand(&self) -> HostedDemand;
}
/// Per hosted label: the queued jobs no runner holds, the runners to start, and the hosted runners to destroy.
/// `start` counts already-registered idle hosted runners against `queued` and stays under the owner's cap. Each
/// `start` entry names its registration scope: the org for an org, or the repository for a user-owned repository.
/// `stale` lists hosted runners idle for 10 minutes [guess] after registration, or whose job completed.
pub struct HostedDemand { pub labels: Vec<(Box<str>, LabelDemand)>, pub stale: Vec<RunnerId> }
pub struct LabelDemand { pub queued: u32, pub oldest: Option<Timestamp>, pub start: SmallVec<[(RunnerScope, u16); 2]> }
```

Job state lives in the owner's Actions groups, which `design/DESIGN.md` defines ("Shape", `ChainId::Actions`, `ActionsEvent`, and `actions_shard`, and "Failure modes", `actions state`).

Invariants:

- **One runner at a time.** `Queued -> Assigned` commits against the job's version, so one runner wins.
- **Leases.** A lease that expires, with no `renewjob` for 10 minutes, moves the job to `Completed { Failure }` with GitHub's infrastructure-failure category, or back to `Queued` if it was never acquired. A lease is extended only by a recorded stall (`design/DESIGN.md`, `ExtendLeases`), never by idle time, so a dead runner's job fails at its lease end.
- **Few writes per job.** A job writes only on state changes, and the lease is rewritten only when less than 5 minutes remain. A 2-hour job writes it about 24 times, not 120.
- **Idempotent run creation.** A run is created at most once per `(chain, index, workflow path)`. One `RunCreated` event carries the run and every `JobSpec`, so no reader sees a run without its jobs, and `decide` refuses a second one for the same key. A 256-job matrix at a few hundred bytes a job fits in one entry [inference]. A re-run commits `RunCreated` again with the same `RunId`, `attempt + 1`, and fresh `JobId`s for the jobs it reruns.
- **Queue limit.** A job `Queued` for 24 hours gets `QueueExpired` and completes with `Failure`, as GitHub ends a self-hosted job that waits that long [unverified: the limit and the conclusion]. The leader's lease alarm covers it, so no job waits forever.
- **Completion.** `completejob` commits `Finished` on the Actions shard. The check-run conclusion and the `RequiredCheck` entry follow on the repository chain as "Multi-chain operations", Workflow run, says, each keyed by (job, attempt).
- **Isolation.** If Actions state is impaired, job dispatch stops and merges that require checks wait. Git, other merges, and the API keep serving.

Label matching: the `RunnerBroker` of each (owner, runner scope) (`design/DESIGN.md`, Shape, Identity) keeps an in-memory table of the distinct label sets of the queued jobs that its runners may take, each with its hash and, per shard, the oldest such job. A `Repo` scope reads one shard, its repository's. An `Owner` scope reads all 16 of the owner's shards. A broker loads each shard's queued summary when it starts. After that it applies the hint that each shard's leader sends after a commit that moves a job into or out of `Queued`. Each hint carries the shard's count of such moves, and the broker reloads a shard whose hint skips a count or that sent none for 30 s [guess]. A runner's poll tests each queued set for being a subset of the runner's labels in memory, then asks the shard that holds the oldest matching job to commit `Matched(runner)` against that job's version, and only a committed match becomes the runner's message. A lost hint delays a job by at most the reload interval and never assigns it twice. Labels are lowercased at registration and job creation [unverified that GitHub matches labels case-insensitively]. Check suites shard 16 ways above 256 runs.

## Actions

**Workflow engine.** The server owns triggers and filters, matrix expansion, the `needs` graph, job-level `if`, concurrency groups, reusable workflows and `secrets: inherit`, environments and approvals, `GITHUB_TOKEN` minting and `permissions`, OIDC, label matching, job outputs, and retention (`research/actions-protocol.md#62`). The parser is a Rust port of the runner's MIT `GitHub.Actions.WorkflowParser`, including `MatrixBuilder`, with `workflow-v1.0.json` as data and `@actions/workflow-parser` as a differential oracle (`#59`, `#60`). The template-token and context-data JSON converters are ported byte-exact (`#19`). Triggering is a chain consumer (`ActionsTrigger`), so a run exists only if its push or pull request entry committed. The consumer reads `.github/workflows/*.yml` at the pushed SHA from the git cache.

**Runner protocol.** Graftyard returns `use_v2_flow: true` and implements runner admin registration, the broker, the run service, the launch service, and the results service (`research/actions-protocol.md#5`, `#6`, `#12`, `#16`, `#17`, `#22`).

- The legacy distributedtask family is frozen and needs the VSS location service. The broker family is where GitHub invests, and the launch service is the one place Graftyard controls which bytes run for each `uses:` (`#17`).
- Each Graftyard release pins one tested runner version. It sends `RunnerRefresh` to older runners with a download URL that the install itself serves from the actions pack's runner tarballs.
- Each release publishes the next runner floor one release ahead, with a warning to runners below it, and refuses versions below the current floor with `RunnerVersionTooOld` (`#25`).
- JIT configs always set `GitHubUrl` and `IsHostedServer=false` (`#2`).
- ARC scale-set endpoints (`_apis/runtime/runnerscalesets`, sessions, message queue, `acquirejobs`, `generatejitconfig`) ship at T1 against a pinned ARC range (`#30`).

**Toolkit hostname heuristics and SHA pins.** Graftyard is GHES to the toolkit, and nothing on the server can change that, because `isGhes()` reads only `GITHUB_SERVER_URL` (`research/actions-protocol.md#43`, `#44`). The T0 contract is the GHES contract: cache v1 for `actions/cache@v4+`, legacy artifacts for `upload-artifact@v3.2.2` and `download-artifact@v3`, and the runner protocol.

- Graftyard never alters the bytes of an action. The launch service resolves every `uses:` to the exact upstream bytes, from the install's `actions` mirror org. The mirror holds byte-for-byte copies of upstream tags and commits. Managed syncs it directly, and installs load it from the signed actions pack.
- For `upload-artifact@v4+`, `download-artifact@v4+`, `upload-pages-artifact`, and `deploy-pages`, the vendor publishes forks under `graftyard-actions/*`. They differ from upstream only in the `isGhes()` test, which also trusts `GRAFTYARD_ACTIONS=true`, set by every job.
- CI builds each fork release reproducibly from the upstream tag, and regenerates a signed **pin map** from upstream SHA to fork SHA for every upstream release, so the map is derived, not hand-kept.
- Substitution is an org policy with two settings. With tag substitution on (the managed default for orgs migrating from github.com), `actions/upload-artifact@v4` resolves to `graftyard-actions/upload-artifact@v4`. SHA pins are substituted only when an org admin has approved the vendor's signed pin map, and then only for SHAs that appear in it.
- Every substitution prints one log line naming both references and both SHAs, and the job's OIDC token and run metadata record the bytes that ran.
- An unapproved SHA pin to `upload-artifact@v4+` fails exactly as it does on GHES. Its annotation links the pin-map approval page and names the fork SHA that matches the pinned upstream SHA, so a team can pin the fork directly (graft from `history/arena/candidate-2`).
- This beat Gitea's JavaScript rewriting (`#57`), because rewriting changes the bytes behind a SHA without anyone approving it and depends on string patterns in 118 bundles.
- An install loads action packs with `graftyard actions import`, and an in-install sync job pulls new packs when the install has outbound access, as GHES's `actions-sync` does [unverified]. The pack carries an allowlist of third-party actions, and an admin can add any public action to the install's `actions` mirror [unverified that GHES's `actions-sync` allows the same].

**Hosted runners.** Managed hosted runners run unmodified `actions/runner`, one job per sandbox, destroyed after the job. A runner gets a JIT config and reaches Graftyard only through the public front, exactly like a customer's runner, so a compromised runner holds only that job's runtime token, `ghs_` token, and OIDC capability. The runner's environment has no trust into and no route to Graftyard's own accounts other than the front. `design/DESIGN.md` runs them on Sandboxes in a separate runner account ("Stack" and "Deployment modes").

- The hosted-runner service learns of work only from the `Owner`-scope broker's `demand`, served at `GET /api/graftyard/v1/owners/{owner}/actions/hosted/demand`. It authenticates as the install's `graftyard-hosted-runners` App, installed on each owner that enables hosted runners with self-hosted-runner write permission.
- A `workflow_job` webhook to that App, with retries on, prompts a demand read. A sweep of the App's installations every 5 minutes [guess] catches a lost webhook.
- For each `start` entry it calls `generate-jitconfig` for the named scope with the hosted label, and starts a sandbox that runs the runner with that config. It destroys the sandbox when the runner exits or appears in `stale`, then deletes the runner's registration.
- Each owner runs at most 20 hosted runners at once [guess], unless its `OrgPolicy` sets another cap. `decide` refuses a hosted registration past the cap, so two overlapping demand reads never start more runners than the cap. A job whose labels match no hosted label waits like a job with no runner, until the queue limit.

Every value in the hosted label table is a [guess]. Hosted runners are Linux x64 only [guess: the Sandbox CPU architecture is unchecked].

| Hosted label | vCPU | Memory | Disk |
| --- | --- | --- | --- |
| `ubuntu-latest`, `ubuntu-24.04` | 2 | 8 GiB | 16 GB |
| `ubuntu-22.04` | 2 | 8 GiB | 16 GB |
| `ubuntu-24.04-4core` | 4 | 12 GiB | 20 GB |

**Cache.** Cache v1 (`_apis/artifactcache`) at T0, and twirp `CacheService` v2 at T1 with the Azure dialect shim that accepts Put Block, Put Block List, and Put Blob and returns `x-ms-request-id` (`#35`, `#39`). Downloads are presigned blob-store URLs (`#36`). Entries are scoped by repository and ref, with GitHub's restore order (current ref, then default branch), and LRU eviction per repository at 10 GB.

**Artifacts.** Legacy pipelines artifacts (`#40`) at T0. Twirp `ArtifactService` v4 at T1, with uploads through the block shim that maps 8 MiB blocks onto multipart parts in order, and downloads by 302 to presigned blob-store URLs (`#37`, `#38`).

**Logs.** The results service twirp with Graftyard-signed URLs that accept `comp=appendblock` and `seal=true` (`#22`, `#23`). Blocks land as chunk objects, which are compacted per step. Live logs stream over the `FeedStreamUrl` WebSocket and `WorkflowStepsUpdate`. The job message's `FeedStreamUrl` names the job's live-log relay. The relay takes the runner's WebSocket and sends each line to every web viewer's WebSocket. It keeps only a short tail for viewers that join late and stores nothing, because the step logs already land as blobs. A viewer that finds no runner stream on its relay reads the step log blobs instead. If the relay's socket breaks, the runner reconnects or drops live output [unverified], and its uploads through the results service lose nothing. `design/DESIGN.md` places the relay (`LogRelay`).

**Secrets and variables.** `gh secret set` seals each value to the owner's public key with libsodium's `crypto_box_seal`, and Graftyard stores the sealed bytes as sent [inference from the `rest:actions` row, "secrets (sealed box)"]. Repository and environment secrets and variables are `RepoEvent::ActionsSecret` and `ActionsVariable` entries. Org secrets and variables, and the owner's key pairs, are `OrgPolicy` rows on the owner's `Authz` chain in the same formats.

```rust
/// One X25519 key pair per owner. `GET .../actions/secrets/public-key` returns the newest unretired `key_id` and
/// `public`. The private half is stored only wrapped.
pub struct SealKey { pub key_id: Box<str>, pub public: [u8; 32], pub private: Wrapped, pub created: Timestamp,
                     pub retired: Option<Timestamp> }
/// AES-256-GCM under the wrapping key slot that `slot` names (`design/DESIGN.md`, `SecretName::SecretsWrapKey`).
pub struct Wrapped { pub slot: u8, pub nonce: [u8; 12], pub ciphertext: Box<[u8]> }
/// Upper-case letters, digits, and underscores, not starting with `GITHUB_` [unverified as GitHub's exact rule].
pub struct ActionsName(Box<str>);
pub enum SecretScope { Repo, Env(EnvName), Org { visibility: SecretVisibility, repos: SmallVec<[RepoId; 4]> } }
pub enum SecretVisibility { All, Private, Selected }
/// `sealed` is the `crypto_box_seal` output the client sent, under the owner's key `key_id`.
pub struct SecretRecord { pub sealed: Box<[u8]>, pub key_id: Box<str>, pub updated: Timestamp }
pub struct VariableRecord { pub value: Box<str>, pub updated: Timestamp }
```

- **Who decrypts.** Two components decrypt to use a value. Job-message assembly at `acquirejob` opens the secrets the job may read: its repository's, its environment's once approved, and the org secrets visible to its repository. A job for a pull request from a fork gets none, as on GitHub. Webhook signing unwraps a hook's secret (`HookRecord.secret`) or an App's `webhook_secret`, which is `Wrapped` bytes too, to compute the HMAC. `design/DESIGN.md` puts the wrapping key behind `graft-ops`, which also runs rotation ("Shape", `SecretName`).
- **Rotation.** A new `SealKey` is minted every 90 days [guess] and on demand. Each secret sealed to an older key is opened and resealed to the new one inside the rotation job, which returns no plaintext. Then the old key's `retired` is set, and the key is dropped once no record names it. The wrapping key has two slots. Its rotation rewraps every `Wrapped` value under the new slot, then clears the old slot.

**OIDC.** The issuer is `https://HOST/_services/token`, which in managed mode is `https://graftyard.dev/_services/token` for the whole service. The claim set and legacy `sub` format are GHES's, with the `OWNER@ID/REPO@ID` format as an opt-in (`#41`, `#42`). Private signing keys never leave the signer, and the JWKS under the issuer publishes every current and previous key epoch.

**Coping with GitHub changing the toolkit.** The runner is pinned per release. A nightly conformance job runs the newest runner, the newest major of each first-party action, and the newest `@actions/cache` and `@actions/artifact` against a canary, and opens a ticket on the first failure. Fork rebuilds and pin-map regeneration run in the same job. If GitHub merges a vendor-neutral escape, such as trusting `ACTIONS_RESULTS_URL` (toolkit issue #2439, `#51`), the forks become unnecessary and the substitution policy is retired.

## Event fan-out

Consumers read chains, never a separate outbox, so an event exists if and only if its change committed. `design/DESIGN.md` delivers events from leader cursors through Queues to consumer Workers ("Shape" and "Failure modes", `event pipeline`).

```rust
pub enum Consumer { Webhooks, ActionsTrigger, RunOwner, PrSync, MergeQueue, Notify, Search, Audit, Counts, Insights }
/// Generated from the GHES 3.22 webhook schema. Each event captures at commit every mutable field its payload needs.
/// `issue` holds the title, body, author, labels, and assignees, because a pull request is also an issue.
pub struct PrCapture { pub head_sha: ObjectId, pub base_sha: ObjectId, pub state: PrState, pub labels: Vec<LabelName>,
                       pub issue: IssueCapture, pub changes_from: Option<Changes> }
pub static SUBSCRIPTIONS: &[(EventKind, &[Consumer])] = &[/* .. */];   // only deployed consumers subscribe
/// One automatic attempt, as on GitHub; manual redelivery for 3 days (research/wire-census.md#37). Rows live in the
/// hook owner's delivery store, keyed by `guid`, and `expires` is 3 days after the last attempt.
pub struct HookDelivery { pub guid: Uuid, pub hook: HookId, pub entry: (ChainId, Index, u16), pub attempts: SmallVec<[Attempt; 4]>,
                          pub expires: Timestamp }
/// Core tier of the repository chain (`RepoEvent::Hook`). `secret` is `Wrapped` as "Actions", Secrets and variables,
/// says, and only webhook signing unwraps it. `retry` opts in to automatic retries ("Known deviations from GitHub").
pub struct HookRecord { pub url: Url, pub events: SmallVec<[EventKind; 8]>, pub content_type: HookContentType,
                        pub active: bool, pub insecure_ssl: bool, pub secret: Option<Wrapped>, pub retry: bool }
pub enum HookContentType { Json, Form }
/// Webhook attempts in flight at once per hook owner, compiled [guess].
pub const HOOK_OWNER_CONCURRENCY: u16 = 50;
/// The `Webhooks` consumer's cursor per chain. `read` passes an entry once its `HookDelivery` rows are recorded.
/// `replay_from` trails at the oldest entry with a delivery not yet finished, its one attempt or its retries.
pub struct HookCursor { pub read: Index, pub replay_from: Index }
```

- **Order.** Events reach each consumer in chain order, which is commit order. Each hook gets one chain's webhooks in chain order, so a webhook for a repository never arrives before an earlier one from the same chain to the same hook.
- **Idempotence.** Each consumer handles each event idempotently by (restore generation, chain, index, event position in the entry).
- **Delivery identity.** The `X-GitHub-Delivery` GUID derives from (restore generation, chain, index, event position, hook). A redelivery after a crash keeps its GUID, and a restore never reuses one (`research/wire-census.md#36`). The restore generation changes with each whole-install restore [inference: `design/DESIGN.md` restores through a new genesis, "Install operations"].
- **Nothing is lost while a consumer is down.** A cursor advances only after its work is recorded, so a consumer outage of any length only delays.
- **Isolation.** Each consumer has its own cursors, so a stuck consumer holds only its own work. A handler that fails 10 times on one entry parks that chain for that consumer, and an operator command replays it. Oldest pending work and parked chains per consumer are SLO signals and deploy gates.
- **Webhooks.** The consumer renders each payload from the entry's captured fields, so a delayed `pull_request.synchronize` still shows its own head SHA. For each event it records one `HookDelivery` row per matching active hook in the hook owner's delivery store, then advances `read`. The cursor never waits on a delivery attempt. `design/DESIGN.md` makes the store a `HookSequencer` object per hook owner ("Shape", Identity).
- **Webhook order.** The store's sequencer keeps one queue per (chain, hook) and runs at most one attempt at a time on each queue, so order holds per (chain, hook). Different hooks, and one hook's queues for different chains, move independently. A retried delivery holds its queue until it succeeds or its 3 days end.
- **Webhook attempts.** An attempt POSTs with a 10 s deadline. Each delivery gets one automatic attempt, as on GitHub (`research/wire-census.md#37`). A hook with `retry` set gets automatic retries with backoff for 3 days under the same GUID. At most `HOOK_OWNER_CONCURRENCY` attempts run at once per hook owner, picked round robin across queues. Hooks whose last attempt failed or timed out share at most half of those slots.
- **A slow hook delays only itself.** It holds one slot per queue for at most 10 s, and the owner's other hooks keep at least 25 slots. No other hook's queue and no cursor waits on it.
- **A lost store loses no delivery.** A delivery store lost with its site is rebuilt by replaying each cursor from `replay_from`. That redelivers finished deliveries in the range under their own GUIDs, which is at-least-once delivery.
- **Hook management.** The consumer queues a `ping` delivery when it reads a new `Hook` entry, and `POST .../hooks/{id}/pings` queues one with no chain entry. The deliveries list, `.../deliveries/{id}`, `.../attempts`, and manual redelivery read and write the store.
- **Other consumers.** `PrSync` writes bot `Refs` entries and, after a head push, the `TestMerged` entry of the lazy test merge. `MergeQueue` builds queue groups, ejects failures, and lands groups (`design/DESIGN.md`, "Shape", "Merge queue"). `ActionsTrigger` commits each run's `RunCreated` on its Actions shard. `RunOwner` records run status and writes the repository's check suites, check runs, and required checks ("Multi-chain operations", Workflow run). `Notify` fills each account's `Inbox`, pending invitations included, and `Search` feeds the search index. `Counts` writes coalesced star and watcher counts as bot entries. `Insights` is the agent layer's (`design/DESIGN.md`, agent block). `Audit` finishes multi-chain operations that nobody touched within a minute. The edge writes the `Identity` side of a new credential itself, so `Audit` appends it only when the edge died between the two commits. A consumer whose target is unreachable pauses without counting attempts.

## Record formats and the two-release rule

No stored record has a schema, index, or DDL to migrate, which removes GitHub's index, migration, and dropped-column incident classes (`research/uptime.md#44`).

Every stored format that outlives a release follows a two-release rule (graft from round 1's candidate 2). That covers entry bodies, event kinds, snapshot framing, stored webhook payloads, and every other persisted record. It also covers every message one component sends another: commands and read requests from the edge, and member-to-member calls. Those are sent by one release and read by another during every rollout.

1. Release N learns to read format k+1.
2. Release N+1 starts writing it only after the format is recorded as enabled.
3. A format is enabled only when every component that can read the chain reports a release that reads it.
4. Once a format is enabled, swap-back below the reading release is refused and pages.

Each entry carries its tier tag outside the encoded body. A release refuses an entry in a format not yet enabled, and the tag only lets the core tier apply an entry without decoding its metadata body, so the core tier never stalls on a metadata schema. CI decodes release N+1's corpus with release N and fails on any record N cannot read. A release record's `phase` (enable, retire, or none) says which step it takes ("Release registry and signing").

For messages, rule 2 means a caller sends a new command, message kind, or field only after its format is enabled, so a caller on N+1 still speaks format k to a receiver on N. A receiver that cannot decode a message changes nothing and answers a typed `Unsupported` that names its release. The edge renders it as a retryable 503 with `Retry-After`, never as a refusal of the request (`design/DESIGN.md`, "Shape", the write path).

`design/DESIGN.md` enables formats through `FormatsEnable` in the `Release` group after every site, spare canary, and registered lifeboat runs N+1, adds `verify` on followers, and wires rule 4 and `rollbackBarrier` into the `Release` group, which refuses and pages on any rollback, pointer, or `Abort` to a release that cannot read an enabled format ("Shape" and journey 7).

## Repository repair, tombstones, and retention

**Repair from the chain** (`history/arena/round4/SYNTHESIS.md`, graft 2).

- `graftyard admin repo-restore <repos> --refs <refs> --to <t>` appends one `RestoredTo { to, refs }` entry to each named repository chain, at the last index whose `at` is at or before `t`. Its `decide` reads the targets from the chain's `ref_log` and checks that each is reachable from live or cruft packs.
- `apply` returns those refs to their targets at `to` and leaves every other ref and the metadata tier alone. The `Refs`-shaped difference emits `push` events.
- Any `to` within the last 35 days restores. A restore needs no downtime for other repositories.
- `graftyard admin repo-reset <repo> --to <t>` is the separate whole-fold reset. Its `ResetTo { to }` entry makes both tiers their state at `to`, by loading the newest snapshot at or below `to` and replaying to it. Entries between stay in the chain as history.
- Both start as customer-signed `Release` entries, `RepoRestore` and `RepoReset`, not intents, so the vendor cannot sign them. `design/DESIGN.md` turns each into an idempotent command keyed by the `Release` index ("Shape", Release group, and "Install operations"). In managed, a restore is a support action under the cell's customer key.
- `ref_log` keeps 35 days of ref values as GC roots, each tier keeps a snapshot at or below every index in the window, and the vault keeps packs 35 days past GC (`design/DESIGN.md`, "Shape", Packs).

**Tombstones.**

- Deleting a repository appends `Deleted { retain_until }` to its chain, 90 days by default, and turns its name entry `Deleted`. Deleting an org appends `Deleted { retain_until }` to its `Authz` chain.
- Undelete is chain-local. `graftyard admin undelete` is an audited site-admin API call, not an intent. It appends `Undeleted` and restores the name entry or login before `retain_until`.
- A deleted repository's fold keeps its refs until `retain_until`, so GC keeps its objects that long.
- A deleted repository or org loses nothing within 90 days, and comes back in minutes [guess].

**Retention.**

| What | Kept | Source |
| --- | --- | --- |
| Deleted repositories and orgs | 90 days by default | `Deleted { retain_until }` |
| Objects outside every member chain's refs and `ref_log` | 14 days, then GC deletes them at a capped rate | "Refs and pushes" |
| Pack objects that no entry names | 24 hours, then the orphan sweeper deletes them | "Refs and pushes" |
| Chain entries in the live log | 30 days, then archive objects, so every fold rebuilds from genesis | "Folds"; `design/DESIGN.md` adds `LOG_MAX_BYTES` |
| Check runs and statuses | 90 days, settable from 1 to 400, as GHES 3.22 does | "Issues and pull requests", `RepoSettings.check_retention_days` |
| Webhook deliveries | Manual redelivery for 3 days | `research/wire-census.md#37` |
| Actions caches | LRU per repository at 10 GB | "Actions" |
| Retired owner and repository pairs | Forever | "Names" |
| Repair window for `RestoredTo` and `ResetTo` | 35 days | `design/DESIGN.md`, "Downtime" |

## Release registry and signing

Graftyard ships one signed release in every mode. `design/DESIGN.md`, "Deployment modes", lists its Cloudflare artifacts. The platform-independent contents are:

- the web assets;
- the record formats the release reads and the formats it may enable;
- the fold schemas;
- the release `phase`;
- the next runner floor;
- a signed actions pack with the `actions/*` mirror, the `graftyard-actions/*` forks, the pin map, and runner tarballs.

**Keys.** The vendor's release key signs only releases, action packs, and pin maps. Each install's own keys sign that install's intents and config. `design/DESIGN.md` names them ("Install operations", Keys).

**The registry.** The vendor's existing release registry and signing machinery transfer unchanged (`research/byoc-grounding.md`, item 1). Its facts:

- A release is keyed by the SHA-256 of `release.json` (`research/byoc-grounding.md`, finding 12).
- The release record holds `builtAt`, `ciRunId`, `compatibility { comparedWith, reasons, rollbackBarrier }`, an optional digest-pinned deployer image, per-file SHA-256 and size, `label`, `publishedAt`, `releaseManifestSha256`, and `sourceCommit`. The signature is ECDSA P-256 (`ECDSA_SHA_256`) over the raw record bytes (finding 13).
- Every registry write is create-only, and nobody may overwrite or delete. Only the publisher may write or sign (finding 14).
- Retention is by tag. Development bundles expire after 180 days. `release retain` retags a release `retention=customer`, which never expires. Records and signatures never expire (finding 15).
- Rollback is a deploy to an older release that was verified on that install, is not expired, and has no barrier on its `comparedWith` chain. A gap in the chain blocks it. `rollback-check` runs the same rule (finding 34).

**What Graftyard adds to the registry.**

- Graftyard publishes under its own key, or its own product prefix, so one product's publisher cannot sign for another (`research/byoc-grounding.md`, implication 1).
- The release record gains `product` and `phase` fields (implication 12). The phase (enable, retire, or none) feeds the compatibility check, so only a retire phase sets `compatibility.rollbackBarrier` (implication 9). Recovery after a barrier is roll-forward only.
- `comparedWith` is computed per release line, so a customer pinned at N-2 keeps its rollback (finding 34).
- Every retire-phase release is retagged `retention=customer` (finding 15).

**Release timing per mode.**

| Mode | Release timing | Rate limits |
| --- | --- | --- |
| Managed | Weekly vendor waves, canary first. Inside an install or cell, `design/DESIGN.md` rolls one site a day | On |
| BYOC | Waves after the managed soak. The customer may pin up to N-2, and security fixes take an expedited lane | Off by default |
| Self-hosted | The customer pulls | Off by default |

**While the vendor is unreachable.** The install keeps its release and every release it already holds, and no upgrade or fix arrives. Action packs already imported keep resolving, and new upstream tags wait for the next pack. The approved pin map keeps working, and new upstream SHAs get no fork mapping until the next pack. `design/DESIGN.md` states that no runtime path calls the vendor ("Deployment modes").

## Public service concerns

**Anonymous access and rate limits.**

- Anonymous callers get public reads only, keyed by source IP (the /64 for IPv6).
- Anonymous REST allows 60 requests per hour (`research/wire-census.md#18`). GraphQL requires authentication, as on GitHub [unverified]. Anonymous git allows 600 negotiations per hour and 8 concurrent per IP [guess].
- Authenticated limits follow GitHub's numbers: 5,000 REST requests per hour per user, 1,000 per repository for `GITHUB_TOKEN`, 5,000 GraphQL points per hour, and secondary limits of 100 concurrent requests and 80 content-creating requests per minute (`#18`, `#35`).
- Limits do not multiply with the number of serving processes. One `RateCounter` object per key holds the window's total, and each isolate adds its own count every 20 requests or 1 s [guess] (`design/DESIGN.md`, Shape, Identity). So `x-ratelimit-remaining` overstates by at most 20 requests, or one second of requests, for each other isolate serving that key, and enforcement errs the same way. GitHub documents its own counts as approximate, with per-region variation (`#18`). A lost counter refunds at most one window, and an isolate that cannot reach the counter counts locally against the full limit.
- Excess gets 429 before any git byte. Under load, a repository admits authenticated traffic first and sheds anonymous requests first, with 503 and `Retry-After`.
- Identical public clones are served from a response cache keyed by wants, haves, and capabilities, so a clone storm at one tip costs reads instead of `pack-objects` CPU [inference: identical full clones dominate anonymous traffic]. `design/DESIGN.md` supplies it as the clone cache ("Shape").

**Abuse and spam.**

- Signup sits behind limits per IP and per ASN, with email verification and 24 hours of reduced content-creation limits for new accounts.
- Repository creation and forks are capped per account per hour, because each writes the owner's `Authz` chain.
- Suspension is one `Account` write. Every write-class request reads that row fresh, so the next write fails, and reads fail within the token read bound of `design/DESIGN.md`. Content is hidden at render time.

**Takedowns.**

- A repository takedown is a `Takedown` entry on its chain. Every request for that repository reads it through the fresh core tier, so it applies at once to git, the API, raw files, and archives.
- A whole fork network's takedown is a `Takedown` entry on the network chain. First-touch completion copies it onto every member chain in parallel, and each member stops serving when its entry commits [inference: minutes for the largest networks].
- Country-scoped takedowns return 451 [unverified that GitHub uses 451], using a GeoIP table at the edge.
- An account takedown is a suspension.

**Public content behind a CDN.** Managed enables subdomain isolation (`research/wire-census.md#33`). Only immutable public bytes are cached: raw files and archives keyed by commit SHA, release assets, and avatars. The repository's core tier is checked first, then the request gets a 302 to a signed URL that lives 10 minutes and carries the repository's `content_epoch`. A takedown or a switch to private bumps the epoch in its own entry, so no new URL is issued and old ones expire within 10 minutes, with no cache invalidation on the request path. Git smart HTTP and the APIs are never cached. Public repositories advertise `bundle-uri` for clients that enable it, which default clients do not [inference], so the clone response cache carries anonymous clone load.

**Residency.** An org or enterprise with residency keeps its owner chains, all its networks, its Actions runners, and its search documents in its residency domain. `design/DESIGN.md` supplies the residency placements (`Us2w`, `Eu2w`, `Apac2w`, "Shape") and puts search documents in the domain's own `SearchShard` objects, under the domain's DO jurisdiction.

- A fork into a resident org, of a network that lives outside the residency, is created detached: a new network inside the residency, objects copied, and `fork: true`. A pull request from it back upstream gets 422 with a message that names the residency rule.
- A fork of a resident network by anyone joins that network.
- A transfer that would move a resident network's owner outside its residency, or bring an outside network into a resident org, gets the same kind of 422 ("Known deviations from GitHub").
- Resident orgs' Actions secrets are ciphertext on the org's `Authz` chain.
- Everything on the owner's `Identity` chain stays in the residency domain: password hashes, TOTP seeds, recovery codes, WebAuthn credentials, sessions, plaintext email addresses and their verification state, IdP configuration and SCIM tokens, sign-in history with IP addresses, and the identity audit log. App and OAuth app client secret hashes live on the owner's `Authz` chain, which is placed in the same residency domain.
- In managed, the `Names` and `Tokens` groups are placed on three continents, so a login, a token hash, and an SSH key fingerprint leave the residency domain [inference from `design/DESIGN.md`, "Not needed here" and "Shape", Identity]. An enterprise that needs every identity fact in one geography, logins and token hashes included, gets its own namespace through BYOC, as GitHub offers `*.ghe.com`.

**Enterprise managed users.** An enterprise claims a shortcode, and the claim reserves the whole login suffix `_<shortcode>`. Managed users get `<base>_<shortcode>` logins that only the enterprise's SCIM endpoint can create, one claim per enterprise rather than one per user. A public login cannot collide, because public logins contain no underscore [inference from GitHub's EMU naming].

## Frontend and native app

**Web.** React 19 with Primer React, the stack GitHub itself ships (`research/clients.md#62`-`#64`).

- The app reads data through the same public GraphQL and REST endpoints that clients use, with persisted queries, plus a small private `/_t/ui/*` API for diffs, file trees, and blame computed by the git cache.
- Dogfooding the public API means a UI regression is often a wire regression caught by the same oracles.
- There is no Node server-side rendering tier, because that would add a runtime and a stateful-looking fleet to every mode.
- The edge serves the HTML shell. For the first route, it runs that route's persisted GraphQL query and inlines the JSON, so first paint needs no second round trip.
- Static assets ship inside the edge build and are served with immutable cache headers.

**Staying up when backends degrade.**

- The shell and assets come from the edge, so the UI loads whenever the edge does.
- Each page region (file tree, diff, checks, timeline, search box) is its own error boundary with its own query, so a search outage empties the search box and nothing else (`research/uptime.md#49`).
- The client retries only idempotent reads, with a retry budget and jitter, and it never polls faster than `x-poll-interval`.
- Long diffs virtualize lines, following GitHub's 2026 diff work (`research/clients.md#62`).

**GPUI native app.** It does not earn a place at launch. Unmodified gh, GitHub Desktop, VS Code, and JetBrains already cover native workflows against a GHES-shaped host, and that compatibility is the product. GPUI is pre-1.0 with frequent breaking changes and is tied to Zed's repository (`research/clients.md#57`-`#59`). One narrow scope could justify it later: an offline-capable review inbox for very large pull requests, if the web diff misses its latency target on pull requests above 10,000 changed lines. That decision waits for measurements from the web app, and the app would be a pure API client with no private endpoints.
