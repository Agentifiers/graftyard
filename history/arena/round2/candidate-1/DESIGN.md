# Network-placed cells under a replicated global tier: global facts commit in one DSQL cluster and every request reads them from local replicas

## Problem

The operator made managed Tributary one public namespace (`FRAME.md`, "Operator decision, 2026-10-24"). Three kinds of fact now span the service. A login is unique across users, orgs, and enterprises. A repository name is unique per owner and must resolve at any edge. An authorization fact (a membership, team, grant, credential, or installation) can be needed by any request in any cell, because any user may fork, push to, review, or merge in any repository they can reach. `DESIGN.md` put all three inside a tenant and each tenant inside one cell (`DESIGN.md:208-213`), so none of its mechanisms decides a global fact.

Two GitHub failures make the shape non-obvious. On February 9, 2026 a cache TTL cut from 12 h to 2 h plus a tenfold read increase from two client releases overloaded GitHub's core auth and user-management database (`research/uptime.md#47`). In May 2023 expensive writes on a shared permissions cluster broke GitHub App tokens for about 12 hours (`#44`). Any design in which requests read one store of global facts synchronously inherits both: that store's load tracks request rate and every client release, and every cache knob moves load onto it. The opposite move, partitioning identity and repositories by owner, breaks the public workflow, because `bob/widgets` and `acme/widgets` would sit in different cells and the merge that moves `acme`'s base ref and reads `bob`'s head ref could no longer be one transaction (`DESIGN.md:439`).

This design separates writing a global fact from reading it. One multi-Region Aurora DSQL cluster, `global`, commits every name, account, credential, org, membership, team, grant, app, and installation. Each data cell holds a replica of the authorization graph inside its own `core` cluster, and each front cell holds a projection of routes and credentials on local disk. Both are fed by `global`'s gap-free change log, which they pull. No request reads `global`. Its read load is a fixed set of followers, one per data cell and one per front cell, so a client release that multiplies reads lands on replicas that scale with the fleet. Repositories are placed by fork network. GitHub opens pull requests only between repositories of one network (https://docs.github.com/en/rest/pulls/pulls, "cross-repository pull requests in the same network"), so every pull request's head and base live in one cell's `core`, and the one-transaction merge holds for every pull request. Stars, notifications, repository cards, and per-account indexes move through pull-based inter-cell logs.

## Usage (caller's view)

```sh
gh auth login --hostname tributary.dev                 # GHES-classified: not github.com, *.ghe.com, or github.localhost
git clone https://tributary.dev/torvalds/linux.git     # anonymous, public
gh repo fork acme/widgets --clone && cd widgets
git switch -c fix && git commit -am fix && git push origin fix
gh pr create --repo acme/widgets --head bob:fix --fill
```

gh treats `tributary.dev` as GHES and derives `/api/v3/` and `/api/graphql` (`research/clients.md#1`, `#3`). Names used below: a **front cell** is a Regional fleet of `edge` nodes with its own NLB and a projection on local disk, and it holds no canonical state. A **data cell** is `DESIGN.md`'s cell (`core`, `aux`, buckets, node fleet). **`global`** is the one DSQL cluster for global facts. The **replica** is the `g_*` tables in each data cell's `core`. Each account has an **index cell** that holds its stars, watches, notifications, and feed.

**Journey 1: anonymous `git clone https://tributary.dev/torvalds/linux.git`.**

1. Route 53 latency records, health-checked (data plane, `research/uptime.md#15`), return the nearest Region's front cells as multivalue answers, one per front cell NLB.
2. The NLB passes TCP to a front `edge`, which terminates TLS with the service certificate from memory. No `Authorization` header, so the principal is `Anonymous`, keyed by source IP (the /64 for IPv6).
3. Front rate limits for anonymous callers: REST at 60 per hour (`research/wire-census.md#18`), git at 600 negotiations per hour and 8 concurrent per IP [guess], held as per-node approximate counters, which GitHub documents as acceptable (`#18`). Excess gets 429 before any git byte.
4. Route from the projection on local disk: login key `torvalds` gives account 1024, `(1024, linux)` gives `Live(repo 2325298)`, the repo gives network 2325298, the network gives data cell `c3` at generation 7. Four point lookups, no network I/O.
5. The edge forwards over PrivateLink to `c3`'s internal NLB in the same Region (for a Paired cell, its nearer Region) with a `Principal` assertion signed by the front cell, stamped `asked_at`.
6. `c3`'s `git` role reads one `core` snapshot: `network_state` (Serving), the repository row (public, no takedown), and the 40 shard versions (`DESIGN.md:287-291`). A public repository needs no grant read. Upstream `upload-pack` runs on the cache. `uploadpack.packObjectsHook` points at a response cache on NVMe keyed by the request's wants, haves, and capabilities, so a clone storm at one tip costs disk reads, not `pack-objects` CPU [inference: identical full clones dominate anonymous traffic].
7. When the repository's upload-pack concurrency is full, the rendezvous set widens (`DESIGN.md:319`) and anonymous requests are shed first with 503 and `Retry-After`.

Stores: front projection (local disk); `c3` `core` (`network_state`, `repository`, `ref_shard`, `ref`); `c3` NVMe; S3 packs on a cache miss. Synchronous dependencies outside `c3`: the front cell carrying the connection, and nothing else. `global` is not touched.

**Journey 2: `bob` forks `acme/widgets`, pushes, opens a pull request, and a maintainer merges.**

1. `POST /api/v3/repos/acme/widgets/forks`. The front checks the `gho_` prefix and CRC offline, looks up the token hash in the projection (account `bob`, scopes), and routes by `acme/widgets` to network N's cell `c7`.
2. `c7`'s `api` authorizes read on `acme/widgets` from its `core` snapshot (public). It opens one `global` transaction on its writer pool: insert `repo_name(bob, widgets) = Live(R2)` and `repo_dir(R2) = { owner: bob, network: N, visibility: public }`, appending both to bob's log partition. A taken name gets GitHub's numbered suffix [unverified exact rule]. `R2` comes from `c7`'s leased id range, so no extra round trip.
3. `c7` then commits one `core` transaction that creates repository R2 in network N and copies branch ref rows in chunks of 1,000 (only the default branch with `default_branch_only`). It returns 202, as GitHub's fork API does. If `c7` crashes between steps 2 and 3, `c7`'s `global-apply` sees a `repo_dir` row placed in `c7` with no local rows and creates them. The applier is the one owner of that convergence.
4. `git push origin fix` to `bob/widgets`. If the front's projection has not yet seen R2, the miss is not trusted: the edge waits until the watermark of bob's partition passes `asked_at` (about 1 s), then routes to `c7`. `c7` authorizes write (bob owns R2, read from `g_repo_dir` in the same snapshot) and runs `commit_push` (`DESIGN.md:272-285`) into network N.
5. `gh pr create --repo acme/widgets --head bob:fix` routes by the base repository to `c7`. The head repository is in network N, as GitHub requires. The PR row, the head ref row `(R2, fix)`, and the base ref row live in `c7` `core`. Notifications to `acme`'s watchers leave through `c7`'s inter-cell log (journey 4).
6. A maintainer runs `gh pr merge`. The GraphQL `mergePullRequest` names the PR by node id, which carries the repository id first (`DESIGN.md:361-364`), so the front routes it to `c7`. The merge is `DESIGN.md`'s transaction, unchanged (`DESIGN.md:423-436`): compare-and-set the head ref `(R2, fix)`, the PR row, and `acme/widgets`'s `main`, all in `c7` `core`.

Where the invariant holds: everywhere. Every pull request's head and base share a network, a network lives in one cell, so the merge is one `core` transaction for every PR, including every cross-owner PR. What weakens is authorization freshness, not merge atomicity. The maintainer's write permission comes from `g_team_member` and `g_repo_grant` in the replica, read in the same snapshot as the refs, and the replica trails `global` by the apply lag (target p99 under 1 s). A grant committed before the request is never missed (the denial rule in "Identity, orgs, and authorization"). A removal committed within the last second can still allow one merge. Branch rules, reviews, and check runs are `c7`'s own rows and stay exact. Synchronous dependencies outside `c7`: step 2 writes `global`, so forks fail with 503 while `global` is unreachable. Steps 4 to 6 touch nothing outside `c7` except the front.

**Journey 3: two users race to create `newco`, then `newco/a` becomes `newco/b`.**

1. Both send the web form (GHES admins can also use `POST /admin/organizations`). The front routes identity writes to the caller's index cell. Each cell's `identity` handler opens one `global` transaction: insert `name("newco") = Live { account: O, kind: Org }`, `account(O)` with `index_cell` set to the handling cell, and `membership(O, caller, Admin)`. The primary key on `name.key` lets exactly one commit. The loser gets a unique violation or SQLSTATE 40001, retries on a fresh snapshot, sees the row, and returns 422 with `errors[].code = "already_exists"` (`research/wire-census.md#20`). No replica decides the race.
2. Within about 1 s every data cell's applier and every front publisher have the claim. A `GET /orgs/newco` that reaches a front still behind waits on the watermark, then routes.
3. `gh repo create newco/a` is an identity write in `newco`'s index cell `c9`: one `global` transaction claims `repo_name(O, a)` and writes `repo_dir(Ra)` with a new network placed in `c9`; then `c9` `core` creates the repository. 201.
4. `PATCH /api/v3/repos/newco/a` with `{"name":"b"}` routes by `(O, a)` to `c9`. `c9` authorizes admin from the replica. One `global` transaction sets `repo_name(O, a) = Alias(Ra)`, inserts `repo_name(O, b) = Live(Ra)` (a collision returns 422), and sets `repo_dir(Ra).name = "b"`. Cells store no repository name. They render `full_name` from `g_repo_dir`, so the name has one source of truth. When `c9`'s applier applies the change, it inserts the `repository` `renamed` outbox event in the same transaction as the replica rows, so the webhook fires when `c9` starts rendering the new name.
5. `GET /api/v3/repos/newco/a`: the front finds `Alias(Ra)` and forwards to `c9` with `via_alias`. `c9` authorizes read. A permitted caller gets `301` with `Location: https://tributary.dev/api/v3/repositories/<Ra>` and `{"message":"Moved Permanently","url":...}`. Anyone else gets 404, so an alias never confirms a private repository (`research/wire-census.md#21`, `#23`). `git ls-remote https://tributary.dev/newco/a.git` resolves the same alias under the git route class, and `c9` serves the advertisement with 200 (`#23`). Creating a new `newco/a` later overwrites the alias with a `Live` entry, which breaks the redirect as on GitHub (`#23`).

Synchronous dependencies outside the serving cell: steps 1, 3, and 4 write `global`. Steps 2 and 5 read only the front's projection and `c9`.

**Journey 4: `alice` belongs to `acme` (repositories in `c7`), `beta` (`c12`, EU residency), and `gamma` (`c20`). Her index cell is `c4`.**

1. `gh repo list` sends a GraphQL query rooted at `repositoryOwner(login: "alice")` [inference on gh's exact query]. The front routes `/api/graphql` to `c4`. `c4`'s executor sees an account root, which any cell answers from the replica plus the card replica, and runs it locally.
2. `GET /api/v3/user/repos` routes to `c4`. One `c4` `core` snapshot reads `g_repo_dir` by owner `alice`, `g_repo_grant` for alice and for her teams (`g_team_member`), and `g_membership` for `acme`, `beta`, and `gamma` with each org's base permission and `g_repo_dir` by those owners, then joins `repo_card` for `pushed_at`, `stargazers_count`, and the other listed fields. Sorted, paginated, with `Link` headers. No other cell is read. Cards trail their repository cells by at most 60 s of coalescing.
3. GraphQL `viewer { repositories(first: 20) { nodes { name pullRequests(states: OPEN) { totalCount } } } }`: `name` comes from `c4`. `pullRequests` lives in each repository's cell, so the executor batches by cell and sends sub-requests carrying alice's principal to `c7`, `c12`, and `c20`. This is the journey's one synchronous cross-cell dependency. A dead cell yields `data` for the others plus an `errors` entry with the path of its nodes.
4. `PUT /api/v3/user/starred/beta/tool` routes by repository to `c12`. One `c12` `core` transaction inserts `star(repo, alice)`, bumps one of 16 sharded counter rows, and emits `Starred`. The `IndexFeed` consumer appends `StarChanged { alice, repo, on: true, version }` to `c12`'s inter-cell log partition addressed to `c4`. `c4` pulls it within 200 ms and upserts `starred(alice, repo)` if its version is newer. `GET /user/starred` on `c4` lists it. `CardPublish` later folds the new count into `c12`'s card partitions.
5. A comment mentioning alice lands on `gamma/svc#12` in `c20`. `c20`'s `Notifications` consumer computes recipients from `c20` rows (watchers, participants, mentions), groups them by `g_account.index_cell`, and appends `Notify { alice, thread, reason: Mention }` to the partition for `c4`. `c4` writes the thread into its `aux`, sends email through SES, and serves `GET /notifications`.

Synchronous dependencies outside the serving cell: only step 3's federated sub-requests.

**Journey 5: GitHub App `deploybot` (app 77) installed on `acme` (installation 501, repositories in `c7`) and `beta` (installation 502, repositories in `c12`).**

1. `POST /api/v3/app/installations/501/access_tokens` with the app's RS256 JWT. The front verifies the JWT against app 77's public keys in the projection and routes installation-scoped requests to the installation account's index cell (`acme`'s, `c7`). `c7` reads `g_installation(501)` (not suspended, permissions, repository selection) from its replica and mints a stateless `ghs_` JWS signed with its own key epoch, `kid = c7/14`. Nothing is written anywhere.
2. `GET /api/v3/repos/acme/api/pulls` with that token: the front verifies the signature with `kid`'s public key, `exp`, and installation 501's `not_before`, all from the projection, and asserts `App { 77, 501 }`. `c7` `authorize` intersects the claims with `g_installation_repo` and the installation's current permissions in the same `core` snapshot (`DESIGN.md:378`).
3. The same flow for 502 mints in `beta`'s index cell and calls `c12`. A JWS from one cell's key verifies at every front, because verifying keys for every cell are in every projection.
4. Uninstalling from `beta` is a `global` write that bumps installation 502's `not_before`. It returns once every follower covers it, at most 5 s, after which no front accepts an older token.

Synchronous dependencies outside the serving cell: none. Minting and verification read only local replicas and projections.

## Shape

The shape adds three things to `DESIGN.md`'s cell and removes the tenant. `global` is a fourth DSQL cluster that every data cell writes for global facts and pulls from. Front cells are a stateless Regional tier that authenticates and routes. Inter-cell logs carry derived data between data cells. Everything in `DESIGN.md` inside a cell (refs as rows, packs in S3, disposable caches, `core` and `aux`, Paired durability, the per-consumer outbox, Actions) stays, with the network replacing the tenant as the key and fence.

### Names and the directory

```rust
/// 1..=39 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen. GitHub documents the
/// character set and the 39 limit (docs.github.com, "Username considerations for external authentication");
/// the single-hyphen rule is GitHub's signup error text [unverified as a documented rule]. Parsing rejects RESERVED.
pub struct Login(Box<str>);
/// ASCII lowercase of a Login: the only form the namespace compares. Display case is kept on the account.
pub struct LoginKey(Box<str>);
/// 1..=100 of [A-Za-z0-9._-], not "." or "..", a trailing ".git" stripped [unverified exact GitHub limits].
pub struct RepoName(Box<str>);
pub struct RepoNameKey(Box<str>);
/// Every first path segment the host serves, so no login can shadow a route on the bare GHES-shaped host.
pub const RESERVED: &[&str] = &["api", "login", "logout", "session", "settings", "orgs", "organizations",
    "users", "repositories", "notifications", "search", "status", "site", "raw", "gist", "codeload", "uploads",
    "_services", "_t", "_codeload", "_registry" /* generated from the router's table; CI fails on drift */];

pub struct AccountId(DbId);
pub enum AccountKind { User, Org, Enterprise }

/// `name(key)` in `global`: one row per login key, shared by users, orgs, and enterprises.
pub enum NameClaim {
    Live { account: AccountId, kind: AccountKind },
    /// The account renamed away. Old URLs resolve to it until someone claims the key, which overwrites the row.
    Former { account: AccountId },
    Reserved,
}
/// `repo_name(owner, key)` in `global`. Keyed by owner id, so an owner rename rewrites no repository row.
pub enum RepoEntry { Live(RepoId), Alias(RepoId) }
/// `retired(owner_login, repo)`: a string pair kept forever and checked by every repository claim.
pub struct Retired { pub owner: LoginKey, pub repo: RepoNameKey }
pub struct RepoDir { pub id: RepoId, pub owner: AccountId, pub name: RepoName, pub network: NetworkId,
                     pub visibility: Visibility, pub life: RepoLife }   // Live | Deleted { restorable_until }
pub struct NetworkDir { pub id: NetworkId, pub cell: CellId, pub generation: u64 }

/// 64 partitions of `global`'s change log. The root is the org for org-scoped rows, otherwise the account.
pub struct GlobalPart(u8);
pub fn part_of(root: AccountId) -> GlobalPart { todo!() }

/// The only writer of `global`. Each row write appends its after-image to its root's partition through the
/// same handle, so a fact is in `global` exactly when its change is in the log. No other code holds a connection.
pub trait GlobalStore {
    async fn run_tx<T, F>(&self, ctx: TxCtx, f: F) -> Result<Committed<T>, StoreError>
    where F: Fn(&mut GlobalTx<'_>) -> Result<T, DomainError>;   // Fn: may rerun on 40001
}

/// A replica's answer. `Absent` is constructible only from a snapshot whose watermark, for every partition the
/// answer depends on, is at or after `asked_at + CLOCK_MARGIN`. Otherwise the answer is `Behind`.
pub enum Lookup<T> { Found(T), Absent, Behind { parts: SmallVec<[GlobalPart; 2]> } }
pub const CLOCK_MARGIN: Duration = Duration::from_millis(50);
pub const BEHIND_WAIT: Duration = Duration::from_secs(2);   // then 503 with Retry-After, never 401 or 404

pub struct Resolved { pub repo: RepoId, pub cell: CellId, pub via_alias: bool }
pub fn resolve_repo(p: &Projection, owner: &LoginKey, repo: &RepoNameKey, asked_at: Timestamp) -> Lookup<Resolved> { todo!() }
```

Invariants:

- **One namespace, one key.** Users, orgs, and enterprises share `name`. Logins are ASCII, so lowercasing is exact and needs no Unicode folding. GitHub logins are case-insensitive [inference].
- **Claims are `global` unique keys.** Creating an account, org, or repository, renaming, transferring, and deleting are each one `global` transaction of 3 to 8 rows. The replica is read only to fail fast with a friendly 422. The race in journey 3 is decided by the primary key.
- **Negative answers wait for freshness.** A replica never answers "no such name", "no such token", or "not allowed" about a fact it may not have seen. `Lookup::Absent` requires the partition watermark to pass `asked_at`. A miss that is `Behind` waits up to 2 s for the local store to advance and never asks `global`. Positive answers are served at once, and their staleness is bounded by the apply lag. This one rule makes "created a second ago" work everywhere without a read of `global`.
- **Renames.** A repository rename leaves `Alias(id)` at the old key, which the API answers with 301 and git with 200 (`research/wire-census.md#23`). An owner rename leaves `Former { account }` at the old login: GitHub frees an old username for anyone to claim and keeps old repository links and pushes working (https://docs.github.com/en/account-and-profile/reference/username-reference). The route lookup follows `Former` to the account, then `(account, repo key)`. A new claim of the old login overwrites `Former` and ends the redirect.
- **Retirement.** When an owner renames or is deleted, each public repository that had more than 100 clones or more than 100 Actions uses in the prior week, or holds a Marketplace action, gets a `retired(old_login, repo)` row in the same transaction, and later claims of that pair fail with "The repository `<REPOSITORY_NAME>` has been retired and cannot be reused." (same docs page). The handler reads these from the local card replica (`clones_7d`, `actions_7d`, `marketplace`), which trails by at most 60 s.
- **Transfers** set `repo_dir.owner`, write the new `repo_name`, and leave `Alias` under the old owner. The network does not move.
- **Ids.** `global.id_block(kind)` leases each data cell ranges of 2^20 ids. A cell keeps two unused ranges, at least 24 hours of creation [guess], so it keeps allocating while `global` is down. Nodes lease 1,000 at a time from the cell's `id_range` row in `core`, as `DESIGN.md:381` does today. Ids are unique service-wide and stay below 2^53. Account ids come from `global` directly. Issue numbers stay per repository in the repository's cell.

Access patterns: resolving `owner/repo` is two point lookups in the projection (login key, then `(owner, repo key)`), plus `repo` to network to cell. A claim is one `global` transaction. Owner listings (`/users/{u}/repos`, `/orgs/{o}/repos`) read `g_repo_dir` by its owner index in any cell and join `repo_card`. Against `#47`: name reads never reach `global`, so a surge of name lookups lands on projections and replicas, and there is no TTL to cut.

### Identity, orgs, and authorization

```rust
pub struct UserId(AccountId); pub struct OrgId(AccountId); pub struct TeamId(DbId);
pub enum OrgRole { Member, Admin, BillingManager }

/// The authorization graph. Every row lives in `global` and is replicated into every data cell's `core` as `g_*`.
pub struct Membership { pub org: OrgId, pub user: UserId, pub role: OrgRole }
pub struct TeamMember { pub team: TeamId, pub user: UserId, pub maintainer: bool }
pub enum Grantee { User(UserId), Team(TeamId) }
/// Outside collaborators and team grants on one repository.
pub struct RepoGrant { pub repo: RepoId, pub grantee: Grantee, pub permission: RepoPermission }
pub struct OrgPolicy { pub org: OrgId, pub base_permission: Option<RepoPermission>, pub sso: Option<SsoConfig>,
                       pub residency: Residency, pub custom_roles: Vec<CustomRole> }

/// Front projection row for a stored token (ghp_, github_pat_, gho_, ghu_, ghr_). The `core` replica keeps the
/// metadata without the hash, for settings pages and the credential-authorization API.
pub struct CredentialEntry { pub hash: [u8; 32], pub id: CredentialId, pub kind: TokenKind, pub account: AccountId,
    pub scopes: ScopeSet, pub expires: Option<Timestamp>, pub revoked: bool, pub sso_orgs: SmallVec<[OrgId; 2]> }

/// Installation tokens are signed by the minting cell. Every projection carries every cell's verifying keys.
pub struct Kid { pub cell: CellId, pub epoch: u32 }
pub struct VerifyingKey { pub kid: Kid, pub public: Ed25519Public, pub not_after: Timestamp }
pub struct InstallationClaims { pub app: AppId, pub installation: InstallationId, pub repos: RepoScope,
    pub permissions: PermissionSet, pub jti: Ulid, pub iat: Timestamp, pub exp: Timestamp, pub kid: Kid }

/// What a front cell asserts to a data cell: signed with the front cell's key, sent over mTLS on PrivateLink.
pub struct Principal { pub actor: Actor, pub scopes: ScopeSet, pub credential: Credential,
                       pub index_cell: CellId, pub asked_at: Timestamp, pub front: FrontCellId }

/// Front. Pure over the projection: no network I/O, no DSQL.
pub fn authenticate(h: &RequestAuth, proj: &Projection, asked_at: Timestamp) -> Lookup<Principal> { todo!() }
/// Data cell. Pure over one `core` snapshot, which holds the cell's own rows and the `g_*` replica together.
pub fn authorize(p: &Principal, action: Action, on: &ResourceRef, snap: &CoreSnapshot) -> Decision { todo!() }
/// `Behind` replaces a Forbidden or Hidden whose deciding `g_*` rows sit in partitions behind `p.asked_at`.
/// The caller retries on a new snapshot for up to BEHIND_WAIT, then answers 503.
pub enum Decision { Allow, Forbidden, Hidden, Behind }

/// Member, team, grant, credential, suspension, and uninstall writes return once every follower cursor in
/// `global.follower` covers the commit, or after 5 s. `lagging` feeds the observer route.
pub async fn revoke(g: &dyn GlobalStore, r: Revocation) -> Result<RevocationReceipt, StoreError> { todo!() }
pub struct RevocationReceipt { pub covered: bool, pub lagging: Vec<FollowerId> }
```

Decisions:

- **`global` owns the authorization graph, cells own content.** `global` holds names, accounts, stored-token hashes, SSH user and deploy keys under one service-wide fingerprint index, GPG keys, sessions, orgs, memberships, teams, repository grants, org policies (base permission, SSO, residency, custom roles, org rulesets, org webhooks, org Actions secrets as ciphertext), apps, installations with their repository selections, and verifying keys. Cells hold everything else: repository content and metadata, branch rules, issues, pull requests, Actions, stars, watches, notifications. Repository grants are in `global` so that `GET /user/repos` and search visibility can be computed in any cell from one replica, while the repository's own cell enforces the same rows.
- **Authenticate at the front, authorize in the data cell.** The front projection holds what authentication and routing need: routes, token hashes, SSH fingerprints, app public keys, installation `not_before` and suspension, each account's index cell and suspension, and verifying keys. The data cell's replica holds the authorization graph and display facts. Neither holds password hashes, 2FA seeds, recovery codes, or OAuth client secrets. Only `global` does, and only the `identity` handler reads them, inside a sign-in transaction that also inserts the session row. Sign-in is therefore a write and fails while `global` is unreachable. Existing sessions keep working: the cookie is a signed session id with `iat`, checked at the front against the service session key and the account's `sessions_not_before`.
- **Stored tokens.** Creating a PAT or finishing an OAuth flow writes the hash to `global`. A first use that reaches a front before the hash does is `Behind` and waits about 1 s. A revoked token stays in the projection as `revoked` for 30 days, so it gets 401 at once instead of waiting. A burst of random tokens with valid CRCs costs local lookups and a bounded wait: at most 16 waiters per IP, after which the front answers 503.
- **Installation tokens stay stateless** (`DESIGN.md:377`, `research/wire-census.md#30`) but are signed by the minting cell's key epoch. Private keys never leave their cell. Per-installation `not_before` lives in `global` and reaches every front through the projection. The deny list for a job's `GITHUB_TOKEN` jti stays cell-local, because that token's scope is one repository and only that repository's cell honors it.
- **Denials wait, grants race.** A grant committed before the request is always honored, because a denial that depends on a `Behind` partition is retried. A removal can be missed for the apply lag. `revoke` closes that window for callers who wait on it: after it returns with `covered`, no front and no cell honors the removed fact.
- **SSO** attaches to orgs and enterprises, as on GitHub Enterprise Cloud. The SAML or OIDC endpoint writes `sso_authorization(credential or session, org, expires)` to `global`. The front passes the credential id, and the cell checks `g_sso_authorization` when the resource's owner enforces SSO. Break-glass owners are pre-provisioned local accounts with hardware keys (`research/uptime.md` implication 25).
- **First-party OAuth apps** (`DESIGN.md:352-359`) are seeded once in `global` with their real client ids.

Access patterns: every HTTPS request does one projection lookup by token hash and one assertion check in the cell. A permission check reads `g_membership`, `g_team_member`, `g_repo_grant`, and `g_org_policy` by primary key in the snapshot the operation already reads. `global` sees writes only, at roughly the rate of signups, token creation, and membership changes.

Against `#47` and `#44`: no request reads `global`. Its read load is the follower set, one applier per data cell and one publisher per front cell, each pulling 64 partitions in one query every 100 ms, about 1,300 queries per second at 100 data cells and 30 front cells [guess]. That load is fixed by fleet size, not by request rate. Replicas are pushed, not expired, so there is no TTL whose change moves load onto `global`. Expensive writes on `global`, such as an org-wide membership saga, slow only other `global` writes. Writers are admitted by class, with revocations first, and token verification never touches `global`. Each data cell holds at most 8 writer connections to `global` per Region and each follower one, so 100 Paired cells and 30 fronts use about 1,800 of DSQL's 10,000 connections and reconnect within 20 s at 100 new per second (`research/uptime.md#27`). `global` runs read-write in us-east-2 and us-west-2 with a us-east-1 witness [inference that DSQL accepts this trio], which keeps us-east-1 out of the write quorum (implication 22). Writers use their nearer endpoint and switch after three failures, and DSQL needs no failover call (`#26`). Identity writes from Europe and Asia pay one cross-continent round trip [guess: 80 to 200 ms].

### Placement and routing

```rust
pub struct CellId(u16); pub struct FrontCellId(u16);
pub struct NetworkId(DbId);                                      // the root repository's id; every fork carries it
pub struct RepoKey { pub network: NetworkId, pub repo: RepoId }  // every repository table now leads with network

/// The write fence, keyed by network: one row in each of the cell's `core` and `aux`. Every network-scoped
/// run_tx reads it on its own snapshot and refuses unless Serving (ACT-3's fence, unit changed). Account index
/// rows use the same fence keyed by account.
pub enum NetworkState { Serving, Frozen { generation: u64 }, Moved { to: CellId, generation: u64 } }

/// Each front node's local store: an embedded B-tree on NVMe, built from an S3 snapshot plus segments.
pub struct Projection { /* names, repo entries, networks, credentials, keys, accounts */ pub applied: [Timestamp; 64] }

pub enum RouteClass {
    Repo { owner: LoginKey, repo: RepoNameKey, git: bool },  // /{o}/{r}.git/*, /api/v3/repos/{o}/{r}/*, raw, codeload
    RepoId(RepoId),                                           // /repositories/{id}/*, node ids with the repo id first
    Viewer,                                                   // /user/*, /notifications, /api/graphql
    Account(LoginKey),                                        // /users/{u}/*, /orgs/{o}/*
    Installation(InstallationId),                             // /app/installations/{id}/*, /installation/*
    IdentityWrite,                                            // sign-in, OAuth token exchange, signup, org create
}
pub enum Target { Cell { cell: CellId, repo: Option<RepoId>, via_alias: bool }, Index(CellId) }

/// Pure. Called once per HTTP request and once per SSH exec.
pub fn route(proj: &Projection, class: &RouteClass, p: &Principal) -> Lookup<Target> { todo!() }

/// New networks only. A fork inherits its network's cell, whoever owns the fork.
pub fn place_network(owner_index: CellId, residency: Residency, cells: &[CellLoad]) -> CellId { todo!() }

pub enum MoveStep { Clone, Freeze, Drain, Flip, Redirect, Forget }
/// ACT-3's migration with the network as the unit. Flip is one `global` transaction that sets network.cell and
/// bumps generation. Fronts route by it within seconds, and the old cell proxies until its own applier sees Flip.
pub async fn move_network(n: NetworkId, from: CellId, to: CellId) -> Result<(), MoveError> { todo!() }
```

Decisions and invariants:

- **The network is the unit of placement, fence, and migration.** A new network goes to the owner's index cell when it has headroom and matches the owner's residency, otherwise to the residency-pair cell with the most headroom (`DESIGN.md:213`). Every fork goes to its network's cell, so `bob/widgets` sits beside `acme/widgets`. A transfer changes `repo_dir.owner` and moves no data. A network above 20% of a cell's limits moves to a dedicated cell. One network is never split, so the largest network must fit one cell ("Open questions and risks").
- **The merge invariant follows from the unit.** A pull request's head and base are in one network, so in one `core`, so `merge` (`DESIGN.md:423-436`) is unchanged.
- **Each cell's applier owns convergence with `global`.** Every `repo_dir` row placed in the cell gets its local rows, created idempotently by repository id if the request that claimed it died first. Every `global` change that concerns the cell's repositories (rename, transfer, visibility, grant) inserts its webhook outbox event in the same transaction that applies it. Account-scoped changes (membership, team, org settings) emit their events in the account's index cell only, so each event has exactly one producer.
- **Front cells are the router, built as cells** (`research/uptime.md#8`-`#10`). Each Region with data cells runs at least three front cells, each with its own NLB, Auto Scaling group, control bucket, and publisher. Route 53 latency records per Region return multivalue answers across front cells, with health checks that send a synthetic routed request through each front cell. A front cell holds no canonical state.
- **Projections come from S3, not from `global`.** Each front cell's publisher (one active under a lease) pulls `global`'s log and writes 250 ms segments and an hourly full snapshot to the front cell's control bucket with `If-None-Match: *`. Edges tail their own bucket and keep the last good store on disk. A node boots from its own Region's bucket, so a fleet restart reads S3 and puts no load on `global`. The publisher holds any segment that moves more than 5 networks between cells without a signed move record (`DESIGN.md:181`).
- **Data cells accept only fronts and peers.** Each data cell exposes an internal NLB per Region through PrivateLink to front cells and peer cells, with mTLS, and accepts only front-signed `Principal` assertions.
- **SSH routes like github.com.** Fronts hold one service host key set, authenticate the client key by its service-wide unique fingerprint in the projection, read the repository path from the exec command, and route it like HTTPS (`research/wire-census.md#45`). ACT-6's per-tenant ports and host keys are gone.
- **Node ids route by repository.** Repository-scoped node ids carry the repository id first (`DESIGN.md:361-364`, `research/wire-census.md#27`), so `node(id:)` and `/repositories/{id}` route by `RepoId`. Tributary issues only new-format ids. Legacy ids appear only in GHES imports, which keep a `legacy_id` index in the imported cell.
- **GraphQL** enters at the caller's index cell. When every root field targets one other cell (`repository(...)`, a node id), the executor proxies the whole query there. Otherwise it delegates subtrees per cell in batches and returns partial data with `errors` paths when a cell fails. Cost accounting happens once, at the entry executor.

Access pattern: per request, two to four point lookups in the projection and one forwarded hop. The data cell then reads one snapshot, as in `DESIGN.md`. Against `#47`: routing is a pure function over a local file, and nothing on the read path plays the part of GitHub's auth and user database.

### Cross-cell features

```rust
/// One gap-free, commit-ordered log, used twice: `global`'s change log, and each data cell's inter-cell log in `aux`.
/// An append bumps its partition's head row inside the writer's own transaction. DSQL's write-write conflict on that
/// row orders appends, so seq order is commit order and a follower never skips a late commit, unlike the outbox's
/// ULID order (`DESIGN.md:557`). A retried append starts after the winner committed, so `at` grows with `seq` [inference].
pub enum LogId { Global, Xcell(CellId) }
pub struct LogPart { pub log: LogId, pub part: u16 }
pub struct Entry<E> { pub part: LogPart, pub seq: u64, pub at: Timestamp, pub body: E }
pub trait Append { fn append<E: Encode>(&mut self, part: LogPart, body: &E) -> Result<u64, DomainError>; } // on Tx, GlobalTx

/// Pull-based. The follower keeps its cursor in its own cluster and advances it in the same transaction as the apply,
/// so a crash repeats nothing. Idle partitions of `global` get a heartbeat entry from any follower that sees the head
/// older than 200 ms, so watermarks advance without a designated writer.
pub async fn follow<E>(src: &dyn Tail<E>, part: LogPart, dst: &dyn CellStore,
                       apply: &dyn Fn(&mut Tx<'_>, &Entry<E>) -> Result<Change<()>, DomainError>) -> Result<(), FollowError> { todo!() }

/// Inter-cell partitions in each source cell: one per destination cell, 8 card partitions every data cell follows,
/// and 8 search partitions only the search tier follows.
pub enum XcellDest { Cell(CellId), Cards(u8), Search(u8) }
pub enum XcellEvent {
    StarChanged { user: UserId, repo: RepoId, on: bool, version: u64 },
    WatchChanged { user: UserId, repo: RepoId, level: WatchLevel, version: u64 },
    Notify { user: UserId, thread: ThreadRef, reason: NotifyReason, at: Timestamp },
    Feed { account: AccountId, item: FeedItem },
    Card(RepoCard),        // to `Cards`; at most one per repository per 60 s
    SearchDoc(SearchDoc),  // to `Search`
}
/// Listing fields owned by the repository's cell and replicated to every data cell as `repo_card`.
pub struct RepoCard { pub repo: RepoId, pub version: u64, pub pushed_at: Timestamp, pub updated_at: Timestamp,
    pub stargazers: u32, pub forks: u32, pub open_issues: u32, pub language: Option<Lang>, pub description: Box<str>,
    pub default_branch: RefName, pub archived: bool, pub clones_7d: u32, pub actions_7d: u32, pub marketplace: bool }

/// Destination side. Last writer wins by `version`, so duplicates and reordering change nothing.
pub fn apply_xcell(tx: &mut Tx<'_>, from: CellId, ev: &XcellEvent) -> Result<Change<()>, DomainError> { todo!() }
```

Decisions:

- **One log mechanism, two uses.** `global`'s change log has 64 partitions keyed by aggregate root. Each data cell's inter-cell log has one partition per destination cell, 8 card partitions, and 8 search partitions. Writers batch, one append per transaction per partition, so a head row commits a few times per second, not once per event.
- **Every account has an index cell**, recorded on `global.account`, which holds its starred list, watch subscriptions, notifications, and activity feed. It is set at creation to the handling cell, which is near the account's first activity, and moves under the network fence pattern keyed by account.
- **Producers.** The per-consumer outbox (`DESIGN.md:496-561`) gains `IndexFeed` (stars, watches, feed items), `CardPublish` (coalesces card changes per repository), and `SearchFeed`. `Notifications` now computes recipients in the source cell and appends to the partition of each recipient's index cell, in `aux`, so fan-out never writes `core`.
- **Followers.** Each data cell runs one `xcell-follow` lease per source cell and partition addressed to it and pulls every 200 ms. A destination outage pauses only that destination's pulls, and the source never waits. Destinations admit each source at a fair share, so one source's storm cannot starve the others.
- **Retention and repair.** Log entries live 3 days. Stars, watches, feeds, and cards are derived, so a follower that falls further behind rebuilds them by a resync scan of the source rows. Notifications are not derived: a destination down for more than 3 days loses them ("Tradeoffs accepted").
- **Cards make listings local.** With `g_repo_dir` and `repo_card` in every cell, `/user/repos`, `/users/{u}/repos`, `/orgs/{o}/repos`, `/user/starred`, and `viewer.repositories` are answered by one cell. Fields that change on every push, such as `pushed_at`, lag by up to 60 s.
- **Search** is a sharded OpenSearch tier per residency domain (US, EU, APAC) that follows every cell's search partitions. A query filters by public visibility or by the viewer's reach, computed from the replica as owner ids whose base permission grants read plus explicit repository ids. A domain that misses its deadline yields `incomplete_results: true`. Failure stays on search endpoints (`research/uptime.md#49`, implication 18).
- **Org runners.** An org-level runner registers in the org's index cell. Jobs in other cells that match no repository-level runner are offered to it through the inter-cell log. The broker claims a job with one compare-and-set RPC to the job's cell, and `RunServiceUrl` names that cell's route, so acquire, renew, logs, and completion go to the job's cell. The claim is the one cross-cell call in Actions.

Against `#47`: pull-based logs mean a slow destination cannot slow its sources, and each cell sizes its own follower budget. A viral repository's fan-out is bounded per source by per-thread caps, coalescing, and fair-share admission at destinations.

## Public service concerns

**Anonymous access and rate limits.** Anonymous callers get public reads only, keyed by IP at the front: REST at 60 per hour, GraphQL refused as on GitHub, and git at the journey 1 limits. Authenticated limits follow GitHub's numbers: 5,000 REST per hour per user, 1,000 per repository for `GITHUB_TOKEN`, secondary limits of 100 concurrent requests and 80 content-creating requests per minute (`research/wire-census.md#18`, `#35`). Counters are per front node and approximate, because GitHub documents per-Region variation (`#18`). Inside a data cell, each repository's rendezvous set admits authenticated traffic before anonymous traffic.

**Abuse and spam.** Signup is a `global` write behind front limits per IP and per ASN, with email verification and 24 hours of reduced content-creation limits for new accounts. Repository creation and forks are capped per account per hour, because each is a `global` claim. Suspension is one `global` write: within the apply lag fronts reject the account's credentials and cells hide its content at render time from `g_account.suspended`. Abuse that targets one repository (clone floods, comment spam) stays in that network's cell, and a network that dominates a cell moves to a dedicated cell.

**Takedowns.** A takedown is a `takedown(network or repo, scope, reason)` row in the repository's own `core`, which every request for that repository reads, so it applies at once to git, API, raw, and archives. DMCA takedowns of whole fork networks are one network-scoped row. Country-scoped takedowns return 451 using a GeoIP table on the front, passed in the assertion.

**CDN for public content.** Managed enables subdomain isolation (`research/wire-census.md#33`): `raw.tributary.dev`, `codeload.tributary.dev`, and `objects.tributary.dev` sit behind CloudFront with front cells as the origin. Public responses carry `Cache-Control: public, max-age=300`, SHA-addressed archives `max-age=3600`, and private responses `no-store`, so a takedown reaches cached copies within the TTL even if a CloudFront invalidation, a control-plane call, fails. Each subdomain has a Route 53 failover record to the front cells directly. Git smart HTTP is not cached. Public repositories advertise `bundle-uri` with CloudFront bundles for clients that enable it, which default clients do not [inference], so the pack response cache in journey 1 carries anonymous clone load.

**Enterprise orgs: residency and SSO.** An org or enterprise with residency places all its networks, forks of them included, its Actions runners, and its search documents in its residency domain. Repository content, issues, pull requests, Actions data, and audit stay there. Logins, account ids, memberships, grants, and credential hashes are global facts and live in `global`'s US Regions, and the residency offer says so. An enterprise that needs identity facts resident too gets its own namespace through BYOC, as GitHub offers `*.ghe.com`. SSO enforcement is described in "Identity, orgs, and authorization". Enterprise-managed users get logins with an enterprise suffix (`alice_acme`) that only the enterprise's SCIM endpoint can claim [inference from GitHub's EMU naming].

## Failure modes

New stateful components on the request path. Data cell rows from `DESIGN.md:722-772` stay, rekeyed from tenant to network.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| `global` cluster | instance | `global` transactions in flight see errors or 40001 | Bounded retries under each writer's retry budget. Claims carry the request id, so a retried commit is idempotent. |
| `global` cluster | az | None for data; DSQL spans AZs | Writers reconnect inside their 8-connection budget. |
| `global` cluster | region | One of two read-write endpoints | Writers and followers switch to the other endpoint. The witness keeps quorum, and DSQL has no failover call (`research/uptime.md#26`). |
| `global` cluster | control-plane | None at runtime | Created once. IAM auth tokens are signed locally. Nothing at runtime calls a management API. |
| `global` cluster | bad-deploy | Identity writes service-wide: signup, sign-in, token creation, org and repository creation, forks, renames, membership and grant changes. Fetches, pushes, merges, issues, Actions, stars, and installation token minting keep serving in every cell | Schema changes only in a dedicated release after two weeks against a staging `global`. Writers run a fixed prepared-statement set checked in CI. The `identity` handler rolls with the data cell waves, so a bad handler stops writes from one cell first. Log record kinds follow a two-release rule: followers learn a kind one release before any writer emits it. |
| `global` cluster | bad-config | Same as bad-deploy | Writer config ships in the signed cell waves. `global` has no runtime config beyond its schema. |
| `global` cluster | noisy-tenant | `global` write capacity: a signup flood, an org-wide membership saga, a rename storm | Per-account and per-IP write buckets at the front, sagas of at most 1,000 rows under a velocity cap, write classes with revocations admitted first, and 64 partitions so one org's saga contends on one partition head. |
| Cell replica (`global-apply`, `g_*` tables) | instance | Apply lag in one cell grows by the lease timeout (10 s) | The lease moves to another worker. The cursor commits with each apply, so nothing repeats. |
| Cell replica | az | None; the rows are in `core` | As `core`'s `az` row. |
| Cell replica | region | Paired cell: the applier's Region | The applier lease is a `core` row, so it moves to the surviving Region. |
| Cell replica | control-plane | None | No AWS management call on the apply path. |
| Cell replica | bad-deploy | One cell's freshness. A poison record halts one partition in that cell | Appliers roll in cell waves. A partition that fails 10 times parks and alarms. Its lookups turn `Behind`, so new names, new grants, and new tokens in that partition get 503 after 2 s while existing facts keep serving. Swap-back restores apply. |
| Cell replica | bad-config | One cell | Signed config waves with last known good. |
| Cell replica | noisy-tenant | A bulk `global` change applies into every cell's `core` at once, which is correlated load on every cell | Apply runs at a capped rate (2,000 rows per second per cell [guess]) inside the worker connection share, so lag rises instead of `core` load. Revocations apply ahead of bulk records. |
| Front cell (edge, projection) | instance | Connections on that node | NLB health checks with the 33% removal cap. Clients retry. |
| Front cell | az | One third of a front cell's nodes | Static sizing at 66% per AZ and zonal shift (`research/uptime.md#2`, `#20`). |
| Front cell | region | That Region's front cells | Route 53 health checks move clients to the next Region's front cells. Data cells are reachable cross-Region over PrivateLink and peering. |
| Front cell | control-plane | None | Projection, certificates, and keys are on local disk. A node boots from its own Region's control bucket. DNS records are pre-created. |
| Front cell | bad-deploy | One front cell in one Region | Waves move one front cell at a time and one AZ at a time inside it, with automatic swap-back on 5xx rate. The per-front-cell health check drops a failing front cell from DNS answers. |
| Front cell | bad-config | A bad projection segment, such as a mass network flip, could reach every front | The publisher holds segments that move more than 5 networks without a signed move record. Segments carry a schema version and are validated before apply. Edges keep the last good store. |
| Front cell | noisy-tenant | One client or one repository saturating a front cell | Per-IP, per-token, and per-repository buckets, shuffle-sharded edge subsets per repository (`research/uptime.md#7`), and at most 16 `Behind` waiters per IP. |
| Inter-cell logs and index tables | instance | A follower lease holder dies | The lease moves. Cursors commit with applies. |
| Inter-cell logs and index tables | az | None; logs and indexes are in `aux` | As `aux`'s `az` row. |
| Inter-cell logs and index tables | region | Paired source or destination loses a Region | Followers pull from either Region's endpoint. |
| Inter-cell logs and index tables | control-plane | None | Only DSQL data operations and the PrivateLink data path. |
| Inter-cell logs and index tables | bad-deploy | A source emitting malformed events stalls its partitions at every destination | Each destination parks per source, so other sources continue. Derived indexes rebuild by resync. Producers change event shapes under the two-release rule. |
| Inter-cell logs and index tables | bad-config | One cell's follow or admission settings | Signed config waves with last known good. |
| Inter-cell logs and index tables | noisy-tenant | A viral repository's star and notification storm | Per-thread fan-out caps, card coalescing, and per-source fair share at each destination. All of it lands in `aux`. |
| Search tier | instance | Search queries slow down | Multi-AZ domains. Only search endpoints depend on it. |
| Search tier | az | Reduced search capacity | Multi-AZ domains. Git, pull requests, and API writes never touch it. |
| Search tier | region | That residency domain's search | Queries return `incomplete_results: true` for the missing domain. |
| Search tier | control-plane | None at runtime | Domains are pre-created. A rebuild uses a new index, not a new domain. |
| Search tier | bad-deploy | Wrong or missing results in one domain | Mapping changes build a new index and flip an alias (`research/byoc-grounding.md#41`). |
| Search tier | bad-config | One domain | Same alias flip, old index kept until the new one verifies. |
| Search tier | noisy-tenant | One caller or botnet driving query cost (`research/uptime.md#49`) | Per-user and per-IP query cost limits at the front. Anonymous search is limited as on GitHub. |

## Deployment modes

Every mode runs the same binary, the same `global` schema, the same followers, and the same front code. The namespace belongs to whichever `global` an install has, so a BYOC or self-hosted install has its own namespace, as each GHES instance does.

| Aspect | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Namespace | One public namespace | The install's own | The install's own |
| `global` store | Dedicated multi-Region DSQL cluster | A `global` schema in the install's `core` cluster (DSQL allows 10 schemas, `research/uptime.md#27`) | Same as BYOC |
| Data cells | Many, placed by network | One | One |
| Front | Front cells per Region | The `edge` role on the install's nodes | Same as BYOC |
| Host | `tributary.dev` with `raw.`, `codeload.`, `objects.` subdomains | Customer domain, subdomain isolation optional | Same as BYOC |
| Followers | One applier per data cell, one publisher per front cell | One applier, one publisher | Same as BYOC |
| Inter-cell logs | Between data cells | The one cell follows its own log | Same as BYOC |
| Search | Tier per residency domain | Optional install OpenSearch domain | Same as BYOC |
| Rate limits | On | Off by default | Off by default |
| Signing keys | Per cell epoch | The install's one cell | Same as BYOC |

In a single-cell install the applier still copies `global` rows into `g_*` tables, inside one cluster, and the inter-cell log is followed by its own cell. That costs a second copy of a small graph and keeps one code path: no handler has a "read `global` directly" branch. Installs never talk to the managed namespace, so their logins never collide with it.

## Changes to DESIGN.md

Deleted:

- Summary bullet "Each tenant is one GHES-shaped hostname" (`DESIGN.md:10`), the managed quickstart's host label, tenant map, and per-tenant DNS and certificate (`:65-67`), and the `--hostname acme.tributary.cloud` example (`:72-73`).
- From "Tenancy and cell placement" (`:135-215`): `TenantId`, `TenantHost`, `TenantRoute`, `RouteState`, `TenantMap`, `TenantMapSource`, `route(map, sni)`, `migrate_tenant`, and the bullets "The tenant is the host", "Fork networks live inside a tenant", "SSH resolves the tenant from the port", and "Ids are tenant-local".
- ACT-6 in full (`:874`): per-tenant NLBs, target ports, and host keys, the `git-ssh` oracle clause about one key in two tenants (`:599`), the per-tenant NLB tradeoff (`:900`), and the NLB quota open question (`:926`).
- `tenant` as the leading key of every table (`:223`, `:305`) and `id_block(tenant, kind)` (`:381`).
- Per-tenant signing keys that move with the tenant (`:377`, `:831`, the key sentence of ACT-9 at `:877`).
- "Seeded into every tenant" for first-party OAuth apps (`:352`, `:684`) and "SAML and OIDC per tenant" (`:382`).
- The per-tenant credential and permission snapshot in S3 (`:313`, `:376`, the snapshot sentence of ACT-14 at `:882`).
- The bullet "Users, orgs, teams, and permissions are rows in `core`" (`:379`).
- The alternative "A github.com-style shared namespace across tenants" (`:910`) and the tradeoff "one host per tenant" (`:896`).

Replaced:

- `TenantState` and `tenant_state` by `NetworkState` and `network_state`, plus an account-keyed fence for index rows. `run_tx` reads the fence of the network it writes. The frozen git-path list (`:786`) names `network_state` and adds the `g_*` tables that `authorize` reads.
- `RepoKey` becomes `{ network, repo }`. `OutboxPartition` becomes `hash(network or account) % 256`.
- The `edge` role splits into the front `edge` (TLS, SSH with the service host key, `authenticate` from the projection, `route`, rate limits) and data-cell ingress (verifies the front assertion). `trib-edge` changes to match.
- "Identity and tokens" (`:325-382`) becomes "Identity, orgs, and authorization" above. Stored-token hashes live in `global`. Installation keys are per cell epoch with `Kid`. Ids come from `global` ranges.
- Bounded stale fetches (`:313`) keep their mechanism. The time limit is signed per network class, and authorization comes from a per-cell snapshot of exactly the `g_*` rows the cell's repositories need, which `global-apply` writes to the cell's control bucket.
- `CacheHint` (`:523`, `:561`) stops writing per-tenant snapshots and writes that per-cell snapshot. `Notifications` writes inter-cell partitions.
- Admission (`:323`) and every `noisy-tenant` failure row move from tenant to account, network, and IP.
- `rest:search` (`:652`) moves to the residency-domain search tier. `rest:activity` (`:603`) gains index cells for stars, watches, and notifications.
- The hostname text (`:682`) names one managed host with subdomain isolation. Self-hosted and BYOC keep the customer host.
- The Deployment modes table (`:796-811`) loses "Tenants per cell" and gains the rows above.
- ACT-3's fence is keyed by network, ACT-9 keeps `not_before` with keys per cell epoch, and ACT-14's snapshot unit is the cell.
- Milestone 4 (`:853`) migrates a network instead of a tenant.

Added:

- A "Global namespace" section with this design's four Shape subsections, placed before "Repository storage and ref transactions".
- Crates `trib-global` (`GlobalStore`, `Lookup`, the log) and `trib-xcell` (events, `follow`, `apply_xcell`). Roles `global-apply`, `front-publish`, `xcell-follow`, and the `identity` handler. Deployable front cells in `tributary-fleet`.
- SLOs: identity writes 99.95%; replica freshness p99 under 1 s in data cells and under 2 s at fronts; revocation coverage under 5 s.
- Failure rows above. Drills: a weekly 30-minute cut of the canary data cell's path to `global`, which must show zero failed fetches, pushes, or merges, and a monthly front cell evacuation in one Region.
- Change safety: the `global` schema rule and the two-release rule for log records.
- Milestone 0 adds a partition-head throughput test on a multi-Region `global`. Milestone 2's gate adds a fresh `gh auth login` followed within 100 ms by `gh api user` through a different Region's front.
- Known deviations: listing fields trail pushes by up to 60 s, and new names and tokens can take up to 2 s to resolve at a distant front.

## Tradeoffs accepted

- We accept that identity writes (signup, sign-in, token creation, org and repository creation, forks, renames, membership and grant changes) stop everywhere while `global` is unreachable or broken, in exchange for fetches, pushes, merges, issues, Actions, and stars that keep serving in every cell with no read of `global`.
- We accept that a removal of access can be honored for up to the apply lag (p99 under 1 s), and up to 5 s for callers of `revoke`, in exchange for authorization read from the same snapshot as the refs.
- We accept that an unknown name or token waits up to 2 s, and gets 503 instead of 404 or 401 while its partition is stale, in exchange for never answering "no" from a replica that may not have seen the fact.
- We accept a full copy of the authorization graph and of every repository card in every data cell, with N-fold storage and apply writes [guess: tens of GB per cell at launch scale], in exchange for listings and permission checks that read one cell.
- We accept `global` in US Regions, so identity writes from other continents pay a cross-continent round trip and identity facts are not resident, in exchange for one strongly consistent namespace.
- We accept that a fork network is never split, so the largest network must fit one cell, in exchange for one-transaction merges for every pull request.
- We accept that a compromised data cell can mint installation tokens for any installation until its `kid` is revoked, and a compromised front cell can assert any principal, in exchange for token minting and verification with no shared service on the path.
- We accept synchronous cross-cell calls for GraphQL queries that span cells and for org-runner job claims, with partial results on failure.
- We accept that notifications for a destination cell down longer than 3 days are lost.
- We accept that single-cell installs run an applier that copies tables within one cluster, in exchange for one code path in every mode.

## Alternatives considered

- **Owner-home cells.** Each owner's repositories and identity live in the owner's home cell, and a thin global directory maps names to home cells. It lost because cross-owner forks then span cells: `bob/widgets` sits in bob's cell and `acme/widgets` in acme's, so a merge of bob's pull request reads a head ref in one cluster and moves a base ref in another, which needs a saga or a copy of the head objects and loses the one-transaction merge for the most common public pull request. A network's objects would also be duplicated across every cell that holds one of its forks.
- **One global DSQL cluster for everything, no cells.** Every fact, refs included, in one multi-Region cluster gives one transaction for anything. It lost because one cluster is one blast radius for bad query plans, migrations, and engine regressions, the database incidents that dominate GitHub's core outage minutes (`research/uptime.md#43`, `#44`), and DSQL's 10,000 connections per cluster (`#27`) would cap the whole service.
- **A synchronous global identity service behind caches.** This is GitHub's shape: cells call a central auth and user store and cache the answers. It lost because the store's load scales with requests and with every cache setting, which is exactly the February 2026 failure (`#47`) and the May 2023 App token outage (`#44`).
- **The directory in DynamoDB global tables.** MREC exposes partial transactions in other Regions (`#30`), and MRSC has no transactions (`#29`), so a claim, its account row, and its first membership could not commit together.

## Open questions and risks

- Does DSQL accept a us-east-2, us-west-2, and us-east-1 witness trio, or any cross-continent pair, and what is its commit latency? (`research/uptime.md` open question 3.)
- Does DSQL detect write-write conflicts on the partition head row so that a retried append always starts after the winner's commit? The log's gap-free and monotonic-time claims rest on it. [unverified]
- How far apart are DSQL transaction timestamps and EC2 clocks? `CLOCK_MARGIN` assumes 50 ms. [unverified]
- How many appends per second does one partition head sustain on a multi-Region cluster, and do 64 partitions carry a signup flood? Milestone 0 measures it.
- How large do projections grow (routes plus credential hashes, a guessed 100 GB at GitHub scale), and how long does a front node take to boot from S3 at that size?
- Can the largest public fork network fit one cell's DSQL and cache limits, and what happens when it cannot?
- Is US residency of identity facts acceptable to EU enterprise customers, or does every such customer need BYOC?
- Index cell hot spots: an account that stars or watches millions of repositories, or receives millions of notifications, concentrates on one cell.
