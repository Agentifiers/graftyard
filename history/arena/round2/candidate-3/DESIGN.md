# One ledger: all managed metadata in one logical DSQL database, striped by fork network

## Problem

The operator made managed Tributary one public namespace (`FRAME.md`, "Operator decision, 2026-10-24"). `DESIGN.md` assumed the opposite. Every key led with `tenant_id`, a tenant was a hostname, and a cell owned everything about its tenants (`DESIGN.md:208`-`212`). This candidate tests the most direct answer to the change. Cells stop owning metadata. All metadata for the public service (logins, identity, orgs, repositories, refs, pull requests, stars) lives in one logical Aurora DSQL database, called the ledger, which every node in every cell can reach. Then every cross-owner feature becomes a transaction in one database, and the global namespace needs no directory service, no cross-cell protocol, and no owner placement.

The question is whether one DSQL cluster can be that database at github.com scale. The engine is not the problem. Its quotas, its Region model, and our own change safety are. The table compares one cluster's limits with what a github.com-like service needs.

| Limit of one DSQL cluster | What github.com-like scale needs | Verdict |
| --- | --- | --- |
| 256 TiB storage, with an approved increase ([quotas](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/CHAP_quotas.html)) | GitHub's MySQL fleet held 300+ TB across 50+ clusters in 2023 ([GitHub blog](https://github.blog/engineering/infrastructure/upgrading-github-com-to-mysql-8-0)). Packs stay in S3 here, but issues, comments, timelines, and refs do not | Does not fit with growth [inference] |
| 100 new connections per second, not configurable, burst 1,000 (same quotas page) | A fleet of 2,000 nodes holding 30 connections each to one cluster [guess] needs 10 minutes to reconnect after a mass drop | Recovery time grows with the fleet [inference] |
| 10,000 connections, configurable to an unpublished ceiling | The same 60,000 connections | Unknown [unverified] |
| The two read-write Regions of a multi-Region cluster peer within one continent group ([AWS Backup DSQL restore](https://docs.aws.amazon.com/aws-backup/latest/devguide/restore-auroradsql.html), [multi-Region launch](https://aws.amazon.com/about-aws/whats-new/2026/07/amazon-aurora-dsql-adds-multi-region-clusters-four-more-regions/)) | EU data residency, and writable endpoints near European and Asian users | One cluster cannot [inference] |
| Write throughput: adjudicators and storage shard by key range, "millions of transactions per second" ([DSQL paper](https://arxiv.org/html/2607.13276v2), sections 2 and 7) | 3.35 billion pushes per month, about 1,300 per second on average (`research/git-storage.md#5`) | Fits [inference] |
| One catalog: DDL runs one statement per transaction and conflicts with in-flight transactions (`research/uptime.md#27`, OC001) | Canary waves for our own migrations, the largest source of GitHub's core outage minutes (`research/uptime.md#43`, `#44`) | No unit to canary [inference] |

**The verdict.** DSQL's internal scaling holds, but one cluster does not. The ledger stays one logical database: one schema, one id space, one transaction API, and any node can reach any row. Physically it is one `graph` cluster plus network stripes. `graph` holds everything that crosses owners: names, identity, authorization, and the social graph. Each stripe holds every row that belongs to a set of fork networks. The split follows one fact. GitHub only lets a pull request connect repositories in one repository network, and the network shares git data ([GitHub forks reference](https://docs.github.com/en/pull-requests/reference/forks)) [inference that PRs never cross networks]. So every transaction that GitHub semantics require to be atomic (push, PR sync, merge, review) touches one network, and every cross-owner write that is not inside a network (name claim, star, follow, membership, installation) touches only `graph`. Cells remain, but only as compute: node fleets, NVMe git caches, Actions brokers, and S3 placement. No cell owns a row.

Two costs remain, and the rest of this document bounds them. `graph` is a singleton that every request depends on for authentication and name resolution. And every stripe, like every cell before, rides on one DSQL engine that AWS upgrades on its own schedule (`review/corebet-a.md`, row d).

## Usage (caller's view)

The service has one host, `tributary.dev`, with the GHES subdomain-isolation names (`raw.`, `codeload.`, `uploads.`) under one certificate. Clients classify it as GHES because it is not `github.com` or `*.ghe.com` (`research/clients.md#1`). Users type `gh auth login --hostname tributary.dev` once.

**Journey 1. Anonymous `git clone https://tributary.dev/torvalds/linux.git`.**

1. Route 53 latency records send the caller to the nearest Region's NLB, which passes TCP 443 to an `edge` process (data plane only, `research/uptime.md#15`).
2. The request has no credential, so the principal is `Anonymous`. The edge charges the caller's IP (IPv4 /32, IPv6 /64) in node-local buckets: 60 API requests per hour, as GitHub documents ([rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)), and for git 8 concurrent upload-packs and 120 new upload-packs per hour [guess, GitHub publishes no anonymous git limit]. Each node enforces its share of a Region-wide limit, which GitHub's approximate counters allow (`research/wire-census.md` implications).
3. The edge resolves `torvalds/linux` in its `NameCache`: login `torvalds` to an `OwnerId`, then `(owner, "linux")` to `RepoId` R with `NameEpoch` e. A miss asks the cell's `api` role, which reads `graph` once, single-flight per cell. R's low 12 bits are its slot. The in-memory `SlotMap` gives stripe `net-am-03` and its two serving cells, one in us-east-2 and one in us-west-2. The edge forwards to the serving cell in its own Region, or the nearer one.
4. The `git` role on R's rendezvous node makes the one stripe snapshot read that `DESIGN.md` already makes (`DESIGN.md:287`-`291`), extended by two rows: `slot_state(slot)` and `repo_head(R)`. `repo_head` says public, no takedown, and epoch e, which matches the cache. Anonymous reads of public repositories need no grant, so `graph` is not read. Upstream `upload-pack` serves from the NVMe cache. Hot public repositories widen their rendezvous set from 3 to 9 nodes (`DESIGN.md:319`) and use git's `uploadpack.packObjectsHook` to reuse identical clone packs [inference].
5. Stores touched: the edge's `NameCache` and `SlotMap`, stripe `net-am-03`, the stripe's S3 pack buckets, and NVMe. The serving cell holds no metadata, so the stripe is always outside the cell, by design. On a warm path nothing else is touched. `graph` is touched synchronously only on a name-cache miss.

**Journey 2. `bob` forks `acme/widgets`, pushes, opens a PR, and a maintainer merges.**

1. `gh repo fork acme/widgets` sends `POST /api/v3/repos/acme/widgets/forks`. The edge authenticates bob's `gho_` token from `AuthCache` and routes by the path's repository to a serving cell of its stripe.
2. The `api` role checks read access from `repo_head` (public). Saga step 1 is one `graph` transaction, forwarded to an `api` node in a `graph` Region if this cell is elsewhere. It inserts the name row `(bob, widgets)` as `Live` for a new `RepoId` R2 that is allocated in R1's slot, the repository row in state `Creating` with network N1, and an outbox event. The API answers 202 with the repository body, because GitHub forks asynchronously too ([forks API](https://docs.github.com/en/rest/repos/forks)).
3. Step 2 is one stripe transaction, run inline and retried by the `RepoProvisioner` consumer from its pending row. It inserts `repo_head(R2)` and copies R1's ref rows, at most 900 per transaction. No pack bytes move, because the network shares one object store. Step 3 is one `graph` transaction that marks R2 `Live`.
4. `git push origin fix` resolves `bob/widgets` to R2. R2's slot is R1's slot, so it reaches the same stripe. `commit_push` runs exactly as in `DESIGN.md` journey 1, keyed by network instead of tenant.
5. `gh pr create --repo acme/widgets --head bob:fix` creates the PR row with `HeadSpec { repo: R2, branch: "fix" }` in one stripe transaction. The allocation rule guarantees that both repositories are in the same stripe.
6. A maintainer runs `gh pr merge`. The merge is `DESIGN.md` journey 2 unchanged, all in one stripe transaction. It compare-and-sets bob's `fix` ref row, the PR row, and acme's `main` ref row together. **The merge-in-one-transaction invariant holds for every cross-owner PR.** One input weakens: the maintainer's write permission and CODEOWNERS team membership come from `AuthCache` (normally seconds behind `graph`, at most 5 minutes) and not from the merge snapshot, as `DESIGN.md:379` had it. Branch rules, reviews, checks, and the PR's `maintainer_can_modify` stay in the snapshot.
7. Synchronous dependencies outside the serving cell: the stripe for every step, and `graph` for steps 2 and 3 only.

**Journey 3. A race for `newco`, then `newco/a` becomes `newco/b`.**

1. Two sign-up requests run `claim_login("newco")` on different `api` nodes. Each is one `graph` transaction that inserts `login("newco")`, the org row, and the creator's owner membership. Both write the same primary key, DSQL detects the write-write conflict at commit ([DSQL paper](https://arxiv.org/html/2607.13276v2) section 5.2), and the loser retries, reads the row, and returns 422 `already_exists`. No half-created org can exist, because the three rows commit together.
2. `PATCH /repos/newco/a {"name":"b"}` runs the rename saga. A `graph` transaction makes `(newco, b)` `Live` for R and turns `(newco, a)` into `Alias { repo: R, settled: false }`. A stripe transaction bumps `repo_head(R).name_epoch` to e+1. A `graph` transaction sets the alias `settled`.
3. `GET /api/v3/repos/newco/a` hits a cached `(newco, a) -> R, e`. The stripe read shows e+1, so the `api` role re-resolves in `graph`, finds the alias, and answers `301` with `Location: https://tributary.dev/api/v3/repositories/<R>` and GitHub's body (`research/wire-census.md#23`). `git fetch https://tributary.dev/newco/a.git` takes the same path and is served from R with 200.
4. A later `POST /orgs/newco/repos {"name":"a"}` must wait until the alias is settled. It then removes the alias, bumps R's epoch again, and only then makes the new repository `Live`, so no node can still route `newco/a` to R.
5. Synchronous dependencies: `graph` for the claim, the rename, and the re-resolution after a stale epoch. The stripe for the epoch bump and for the epoch check every request makes. A warm cache with a current epoch needs no `graph` read.

**Journey 4. `alice` in `acme` (Americas stripe), `euco` (EU residency stripe), and `apco` (Asia-Pacific stripe).**

1. `GET /user/repos` and `gh repo list` are one `graph` snapshot read: owned repositories, collaborator rows, team grants through her memberships, and org base permissions, with list fields from the `repo_summary` projection. No stripe is read.
2. GraphQL `viewer { repositories(first: 30) { nodes { name pushedAt defaultBranchRef { target { oid } } } } }` reads the list from `graph`. `defaultBranchRef.target` needs ref rows, so the `api` role groups the page's repositories by stripe and sends one batched read to a serving cell of each stripe in parallel, one snapshot per stripe. Latency is the slowest stripe's round trip.
3. `PUT /user/starred/euco/x` is one `graph` transaction. `INSERT ... ON CONFLICT DO NOTHING` makes it idempotent, and a sharded counter row changes only on insert. A projection consumer later writes the count into the EU stripe's `repo_head`.
4. A comment on a PR in `euco/x` commits in the EU stripe with its outbox event. The `Notifications` consumer, leased in a serving cell of that stripe, reads thread participants from the stripe and watchers from `graph` in chunks, and writes `notification(alice, thread, event)` idempotently into `inbox-eu`. `GET /notifications` merges three indexed range reads, one per continent `inbox`.
5. Synchronous dependencies: `graph` for listings and the star, each continent's `inbox` for notifications, and stripes only for git fields.

**Journey 5. One GitHub App installed on `acme` and `euco`.**

1. The app signs a JWT with its private key and calls `POST /app/installations/{id}/access_tokens`. Any `api` node verifies the JWT against the app's public key in `graph` (cached), reads the installation row, and mints a stateless `ghs_` JWS signed with the service key epoch (`research/wire-census.md#30`). The claims carry the installation, repository scope, and permissions. No key belongs to a tenant or a cell.
2. The app calls `GET /repos/acme/widgets/pulls` and `POST /repos/euco/x/issues`. Any edge verifies the token offline against the in-memory key ring and the pushed deny list. `authorize` intersects the claims with the installation's current rows from `AuthCache`. Each request routes by repository to its own stripe.
3. Synchronous dependencies: `graph` to mint, as on GitHub. Calls need nothing beyond the target stripe.

## Shape

The ledger is several DSQL clusters behind one Rust API. `graph` is one multi-Region cluster in the Americas group: us-east-2 and us-west-2 read-write, with a us-east-1 witness, so us-east-1 never hosts an endpoint (`research/uptime.md` implication 22). Each stripe is one `net` cluster (`DESIGN.md`'s `core` without identity) and one `aux` cluster (Actions, webhook deliveries), multi-Region inside one continent group, plus its pack buckets. Each continent group has one `inbox` cluster for notifications and audit. Each cluster lives in its own AWS account. One rule holds everywhere: **a transaction touches exactly one cluster**. Work that spans clusters is an outbox saga of idempotent steps, which `DESIGN.md` already uses for bulk operations (`DESIGN.md:125`).

### Names and the directory

```rust
/// GitHub login rules: 1..=39 of [A-Za-z0-9-], no leading, trailing, or doubled hyphen.
/// Display case is kept; `key()` is ASCII lowercase and is the only form compared or indexed.
pub struct Login(String);
pub struct LoginKey(String);
/// 1..=100 of [A-Za-z0-9._-], not "." or ".."; the router strips a trailing ".git".
pub struct RepoName(String);
pub struct RepoNameKey(String);
pub struct OwnerId(DbId);                 // users, orgs, and enterprises share one id space and one login table
pub enum OwnerKind { User, Org, Enterprise }

/// graph: login(key). One row per key, forever.
pub enum LoginRow {
    /// Lookups for (login, repo) try `owner` first, then each earlier owner, newest first (GitHub's redirects).
    Claimed { owner: OwnerId, kind: OwnerKind, previous: SmallVec<[OwnerId; 4]> },
    /// Released by rename or deletion. Lookups fall through to `previous` until the key is claimed again, and
    /// graph refuses a claim before `claimable_at` = release + LOGIN_QUARANTINE.
    Released { previous: SmallVec<[OwnerId; 4]>, claimable_at: Timestamp },
    Reserved,                             // service paths: api, login, settings, orgs, ...
}
pub const LOGIN_QUARANTINE: Duration = Duration::from_secs(60 * 60);
pub const LOGIN_CACHE_MAX: Duration = Duration::from_secs(50 * 60);

/// Lives in repo_head in the stripe. Bumped whenever the set of names that resolve to the repository changes.
pub struct NameEpoch(u32);

/// graph: repo_name(owner, name_key). A partial unique index allows one Live row per (owner, name_key).
pub enum NameRow {
    Live { repo: RepoId, epoch: NameEpoch },
    Alias { repo: RepoId, epoch: NameEpoch, settled: bool },
    Retired,                              // GitHub's popular-namespace rule; never reusable
}

pub enum Resolution {
    Canonical { repo: RepoId, epoch: NameEpoch },
    Moved { repo: RepoId, epoch: NameEpoch },   // API: 301 to /repositories/{id}. git: serve the repository.
    NotFound,
}

/// Node-local cache, never the source of truth. A login entry is valid until `valid_until`: LOGIN_CACHE_MAX after
/// it was confirmed, or `claimable_at` for a Released row. Repository entries never expire; they are checked
/// against repo_head.name_epoch inside the stripe read every request already makes.
/// A (login, repo) entry is used only while the login's entry is valid.
pub struct NameCache { /* LoginKey -> (LoginRow, valid_until); (LoginKey, RepoNameKey) -> Resolution */ }
impl NameCache {
    pub fn lookup(&self, owner: &LoginKey, repo: &RepoNameKey, now: Timestamp) -> Option<Resolution> { todo!() }
}
/// Pure. `Stale` sends the request to Directory::resolve once, then the request proceeds or answers NotFound.
pub fn check_epoch(cached: NameEpoch, head: &RepoHead) -> EpochCheck { todo!() }
pub enum EpochCheck { Current, Stale }

pub trait Directory {
    /// One graph snapshot read in the api role, single-flight per cell.
    async fn resolve(&self, owner: &LoginKey, repo: &RepoNameKey) -> Result<Resolution, DirError>;
}

/// One graph transaction: login row, owner row, and the creator's owner membership commit together.
pub async fn claim_login(g: &Graph, login: Login, kind: OwnerKind, creator: UserId) -> Result<OwnerId, ClaimError> { todo!() }
pub enum ClaimError { Taken, Quarantined { until: Timestamp }, Reserved, Invalid, Store(StoreError) }

/// Every change to repository names is this saga. Each step is idempotent on the saga id, and a pending
/// outbox row in graph drives it forward to completion, never back.
pub enum NameSaga { Create { owner: OwnerId, name: RepoName, fork_of: Option<RepoId> },
                    Rename { repo: RepoId, to: RepoName }, Transfer { repo: RepoId, to: OwnerId },
                    Delete { repo: RepoId } }
pub enum NameStep { BindInGraph, BumpEpochInStripe, SettleInGraph }
pub async fn run_name_saga(l: &Ledger, id: SagaId, s: NameSaga) -> Result<RepoId, SagaError> { todo!() }
```

Invariants:

- **Uniqueness under races.** `login.key` is a primary key, and `Live` repository names are unique per owner through a partial unique index (partial indexes since 2026-09-15, `research/uptime.md#28`). Two concurrent claims write the same key, so DSQL aborts one at commit. Its retry sees `Taken`.
- **Case folding.** GitHub logins and repository names are ASCII, so ASCII lowercase is exact. DSQL's C collation is enough (`DESIGN.md:132`). Display case is a separate column.
- **No node routes a name to a different owner or repository than `graph`, except to a redirect target that `graph` itself would serve.** A cached repository entry is checked against `name_epoch` in the stripe snapshot. Every saga bumps the epoch of each repository that a name used to resolve to, including a repository reached through an earlier owner of the login, before the new binding becomes `Live`. A cached login entry expires before its key can be claimed again, because a release sets `claimable_at` one hour out and a confirmed entry lives at most 50 minutes. This rule is the reason for both constants.
- **Redirects match GitHub.** A repository rename leaves an alias until the owner reuses the name ([renaming a repository](https://docs.github.com/en/repositories/creating-and-managing-repositories/renaming-a-repository)). An owner rename leaves the old login `Released`, and lookups for `old/<repo>` fall through to the previous owner until someone claims the login and creates that repository name ([username changes](https://docs.github.com/en/account-and-profile/concepts/username-changes)). Org endpoints under the old login answer 404, as on GitHub ([renaming an organization](https://docs.github.com/en/organizations/managing-organization-settings/renaming-an-organization)).
- **Retirement.** When an owner is renamed or deleted, each public repository that has a Marketplace action, more than 100 clones, or more than 100 Actions uses in the prior week gets a `Retired` row for `old_login/name` ([username reference](https://docs.github.com/en/account-and-profile/reference/username-reference)). Weekly clone and Actions-use counts come from a `repo_weekly_usage` projection that edge logs feed. Creating a retired combination fails with GitHub's message.
- **Halfway failures.** `claim_login` is one transaction. A repository create, fork, rename, transfer, or delete that stops after `BindInGraph` leaves the name held by a `Creating` repository, or by an unsettled alias, and the pending row finishes the saga. A client retry with the same request id gets the same repository back. Nobody else can take the name in between.
- **Transfers move no data.** A transfer rebinds the name and owner in `graph` and bumps the epoch in the stripe. The network keeps its slot, so refs, packs, PRs, and Actions history stay where they are.

Access patterns: every request costs two hash lookups in `NameCache` plus an epoch compare inside a stripe read it already makes. A miss costs one `graph` primary-key read. Renames cost three small transactions.

### Identity, orgs, and authorization

```rust
pub struct UserId(OwnerId); pub struct OrgId(OwnerId); pub struct EnterpriseId(OwnerId);
pub struct Principal { pub actor: Actor, pub scopes: ScopeSet, pub credential: Credential, pub sso: SsoSessions }
pub struct InstallationClaims { pub app: AppId, pub installation: InstallationId, pub account: OwnerId,
    pub repos: RepoScope, pub permissions: PermissionSet, pub jti: Ulid, pub iat: Timestamp, pub exp: Timestamp,
    pub key_epoch: u32 }                  // no tenant: any node verifies any installation token
/// One key ring per deployment. Epoch keys are KMS-wrapped on local disk and decrypted at node start.
pub struct ServiceKeyRing { pub current: EpochKey, pub previous: EpochKey }

/// Computed by the api role from one graph snapshot: owner, collaborator, team, org base, enterprise policy,
/// SSO requirement, and installation rows. `authz_epoch` is the owner's epoch at that snapshot.
pub struct Grant { pub level: Option<Permission>, pub sso_required: Option<OrgId>, pub authz_epoch: u64 }

pub enum Use { Read, Write }
pub const CONFIRM_EVERY: Duration = Duration::from_secs(5 * 60);   // background re-read per active entry
pub const MAX_AGE_READ: Duration = Duration::from_secs(2 * 3600);
pub const MAX_AGE_WRITE: Duration = Duration::from_secs(15 * 60);

/// The only way a request learns graph-owned auth facts. Edges keep a node copy. The api role keeps the cell
/// copy and is the only role that reads graph for it, single-flight. Hints from graph's outbox evict entries
/// immediately. A request never waits on graph for a key the cell has seen.
pub struct AuthCache { /* credential hash -> Principal; (actor, repo) -> Grant; owner -> newest authz_epoch */ }
pub struct Confirmed<T> { pub value: T, pub confirmed_at: Timestamp }
impl AuthCache {
    /// Err(Stale) when confirmed_at is older than the max age for `u`; Err(Unknown) on a cold miss with graph down.
    pub async fn principal(&self, c: &CredentialKey, u: Use) -> Result<Confirmed<Principal>, AuthError> { todo!() }
    pub async fn grant(&self, p: &Principal, repo: RepoId, owner: OwnerId, u: Use) -> Result<Confirmed<Grant>, AuthError> { todo!() }
}
/// Pure. Graph facts come from the cache; repository facts (visibility, archived, takedown, branch rules,
/// maintainer_can_modify) come from the stripe snapshot the request already holds.
pub fn authorize(p: &Principal, a: Action, head: &RepoHead, g: Option<&Grant>, pr: Option<&PrView>) -> Decision { todo!() }
pub enum Decision { Allow, Forbidden, Hidden, Blocked451, SsoRequired(OrgId) }
```

Ownership. `graph` owns users, orgs, enterprises, logins, emails, credential hashes, SSH keys (fingerprint unique across the service, as on github.com), GPG keys, teams, memberships, collaborators, org base permissions, OAuth apps, GitHub Apps and installations, sessions, SSO configuration and SSO sessions, stars, follows, and watches. The stripe owns the facts the git and merge paths enforce next to refs: `repo_head` (visibility, archived, takedown, default branch, `name_epoch`), branch rules, and PR rows. Each fact has one owner. The other side sees a projection with a source version.

Decisions:

- **Revocation is bounded and pushed.** Every `graph` transaction that changes a grant, membership, credential, installation, or SSO session bumps `owner_authz_epoch(owner)` and emits a hint. The `CacheHint` consumer fans hints to every Region. A node evicts on arrival, so revocation normally lands within seconds. If the hint path stalls, the 5-minute confirm bounds it, and hint lag above 30 s pages. Stateless `ghs_` tokens keep `DESIGN.md`'s installation `not_before` deny list (ACT-9).
- **While `graph` is down, cached facts do not get staler,** because revocations are `graph` writes and none can commit [inference]. A partial partition can hide a revocation from one node. The deep health check marks a node that cannot reach `graph`, while its peers can, as unfit for writes, and `MAX_AGE_WRITE` bounds the rest. Reads keep `DESIGN.md`'s 2-hour stale limit (ACT-14).
- **This is not GitHub's February 2026 auth outage** (`research/uptime.md#47`). There, a TTL cut multiplied reads on a shared auth cluster. Here `graph` sees one read per active key per cell every 5 minutes, plus cold misses. The confirm interval has a compiled floor, and each cell's refresher has a fixed read budget, so a config change or a client release moves latency, not database load. The path is the same whether `graph` is up or down, so there is no fallback mode to rot (`research/uptime.md#5`).
- **SSO attaches to orgs and enterprises**, as on GitHub Enterprise Cloud. `Grant.sso_required` must match an SSO session the credential holds. Enterprise managed users get logins with the enterprise's `_shortcode` suffix ([username considerations](https://docs.github.com/en/enterprise-cloud@latest/admin/managing-iam/iam-configuration-reference/username-considerations-for-external-authentication)), so they never collide with public logins.
- **First-party OAuth clients** (`DESIGN.md:353`) are seeded once per deployment. Device and web flows run against `graph` and are among the operations that need it.

Access patterns: a request with a warm credential costs one hash lookup and one epoch compare. A merge adds one `Grant` lookup. `graph` read load scales with distinct active credentials, not with requests.

### Placement and routing

```rust
pub const SLOTS: usize = 4096;
pub struct SlotId(u16);                   // fixed forever for a network; forks and split-off networks keep it
/// Repository-scoped ids (repos, networks, issues, PRs, comments, check runs, pushes) are (seq << 12) | slot,
/// with seq leased from id_block(slot, kind) in the stripe that holds the slot, so ids move with the slot and
/// stay below 2^53 (research/wire-census.md#28). graph ids are plain sequences from graph's id_block.
pub struct DbId(i64);
impl DbId { pub fn slot(self) -> SlotId { todo!() } }

pub enum ContinentGroup { Americas, Europe, AsiaPacific }
pub struct StripeId(u16);
pub struct Stripe { pub id: StripeId, pub group: ContinentGroup, pub durability: Durability,   // Paired in managed
                    pub net: DsqlEndpoints, pub aux: DsqlEndpoints, pub packs: PackBuckets, pub cells: [CellId; 2] }
/// Compute only: one Region's node fleet and its NVMe caches. A cell holds no canonical row.
pub struct Cell { pub id: CellId, pub region: Region, pub azs: [AzId; 3], pub limits: CellLimits }

/// The one logical database.
pub struct Ledger { pub graph: DsqlEndpoints, pub stripes: Vec<Stripe>, pub inbox: [DsqlEndpoints; 3] }

/// Fence row slot_state(slot) in the stripe's net and aux clusters. Every Tx<Net> reads it (ACT-3, rekeyed).
pub enum SlotState { Serving, Frozen { generation: u64 }, Moved { to: StripeId, generation: u64 } }

/// One copy per Region in S3, written with If-Match, at most 5 slot changes per generation without a signed
/// override, last known good kept on local disk (ACT-7 unchanged). 4,096 entries fit in every edge's memory.
pub struct SlotMap { pub generation: u64, pub slots: Box<[StripeId; SLOTS]>, pub stripes: Vec<StripeRoute> }
pub struct StripeRoute { pub stripe: StripeId, pub cells: [CellId; 2], pub state: RouteState }
pub fn route(map: &SlotMap, repo: RepoId) -> &StripeRoute { todo!() }          // pure, two array reads

pub struct Graph; pub struct Net; pub struct Inbox;
/// Bound to exactly one cluster. No API holds two, so a cross-cluster transaction cannot compile.
pub struct Tx<'a, K> { _k: PhantomData<&'a K> }
impl Ledger {
    /// Refuses with NotLocal unless this node's Region hosts a read-write endpoint of the cluster; callers
    /// elsewhere forward the command to an api node there. No interactive transaction crosses a continent.
    pub async fn graph_tx<T, F>(&self, ctx: TxCtx, f: F) -> Result<Committed<T>, StoreError>
        where F: Fn(&mut Tx<'_, Graph>) -> Result<Change<T>, DomainError> { todo!() }
    pub async fn net_tx<T, F>(&self, slot: SlotId, ctx: TxCtx, f: F) -> Result<Committed<T>, StoreError>
        where F: Fn(&mut Tx<'_, Net>) -> Result<Change<T>, DomainError> { todo!() }
    pub async fn inbox_tx<T, F>(&self, g: ContinentGroup, ctx: TxCtx, f: F) -> Result<Committed<T>, StoreError>
        where F: Fn(&mut Tx<'_, Inbox>) -> Result<Change<T>, DomainError> { todo!() }
}
/// A new root network goes to the least-loaded stripe in the owner's residency group, else the creator's
/// nearest group. Forks take the parent's slot.
pub fn place_network(residency: Option<ContinentGroup>, near: ContinentGroup, load: &[StripeLoad]) -> SlotId { todo!() }
/// DESIGN.md's Clone, Freeze, Drain, Flip, Redirect, Forget, keyed by slot. Never across continent groups.
pub async fn move_slot(slot: SlotId, to: StripeId) -> Result<(), MoveError> { todo!() }
```

Decisions and invariants:

- **The stripe is a storage partition, not an owner home.** A slot holds many networks, and a network's slot never changes. Stripes exist for three reasons from the scale table: storage and connection quotas, continent groups, and a canary unit for schema waves. At launch the Americas group runs 4 stripes and Europe and Asia-Pacific 2 each [guess]. A stripe splits by moving slots, with the ACT-3 fence and a 60 s write freeze per slot.
- **Keys lead with the network id** in every stripe table (`ref(network, repo, name)`, `pull_request(network, repo, number)`), replacing `tenant_id`. A cross-owner merge therefore touches one key range, which DSQL's range partitioning keeps on few adjudicators [inference from the DSQL paper, sections 4.1 and 5.1.1].
- **Each stripe is served by two cells,** one in each of its Regions, both active. Only those cells' `git`, `api`, `worker`, and `actions` roles connect to the stripe, so each stripe sees a bounded fleet: 2 cells of at most 150 nodes at 20 connections each is 6,000 connections, which reconnect within 60 s at 100 per second [guess sizes]. Moving a stripe to other cells moves no rows, only cache warmth and runner sessions.
- **Only the `api` and `worker` roles connect to `graph`,** with at most 96 connections per cell between them [guess]. 40 cells hold 3,840 connections, and losing one AZ's third reconnects inside DSQL's 1,000-connection burst. `worker` needs `graph` only for `graph`'s own outbox consumers and saga steps.
- **Routing is statically stable.** A request becomes a slot through the cached name or the id, and the slot becomes cells through the in-memory `SlotMap`. Ids embed the slot, so `/repositories/{id}`, GraphQL node ids, and check-run ids route without a lookup. The edge needs `graph` only for a name it has never seen.
- **Residency.** An enterprise with a residency group gets new root networks only in that group's stripes. Its packs, Actions state, notifications, and audit stay there. Its namespace record (logins, membership edges, credential hashes) stays in `graph`. A fork lives with its network, so a residency org's fork of an outside public repository is outside residency, and enterprise policy can forbid such forks.

### Cross-cell features

In this design cross-owner features are not cross-cell, because cells hold no rows. What matters is which clusters each feature touches.

| Feature | Clusters | Atomicity |
| --- | --- | --- |
| Login claim, org create, membership, grant, team, app install, star, follow, watch | `graph` | One transaction |
| Push, PR create and sync, review, merge, across any owners in one network | One stripe | One transaction |
| Repository create, fork, rename, transfer, delete, visibility change | `graph`, stripe, `graph` | Saga of three idempotent steps |
| `GET /user/repos`, `viewer.repositories`, `/user/starred` | `graph`, plus batched stripe reads for git fields | One snapshot per cluster |
| Notifications, audit | Stripe or `graph` event, then the continent `inbox` | Idempotent insert keyed by event |
| Issue transfer between networks | Two stripes | Saga, copy then tombstone |

```rust
pub struct Star { pub user: UserId, pub repo: RepoId }
/// INSERT .. ON CONFLICT DO NOTHING. Bumps star_count(repo, hash(user) % 16) and emits only when inserted.
pub fn star(tx: &mut Tx<'_, Graph>, s: Star) -> Result<Change<bool>, DomainError> { todo!() }

/// Projections have one writer each, a consumer, applied when source_version is newer. Requests never write them.
pub struct RepoSummary { pub repo: RepoId, pub pushed_at: Timestamp, pub default_branch: RefName, pub open_issues: u32,
                         pub visibility: Visibility, pub archived: bool, pub source_version: u64 }   // stripe -> graph
pub struct StarCount { pub repo: RepoId, pub stars: u64, pub source_version: u64 }               // graph -> repo_head

pub struct Notification { pub user: UserId, pub thread: ThreadRef, pub event: Ulid, pub reason: Reason }  // inbox
pub async fn viewer_repositories(g: &Graph, viewer: UserId, page: Cursor) -> Result<Page<RepoSummary>, StoreError> { todo!() }
/// Groups ids by stripe and sends one batched snapshot read to a serving cell of each, in parallel.
pub async fn git_fields(s: &StripeClient, repos: &[RepoId]) -> Result<HashMap<RepoId, GitFields>, StoreError> { todo!() }
```

Invariants: stars, follows, and watches are set-membership rows, so a repeated request changes nothing, and counters move only with an insert or delete in the same transaction. A projection converges because each consumer applies the newest `source_version` and drops older ones, the same rule `DESIGN.md:557` uses for search. Notifications are keyed by `(user, thread)` and updated by `event`, so redelivery is harmless. Every cluster has its own outbox and pending rows (`DESIGN.md:496`-`561`), and a consumer leases partitions of one cluster at a time.

Audit events from `graph` (membership, SSO, installation, rename) go to the `inbox` of the owner's residency group, else the Americas, so an org's audit log reads at most its own group plus the Americas.

## Public service concerns

**Anonymous access and rate limits.** Anonymous callers read public repositories only. Rate limits are on in managed: 60 requests per hour per IP for the API, 5,000 per hour per user, and the LFS and GraphQL buckets GitHub documents (`research/wire-census.md#18`, `#35`). Counters are approximate, per node, with each node holding its share of a Region-wide limit. Anonymous git has per-IP concurrency and hourly caps (journey 1) and per-repository anonymous concurrency, so one scraper cannot take a hot repository's rendezvous set. Unknown or malformed tokens fail the CRC check at the edge with no lookup, so credential stuffing cannot reach `graph`.

**Abuse and spam.** Sign-up needs a verified email and passes a per-IP and per-/24 claim limit before `claim_login` runs. New accounts get tight write quotas (repositories, PRs, issues, and stars per hour) that `graph` enforces in the same transaction as the write. A `Spam` outbox consumer scores events from `graph` and every stripe. Its verdict is one `graph` row, `owner.restricted`, which `authorize` reads through `AuthCache` and renders as `Hidden` to everyone but the owner. Name squatting follows GitHub's policy ([username policy](https://docs.github.com/en/site-policy/other-site-policies/github-username-policy)) through a staff-only `graph` transaction that renames the squatter, and that transaction honors the quarantine.

**Takedowns.** A takedown is a stripe write to `repo_head.takedown` or to a network-wide `network_block` row. Every git and API request already reads `repo_head` in its snapshot, so enforcement is immediate. Both answer `451` with the notice URL [unverified that GitHub uses 451 on every path]. Search and listing projections drop the repository on the next event.

**CDN for public content.** CloudFront serves immutable public bytes: codeload archives and raw files keyed by commit SHA, release assets, avatars, and, at T2, `bundle-uri` bundles for hot repositories. The edge never redirects to an unconditional URL. It checks `repo_head` and then answers 302 to a CloudFront signed URL that lives 10 minutes and carries `repo_head.content_epoch`. A takedown or a switch to private bumps the epoch, so no new URL is issued and old ones expire, with no invalidation call on the request path. Git smart HTTP and the API are not cached.

**Enterprise orgs on a shared namespace.** SAML or OIDC attaches to an org or an enterprise account, and SCIM provisions `_shortcode` managed users. Residency pins root networks, packs, Actions, notifications, and audit to one continent group. Logins, membership edges, and credential hashes stay in the Americas `graph`. A customer that needs identity in-region too gets a BYOC install with its own namespace, the same way GitHub keeps GHE.com namespaces separate [inference].

## Failure modes

Every new stateful component on the request path. `net` keeps `DESIGN.md`'s `DSQL core cluster` containment, rekeyed from tenant to slot and from cell to stripe. A stripe's `aux` keeps `DESIGN.md`'s `aux` rows the same way.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| `graph` cluster | instance | `graph` transactions in flight on the failed DSQL host see errors or 40001 | Retries capped at 3 under a 10% retry budget. Writes carry the request id, so a retried commit is idempotent. |
| `graph` cluster | az | None for data. A third of each cell's `api` and `worker` connections to `graph` drop | At most 96 connections per cell, so reconnects fit DSQL's 1,000 burst. Requests keep using `AuthCache` and `NameCache` meanwhile. |
| `graph` cluster | region | One of its two Regions. Cells route `graph` commands to the survivor | DSQL commits through the witness (`research/uptime.md#26`). The `api` role picks the healthy endpoint from per-Region health, data plane only. Cross-continent cells were already forwarding. |
| `graph` cluster | control-plane | None at runtime | Created at install. Nothing at runtime or in recovery calls a DSQL management API. |
| `graph` cluster | bad-deploy | Service-wide: sign-up, OAuth login, first use of a credential on a cell, name claims, renames, forks, listings, stars. Git, merges, and stripe-served API calls continue on cached auth for 15 minutes for writes and 2 hours for reads | `graph` is a singleton, so it gets a shadow canary: a second cluster fed by CDC, where every migration and new prepared statement runs for 7 days under replayed production reads first. Frozen schema, additive tables only, fixed prepared statements with CI plan checks, 2 s statement timeouts. An AWS engine regression is not contained (`review/corebet-a.md`). |
| `graph` cluster | bad-config | One cell's `graph` pool or budgets | Signed config bundles roll in cell waves. Nodes validate them and keep the last known good. |
| `graph` cluster | noisy-tenant | One owner's bulk writes or a star bot, competing for shared query processors and the cell's `graph` connections | Per-owner and per-user write token buckets in the `api` role, per-endpoint shares of each cell's 96 connections, sagas of at most 1,000 rows, and DSQL heat-based splits (DSQL paper, section 7). |
| Network stripe (`net`) | instance | Transactions in flight on the failed host | `DESIGN.md`'s bounded retries and idempotent writes. |
| Network stripe (`net`) | az | A third of the two serving cells' connections | At most 6,000 connections per stripe reconnect within 60 s. |
| Network stripe (`net`) | region | One Region of the stripe. Its other serving cell keeps serving | Paired commit through the witness. Edges forward to the surviving serving cell using per-stripe health. |
| Network stripe (`net`) | control-plane | None at runtime | Pre-created. Slot moves use data-plane writes only. |
| Network stripe (`net`) | bad-deploy | One stripe, the internal canary stripe first. Stale fetches continue | Migrations roll stripe by stripe. Frozen git path (`DESIGN.md:786`) and fixed statements. |
| Network stripe (`net`) | bad-config | One stripe's two cells | Config waves by stripe, last known good. |
| Network stripe (`net`) | noisy-tenant | One network's hot rows and its stripe's shared budget | `DESIGN.md`'s per-repository admission, plus per-network transaction buckets. A slot above 20% of a stripe's limits moves to a stripe of its own. |
| `inbox` cluster | instance | Notification and audit writes in flight | Retries. Writes are keyed by event. |
| `inbox` cluster | az | A third of its connections | Small fleet, reconnects inside the burst. |
| `inbox` cluster | region | One Region of the group | Paired within the group. |
| `inbox` cluster | control-plane | None at runtime | Pre-created. |
| `inbox` cluster | bad-deploy | Notifications and audit for one continent group. Git, PRs, and the API keep serving | `Notifications` and `Audit` consumers pause without parking rows (`DESIGN.md:561`). Rolls after the stripes. |
| `inbox` cluster | bad-config | Same group | Signed config waves. |
| `inbox` cluster | noisy-tenant | One recipient flooded by mentions | Per-recipient notification caps per hour, coalesced per thread. |
| Slot map | instance | One edge's memory copy | Reloaded from local disk. |
| Slot map | az | None | Each Region reads its own S3 copy. |
| Slot map | region | That Region's copy | Other Regions read their own copies. Edges keep the last known good. |
| Slot map | control-plane | None | S3 data-plane reads only. |
| Slot map | bad-deploy | A buggy writer could misroute slots | If-Match generations, at most 5 slot changes per generation without a signed override, and `SlotState` in each stripe refuses writes from a stale route. |
| Slot map | bad-config | Same as bad-deploy | Same guard. Nodes keep the last known good. |
| Slot map | noisy-tenant | None. It has no per-owner state | A hot slot moves under a signed override. |
| Name and auth caches | instance | One node refills | The `api` role refills single-flight under a per-cell read budget. |
| Name and auth caches | az | A third of the cell's nodes refill at once | The cell copy in the `api` role stays warm. Refill is capped per cell, so `graph` sees a bounded burst. |
| Name and auth caches | region | None. Both Regions serve all the time, so both caches are warm | Nothing to do. |
| Name and auth caches | control-plane | None | No AWS API involved. |
| Name and auth caches | bad-deploy | A bug that grants wrongly, in the canary AZ | `authorize` is pure and property-tested. 1% of cached decisions are re-checked against `graph` synchronously, and a mismatch aborts the wave. |
| Name and auth caches | bad-config | Max ages set too long | Signed config, with compiled ceilings of 2 hours for reads and 15 minutes for writes. |
| Name and auth caches | noisy-tenant | A credential sprayed across nodes | CRC rejects random tokens offline. Unknown hashes are cached negatively for 60 s, with per-IP auth-failure limits. |

## Deployment modes

BYOC and self-hosted run the same binary with the ledger collapsed. `graph`, the only stripe's `net`, and `inbox` all point at the install's `core` cluster, and the stripe's `aux` is the install's `aux`. All 4,096 slots map to stripe 0, served by the install's one cell. The code still opens one `Tx<Graph>` or one `Tx<Net>` at a time, even though both reach one cluster, so the code paths are identical and a large install can later add a stripe with `move_slot`. Each install has its own login table, so its namespace is its own, as on GHES. The quarantine, epochs, sagas, and caches run unchanged. They cost a few rows and save a second code path.

| Aspect | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Namespace | One global, `tributary.dev` | The install's own | The install's own |
| Ledger | `graph` + stripes (`net`, `aux`) + 3 `inbox` | `core` and `aux` only | `core` and `aux` only |
| Cells | Compute only, two per stripe | One | One |
| Slot moves | Stripe splits and hot slots | Rare, to add a stripe | Rare |
| Rate limits, spam, takedowns | On | Off by default | Off by default |
| Signing keys | One service key ring | One per install | One per install |

## Changes to DESIGN.md

Deletions first.

- **Summary.** Delete "Each tenant is one GHES-shaped hostname" and the per-tenant stale limit wording.
- **Usage.** Delete the managed quickstart's host label and per-tenant placement. Delete the `acme.tributary.cloud` host from journeys 1 to 3, and delete journey 1 step 2 (SNI to tenant).
- **Tenancy and cell placement.** Delete `TenantId`, `TenantHost`, `TenantMap`, `TenantRoute`, `route(sni)`, `TenantState`, `MAX_ROUTE_CHANGES` keyed by tenant, `migrate_tenant`, the bullets "The tenant is the host", "SSH resolves the tenant from the port", "Ids are tenant-local", "Fork networks live inside a tenant", and tenant placement.
- **Repository storage and ref transactions.** Delete `tenant` from `RepoKey` and from every table key, and delete `tenant_state`.
- **Identity and tokens.** Delete per-tenant signing keys wrapped by the cell key, "seeded into every tenant", SSO "per tenant", the per-tenant credential and permission snapshot in S3, and `id_block(tenant, kind)`.
- **Wire compatibility.** Delete the `git-ssh` oracle clause "one key registered in two tenants of one cell" and the per-tenant ports. Delete "seeded in every tenant at creation".
- **Failure modes.** Delete the per-tenant NLB rows and the tenant-map wording in the edge rows.
- **Deployment modes.** Delete the "Tenants per cell" and managed "Host and certificate" rows and the `tributary-fleet` tenant placement.
- **Tradeoffs accepted.** Delete one NLB per tenant, one host per tenant with no global graph, and the per-tenant migration freeze.
- **Alternatives considered.** Delete "A github.com-style shared namespace". It is now the requirement.
- **Open questions.** Delete the NLB-per-tenant quota question.
- **Review resolutions.** Mark ACT-6 superseded, because one namespace removes the capture attack.

Replacements.

- **Title and Summary.** "Refs are rows in network stripes of one logical DSQL ledger". Managed is one host with one namespace. BYOC and self-hosted keep their own.
- **Tenancy and cell placement** becomes "Ledger, stripes, and routing", with this document's "Placement and routing" types. `SlotState` replaces `TenantState` as the write fence. `move_slot` replaces `migrate_tenant` with the same steps. `SlotMap` replaces `TenantMap` with the same S3, If-Match, cap, and last-known-good rules (ACT-3, ACT-7 rekeyed).
- **Repository storage.** `RepoKey { network, repo }`, with the slot derived from the id. Tables lead with `network`. `run_tx` becomes `Ledger::net_tx`, which reads `slot_state`. `refresh` also reads `repo_head`.
- **Identity and tokens.** `Principal` and `InstallationClaims` lose `tenant`. A service key ring replaces tenant keys. `AuthCache` replaces the S3 credential snapshot for both normal and stale paths (ACT-14 rekeyed). Users, orgs, teams, and permissions move from `core` to `graph`, and `DESIGN.md:379`'s same-snapshot rule narrows to repository-owned facts.
- **Issues and pull requests.** `HeadSpec` keeps "same network as the base repo", now enforced by slot allocation. The merge reads the actor's `Grant` from `AuthCache`.
- **Event outbox.** One outbox per cluster. Stripe partitions are `hash(network) % 256`, and `graph` partitions are `hash(owner) % 256`. `CacheHint` fans `graph` hints to every Region.
- **Service and crate map.** The `edge` routes by path, name, and slot. Only `api` and `worker` connect to `graph`. `trib-store` exposes `Ledger` and `Tx<K>`. `tributary-fleet` places networks, moves slots, and pairs stripes with cells.
- **Wire compatibility.** The `git-ssh` row becomes one service host key per Region on port 22, with keys unique service-wide. The `rest:repos` row adds rename, transfer, and retirement oracles. The `rest:rate-limit` row is on in managed.
- **Failure modes.** The `DSQL core cluster` rows become network stripe rows. The edge's `bad-config` row guards the slot map.
- **Deployment modes.** Use this document's table.
- **Build order.** Milestone 2 adds `graph`, `claim_login`, `NameCache`, and `AuthCache`. Milestone 4's tenant migration becomes a slot move under load, and its gate adds a rename-and-reclaim race harness.

Additions.

- **Names and the directory**, **Public service concerns**, and the `graph`, `inbox`, slot map, and cache failure rows from this document.
- **Known deviations.** A released login is claimable after 1 hour, while GitHub frees it immediately. Repository creation at an alias waits for the alias to settle, which takes seconds.
- **SLOs.** Add "sign-up, sign-in, and name changes" at 99.95%, measured service-wide, because `graph` is one cluster.
- **Milestone 0.** Add a `graph` load run at the modeled miss rate, a unique-key claim race, a full reconnect timing at 6,000 connections, and two questions for AWS: the connection ceiling, and whether engine upgrades roll out per continent group or per cluster.
- **Drills.** A weekly drill blocks `graph` from one canary cell and checks that git, merges, and stripe-served API calls keep working on cached auth.

## Tradeoffs accepted

- We accept one `graph` cluster that every request depends on for first-use authentication and unseen names, and whose bad migration or query stops sign-up, sign-in, renames, forks, and listings service-wide. In exchange, logins, memberships, installations, stars, and viewer listings are all single-transaction or single-snapshot, with no directory protocol and no cross-cell reads. The shadow canary, the frozen schema, and cache-first request paths bound the cost.
- We accept that push and merge authorization reads a cached `Grant`, normally seconds behind `graph` and at most 5 minutes with a stalled hint path, instead of the merge snapshot. The merge itself (head ref, PR, base ref) stays one transaction for every cross-owner PR.
- We accept a one-hour quarantine before a released login can be claimed, in exchange for name caches that can route without `graph`.
- We accept a cross-continent round trip for European and Asian users on `graph` misses and writes, and on pushes to networks placed in another group, as github.com users outside the US see today [inference].
- We accept that residency covers repository data, Actions, notifications, and audit, but not the namespace record, and that forks follow their network.
- We accept more clusters: `graph`, its shadow, three `inbox` clusters, and two per stripe, each in its own account.
- We accept, unchanged from `DESIGN.md:892`, that a DSQL engine regression reaches every cluster at once.

## Alternatives considered

- **Literally one DSQL cluster for all metadata.** It is this design with one stripe and no `graph` split, and it is what BYOC and self-hosted run. For the public service it lost on the scale table. Storage passes 256 TiB at github.com size. Reconnect time grows with the fleet at 100 connections per second. One cluster cannot reach a second continent or meet residency. And our own DDL has no canary unit and conflicts with in-flight transactions for every owner at once.
- **Owner-homed cells with a global name directory.** Each user or org's metadata lives in one cell, and a thin global tier maps names to cells. It lost because a fork network spans owners. A PR from `bob`'s cell into `acme`'s cell cannot compare-and-set both refs in one transaction, so merges need a cross-cell protocol. A transfer moves data between cells. And `viewer` queries and org permissions scatter across cells.
- **Stripes keyed by repository hash, with no network rule.** It spreads load evenly, but a fork and its upstream land in different stripes, so the cross-owner merge stops being atomic and packs cannot be shared by the network.

## Open questions and risks

- What connection ceiling will AWS grant per cluster, and is the 100-per-second rate truly fixed for large customers? These set the stripe count more than storage does.
- Does AWS roll out DSQL engine versions per Region, per continent group, or per cluster? A per-group rollout would give stripes natural waves, and a fleet-wide one stops `graph` and every stripe together.
- Can a cell account authenticate to a stripe or `graph` cluster in another account by assuming a role there, with tokens signed locally? [unverified]
- How fast do DSQL heat splits react to a repository that goes viral? Splits are DSQL control-plane work (DSQL paper, section 7), so a sudden hot range waits on AWS automation [inference].
- How large is `graph` at github.com scale? Users, repositories, grants, and stars suggest single-digit TiB [guess]. A larger result splits `graph` by table family, with stars and follows first, before anything on the auth path.
- Can CDC feed the `graph` shadow at production write rate within the 5 streams per cluster quota?
- Is a one-hour login quarantine acceptable to users who rename and expect the old name to free up at once?
