# Tributary on Cloudflare, candidate 1: one Durable Object per aggregate, upstream git in Containers

## Summary

The bet: every repository, fork network, owner, account identity, stored token, and globally unique name is one SQLite-backed Durable Object whose SQLite file is both the chain and the fold. A commit is one synchronous transaction inside that object, which is single-threaded, so there is no conflict, no retry loop, and no separate fold engine, and every read inside the object is consistent by construction (`research/cloudflare.md`, Durable Objects; `research/cloudflare-git.md` implication 2).
Pack bytes live in R2. Upstream git (index-pack, upload-pack, pack-objects, repack, merge-tree) runs in stateless Containers that hydrate a network's packs from R2. Workers are the front door for HTTPS, REST, GraphQL, and the React app. Spectrum and Cloudflare Tunnel carry SSH to gateway containers. Queues carry dirty markers, never events. Basin holds the audit log and nothing else.
No Cloudflare page says whether a Durable Object's five followers span regions (`research/cloudflare-uptime.md#32`, open question 1). So durability is a class. `Regional` acknowledges on the object's own 3-of-5 follower quorum. `Paired`, the managed and BYOC default, acknowledges only after a standby object under a second location hint has applied the entry and the pack sits in an R2 bucket in each location, with an R2 placement register in a third location as the arbiter that fences a failover.
A GitHub user notices: the same wire, issue and pull request lists that are exact right after a write, HTTPS pushes capped by the zone plan (200 MB on Business, up to 5 GB on Enterprise) so larger pushes go over SSH, networks capped at 12 GB packed, and a global Cloudflare outage that takes the whole service down with it.
A maintainer notices, against `DESIGN.md`: no fold engine, cells, slot moves, node fleet, `Mirror` chains, heartbeats, or in-account operator role. In exchange they own a pairing protocol (lease, seal, compare-and-swap on R2), per-object SQLite migrations under gradual deployment, a git-worker cache layer with a 20 GB disk ceiling, and a platform whose storage products carry no SLA (`research/cloudflare-uptime.md#29`).
Monthly list cost from `cost.py`: $150 Regional and $212 Paired for 50 developers, $5,110 Paired for 2,000, and $80 for the cheapest reliable 50-developer setup.

## Usage (caller's view)

**Developer.** Nothing changes from GHES. `git clone https://git.example.com/acme/mono.git`, `gh auth login --hostname git.example.com`, `ssh://git@ssh.git.example.com/acme/mono.git` for SSH, and `GH_HOST=git.example.com gh pr list`. The conformance oracles are those of `DESIGN.md` "Wire compatibility" and "Known deviations from GitHub", reused unchanged, plus three deviations listed in "Tradeoffs accepted".

**Operator quickstart (BYOC or self-hosted, in the customer's own Cloudflare account).**

```sh
# 1. Account: Workers Paid, a zone on Pro or higher (Business recommended, Enterprise for pushes over 200 MB on HTTPS),
#    and an account API token scoped to Workers, Durable Objects, Containers, R2, Queues, DNS, Spectrum, and Tunnel.
tributary release verify --release tributary-2027.03.1       # release signature, the vendor's existing registry and key
# 2. Durability and placement. Paired names three location hints: primary, standby, and the placement-register bucket.
tributary install plan --account "$CF_ACCOUNT" --zone git.example.com \
  --durability paired --primary enam --standby wnam --arbiter weur
# 3. Terraform for buckets, queues, DNS, the Spectrum SSH app, and the tunnel; wrangler for Workers, object classes, containers.
tributary install apply
# 4. First admin: a one-time sign-in link, valid 15 minutes [guess].
tributary admin bootstrap --first-admin alice
```

**Call sites (Rust on workers-rs, which supports SQLite-backed Durable Objects: https://github.com/cloudflare/workers-rs, "SQLite Storage in Durable Objects").**

```rust
// Worker, POST /{owner}/{repo}.git/git-receive-pack. Five calls, each one deep module.
let who = auth.authenticate(&req, AccessClass::Write).await?;          // isolate cache; Token object on a miss or 5 s re-check
let grant = authz.authorize(&who, &owner, &repo, Action::Push).await?; // AuthzReplica(owner, k): repo id, network, placement, stamp
let (cmds, body) = receive::split_commands(req.body()).await?;          // pkt-line commands; the pack stays a stream
let pack = git.receive(grant.network, packs.stage(body).await?, grant.copies).await?; // R2 staging, index-pack, place, register
let acked = repo(&grant).commit(Push::new(cmds, pack, grant.stamp)).await?;          // one SQLite transaction, then the standby
receive::report_status(&acked)                                          // takes &Acked, so "ok" cannot precede the commit

// `gh issue create --label bug`, then `gh issue list --label bug`, then the same object answers both.
repo(&grant).commit(OpenIssue { title, body, labels: vec!["bug".into()], actor }).await?;
let page = repo(&grant).read(|f| f.issues(&IssueFilter { labels: vec!["bug".into()], ..Default::default() })).await?;
```

## Stack

| Product | Status | Used for |
| --- | --- | --- |
| Workers | GA | Front door: TLS, token and session checks, routing, smart HTTP, REST, GraphQL, OAuth, the React app's static assets, webhook delivery, Queue consumers |
| Durable Objects (SQLite-backed) | GA | Every aggregate: repository, network, owner, identity, claim, token, plus standbys, authorization replicas, check suites, runs, hooks, homes, search shards, and each container's object |
| R2 | GA | Packs and indexes (one bucket per location in the copy set), push staging, clone cache, chain archive, placement registers, format control file, Actions cache, artifacts, and logs |
| Containers | GA | Git workers running pinned upstream git, maintenance workers, SSH gateways running `cloudflared` and a Rust SSH server |
| Sandbox SDK (Sandboxes) | GA | Managed hosted Actions runners, one job per sandbox |
| Queues | GA | Dirty markers from chain objects to consumers, sharded per consumer |
| Spectrum | GA | Public SSH on port 22 |
| Cloudflare Tunnel | GA | Spectrum's private origin: connectors inside the SSH gateway containers |
| DNS | GA | Authoritative zone, `raw.`, `codeload.`, `ssh.` names, custom domains in BYOC |
| WAF and rate limiting rules | GA | Per-IP limits on login, clone, and anonymous API |
| Workers Logs | GA | Request and error logs, 7 days |
| Workers Analytics Engine | GA | Clone and view traffic API, SLO counters, per-object load signals |
| Pipelines (now Basin Pipelines) | GA | Audit event stream from the `Audit` consumer |
| R2 Data Catalog (now Basin Catalog) | GA | Iceberg tables for the audit log |
| R2 SQL (now Basin SQL) | GA | Audit log API queries and enterprise exports |
| Cron Triggers | GA | Sweepers: orphan staged packs, lagging cursors, placement audits, fold digests |
| Workflows | GA | GHES import, network garbage collection, standby seeding and planned moves |
| Email Routing | GA | Reply-by-email to issues and pull requests |
| Email Sending (Email Service) | beta | Notification and password-reset email, off every core path |
| Access | GA | Staff admin console |
| workerd | GA [inference] | Local development, the conformance harness, and the air-gapped single-host mode |

Not used: Workers KV and D1 (single primary, eventually consistent replicas, and KV's June 2025 failure took DO and D1 with it, `research/cloudflare-uptime.md#9`, `#37`, `#38`), Artifacts and K2 (beta, see "Alternatives considered"), Secrets Store (beta; signing keys are Worker secrets), Hyperdrive, Vectorize, AI Search, Load Balancing, Workers for Platforms, Dynamic Workers, Workers VPC, and the Data Localization Suite, which only a residency customer on Enterprise adds.

## Shape

### Objects and placement

Every canonical fact is a chain entry in one object's SQLite, or an immutable R2 object that an entry names. Containers, isolate caches, replicas, readers, the clone cache, and search shards hold only rebuildable state. An object's name is derived from its key, so no directory sits on the request path.

| Object class | Key | Holds | Durability | Placement |
| --- | --- | --- | --- | --- |
| `Repo` | repository id | Chain and fold: refs, rules, collaborators, deploy keys, pull requests, reviews, required checks, issues, comments, labels, FTS5 tables, consumer cursors | Install class | The owner's home hint, chosen at creation (`research/cloudflare.md`, DO location hints) |
| `Network` | network id (root repo id) | Pack manifest, fork membership, maintenance lease, compaction entries | Install class | The root repository's hint |
| `Owner` | account id | Authorization fold: login, members, teams, grants, repository names, policies, rulesets, installations, webhooks, Actions secrets as ciphertext | Install class | Chosen at signup |
| `Identity` | account id | Password hashes, second factors, sessions, SSO links, token list | Install class | As its `Owner` |
| `Claim` | login, verified email, SSH fingerprint, or app slug | Which account holds the key | Install class | First creator's hint |
| `Token` | SHA-256 of a stored token | Account, kind, scopes, expiry, revocation, SSO orgs | Install class | Creator's hint |
| `AuthzReplica` | (owner, k) | Synchronous copy of one `Owner` fold, k in 0..16 | Derived | Spread over hints by owner traffic |
| `RepoReader` | (repo, k) | Read copy of a hot `Repo` fold | Derived | Only above the read threshold ("Scale") |
| `Checks` | (repo, head SHA) | Check suites, runs, and commit statuses for one commit | Regional | The repository's hint |
| `Run`, `RunnerScope` | run id; (owner, label set) | Actions run and job state machines; queued jobs and long polls | Regional | The repository's or owner's hint |
| `Hook` | hook id | Delivery records for 3 days | Regional | The owner's hint |
| `Home` | account id | Stars, watches, notifications, feed, repository cards | Regional | As its `Owner` |
| `SearchShard` | shard number | FTS5 over issues, pull requests, and code across repositories | Derived | Per residency domain |

### Chains and commits

```rust
pub struct Seq(u64);
/// Bumped by each promotion, planned move, or re-protect. Every entry carries it; a standby refuses any other.
pub struct Generation(u32);
pub enum Kind { Repo, Network, Owner, Identity, Claim, Token }
/// "{kind}:{key}:{incarnation}:{a|b}". Incarnation 0 side `a` is the default primary, so a route needs no lookup until an
/// object answers `Moved` or stops answering; then the Worker reads the placement register.
pub struct ObjectName { pub kind: Kind, pub key: Key, pub incarnation: u32, pub side: Side }
pub enum Side { A, B }
pub enum LocationHint { Wnam, Enam, Sam, Weur, Eeur, Apac, ApacNe, ApacSe, Oc, Afr, Me }
pub enum Durability {
    /// Acknowledged once the primary's SQLite commit is confirmed, which the platform does after 3 of 5 followers in
    /// different data centers have it (research/cloudflare.md, DO "Write acknowledgement", "Output gate").
    Regional { primary: LocationHint },
    /// Also acknowledged by a standby object under `standby`, with each pack in a bucket under each hint. The placement
    /// register lives in a bucket under `arbiter`, so any one location can be lost and two remain to decide.
    Paired { primary: LocationHint, standby: LocationHint, arbiter: LocationHint },
}
/// R2 object `placement/{kind}/{key}` in the arbiter bucket. Replaced only by PutObject with If-Match on its ETag, which R2
/// evaluates atomically (research/cloudflare.md, R2 "Conditional writes"). Read only on `Moved`, failure, or move.
pub struct Placement { pub generation: Generation, pub primary: ObjectName, pub standby: Option<ObjectName>, pub state: PairState }
pub enum PairState { Paired, Alone { since: Timestamp, why: AloneReason }, Seeding { standby: ObjectName, upto: Seq } }

/// One row in the object's `log` table, inserted in the same transaction as its fold changes.
pub struct Entry<E> { pub generation: Generation, pub seq: Seq, pub prev: Digest, pub format: Format, pub at: Timestamp,
                      pub requests: SmallVec<[RequestId; 4]>, pub events: SmallVec<[E; 4]> }
pub trait Aggregate { type Event: Encode + Decode;
    /// The only writer of fold tables. Runs inside the commit's transaction, on the primary and, identically, on the standby.
    fn apply(tx: &SqlTx, e: &Entry<Self::Event>) -> Result<(), ApplyError>;
    /// Additive steps only (new tables, new nullable columns, indexes on bounded tables), run per object on first wake.
    const MIGRATIONS: &'static [Migration]; }
pub trait Command<A: Aggregate> { type Out; type Err;
    /// Pure over the fold as of the previous entry. Inputs from other objects arrive as sealed stamps, never as reads.
    fn decide(&self, fold: &FoldView<A>) -> Result<(SmallVec<[A::Event; 4]>, Self::Out), Self::Err>; }

impl<A: Aggregate> Chain<A> {
    /// One `transactionSync`: decide, insert the entry at head + 1, apply. The output gate holds the reply until the platform
    /// confirms the write (research/cloudflare.md, DO "Output gate"). When Paired, the reply also waits for the standby's
    /// `LeaseGrant` covering this seq. A retried request finds its `RequestId` in the recent log and gets the same answer.
    pub async fn commit<C: Command<A>>(&self, cmd: C) -> Result<Acked<C::Out>, CommitError<C::Err>>;
    /// Reads the fold at the acknowledged watermark: waits until every entry committed before the read arrived is acknowledged,
    /// and, when Paired, until a lease covers now. Writes in flight cost a waiting read at most one standby round trip.
    pub async fn read<R>(&self, q: impl FnOnce(&FoldView<A>) -> R) -> Result<R, ReadError>;
    /// Entries after a consumer's cursor, from `log` or, below the trim point, from the R2 archive.
    pub async fn pull(&self, c: Consumer, lease: ConsumerLease, max: u32) -> Result<Batch<A::Event>, PullError>;
    pub async fn advance(&self, c: Consumer, lease: ConsumerLease, to: Seq) -> Result<(), PullError>;
}
pub struct Acked<T> { pub seq: Seq, pub out: T, _sealed: Sealed }   // only `commit` builds one
pub enum CommitError<E> { Rejected(E), Overloaded, Moved(Placement), Fenced, Unavailable }
```

Invariants the structure enforces:

- **One writer, no conflicts.** A Durable Object is single-threaded and `transactionSync` admits no interleaving, so `decide` always sees the head and every command on one aggregate serializes. A merge cannot race a review, a rule change, a required check, or a push on the same repository, because all of them are entries on its chain. This replaces the conditional put, the next-key freshness probe, and the retry budget of `DESIGN.md` "Chains and commits".
- **The fold is the chain's own table set.** `apply` writes fold rows in the transaction that inserts the entry, so no derived copy can lag, and `read` never needs a freshness proof. Lists, filters, and sorts are indexed SQL in the same file (FTS5 included, `research/cloudflare-git.md#31`).
- **Acknowledged means durable in the class.** `Acked` exists only after the output gate opens and, when Paired, the standby applied the same `(generation, seq)`. `report_status` takes `&Acked`.
- **Reads never show an unacknowledged entry.** The primary keeps `acked_seq`, and `read` waits for it to pass the head seen at arrival, so a failover cannot take back anything a reader saw.
- **The log is trimmed, the chain is not.** Every 10 minutes or 10,000 entries the primary writes an archive segment to R2 (each bucket of the copy set, If-None-Match: *) and deletes archived rows older than the slowest consumer's cursor, so a hot chain keeps under an hour of log in SQLite. Every fold can be rebuilt by replaying genesis from the archive.
- **Every cross-object input is a stamp.** A push carries an `AuthzStamp` minted by an `AuthzReplica`, and a fork's `Genesis` carries the parent's `(repo, seq, refs digest)`. `decide` checks the stamp's seal and age, never calls out, so no transaction spans two objects.

### Pairing: what an acknowledged write survives

```rust
/// Primary to standby, in seq order, pipelined. The standby applies with the same `apply` in one transaction and answers
/// only after its own output gate, so the grant implies the entry is confirmed in the standby's location.
pub async fn replicate(standby: &Stub, e: &EncodedEntry) -> Result<LeaseGrant, ReplicateError>;
pub async fn renew(standby: &Stub, generation: Generation) -> Result<LeaseGrant, ReplicateError>;
/// The primary may acknowledge and serve reads only while `sent_at + LEASE` is in the future on its own clock, measured from
/// before it sent. The standby never promotes before `last_grant + LEASE + MARGIN` on its own clock. Only clock rate matters.
pub struct LeaseGrant { pub generation: Generation, pub applied: Seq }
pub const LEASE: Duration = Duration::from_secs(5);
pub const MARGIN: Duration = Duration::from_secs(1);
pub enum StandbyRole { Following { last_grant: Instant, applied: Seq }, Sealed { generation: Generation }, Promoted }
/// Workers call this after a failed or timed-out call to a primary. The standby promotes when reports come from two
/// distinct colos within 10 s, no grant is younger than LEASE + MARGIN, and the register CAS from `generation` to
/// `generation + 1` with itself as primary succeeds. It then seals the old generation and serves, `Alone` until reseeded.
pub async fn suspect(standby: &Stub, from: Colo, generation: Generation) -> Result<Verdict, SuspectError>;
/// Planned move, re-protect, and PITR recovery share one path: seed a new object from the current primary, then CAS.
pub async fn seed_and_flip(from: &ObjectName, to: ObjectName, register: &PlacementRegister) -> Result<Placement, MoveError>;
```

What each class survives, stated without assuming anything about follower geography:

- **Regional.** Loss of the primary's machine or its data center loses no acknowledged write, because 3 of 5 followers in other buildings confirmed it and the platform fences the old instance before a new one starts (`research/cloudflare.md`, DO "Failure"; `research/cloudflare-uptime.md#32`). Loss of a whole region is unknown: the followers sit in "four other data centers nearby", so the design assumes they can all be lost and says the RPO is unknown, not zero ("Downtime").
- **Paired.** Every acknowledged entry is confirmed in two location hints, and its packs are in a bucket under each, before the client hears "done". Loss of either location loses nothing acknowledged. The register in the third location decides who is primary, so a partition between the two sides cannot make two primaries: a promotion needs the register CAS, and the old primary cannot acknowledge after the standby seals, because every acknowledgment needs a fresh grant.
- **Who promotes.** Nobody central. Requests drive it: a Worker that cannot reach a primary reports to its standby, and the standby decides. An idle repository never fails over until someone touches it, and no step calls the Cloudflare API, so a control-plane outage cannot block recovery (`research/cloudflare-uptime.md` implication 9).
- **After a promotion** the survivor serves `Alone`, acknowledging on its own quorum, until `seed_and_flip` builds a new standby under another hint and flips the register to `Paired`. The status API and `/status` show every `Alone` object. A US-only residency pair has only `wnam` and `enam`, so after losing one it stays `Alone` until that region returns, unless the customer allowed a third hint.

### Repository, packs, and git workers

```rust
pub struct ObjectId([u8; 20]);                         // SHA-1 at launch; SHA-256 is a new variant
pub struct RefName(Box<[u8]>);                         // check-ref-format valid
pub struct RefUpdate { pub name: RefName, pub expect: Option<ObjectId>, pub new: Option<ObjectId> }
pub enum Atomicity { AllOrNothing, PerRef }
pub struct PackId(Ulid);                               // fresh per placement, never derived from content
pub struct ManifestVersion(u64);                       // bumped by every registration and compaction in a `Network`
pub struct PackRef { pub id: PackId, pub bytes: u64, pub checksum: [u8; 20], pub copies: BucketSet, pub manifest: ManifestVersion }
/// Built only by `GitWorker::receive`: pack and .idx are in every bucket of the copy set (If-None-Match: *) and the
/// network's `Network` object has committed `PackRegistered`. A pack with zero objects yields None.
pub struct DurablePack { pack: PackRef, _sealed: Sealed }
pub struct Push { pub updates: BoundedVec<RefUpdate, 10_000>, pub atomicity: Atomicity, pub pack: Option<DurablePack>,
                  pub stamp: AuthzStamp, pub options: PushOptions, pub request: RequestId }   // impl Command<Repo>
pub enum NetworkEvent { ForkAdded(RepoId), PackRegistered(PackRef), Compacted { consumed: Vec<PackId>, produced: Vec<PackRef> },
                        Takedown(TakedownEvent) }
pub enum R2Key { Stage { push: PushId }, Pack { network: NetworkId, id: PackId }, Index { network: NetworkId, id: PackId },
                 Archive { chain: ObjectName, from: Seq, to: Seq }, CloneCache { network: NetworkId, key: [u8; 32] },
                 Bundle { network: NetworkId, id: Ulid }, Placement { kind: Kind, key: Key }, Formats }
/// Refs at the acknowledged watermark plus the minimum manifest the git worker needs. One `Repo` call per fetch.
pub struct RefSnapshot { pub repo: RepoId, pub seq: Seq, pub refs: RefsBlob, pub min_manifest: ManifestVersion }

/// A Container class per region set (Containers placement is per class: https://developers.cloudflare.com/containers/concepts/placement/),
/// instance "gw:{network}:{k}" with k in 0..K, K = 1 for most networks. Each instance keeps one bare repository per network
/// it serves, packs from R2, refs written per repository under GIT_NAMESPACE, and pinned upstream git.
pub trait GitWorker {
    async fn fetch(&self, snap: RefSnapshot, req: UploadPackRequest) -> Result<ByteStream, GitError>;
    /// index-pack --strict --fix-thin against the cached network, connectivity against `refs`, place in every bucket, register.
    async fn receive(&self, net: NetworkId, staged: R2Key, refs: RefSnapshot, copies: BucketSet) -> Result<Option<DurablePack>, GitError>;
    /// merge-tree and commit-tree, then the tree-diff check that the result is base plus the pull request's own diff.
    async fn merge_plan(&self, net: NetworkId, input: MergeInput) -> Result<MergePlan, GitError>;
}
pub const NETWORK_PACKED_LIMIT: u64 = 12 << 30;        // a standard-4 has 20 GB of disk (research/cloudflare.md, Containers)
/// An instance takes a receive job only if free disk >= 2 x staged bytes + 1 GB; otherwise the next k.
pub fn admit(free_disk: u64, job: &JobSize) -> bool;
```

Invariants:

- **Bytes before entry.** `Push` takes a `DurablePack`, which only `receive` builds after every copy is in R2 and the network registered it. A registered pack that no entry ever names is swept after 24 hours.
- **A network is one object store.** Every fork shares its network's packs, so a fork costs no bytes and a fork pull request needs no copy. The git worker serves each repository's refs through `GIT_NAMESPACE`, so one bare repository per network holds every fork.
- **Containers are caches.** Losing an instance, or its disk on sleep (`research/cloudflare.md`, Containers "Persistence"), costs a rehydrate from R2. Maintenance runs in a separate `GitMaint` class under the `Network` object's lease: geometric repack with bitmaps, cruft packs for unreachable objects, `Compacted` committed after the new packs are placed, deletion only of objects unreachable from every member's fold for 14 days.
- **The clone cache is exact.** A fetch with no `have` lines (clones, shallow CI checkouts) is keyed by SHA-256 of (network, wanted tips, depth, filter, capabilities, object format). The first request fills `CloneCache` through a single-flight in the git worker's object, and later identical requests stream the stored response from R2, whose egress is free (`research/cloudflare.md`, R2 price). Authorization runs before every cache read.

### Owners, authorization, and identity

```rust
pub enum OwnerEvent { Opened { kind: AccountKind, enterprise: Option<AccountId> }, Login(LoginKey),
    Member { user: AccountId, role: Option<OrgRole> }, Team(TeamEvent),
    Grant { repo: RepoId, grantee: Grantee, permission: Option<RepoPermission> },
    RepoName { key: RepoNameKey, entry: Option<RepoEntry> }, Installation { id: InstallationId, state: InstallState, not_before: Timestamp },
    Policy(OrgPolicy) /* base permission, SSO, rulesets, residency */, Replicas { count: u8, hints: SmallVec<[LocationHint; 16]> },
    Pending(CrossOp) /* a fork, transfer, or creation this owner finishes */, Deleted { retain_until: Timestamp }, Undeleted }
/// `Reserved` is a fork's, creation's, or transfer's commit point and counts as taken.
pub enum RepoEntry { Reserved { repo: RepoId, from: Option<AccountId> },
                     Live { repo: RepoId, network: NetworkId, visibility: Visibility, placement: ObjectName },
                     Alias { repo: RepoId }, Deleted { repo: RepoId, retain_until: Timestamp } }
/// Minted by an AuthzReplica, valid 30 s, carried into the repository's `decide`, which adds collaborator and deploy-key facts.
pub struct AuthzStamp { pub owner: AccountId, pub owner_seq: Seq, pub actor: Actor, pub permission: RepoPermission,
                        pub minted: Timestamp, _sealed: Sealed }
/// The `Owner` primary replicates each committed entry to every replica (at most 6 calls in flight per request,
/// https://developers.cloudflare.com/durable-objects/platform/limits/) and acknowledges a revocation-class entry only when
/// each replica applied it or its lease from this owner expired.
pub struct ReplicaLease { pub owner_seq: Seq, pub until: Instant }   // 10 s, renewed every 3 s while the replica serves
pub struct TokenRecord { pub account: AccountId, pub kind: TokenKind, pub scopes: ScopeSet, pub expires: Option<Timestamp>,
                         pub revoked: Option<Timestamp>, pub sso_orgs: SmallVec<[AccountId; 2]> }
pub enum ClaimRecord { Live { account: AccountId, v: u64 }, Former { account: AccountId, v: u64 }, Reserved }
/// Isolate cache. Positive entries 60 s, write-class re-check after 5 s, negative token entries 10 minutes. Compiled, not config.
pub const POSITIVE_TTL: Duration = Duration::from_secs(60);
pub const WRITE_RECHECK: Duration = Duration::from_secs(5);
pub async fn authenticate(h: &RequestAuth, cache: &IsolateCache, class: AccessClass) -> Result<Principal, AuthError>;
pub fn authorize(stamp: &AuthzStamp, repo: &FoldView<Repo>, action: Action) -> Decision;   // pure, inside the Repo object
```

Token prefixes and formats, stateless `ghs_` installation tokens, first-party OAuth client ids, node ids, and SSO behavior are reused from `DESIGN.md` "Identity and tokens". Installation tokens are signed with Ed25519 keys held as Worker secrets per install, so minting needs no object call. Logins, verified emails, SSH fingerprints, and app slugs are each one `Claim` object, so global uniqueness is a create-if-absent inside one object, with no global table.

### Cross-object operations

Each names its commit point, its finisher, and its abort path. Finishers run from the owning object's alarm, which is guaranteed at least once (`research/cloudflare.md`, DO "Alarms"), and every step is idempotent by id.

| Operation | Commit point | Then, idempotent | Abort |
| --- | --- | --- | --- |
| Create repository | `Owner` commits `RepoName { Reserved }` and `Pending` | `Network` genesis, `Repo` `Genesis`, `Owner` `Live` | Name released after 1 hour if genesis never lands |
| Fork | Fork owner's `Owner` commits `Reserved` and `Pending` | `Network` `ForkAdded`, fork's `Repo` `Genesis { fork_of: (repo, seq, refs) }`, `Live` | `ForkAdded` refused by a taken-down network tombstones the name |
| Transfer | Destination `Owner` commits `Reserved { from }` | `Repo` `Settings { owner }`, destination `Live`, source `Alias` | Source refuses while the repository is archived or locked |
| Org member removal | `Owner` commits `Member { None }` | Replicas apply before the 204; `Home` drops cards later | None |
| Login rename | `Claim` for the new login created | `Owner` `Login`, old `Claim` becomes `Former` | New claim deleted if the owner refuses |
| Token creation | `Token` object created | `Identity` lists it | Swept if no `Identity` row after 1 hour |

A transfer never moves bytes or the `Repo` object: the network, packs, and placement stay, and only names and the owner pointer change. A fork in another owner joins its parent's network, so a fork pull request reads two `Repo` objects and writes one.

### Event fan-out

```rust
pub enum Consumer { Webhooks, ActionsTrigger, PrSync, Notify, Search, Cards, Audit, CacheWarm }
/// Queue message. Queues are at least once and unordered (https://developers.cloudflare.com/queues/reference/how-queues-works/),
/// so a marker only says "this chain has work for you". Order and exactly-once effect come from the cursor in the chain.
pub struct Dirty { pub chain: ObjectName, pub consumer: Consumer, pub head: Seq }
/// One row per consumer in each chain's SQLite. Only the lease holder pulls and advances.
pub struct ConsumerRow { pub consumer: Consumer, pub cursor: Seq, pub lease: Option<ConsumerLease>, pub marked: Option<Timestamp> }
/// GUID from (generation, chain, seq, index, hook), so a redelivery keeps its GUID (DESIGN.md "Event fan-out").
pub struct HookDelivery { pub guid: Uuid, pub hook: HookId, pub entry: (ObjectName, Seq, u16), pub attempts: SmallVec<[Attempt; 4]> }
```

After each commit the chain sends `Dirty` for every subscribed consumer whose cursor is behind and that has no marker in flight, and sets an alarm that re-sends after 30 s while any cursor lags. A Queues outage (`research/cloudflare-uptime.md#9`) delays events and loses none, because the work is the chain itself. An event exists if and only if its change committed, because the entry is both. Webhook payloads render from fields captured in the entry, as in `DESIGN.md` "Event fan-out". `Notify` writes recipients' `Home` objects, `PrSync` commits bot `Refs` entries for `refs/pull/N/head` and `merge`, `Audit` feeds Basin Pipelines, and `CacheWarm` prefetches new packs into the standby location's git workers.

### Issues, pull requests, checks, and Actions

Merge semantics, required checks as `RequiredCheck` entries (graft 1), the tree-diff check, the merge queue, and the issue and pull request model are `DESIGN.md` "Issues and pull requests", unchanged except that the core and metadata tiers collapse into one SQLite file. Check runs and statuses that no rule requires live in a `Checks` object per (repository, head SHA), so a 256-job matrix never touches the `Repo` object. The job state machine and broker contract are `DESIGN.md` "Actions runs and jobs" and "Actions", with `Run` objects for state, `RunnerScope` objects holding runner long polls open inside the object, logs and artifacts in R2, and managed hosted runners in Sandboxes (Docker works as rootful DinD with host networking; no KVM is documented, `research/cloudflare-git.md#27`, `#28`).

### Dominant access patterns, traced

- *Fetch or clone.* Isolate cache (token, login) then `AuthzReplica` (one call, nearest hint) then `Repo::read` for a `RefSnapshot` (one call) then, with no `have` lines, an R2 GET of the clone cache, else `GitWorker::fetch` streaming from the container through its object and the Worker.
- *Push.* The same two calls, R2 staging, `GitWorker::receive` (R2 GETs and PUTs, one `Network` commit), one `Repo` commit, and in Paired one standby call.
- *Issue or pull request list, PR page.* `AuthzReplica`, then one `Repo::read` running indexed SQL; a PR page adds one `Checks` call.
- *Authorization.* The replica's stamp plus the `Repo` fold's collaborators, inside the call the request makes anyway.
- *`/user/repos`, notifications, stars.* The user's `Home` object, plus each org's replica for org listings; cards lag pushes by up to 60 s.
- *Webhook.* `Dirty`, consumer Worker, `Repo::pull`, render, POST, `Hook` records the delivery, `Repo::advance`.

## Commit and read paths

Locations below assume `acme/mono` is Paired with primary under `enam`, standby under `wnam`, and its register bucket under `weur`. "Durable at done" is what exists when the client is told success.

**1. Atomic three-branch push to a busy monorepo, then a 5 GB clone and a 2 GB push.** A developer in Berlin runs `git push --atomic`. The Frankfurt colo's Worker checks the token in isolate memory, re-reading the `Token` object if the cached entry is older than 5 s, and asks `AuthzReplica(acme, k)` under `weur` for a stamp. The discovery GET reads refs from the `Repo` primary in `enam`. The POST's pack streams into the staging bucket with R2 Local Uploads, which lands bytes near the client and stays strongly consistent (`research/cloudflare.md`, R2). The Worker calls `gw:{network}:{k}` in the `ENAM` git-worker class, which pulls the staged pack, runs `index-pack --strict`, checks connectivity, writes pack and index to the `enam` and `wnam` buckets with If-None-Match, and commits `PackRegistered` on the `Network` object, which replicates to its standby. The `Repo` primary then commits one `Refs` entry with three updates under `AllOrNothing`, replicates it to the `wnam` standby, and replies. Merge-queue `Merged` entries (10 a minute), bot `Refs` entries (50 a minute), and this push are serialized entries on one chain: a bot whose expected old value lost gets git's `fetch first`, and a merge whose base moved re-plans. CI clones (2,000 a minute) each make one `AuthzReplica` call and one `RefSnapshot` call, then mostly stream a cached shallow pack from R2. Durable at done: the entry in SQLite confirmed in `enam` and in `wnam`, and the pack and index in both buckets. Next read: any `ls-refs` anywhere shows all three branches or none, because one entry carries them and reads serve the acknowledged watermark.
The 5 GB clone goes to a `standard-4` git worker that holds the network's packs on its 20 GB disk, or hydrates them from R2 first (Milestone 0 bounds this at 60 s), then upload-pack with bitmaps streams through the container's object and the Worker. Response bodies have no size limit, but a runtime update gives open connections only 20 minutes (`research/cloudflare.md`, Workers), so a client below about 4 MB/s can be cut and must restart; clients that enable `transfer.bundleURI` fetch a weekly bundle from R2 instead. The 2 GB push on an Enterprise zone takes the HTTPS path above, with disk of 5 GB of packs, 2 GB staged, and indexes under the 20 GB ceiling. On Pro or Business the edge answers 413 above 100 or 200 MB (`research/cloudflare-git.md#23`), and the push goes over SSH: Spectrum, the tunnel, an SSH gateway container that authenticates the key through its `Claim` object, gets a stamp, streams the pack into staging through R2's S3 API in 64 MB parts, then calls the same `receive` and `commit` through its container outbound handler, which reaches Durable Object bindings without the public zone (https://developers.cloudflare.com/containers/configuration/workers-connections/).

**2. Fork pull request merged while a review and a required check land.** `bob/mono` is a `Repo` in `acme/mono`'s network. The review commits a `Review` entry on the base `Repo`. The check run lands in `Checks(acme/mono, head)`, and because a branch rule requires it, the same call commits a `RequiredCheck` entry on the base `Repo`. The merge reads the base fold and the fork's head from the fork's `Repo` (pinning the head SHA), asks a git worker for `merge_plan`, places the result pack, and commits `Merged` on the base chain. `decide` sees the review and the conclusion if they committed first, and refuses with GitHub's protection text if the conclusion is failing; a conclusion that commits after is a check on a merged pull request. Durable at done: `Merged` in both locations and the merge pack in both buckets. `gh pr view` and `gh pr list` run at once against the same object, so both show the pull request merged. `refs/pull/N/head` moves seconds later through `PrSync`.

**3. Issue labeled `bug`, then two lists.** `OpenIssue` commits `IssueOpened` with its label rows in one transaction. `gh issue list --label bug` and `gh api repos/o/r/issues?sort=updated` both run indexed SQL on that fold at the acknowledged watermark, so the new issue is in both, first by `updated`. Durable at done: the entry in both locations. No index, search cluster, or replica sits between the write and the list (contrast `research/uptime.md#49`).

**4. Member removal, token revocation, and the next push.** The owner's `DELETE /orgs/acme/members/m` commits `Member { None }` on the `Owner` primary, replicates to its standby, then to every `AuthzReplica`. The 204 returns when each replica applied it or its 10 s lease expired. Revoking the token commits on its `Token` object and its standby. Durable at done: the removal in both owner locations and in every replica still holding a lease, and the revocation in both token locations. The member's next HTTPS push is write-class, so the Worker re-reads the token if its cache entry is older than 5 s and gets 401; with a still-valid token, the replica refuses the stamp and git sees "Repository not found". Over SSH the gateway re-checks the key's `Claim` and the stamp the same way. A push that already held a stamp minted before the 204 can still commit, one commit's worth of time, the same bound as `DESIGN.md`. Reads with a revoked token stop within 60 s.

**5. The data center, then the region, holding the newest writes is lost mid-push.** If the primary's data center goes first, a commit in flight either was confirmed by 3 of 5 followers or was not, and the platform restarts the object after fencing the old instance (`research/cloudflare-uptime.md#32`; restart time is undocumented). If the entry was committed but not yet granted by the standby, the restarted primary re-sends it before answering anything; the client saw an error, and its next fetch may show the push landed, which git users already meet on any dropped connection. If all of `enam` is then lost, Workers' calls fail and each reports to the `wnam` standby, which waits until 6 s after its last grant, CASes the register in `weur` to generation n+1, seals n, and serves `Alone`. Every acknowledged entry is there, and every acknowledged pack is in the `wnam` bucket. Unacknowledged entries in `enam` are discarded if that region returns, and the old primary, fenced, answers `Moved`. Git workers in the `WNAM` class hydrate from the `wnam` bucket. Writes resume in about 20 s [guess, Milestone 0 measures]. A Regional repository instead waits for Cloudflare to restore the region, and its acknowledged writes since its last archive segment survive only if Cloudflare's followers or object-storage copy lie outside the region, which no page states.

**6. A global outage like November 18, 2025, and a control-plane outage like November 2, 2023.** In the first, the core proxy returned 5xx everywhere and Workers KV and Access failed with it (`research/cloudflare-uptime.md#13`). Every HTTPS request to Tributary passes that proxy, so web, API, and HTTPS git fail for its duration, and SSH gateways reach objects through the same runtime, so treat SSH as down too [inference]. A developer keeps working in local clones; nothing acknowledged is lost, no failover fires (standby leases ride object-to-object calls, not the HTTP proxy [inference], and promotion needs a primary the standby cannot reach), and nothing needs repair afterwards. In the second, the data plane kept serving (`research/cloudflare-uptime.md#2`), and so does everything here: git, API, web, webhooks, Actions, and failover, because no runtime path calls the Cloudflare API. What stops: deploys and rollbacks (both go through Wrangler or the dashboard, https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/), DNS, Spectrum, and tunnel changes, Logpush, and possibly new container instances if image distribution depends on the control plane [unknown], so warm git-worker pools are sized to carry a day without new starts.

**7. A release that multiplies identity reads tenfold, and one that writes a record shape the previous release cannot read.** Identity reads hit one `Token` object per token and one `Claim` per login, millions of objects with no shared database, and isolate caches with compiled bounds (`research/uptime.md#47`). Tenfold reads raise each object's load tenfold; a CI token used by thousands of isolates can hit its object's 1,000 per second soft limit, whose overloaded errors must not be retried (`research/cloudflare.md`, DO "Throughput"), so the isolate serves the last known good entry for up to 60 s and backs off. The release ships by Workers gradual deployment, which pins each object to one version (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/with-durable-objects/), with an automatic halt on SLO burn. For record shapes, the two-release rule of `DESIGN.md` "Change safety" applies, with one addition forced by pairing: a primary on release N+1 replicates to a standby that may still run N, so release N+1 writes format k+1 only after the signed `R2Key::Formats` control file enables it, which the release tooling does only after every object class reports N+1 at 100% for a 24-hour soak. Objects cache the file and keep the last known good copy if R2 is down. Rolling back below a reading release is refused.

**8. BYOC and fully self-hosted.** BYOC runs the same signed bundle in the customer's Cloudflare account, deployed by the vendor with an account API token whose permissions are a subset of the granting member's (`research/cloudflare-git.md#34`), through Terraform for buckets, queues, DNS, Spectrum, and the tunnel, and Wrangler for Workers, object classes, and containers, because no Terraform container resource exists (`#35`). The customer gets its own namespace, Paired durability within its chosen hints and jurisdiction, and an install that keeps serving if the vendor disappears, since the data plane needs nothing from the vendor. Fully self-hosted means `workerd` on the customer's machines: Durable Objects on `localDisk` storage, which is marked experimental and confined to one runtime instance, containers through Docker, and an R2-compatible object service that Tributary ships, because workerd's R2 binding is implemented by a service you supply (`research/cloudflare-git.md#38`, `#39`). That gets one host, durability of that host's disk plus scheduled exports, no pairing, no SLO, and direct TCP SSH. "Deployment modes" states the rest.

## Scale

Labels: [doc] from `research/cloudflare.md` limits, [inference] arithmetic, [guess] until Milestone 0.

| Pressure | Load | Limit | How this design stays under it |
| --- | --- | --- | --- |
| Busiest repository, requests | Clones 33/s (2,000 a minute) one snapshot each, 70/s API reads (1,000 developers x 2,500 a day over 10 hours), pushes, merges, and bots about 5/s, consumer pulls about 40/s: about 150/s [inference] | 1,000/s soft per object [doc] | Clone bytes and pack generation never touch the object. Above 500/s sustained, `RepoReader`s take reads: each probes the primary once per round trip under single flight, so 16 readers add at most 320 probes a second and serve about 12,000 reads a second [guess] |
| Busiest repository, writes | About 5 entries a second | Serial transactions, about 1 ms CPU each [guess]; Milestone 0 must show at least 500 a second | No retries exist to amplify load. Paired replication is pipelined, so throughput follows the standby's apply rate, not the round trip |
| Busiest repository, merges | 10 a minute on one base branch | One merge per plan time per base branch, plan 0.2 to 2 s in a git worker [guess] | Merge-queue groups land as one entry |
| Busiest repository, storage | Refs (1 M including pull refs) about 0.3 GB, 500,000 issues and pull requests and 5 M comments with FTS5 about 5 GB, log under an hour [guess] | 10 GB per object, 2 MB per row [doc] | Log trimmed to R2 archive; alarm at 7 GB, after which comment bodies over 2 KB move to content-addressed R2 objects and FTS5 keeps their first 2 KB |
| Largest owner, reads | 20,000 developers plus CI: about 5,000 authorizations a second at peak [guess] | 1,000/s per object [doc] | Up to 16 `AuthzReplica`s per owner, placed across hints by traffic, about 16,000/s; the `Owner` primary sees only writes and lease renewals |
| Largest owner, writes and storage | SCIM bursts of 100 a second [guess]; 500,000 repository names, 100,000 members, 5 M grants, about 0.5 GB [inference] | 1,000/s, 10 GB [doc] | Audit lives in Basin, sessions in `Identity`, stars and notifications in `Home`, so the `Owner` fold holds only what authorizes |
| Push body on HTTPS | GitHub enforces a 2 GB push ("Push size: This limit is enforced at 2GB", https://docs.github.com/en/repositories/creating-and-managing-repositories/repository-limits) | 100 MB Free and Pro, 200 MB Business, up to 5 GB Enterprise [doc] | Managed runs on Enterprise. Below Enterprise, larger pushes go over SSH, where no body limit applies |
| Container disk | A 5 GB network plus a 2 GB push plus indexes, about 8 GB | 20 GB per instance, ephemeral [doc] | Serving and maintenance in separate classes; disk admission per job; networks capped at 12 GB packed, enforced at push, above the 10 GB that the same GitHub page recommends as a maximum |
| Containers, account | Managed peak git CPU [guess: hundreds of vCPU] | 1,500 vCPU, raisable [doc] | Clone cache absorbs repeated CI clones; quota raise before launch |
| Container instances per class | Hot networks with K up to 8 | `max_instances` defaults to 20 [doc] | Set per class at install; one class per region set |
| R2 per key | Placement CAS, clone-cache fill | 1 write a second per key [doc] | Every other key is a ULID or digest written once; fills are single-flight |
| Busiest fork network | Every push to any fork commits one `PackRegistered`; a network of 100,000 forks at 20 pushes a second [guess] | 1,000/s per object [doc] | Registration is one small transaction per push; compaction entries are rare; fetches read the manifest only when a git worker's copy is older than the snapshot's `min_manifest` |
| Queues | Managed peak 3,800 pushes a second x 8 consumers [inference from `research/git-storage.md#5`] | 5,000 messages a second per queue [doc] | 16 queues per consumer by chain hash |
| Fleet | 3.35 billion pushes a month, about 1,275 a second [doc, `research/git-storage.md#5`] | Objects unlimited (https://developers.cloudflare.com/durable-objects/platform/limits/) | Nothing is shared fleet-wide except per-key `Claim` and `Token` objects |

## Downtime

| Incident | Evidence | This design | Effect |
| --- | --- | --- | --- |
| GitHub: cross-country promotion after a 43 s partition | Oct 2018, 954 unreplicated writes (`research/uptime.md#40`) | A promotion needs the register CAS in a third location and a sealed standby, and nothing unacknowledged was ever readable | removed |
| GitHub: database overload | Shared permissions cluster (May 2023); auth database at 10x reads (Feb 2026) (`#44`, `#47`) | No shared database: one object per owner, token, and name, replicas for the largest owners, compiled cache bounds | contained |
| GitHub: schema change on the store | Misconfigured index, migration at peak, key overflow (`#44`) | Additive per-object migrations on wake under gradual deployment, so a bad migration reaches only the objects assigned that version; 64-bit ids | contained |
| GitHub: record shape the old release cannot read | Dropped column still read (Aug 2025) (`#44`) | Two-release format rule plus the `Formats` control file gated on 100% of every class | contained |
| GitHub: engine upgrade | Version bug; major upgrade under load (`#44`) | Cloudflare upgrades SQLite and the runtime on its own schedule, which Tributary cannot stage | exposed |
| GitHub: failover with no writable primary | Redis failover, partial disk (`#44`, `#47`) | No primary election inside Tributary for Regional objects; Paired failover is lease-fenced and drilled monthly | mitigated |
| GitHub: connection storms | Pool exhaustion (`#44`) | No connection pools; calls are object RPCs | removed |
| GitHub: config and deploy | Sharding-middleware redeploy, LB change, expired certificate (`#45`) | Gradual deployment per object with auto halt; certificates are Cloudflare-managed; no sharding tier | contained |
| GitHub: DNS dependency | Site DNS down 19 h; DNS cascade (`#46`) | Object calls are by name inside the runtime, not DNS; Cloudflare DNS is itself a global dependency | mitigated |
| GitHub: capacity | LB tier failed to scale; retry loop (`#48`) | No tier to scale; per-object limits isolate hot spots; overloaded errors are never retried | contained |
| GitHub: merge logic bug | Squash queue reverted 2,092 PRs (`#49`) | Tree-diff check before every `Merged`; `RestoredTo` per repository and 30-day PITR per object | contained |
| GitHub: search dependency | Elasticsearch overload emptied lists (`#49`) | Lists are SQL in the `Repo` object; search shards serve only `/search` | removed |
| Cloudflare: 1.1.1.1 lookup failures, Oct 2023 | `research/cloudflare-uptime.md#1`, 15% SERVFAIL at peak | Tributary never resolves through 1.1.1.1; clients using it as their resolver may fail to find the host | mitigated |
| Cloudflare: 1.1.1.1 BGP hijack and route leak, Jun 2024 | `#5`, 300 networks in 70 countries | As Oct 2023 | mitigated |
| Cloudflare: 1.1.1.1 prefixes withdrawn, Jul 2025 | `#10`, 62 min, global | As Oct 2023 | mitigated |
| Cloudflare: 1.1.1.1 CNAME ordering release, Jan 2026 | `#15`, about 20 h to revert | As Oct 2023 | mitigated |
| Cloudflare: control plane and analytics, Nov 2023 | `#2` | No runtime path calls the API; deploys, rollbacks, and config wait | contained |
| Cloudflare: PDX01 power, Mar 2024 | `#3` | Same as above; recovered in minutes | contained |
| Cloudflare: CDN errors from a DDoS rule, Jun 2024 | `#4`, 1.4 to 2.1% of requests | Front door shares the proxy; retried by clients | exposed |
| Cloudflare: lost logs, Nov 2024 | `#6` | Audit is chain entries fed to Basin from cursors, not logs; request logs can be lost | mitigated |
| Cloudflare: R2 Gateway disabled, Feb 2025 | `#7`, 59 min, 100% errors | Pushes, uncached clones, and failovers stop; ref reads, lists, API writes, and cached-pack fetches on warm git workers continue | exposed |
| Cloudflare: R2 credential deploy, Mar 2025 | `#8`, writes 100%, reads 35% failed | Same as above for writes | exposed |
| Cloudflare: KV storage dependency, Jun 2025 | `#9`, DO and D1 errors to 22%, Queues down | No KV on any path; DO errors hit commits and reads directly; events wait in chains | exposed |
| Cloudflare: AWS interconnect, Aug 2025 | `#11`, regional | No origin outside Cloudflare | removed |
| Cloudflare: dashboard and API, Sep 2025 | `#12` | As Nov 2023 | contained |
| Cloudflare: global proxy panic, Nov 2025 | `#13`, about 3 h | Total outage of HTTPS and SSH; no data loss, no repair | exposed |
| Cloudflare: FL1 killswitch, Dec 2025 | `#14`, 25 min, 28% of HTTP | Front door shares the proxy | exposed |
| Cloudflare: Miami route leak, Jan 2026 | `#16`, 25 min, regional | Anycast reroutes; objects in other locations unaffected | mitigated |
| Cloudflare: BYOIP withdrawal, Feb 2026 | `#17` | Tributary uses Cloudflare-assigned addresses | removed |
| Cloudflare: regional DO and R2 incidents, Oct 2026 sample | `#20`, 7 DO incidents in 13 days, most regional | Paired objects fail over to the other hint; Regional objects in that region wait | mitigated |

RPO and RTO per durability class. Write RTO for Paired is detection plus lease, not a Cloudflare restart.

| Durability class | Data | RPO | RTO | Mechanism |
| --- | --- | --- | --- | --- |
| Paired chain objects and packs | Pushes, merges, issues, owners, identity, tokens, claims | 0 for the loss of any one location | About 20 s for writes after the first failed request [guess]; reads the same | Standby grant before ack, two buckets, register CAS in a third location |
| Paired, while `Alone` after a failover | Writes acknowledged during the window | As Regional | As Regional | Ends when `seed_and_flip` completes |
| Regional chain objects | Same | 0 for a machine or data center (documented); unknown for a region | Platform restart for a data center [unknown]; region return for a region | 3-of-5 followers; 30-day PITR (`research/cloudflare.md`, DO "PITR") |
| Regional work objects | Check runs, Actions state, deliveries, homes | As Regional | As Regional; runs re-queue | Re-run jobs; notifications rebuilt from chains |
| One repository, operator error | Its refs | Any seq in 30 days | Minutes [guess] | `RestoredTo` entry, or PITR into a new incarnation through `seed_and_flip` |
| Derived state | Replicas, readers, clone cache, search, container disks | Rebuildable | Minutes to hours by size | Replay from chains and R2 |

## Failure modes

Components are per install in BYOC and self-hosted and per repository, owner, or network in managed. "Chain objects" covers `Repo`, `Network`, `Owner`, `Identity`, `Claim`, and `Token`, with their standbys.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| chain objects | instance | One aggregate's in-flight requests | Platform restarts the object after fencing; confirmed writes survive; Workers retry a failed read once and never retry `Overloaded` |
| chain objects | data-center | Aggregates whose primaries ran there | Same as instance; Paired standbys keep every acknowledged entry if the restart is slow |
| chain objects | region | Every primary under that hint | Paired: request-driven promotion through the register; Regional: unavailable until the region returns |
| chain objects | provider-wide | Everything | Nothing in Tributary can serve; no data lost; no failover storm, because promotion needs an unreachable primary and a reachable register |
| chain objects | control-plane | None at runtime | Deploys and rollbacks wait; promotion and seeding use only bindings |
| chain objects | bad-deploy | Objects assigned the new version | Gradual deployment pins versions per object; `decide` is pure and property-tested against release N-1; auto halt on commit error rate |
| chain objects | bad-config | Objects whose class config changed, such as a wrong location hint for new objects | Hints and durability are signed install config checked by the release tooling; existing objects never move without `seed_and_flip` |
| chain objects | noisy-tenant | One repository or owner | Per-object limits isolate it; comment and bot caps per repository; readers and replicas for hot ones |
| placement register | instance | One register read or CAS | Retry; reads happen only on `Moved` or failure |
| placement register | data-center | None for data [inference: R2 spans several data centers in a region] | Nothing to do |
| placement register | region | Failovers of pairs that use that bucket | Normal traffic needs no register; such pairs can fail over again once the region returns |
| placement register | provider-wide | Failovers | Nothing fails over during a global outage anyway |
| placement register | control-plane | None at runtime | Bucket pre-created at install |
| placement register | bad-deploy | Wrong placements written | Only `seed_and_flip` and `suspect` write it, always with If-Match; a daily audit compares each register with its objects' generations |
| placement register | bad-config | Arbiter bucket in the same location as a side | Install planner refuses three hints that are not distinct |
| placement register | noisy-tenant | One key's 1 write a second | Writes per key are rare by design |
| pack buckets | instance | Single GETs or PUTs fail | Retries; keys unique per write |
| pack buckets | data-center | None for data (eleven nines across several data centers, `research/cloudflare.md`, R2 "Durability") | Nothing to do |
| pack buckets | region | Paired: one copy; Regional: pushes, uncached clones, cold git workers | Paired: the other bucket serves and the next push places in the survivor plus a new copy after re-protect; Regional: ref reads, lists, and warm-cache fetches continue |
| pack buckets | provider-wide | Pushes, uncached clones, rehydrates (R2 outages of Feb and Mar 2025) | Warm git workers keep serving cached networks; pushes fail before any entry, so nothing half-commits |
| pack buckets | control-plane | None at runtime | Buckets, lifecycle rules, and Local Uploads set at install |
| pack buckets | bad-deploy | Bad packs placed | `index-pack --strict` before placement; GC deletes only after 14 days unreachable; packs are immutable |
| pack buckets | bad-config | A lifecycle rule that deletes packs | Lifecycle rules only on `Stage` and `CloneCache` prefixes, checked by the release tooling; two copies in Paired |
| pack buckets | noisy-tenant | One network's bandwidth | Push bytes and concurrency caps per account and network; 12 GB network cap |
| authz replicas | instance | One replica's callers | Workers pick the next k; the owner waits at most a 10 s lease for it on revocation |
| authz replicas | data-center | Replicas there | Same |
| authz replicas | region | Replicas under that hint | Workers fall back to replicas under other hints, then to the `Owner` primary |
| authz replicas | provider-wide | Everything | As chain objects |
| authz replicas | control-plane | None at runtime | Replica count is an `Owner` entry, not platform config |
| authz replicas | bad-deploy | Wrong decisions from one version | Replicas apply the same `apply` as the owner; a digest of the fold rides each lease renewal and a mismatch drops the replica |
| authz replicas | bad-config | Too few replicas for a huge org | Count follows the owner's measured load from Analytics Engine; overload sheds reads to other k |
| authz replicas | noisy-tenant | One org's authorization storm | Its own replicas only |
| git workers | instance | Clones and pushes in flight on it; its cache | Clients retry; next k or a fresh instance hydrates from R2; staged pushes resume from staging |
| git workers | data-center | Instances there | Containers may restart elsewhere (`research/cloudflare.md`, Containers "Placement"); hydrate |
| git workers | region | That class's instances | The standby location's class, prewarmed by `CacheWarm` for hot networks |
| git workers | provider-wide | Everything | As chain objects |
| git workers | control-plane | New instances may not start [unknown] | Warm pools sized for a day of load; `sleepAfter` long for hot networks |
| git workers | bad-deploy | A git or supervisor regression in one image | Image per release, gradual by object version; conformance corpus of real git clients before release |
| git workers | bad-config | Wrong instance type or `max_instances` | Per-class config checked against the largest network's size at deploy |
| git workers | noisy-tenant | One network's clone storm | Clone cache, K per network, per-IP WAF limits, per-network CPU admission |
| event pipeline | instance | One consumer invocation | Lease expires; the next `Dirty` or alarm resumes from the cursor |
| event pipeline | data-center | Consumers there | Same |
| event pipeline | region | Consumers and markers there | Markers re-sent by chain alarms; cursors are in chains |
| event pipeline | provider-wide | All events | Delayed, none lost; chains hold the work |
| event pipeline | control-plane | None at runtime | Queues pre-created |
| event pipeline | bad-deploy | One consumer's handler | A handler that fails 10 times on one entry parks that chain for that consumer only; others proceed |
| event pipeline | bad-config | A consumer unsubscribed by mistake | Subscriptions are compiled; a skipped kind is replayable from the cursor |
| event pipeline | noisy-tenant | A viral repository's notifications | Per-thread caps in `Home`; 16 queues per consumer |
| work objects | instance | One run, check suite, hook, or home | Restart; runs hold leases that survive restart |
| work objects | data-center | Those objects | Same |
| work objects | region | Regional work objects there | Runs re-queue after lease expiry; check conclusions that matter are also `RequiredCheck` entries on the Paired `Repo`; homes rebuild from chains |
| work objects | provider-wide | Everything | As chain objects |
| work objects | control-plane | Hosted runner sandboxes may not start [unknown] | Self-hosted runners unaffected |
| work objects | bad-deploy | Actions dispatch or deliveries | Separate classes from `Repo`, so git and merges without required checks keep serving |
| work objects | bad-config | One class | Signed config per class |
| work objects | noisy-tenant | A 256-job matrix or one hook owner | One `Checks` object per commit; per-org job caps; per-hook-owner delivery concurrency |
| ssh gateway | instance | Sessions on it | Other tunnel connectors carry new sessions |
| ssh gateway | data-center | Connectors there | At least two gateways per serving region, in different instances |
| ssh gateway | region | Gateways there | Gateways in other region classes register with the same tunnel |
| ssh gateway | provider-wide | SSH | No path |
| ssh gateway | control-plane | Spectrum or tunnel changes | Pre-created; connectors authenticate with stored tunnel credentials |
| ssh gateway | bad-deploy | SSH auth or framing | Gradual image rollout; HTTPS unaffected |
| ssh gateway | bad-config | Spectrum app or tunnel route | Readiness probe runs `git ls-remote` over SSH every minute |
| ssh gateway | noisy-tenant | One client's sessions | Per-key and per-IP session caps; Spectrum overage alarm |
| search and audit | instance | One shard or pipeline write | Retries from cursors |
| search and audit | data-center | Shards there | Same |
| search and audit | region | `/search` and audit queries | `incomplete_results: true`; lists unaffected |
| search and audit | provider-wide | Everything | As chain objects |
| search and audit | control-plane | Pipeline or catalog changes | Pre-created; the audit consumer holds its cursor until the pipeline accepts |
| search and audit | bad-deploy | Wrong search results or audit rows | Rebuild shards from chains; audit tables are append-only and rebuildable from chains |
| search and audit | bad-config | A shard map change | New shards built beside old, then flipped |
| search and audit | noisy-tenant | A search botnet | Per-caller search limits; only `/search` sheds |

## Deployment modes

One signed bundle: the Workers and object classes compiled from the Rust workspace, the git-worker, maintenance, and SSH gateway images, the web assets, the Terraform module, the formats it reads and may enable, and the action pack (`DESIGN.md` "Deployment modes" for the release record and signing, reused).

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| chain objects | Paired; each owner picks a home pair (`enam`/`wnam`, `weur`/`eeur`, `apac`/`oc`) | Paired by default within the customer's hints and jurisdiction; Regional on request | Customer-operated account: as BYOC. Air-gapped workerd: one host, `localDisk`, no pairing |
| placement register | One arbiter bucket per pair, in a third hint | The customer's third hint, or none when Regional | As BYOC, or none on workerd |
| pack buckets | One bucket per hint in the pair, Local Uploads for staging | Same, in the customer's account | As BYOC, or the shipped R2-compatible service on local disk |
| authz replicas | Up to 16 per owner by load | Up to 4 | Up to 4, or 1 on workerd |
| git workers | One class per region set, warm pools sized per tier | Same, sized by the planner | As BYOC, or Docker containers on the host |
| event pipeline | Queues, 16 per consumer | Queues, 1 per consumer | As BYOC; on workerd, chain alarms call consumers directly [inference: no Queues in workerd] |
| work objects | Regional | Regional | Regional, or local |
| ssh gateway | Spectrum on Enterprise, tunnel connectors in gateway containers | Spectrum SSH app on Pro or higher | As BYOC, or direct TCP on workerd |
| search and audit | Search shards per residency domain; Basin for audit | Same | As BYOC; on workerd, FTS5 shards and audit in chains only |

What each mode requires of the customer. **BYOC**: a Cloudflare account on Workers Paid, a zone on Pro or higher, an account API token for the vendor that the customer can revoke at any time (after which the install keeps serving and only upgrades stop), Enterprise for HTTPS pushes over 200 MB or for the `eu` jurisdiction with the Data Localization Suite, and a container quota for its largest network. **Self-hosted, customer-operated**: the same account requirements, and the customer runs `tributary install apply` from its own CI with the release key it verifies. **Air-gapped workerd**: one Linux host with Docker, local disk sized at three times the git data, and acceptance that workerd's object storage is experimental and single-instance, so availability and durability are that host's.

## Cost

`python3 arena/cloudflare/candidate-1/cost.py` prints these from the `TIERS` and `ACTIVITY` workload in `scripts/cost_model.py` and the `cf.*` prices in `research/prices.json`. Per-operation counts are [guess]es until Milestone 0. Runner compute, the vendor fee, and an Enterprise contract are excluded; 300 and 2,000 developers are priced on Business, which caps HTTPS pushes at 200 MB.

| Developers | Regional | Paired |
| --- | --- | --- |
| 50 | $150 | $212 |
| 300 | $796 | $930 |
| 2,000 | $4,287 | $5,110 |

Where the money goes. At 50 developers, warm git-worker and SSH-gateway memory is 44% (Regional) to 49% (Paired) of the bill. At 2,000, Spectrum SSH overage at $1 a GB (30% of developer git bytes over SSH [guess]), container egress for uncached clone bytes, and R2 storage lead; the clone cache's hit share (80% [guess]) is the largest lever, because R2 egress is free and container egress is not. Database-like costs (object requests and duration, rows written, SQLite storage) stay under 20% at every tier.

Cheapest reliable setup for 50 developers: $80 a month (`cost.py --cheapest`). Regional durability, the Pro plan billed annually ($20), Workers Paid ($5), one `standard-1` git worker that sleeps outside working hours and rehydrates on demand, and two `lite` SSH gateways. It keeps automatic recovery from any machine or data-center loss, because objects restart on their own and containers rehydrate from R2; it gives up region-loss RPO and costs a cold start of a few seconds on the first fetch of the morning.

## Build order from zero

Durations are [guess]es for a team of about 10.

**Milestone 0: measurements that would make this design wrong** (one month, a Workers Paid account plus an Enterprise trial zone, before any git code).

| Measurement | Pass | Fail |
| --- | --- | --- |
| Commit rate: one `Repo` object, 1 KB entries, entry plus five fold rows per `transactionSync`, Regional | At least 500 commits a second sustained; p99 at most 50 ms at 100 a second | Under 200 a second, or p99 over 150 ms |
| Paired commit: primary `enam`, standby `wnam`, pipelined | p99 at most 150 ms at 50 a second; throughput at least 80% of Regional | p99 over 400 ms |
| Object request ceiling with a 10,000-ref snapshot read | At least 1,000 a second before `Overloaded` | Under 500 a second |
| Pairing correctness: a chaos switch drops primary-to-standby and primary-to-Worker traffic, 10,000 induced failovers, Knossos per chain | Zero lost acknowledged entries, zero reads of unacknowledged entries; write RTO p99 at most 30 s | Any loss or stale read; RTO p99 over 120 s |
| Owner revocation to 16 replicas across three continents | 204 p99 at most 500 ms | Over 2 s |
| Container hydrate: 5 GB of packs from R2 onto a `standard-4` | At most 60 s | Over 180 s |
| 5 GB clone, container to object to Worker to client, 30 runs | At least 50 MB/s, zero cuts outside runtime updates | Under 20 MB/s, or any unexplained cut |
| 2 GB chunked push on Enterprise, and 250 MB chunked on Business | Accepted on Enterprise; Business answers 413 cleanly | Enterprise rejects chunked bodies under 5 GB |
| SSH: Spectrum to tunnel to `cloudflared` in a container, 100 concurrent clones, connector killed | All succeed; new sessions recover within 10 s | No route works, or recovery over 60 s |
| Clone cache: 2,000 shallow 80 MB clones a minute | At least 99.9% success, time to first byte p99 at most 1 s | Under 99% |
| Dirty-marker pump at 2,000 entries a second | Consumer lag p99 at most 2 s | Over 10 s |

Also before Milestone 1, a written answer from Cloudflare on whether SQLite followers span regions and what a region loss does to an object; if they span regions, `Regional` becomes the default and Paired an option. Flip conditions: a commit-rate fail splits hot repositories into a refs object and a metadata object (the tier split of `DESIGN.md` "Folds", as two objects); a pairing fail drops Paired and states region-loss RPO as unknown everywhere; an SSH fail makes SSH a non-Cloudflare dependency that the operator must approve or drop; a hydrate or clone fail lowers `NETWORK_PACKED_LIMIT` to what passes.

**Milestone 1: chains and git over HTTPS, Regional** (3 months). `Chain<A>`, `Repo`, `Network`, git workers, staging, receive, fetch, the archive. Gate: 64 unmodified git 2.51 clients doing atomic and non-atomic pushes and clones on one repository with instances killed, Knossos per ref and Elle per push, zero lost acknowledged pushes, at least 200 pushes a second to one repository, push p99 under 1 s.

**Milestone 2: pairing** (2 months). Standby, leases, register, `suspect`, `seed_and_flip`, PITR into a new incarnation. Gate: Milestone 0's pairing harness at 10x the failovers, with git clients pushing throughout, zero loss.

**Milestone 3: issues, pull requests, merge, checks** (3 months). Gate: journeys 2 and 3 through unmodified `gh`; the cli/cli `issue` and `pr` acceptance suites; 10,000 races of a required check against a merge with zero merges that skipped a failing check; a planted reverting squash refused by the tree-diff check.

**Milestone 4: owners, identity, tokens, replicas, OAuth** (2 months, partly parallel). Gate: journey 4 over HTTPS and SSH with a push in flight; two concurrent signups of one login with exactly one success; a 10x identity read storm from a canary version with zero object overload outside the storm's own tokens.

**Milestone 5: events, webhooks, Actions** (3 months). Gate: a consumer stopped 30 hours resumes with zero lost events; Argo CD and Jenkins react to pushes; a pinned `actions/runner` runs checkout, cache, upload-artifact, and OIDC on a Sandbox runner.

**Milestone 6: SSH, scale features, three modes** (3 months). Readers, clone cache, bundles, SSH gateways, Terraform and Wrangler install, the air-gapped workerd build, GHES import through Workflows. Gate: journey 1 at full load; journey 8 in a fresh customer account from the signed bundle, then an upgrade under load with zero failed requests attributable to it; a monthly regional drill that blackholes one hint for a canary org and loses nothing.

**Milestone 7: breadth.** The remaining rows of `DESIGN.md` "Wire compatibility" at their tiers.

## Compared with the AWS design

| Concern | AWS design (`DESIGN.md`) | This design | Evidence |
| --- | --- | --- | --- |
| Commit point | Conditional put of the next entry in DynamoDB, retried on conflict | One SQLite transaction in a single-threaded object, never retried | `research/cloudflare.md`, DO; `DESIGN.md` "Chains and commits" |
| Reads and lists | Node-local folds proven fresh by a strong next-key probe; two tiers, snapshots, refolds | The object's own SQLite; no proof, no snapshot, no refold | Same |
| Region-loss RPO | Zero in Paired cells, from MRSC's documented semantics and SLA | Zero in Paired only through Tributary's own lease, seal, and register protocol; unknown for Regional | `research/cloudflare-uptime.md#40`, implication 5 |
| Biggest outage exposure | One AWS Region; AWS deploys zonally | Global: one Cloudflare config push can take down every request (Nov 2025, Dec 2025) | `research/cloudflare-uptime.md#13`, `#14`, implication 1 |
| SLA under the store | 99.999% Global Tables | None for Durable Objects; 99.9% R2 and Queues | `research/cloudflare-uptime.md#27`, `#29` |
| Hot repository | One DynamoDB partition, 1,000 write units a second, plus preferred tail readers | One object, 1,000 requests a second soft, plus `RepoReader`s | `research/cloudflare.md`; `DESIGN.md` "Scale" |
| Large owner authorization | `Mirror` chains in every hosting cell, heartbeats, 30 s leases | Up to 16 synchronous `AuthzReplica`s, 10 s leases | This document, "Owners" |
| Git engine | Upstream git on NVMe nodes that never stop | Upstream git in containers with 20 GB ephemeral disk; networks capped at 12 GB | `research/cloudflare-git.md#16`-`#18` |
| Push size | Limited only by GitHub's own 2 GB | 200 MB on HTTPS below Enterprise; SSH or Enterprise beyond | `research/cloudflare-git.md#23` |
| SSH | Front cells terminate SSH | Spectrum and a tunnel into gateway containers, unverified end to end | `research/cloudflare-git.md#26` |
| Operator burden | Cells, slot moves, node rolls, an in-account operator role, OpenTofu | No servers, no cells, no operator role; Terraform plus Wrangler; Cloudflare runs storage | `DESIGN.md` "Install operations" |
| Region choice | 34 Regions, MRSC in 15 | Eleven location hints, best effort; jurisdictions `eu`, `us`, `fedramp`; no country-level residency | `research/cloudflare-git.md#40`-`#42` |
| Cost at 50 developers | $1,287 one Region, $226 small-install profile | $150 Regional, $212 Paired, $80 cheapest | `research/costs.md`; `cost.py` |
| Cost at 2,000 developers | $12,432 one Region, $20,636 Paired | $4,287 Regional, $5,110 Paired, plus any Enterprise contract | Same |
| Self-hosted | Same artifact in the customer's AWS account | Same artifact in the customer's Cloudflare account; true on-premises only as single-host workerd | `research/cloudflare-git.md#38`, `#39` |

## Tradeoffs accepted

- We accept a home-grown pairing protocol (lease, seal, register CAS) to state RPO zero for a region loss, because no Cloudflare page states it, and we accept that its correctness rests on Milestone 0 and Milestone 2 harnesses rather than a vendor SLA.
- We accept that a Paired object after a failover acknowledges alone until reseeded, and that a US-only pair cannot reseed while one of its two US hints is down.
- We accept a global blast radius from Cloudflare's own config and proxy failures, which nothing in Tributary can contain, in exchange for no servers and no regional failover machinery for the front door.
- We accept that every write to one repository serializes on one object near 1,000 requests a second, in exchange for every input to a decision living in one transaction.
- We accept that in Paired, the first read or write after 5 s of quiet waits one round trip to the standby to renew the lease, in exchange for leases that cost nothing while a repository is idle.
- We accept a 12 GB packed limit per fork network and a 200 MB HTTPS push limit below Enterprise, as known deviations from GitHub, in exchange for git in containers with 20 GB disks and a Workers front door.
- We accept that a slow client's clone can be cut by a Workers runtime update after 20 minutes, and that clients must retry or enable bundle URIs.
- We accept SSH over Spectrum and a tunnel into containers at $1 a GB below Enterprise, unverified until Milestone 0, in exchange for no origin outside Cloudflare.
- We accept per-object SQLite migrations that run on wake, restricted to additive steps, in exchange for no fleet-wide migration and no shared schema.
- We accept that Cloudflare upgrades SQLite and the runtime on its own schedule, which is GitHub's "engine upgrade" incident class left exposed.
- We accept that cross-repository lists, cards, notifications, and search lag by seconds, and that `refs/pull/N/head` lags a push by seconds.
- We accept that a fork pull request merges the head pinned at plan time even if the fork moves within the commit round trip, as `DESIGN.md` does.
- We accept that the fully self-hosted mode is one host on experimental workerd storage, with no SLO.

## Alternatives considered

- **Cloudflare Artifacts as the git store.** It is one Durable Object per repository with synchronous multi-data-center replication, which validates this shape, but new repositories are capped at 1 GB, receive-pack is v1 only, `filter` is unsupported, and it is beta (`research/cloudflare-git.md#3`-`#7`, implication 1).
- **Git in WebAssembly inside Workers and objects, no containers** (dgit, git-on-cloudflare). No WASM git library computes deltas or bitmaps, so clone cost grows with history, and a large history exceeds 128 MB and 5 minutes of CPU (`research/cloudflare-git.md#9`, `#11`-`#15`). It would remove the 20 GB disk ceiling and the container cost, and it lost on clone performance and GHES-size repositories.
- **Refs in objects, metadata in D1.** D1 has one primary in one location, asynchronous replicas, and shared the June 2025 storage failure, and it splits each pull request's state across two stores with no shared commit point (`research/cloudflare-uptime.md#9`, `#37`).
- **Regional only, trusting the platform.** Simpler by a whole protocol and cheaper, and probably right if Cloudflare confirms followers span regions. It lost because priority 2 is never losing an acknowledged push, and no page supports that claim for a region.
- **K2 or Queues as the event log.** K2 is beta with a 10 GB account cap; Queues are unordered and retain 14 days. Chains already are the ordered, durable log (`research/cloudflare.md`, K2, Queues).
- **The AWS design** keeps zonal deploys and an SLA-backed RPO-zero store, at three to nine times this design's list cost depending on tier and topology, with a fold engine and a node fleet to build and run (`DESIGN.md`; `research/costs.md`).

## Open questions and risks

- Do a SQLite object's five followers span regions, and what is an object's restart time after a data-center loss? (`research/cloudflare-uptime.md` open questions 1 and 2). The Paired default and every Regional RTO rest on the answers.
- Does SQLite object storage still share a third-party dependency with KV, as in June 2025? (`research/cloudflare-uptime.md` open question 3).
- Can a Spectrum SSH app on Pro or Business use a tunnel virtual-network origin, and can `cloudflared` run as a connector inside a Container? Spectrum documents tunnel origins (https://developers.cloudflare.com/spectrum/reference/configuration-options/) but says some options need Enterprise.
- Does the Workers request-body limit apply to chunked `git push` bodies as to sized ones? (`research/cloudflare-git.md#23`).
- Do new container instances start during a control-plane outage, and is container egress billed for bytes returned to a Worker? Both change the warm-pool size and the 2,000-developer bill.
- Location hints are best effort and objects do not relocate (`research/cloudflare.md`, DO "Location"), so a standby could land near its primary. Milestone 0 must check that the two sides' colos differ, and placement must be verified per pair at creation, retrying with a new incarnation if they collide.
- Actions cache v2 and artifacts v4 need the toolkit's Azure Blob dialect, which a Worker must emulate over R2, as `DESIGN.md` "Actions" does over S3.
- Combining DO jurisdictions with location hints for an EU pair (`weur` and `eeur` under `eu`) is not documented on the pages read.
- Code Orange's progressive config rollout is five months old (`research/cloudflare-uptime.md#25`); the global-outage exposure in "Downtime" assumes it can still fail.
