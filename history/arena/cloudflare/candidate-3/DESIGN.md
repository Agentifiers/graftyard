# Tributary on Cloudflare, candidate 3: three-site quorum chains that survive a region, a continent, a bad deploy, and an account

## Summary

The bet: no Cloudflare storage product promises a cross-region RPO of zero (`research/cloudflare-uptime.md` finding 40), so Tributary builds one itself. Every chain (a repository, a fork network, an owner's `Authz` and `Identity`, a token shard, an org's Actions state) is a Raft group of three SQLite-backed Durable Objects on three **sites**. A site is one Cloudflare account pinned to one region, and the default sites sit on three continents (enam, weur, apac). A push is acknowledged only after its entry is on two sites in two regions and two accounts, and its pack bytes are in R2 buckets on two of those sites.

Products: Workers, Durable Objects, R2, Containers (upstream git), Spectrum and Cloudflare Tunnel (SSH), Queues, Workflows, Cron Triggers, DNS, WAF, Sandbox SDK, Email, Analytics Engine, Workers Logs, Basin for analytics only, and workerd for self-hosting and the off-Cloudflare **lifeboat**. KV, D1, Artifacts, and K2 are not used.

A GitHub user notices nothing in normal operation, apart from push latency of about one intercontinental round trip. They notice a lot during the failures that hurt other hosts. A regional Durable Object incident like the 6-hour WEUR one on 2026-10-22 costs them about 15 seconds of write unavailability. An R2-wide outage still accepts pushes under 1 MiB. During a Cloudflare-wide outage, an install with a lifeboat still serves clones and reads.

A Tributary maintainer notices that `DESIGN.md`'s cells, folds, and two-release format rule survive. Its database and failover machinery is replaced by one consensus module they must own and test hard. Releases roll one site per day, and followers on the previous release refuse entries they cannot decode, so an unreadable record shape can never reach a quorum.

## Usage (caller's view)

**Developer, managed.** Nothing changes from GHES targeting (`DESIGN.md`, "Wire compatibility", reused whole).

```
gh auth login --hostname tributary.dev
git clone https://tributary.dev/acme/monorepo.git            # HTTPS, protocol v2, partial clone allowed
git clone git@tributary.dev:acme/monorepo.git                 # SSH through Spectrum, if Milestone 0 passes
git push --atomic origin main release hotfix                  # one entry, two sites, then "ok"
gh pr merge 42 --squash && gh pr view 42 && gh pr list        # all three read the base chain's leader
```

**Developer, during a Cloudflare-wide outage, at an install that runs a lifeboat.** The lifeboat is a DNS-only hostname that the install publishes in advance, plus a `url.insteadOf` snippet:

```
git -c url."https://lifeboat.acme.example/".insteadOf="https://git.acme.example/" fetch    # read-only, ~30 s behind
GH_HOST=lifeboat.acme.example gh pr list                                                    # reads work, writes get 503
```

**Operator, BYOC or self-hosted.**

```
tributary sites add --account $A --region enam --jurisdiction us    # creates the site Worker, DO classes, bucket; checks the bucket's location
tributary sites add --account $B --region weur
tributary sites add --account $C --region apac
tributary placement set default tri --sites A,B,C                    # refuses two sites in one region or one account
tributary release roll 2026.44 --one-site-per-day                    # A Monday, B Tuesday, C Wednesday; the edge by percentage
tributary lifeboat init --host lifeboat.acme.example                 # workerd + git container + local disk, outbound only
```

**Call sites inside the code (Rust compiled to WebAssembly through workers-rs [inference]).**

```rust
// edge Worker, receive-pack: stage, validate, place, commit, report.
let pack = pack_store.place(net, body.tee_to(site_pair)).await?;        // two buckets, If-None-Match: *
let acked = group(ChainId::Repo(repo)).commit(Push { updates, atomicity, pack, actor, request }).await?;
report_status(&acked)                                                    // takes &Acked, so "ok" cannot precede the quorum

// edge Worker, `gh issue list --label bug`: read on the leader, after the lease is proven.
let view = group(ChainId::Repo(repo)).fresh(arrived).await?;            // Fresh only while the leader holds its lease
view.query(IssueFilter { labels: vec!["bug".into()], sort: Sort::Updated, ..Default::default() })
```

## Stack

| Product | Status | Used for |
| --- | --- | --- |
| Workers | GA | Edge Worker (HTTPS git, REST, GraphQL, web assets) in the edge account; one site Worker per site account that fronts its Durable Objects and buckets |
| Durable Objects (SQLite-backed) | GA | Group members: Raft log, lease, folds (SQLite and FTS5), consumer cursors; git cache owners for containers |
| R2 | GA | Pack and index copies (one bucket per site), clone cache, snapshots and archives, Actions blobs, lifeboat change feed |
| Containers | GA | Upstream git: index-pack, upload-pack, merge-tree, repack; SSH front running sshd and cloudflared |
| Sandbox SDK (Sandboxes) | GA | Hosted Actions runners |
| Spectrum | GA | Public port 22 for git over SSH |
| Cloudflare Tunnel | GA | Spectrum origin: cloudflared replicas inside the SSH front containers |
| DNS | GA | Authoritative DNS, including the DNS-only lifeboat hostname |
| WAF and rate limiting rules | GA | Abuse limits on clone, API, and login at the edge |
| Queues | GA | Webhook, notification, search-index, and lifeboat-feed fan-out from chain cursors |
| Workflows | GA | Member replacement after a site loss, imports, repository transfers, network compaction |
| Cron Triggers | GA | Placement re-verification, pack sweeper, GC, fold audit |
| Email Routing | GA | Reply-by-email for issues and pull requests |
| Email Sending (Email Service) | beta | Notification and password email, off every core path |
| Workers Analytics Engine | GA | Per-repository traffic counters and SLO data points |
| Workers Logs | GA | Request and error logs |
| Basin Pipelines | GA | Stream audit entries and SLO events into Iceberg tables |
| Basin Catalog | GA | Iceberg tables for audit retention and SLO history |
| Basin SQL | GA | Operator queries over audit and SLO tables; never serves a user request |
| Access | GA | Staff and admin console sign-in only |
| Data Localization Suite | GA | Managed EU placements: Regional Services so EU traffic is decrypted in the EU |
| workerd | GA [inference] | Self-hosted mode and the lifeboat |

Status cells copy `research/cloudflare.md`, first table.

## Shape

**Sites, not services, are the failure unit.** A site is one Cloudflare account, one region, its own site Worker, its own Durable Object classes, and its own pack bucket. Managed runs cells of three sites. The default cell is A = enam, B = weur, C = apac. Residency cells add D = wnam, E = eeur, F = oc. The edge account holds the zone, the edge Worker, and Spectrum. The edge has no state.

Each fault maps to one site:

- A region loss.
- A bad Tributary deploy, because releases roll one site per day ("Downtime").
- A bad account setting, a revoked or leaked account token, or an operator or abuse action scoped to one account [inference]. The 2025-02-06 abuse action was aimed at one bucket and disabled all of R2 by mistake (`research/cloudflare-uptime.md` finding 7). No account split contains that case, but one does contain the intended scope.
- A noisy neighbour on one account's Container quota.

Every group has one member per site, so each of these faults removes at most one member of every group. That keeps a majority [inference].

Members on other accounts are reached over HTTPS through each site's Worker, with Ed25519-signed requests, because Durable Object bindings do not cross accounts [inference]. Location comes from documented controls plus measurement. DO jurisdictions pin `eu` and `us` (https://developers.cloudflare.com/durable-objects/reference/data-location/). Hints are "best effort" (same page). So a new member fetches `/cdn-cgi/trace`, which names "the Cloudflare data center serving your request" (https://developers.cloudflare.com/fundamentals/reference/cdn-cgi-endpoint/). It refuses to join if that colo maps to the wrong region [inference that a DO subrequest leaves from the DO's colo; Milestone 0]. R2's get-bucket API returns the bucket's `location` (https://developers.cloudflare.com/api/resources/r2/subresources/buckets/methods/get/), so the installer refuses a misplaced bucket.

```rust
// ---- Sites and placement ----
pub enum Region { Enam, Wnam, Weur, Eeur, Apac, Oc }              // DO and R2 hint names that spawn locally
pub enum Jurisdiction { Eu, Us }
pub struct SiteId(u8);
/// One Cloudflare account in one region. Credentials, signing keys, and the site Worker live only here.
pub struct Site { pub id: SiteId, pub account: AccountTag, pub region: Region, pub jurisdiction: Option<Jurisdiction>,
                  pub ring_host: Hostname, pub bucket: BucketName, pub release: Release }
pub enum MemberKind { Full, Witness /* stores (term, index, digest), never content: used for residency */ }
pub struct Member { pub site: SiteId, pub kind: MemberKind, pub do_name: Box<str> /* "{chain}#{generation}" */,
                    pub colo: Colo /* measured at join */ }
/// Built only by `Placement::new`, which refuses two members in one region or one account and fewer than two Full.
pub struct Placement { members: [Member; 3], _private: () }
pub enum PlacementName {
    Tri,     // A enam, B weur, C apac, all Full: survives a region, a continent, a site deploy, an account
    Na3,     // enam, wnam, weur, all Full: lower latency for North America, does not survive an NA-wide event
    Us2w,    // enam (us), wnam (us) Full, weur Witness: content stays in the US
    Eu2w,    // weur (eu), eeur (eu) Full, enam Witness: content stays in the EU
    Apac2w,  // apac, oc Full, wnam Witness
}

// ---- Chains: the reference design's chain model, with groups in place of a table ----
pub enum ChainId { Repo(RepoId), Network(NetworkId), Authz(AccountId), Identity(AccountId),
                   Tokens(TokenShard), Names(NameShard), Actions(AccountId, u8), Release }
pub struct Term(u64);
pub struct Index(u64);
pub struct Digest([u8; 32]);
/// Row in each member's `log` table, keyed by index. `prev` chains digests, so a gap or a changed entry is refused.
pub struct LogEntry { pub term: Term, pub index: Index, pub prev: Digest, pub format: Format, pub tiers: TierSet,
                      pub at: Timestamp, pub requests: SmallVec<[RequestId; 4]>, pub body: EntryBody }
pub enum EntryBody { Events(Vec<Event>) /* <= 1.5 MB including inline packs; DO rows cap at 2 MB */,
                     Config(GroupConfig), Noop /* first entry of every term */ }
/// Membership is in the log, so a change commits like any other entry.
pub enum GroupConfig { Normal(Placement), Degraded { placement: Placement, lost: SiteId },
                       Joint { old: Placement, new: Placement } }
/// One row per member, written before any reply leaves (DO output gate), so a restart never forgets a vote or a promise.
pub struct MemberState { pub term: Term, pub voted_for: Option<SiteId>, pub commit: Index, pub applied: Index,
                         pub promise_until: Timestamp /* no vote for a rival until then */ }
pub const LEASE: Duration = Duration::from_secs(9);              // leader serves reads alone while it holds this
pub const ELECTION_AFTER: Duration = Duration::from_secs(10);    // follower may campaign only after its promise lapses
pub const DRIFT: Duration = Duration::from_millis(500);          // subtracted from the leader's own lease end [guess]

// ---- The write path ----
/// The only write path for chained state. It runs in the leader member. It proves the lease, runs the pure `decide`
/// on the fold at `applied`, and appends at last + 1. Every command queued in the same turn shares one entry. It
/// replicates to both followers at once and returns when the quorum rule of the current config holds:
///   Normal, all Full:     leader + 1 follower       (2 regions, 2 accounts)
///   Normal, 2 Full + W:   both Full members         (2 regions; the witness only votes)
///   Degraded:             surviving Full + Witness  (1 content copy; stated in "Downtime")
pub async fn commit<C: Command>(g: &Leader, cmd: C) -> Result<Acked<C::Out>, CommitError<C::Err>>;
pub trait Command { type Out; type Err;
    fn tiers(&self) -> TierSet;
    fn deps(&self) -> SmallVec<[ChainId; 2]>;    // e.g. the owner's Authz; read through `fresh` on that group
    fn decide(&self, fold: &Fresh, deps: &[Fresh]) -> Result<(SmallVec<[Event; 4]>, Self::Out), Self::Err>; }  // pure
/// Built only inside `commit`, from the acks it counted. `report_status` and every 2xx renderer take `&Acked`.
pub struct Acked<T> { pub index: Index, pub out: T, quorum: QuorumProof }
struct QuorumProof { sites: SmallVec<[SiteId; 3]>, regions: u8 /* >= 2 unless Degraded */ }
/// Followers run this before they store an entry, using the release they run: it must decode, chain to `prev`,
/// use an enabled format, and, for `Refs` and `Merged`, every `expect` must equal the follower's own fold.
/// A refusal means the entry cannot reach a quorum, so a release that writes what the old release cannot read,
/// or decides from a wrong fold, fails its own writes on the first site it reaches.
pub fn verify(e: &LogEntry, fold: &FoldView) -> Result<(), Refusal>;

// ---- The read path ----
/// A fold that includes every entry acknowledged before `arrived`. Only the leader builds one: from its fold while
/// `now < lease_end - DRIFT`, else after one read-index round to one follower, which also renews the lease.
/// A follower answers reads by batching read-index requests to the leader every 10 ms and waiting until applied.
pub async fn fresh(&self, arrived: Instant) -> Result<Fresh<'_>, NotLeader>;
/// Stale mode: entered by a member that cannot reach a quorum for 30 s, or by a signed flag; serves reads and
/// fetches at `applied` for a signed limit (default 4 hours); every write returns 503.
pub fn stale(&self) -> Result<Stale<'_>, StaleError>;
pub enum ReadView<'a> { Fresh(Fresh<'a>), Stale(Stale<'a>) }

// ---- Packs ----
pub struct PackId(Ulid);                                          // fresh for every placement
pub enum PackRef { Inline(Bytes) /* <= 1 MiB, lives in the entry */,
                   Stored { id: PackId, checksum: [u8; 32], bytes: u64, sites: SiteSet } }
/// Built only by `PackStore::place`: the bytes are in the buckets of two sites in two regions, each put with
/// If-None-Match: *, and a container's `index-pack --strict` passed on the same bytes.
pub struct DurablePack { pack: PackRef, _sealed: () }
pub enum R2Key { Pack { network: NetworkId, id: PackId }, Idx { network: NetworkId, id: PackId },
                 Snapshot { chain: ChainId, tier: Tier, upto: Index }, Archive { chain: ChainId, from: Index },
                 CloneCache { key: CloneKey, replica: u8 }, Export { minute: u64, seq: u32 } }
/// Same response bytes for the same key: protocol v2 fetch with no haves is deterministic for a fixed ref state.
pub struct CloneKey { pub network: NetworkId, pub wants: Digest, pub depth: Option<u32>, pub filter: Option<FilterSpec>,
                      pub caps: CapSet }

// ---- Git caches (derived) ----
/// A container-backed Durable Object per (network, site), on the leader's site and one more. Bare repository with
/// reftable on ephemeral disk, rebuilt from R2 packs and the leader's refs. Holds nothing canonical.
pub struct GitCache { pub network: NetworkId, pub site: SiteId }
impl GitCache {
    pub async fn validate(&self, staged: StagedPack, refs: &CoreRefs) -> Result<PackIndex, PackError>;
    pub async fn upload_pack(&self, req: FetchRequest, refs: &CoreRefs) -> Result<PackStream, GitError>;
    pub async fn merge(&self, req: MergeRequest) -> Result<MergePlan, MergeError>;   // includes the tree-diff check
}

// ---- Identity ----
/// 1,024 shards per home family (na, eu, apac). A new token's secret is redrawn until its hash lands in the owner's
/// family, about 3 draws on average, so its group's leader sits near the owner.
pub struct TokenShard(u16);
pub fn token_shard(secret: &[u8; 30]) -> TokenShard;
/// Per-isolate cache. A read-class request may use an entry up to 10 s old. A write-class request always reads the
/// token and the owner's Authz on their leaders (`fresh`), so a revocation binds every write acknowledged after it.
pub const READ_CACHE: Duration = Duration::from_secs(10);
pub const IDENTITY_BUDGET_PER_ISOLATE: u32 = 50;   // group reads per second; compiled, never config

// ---- Lifeboat feed ----
/// Written every 10 s by a Queue consumer from leader cursors: which chains moved and to what index.
pub struct ChangeBatch { pub minute: u64, pub seq: u32, pub chains: Vec<(ChainId, Index)> }
pub struct ExportCursor { pub chain: ChainId, pub index: Index }
```

**Reused from `DESIGN.md` without change:**

- `RepoEvent`, `AuthzEvent`, `IdentityEvent`, `NetworkEvent`, `RefUpdate`, `Push`, and `Atomicity` ("Chains and commits", "Repository chains and packs").
- The issue and pull request model, merge semantics, required checks as chain entries, and the merge queue ("Issues and pull requests").
- The core and metadata fold tiers ("Folds"). They now live in each member's SQLite instead of node files.
- Token formats, stateless installation tokens, and first-party client ids ("Identity and tokens").
- The Actions state machine ("Actions runs and jobs").
- The two-release format rule ("Change safety").
- `RestoredTo`, `ResetTo`, tombstones, and retention.

**Not needed here:** `Mirror` chains and cross-cell heartbeats are gone. Every group is reachable from every edge, so authorization reads the owner's `Authz` group directly. Managed logins are claimed on `Names` groups placed on `Tri`, so a continent loss never stalls a claim or loses a name.

**Invariants:**

- **Acknowledgment point.** A 2xx follows a quorum, and `Acked` exists only after one. At that moment the entry is in the SQLite of two members on two sites in two regions. Each member's copy was confirmed by its own storage before its reply left. A Durable Object write is confirmed "once at least three of these followers respond" across five data centers (`research/cloudflare-uptime.md` finding 32), and the output gate holds replies until writes are durable (https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/). Pack bytes are in two buckets in two regions, each persisted before its 200 (https://developers.cloudflare.com/r2/reference/durability/).
- **One value per index.** This is Raft's log matching. A member stores at most one entry per (term, index), and a leader exists only after a majority voted for it in that term. A minority side can neither commit nor, after its lease, serve a `Fresh` read [inference from the protocol].
- **An event exists if and only if its change committed.** Consumers read the log through cursors, never a separate outbox.
- **One writer per fold** (`apply` on each member), **one owner per fact.**
- **Placement is checked twice.** It is checked at join (colo trace, bucket `location`), then daily by a Cron Trigger. A member found in the wrong region is replaced by `ReplaceMember` (a Workflow).

**Dominant access patterns:**

- *Fetch or clone:* edge, token cache, then the repository leader's `fresh` core tier (one DO request; refs from SQLite). On a clone-cache hit, R2 serves the bytes through the edge with free egress (https://developers.cloudflare.com/r2/pricing/). On a miss, the `GitCache` container runs `upload-pack`, and the edge tees its output to the client and to R2.
- *Push:* edge, then token and owner `Authz` read on their leaders. The pack is teed to two buckets and into the container for `index-pack --strict`, then `commit`.
- *Issue or pull request list:* `fresh` metadata tier, then one indexed SQLite query on the leader.
- *Webhook:* leader alarm, cursor, Queue message, consumer Worker, POST.

## Commit and read paths

Every request enters at the Cloudflare data center nearest the client and runs the edge Worker (edge account). Calls to a group go to the site Worker of the member's site, then to the Durable Object by name with that site's hint and jurisdiction. "Leader site" below means the site of the current leader of the named group.

**Journey 1. Atomic push of three branches to a busy monorepo.** The monorepo's group runs on `Tri`, with the leader on A (enam).

1. The edge authenticates the token on its `Tokens` group leader. This is a write-class request, so it is never served from cache. The edge then reads the org's `Authz` group through `fresh`.
2. The edge streams the request body to the `GitCache` container on site A. In parallel it tees the same bytes to the three site buckets with multipart parts of 16 MiB [guess]. The first two buckets to finish make the pack durable, and the sweeper fills the third if it lags. The container runs `index-pack --strict` and checks connectivity against refs the leader just gave it. A pack of 1 MiB or less skips the buckets and rides inline in the entry.
3. `commit(Push { updates: [main, release, hotfix], atomicity: AllOrNothing })` runs on the leader. It checks each `expect` against the fold and checks rules from the fold and the org's rulesets. It appends one entry and sends it to B and C at once. B or C runs `verify` and stores it. The first ack makes the quorum.
4. Durable at "ok": the entry is on A and B (or A and C), two continents and two accounts. The pack is in at least two of the three buckets, or inside the entry.
5. Merge-queue merges (10 a minute) and bot ref moves (50 a minute) are entries on the same group. They serialize with the push, and group commit packs them into shared entries.
6. CI's 2,000 clones a minute (33 a second) each read refs from the leader. Each is then served from the R2 clone cache, keyed by the requested tips and depth. Each new tip misses once, and the edge tees that miss into the cache.
7. The next read from any client sees the push, because only a leader holding its lease answers `fresh`.
8. A 5 GB clone streams from the container with bitmaps and pack reuse. It finishes well inside the 20 minutes a runtime update allows a running stream (`research/cloudflare.md`, Workers).
9. A 2 GB push needs the Enterprise request body limit of up to 5 GB (`research/cloudflare.md`, Workers). Otherwise the path is the same, with the container on a `standard-4` (12 GiB, 20 GB disk).

**Journey 2. Fork pull request merged while a review and a required check land.** The fork lives in the same network, so its packs share the network's prefix. Its refs are on the fork's own group.

1. The merge plan reads the fork head through the fork group's `fresh`, and the base fold through the base group's `fresh`.
2. The base site's container runs `merge-tree`, `commit-tree`, and the tree-diff check (`DESIGN.md`, "Issues and pull requests"). The result pack is placed in two buckets.
3. `Merge::decide` runs on the base leader. The review, the `RequiredCheck` from `record_check`, and the merge are all entries on the base group, so they serialize. A review or check committed before the merge's entry is in the fold `decide` sees. One committed after it belongs to a merged pull request.
4. Durable at the 200: the `Merged` entry on two sites, and the merge pack in two buckets.
5. `gh pr view` and `gh pr list` run at once. Both go to the base leader, and both read its fold at or after the merge, so both show it merged.

**Journey 3. Issue labeled `bug`, then two lists.**

1. `IssueOpened` plus `Labels` is one entry on the repository group. It is applied to the metadata tier in the same SQLite transaction that records the commit on the leader.
2. `gh issue list --label bug` and `gh api repos/o/r/issues?sort=updated` both run `fresh` on the leader. Each is one indexed query, and both include the issue.
3. Nothing is eventually consistent on this path, and no search index is involved.

**Journey 4. Member removed and token revoked, then a push over HTTPS and SSH.**

1. Removal is an `Authz` entry on the org's group. Revocation is an entry on the token's shard group. Each 204 follows its quorum.
2. The next HTTPS push is write-class, so the edge reads the token on the shard leader and gets 401. If the member used another token, `commit` reads the org's `Authz` through `fresh` as a `dep`, so `decide` refuses with 403 (404 for a private repository).
3. Over SSH, Spectrum (port 22) forwards TCP through Cloudflare Tunnel to a cloudflared replica inside an SSH front container. `trib-sshd` resolves the key fingerprint on its `Tokens` group leader and forwards the git command to the edge as an internal signed request. The same checks refuse it.
4. Reads with a cached token may succeed for up to 10 s after the revocation (`READ_CACHE`).

**Journey 5. The data center, then the region, holding the newest writes is lost mid-push.** The leader is on A (enam). The entry was appended on A and sent to B and C.

- **Data center.** The Durable Object platform fences the failed host and restarts the object. "As long as at least one follower is still running, it will ensure that all confirmed writes are safely persisted" (`research/cloudflare.md`, Durable Objects, failure). If A comes back within the lease, nothing visible happens beyond a slow request. If not, B or C campaigns after `ELECTION_AFTER`, wins with the other's vote, and commits a `Noop`.
  - An entry that reached B or C survives, and the push is applied even though the client saw an error. Git's retry sees its refs already moved.
  - An entry that reached only A was never acknowledged, and the client retries.
- **Region.** The same as above, but A's bucket and A's containers are gone too.
  - Every acknowledged entry is on B or C, and every acknowledged pack is in B's bucket, because packs go to two sites.
  - B becomes leader in about 10 to 15 s [guess, Milestone 0]. The `GitCache` on B, or a new one, rebuilds from B's bucket.
  - `ReplaceMember` (a Workflow) then adds a member on D (wnam) through a `Joint` config. It copies the latest snapshot from R2 plus the log tail, and copies every pack of the group's networks into D's bucket. The group then holds 2-of-3 durability again.
  - What the client sees next: a fresh read from B, with every acknowledged push present.
- **Residency placement (`Eu2w`).** Losing weur leaves eeur (Full) and enam (Witness). They commit `Degraded { lost: weur }` and keep writing with one content copy until `ReplaceMember` restores two. "Downtime" states that RPO.

**Journey 6. Cloudflare-wide outages.** See "Downtime" for the full statement. In short:

- **November 18, 2025 class (core proxy failing everywhere).** Every Cloudflare-hosted surface fails: HTTPS, SSH, API, and web.
  - Developers keep local git.
  - At an install with a lifeboat, they also clone, fetch, read pull requests and issues, and use `gh api` GETs against the lifeboat. It is at most about 30 s behind.
  - No write is accepted anywhere until Cloudflare recovers, and no acknowledged write is lost.
- **November 2, 2023 class (control plane and API).** Developers see nothing. Tributary's request paths make no Cloudflare API call, and creating a repository only names new Durable Objects and keys in existing buckets.

**Journey 7. Bad releases.**

- **Tenfold identity reads.** The edge ships by gradual deployment, by percentage (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/).
  - `IDENTITY_BUDGET_PER_ISOLATE` is compiled. Beyond it, read-class requests use cached entries and write-class requests get 503, so identity group load cannot grow tenfold.
  - A shard that still overloads returns Durable Object `overloaded`, which is not retried (`research/cloudflare.md`, Durable Objects). The damage is limited to the tokens in that shard.
  - Swap-back is automatic when identity request rate per edge request exceeds its compiled ratio at 1%.
- **Record shape the previous release cannot read.** Group members roll one site per day. While site A runs N+1, B and C run N, and their `verify` refuses any entry that N cannot decode. Such an entry cannot reach a quorum, the client gets a 503, and nothing unreadable commits. Formats are enabled only through the `Release` group after every site runs N+1 (`DESIGN.md` two-release rule).

**Journey 8. BYOC and self-hosted.** See "Deployment modes".

- A BYOC company runs the same artifact in its own three accounts, or in one account with three site scripts.
- A self-hosted company runs the same groups on three workerd hosts. That is a weaker Durable Object store per host, but the same cross-host quorum.

## Scale

Numbers are [guess] unless they cite a limit.

| Quantity | Value | Limit it meets | Headroom |
| --- | --- | --- | --- |
| Monorepo leader requests (journey 1) | 33 ref reads/s for CI, about 2 entries/s, about 200 API reads/s [guess] | 1,000 req/s soft per DO (`research/cloudflare.md`) | 4x on the leader. Follower reads batch read-index calls every 10 ms, so each follower adds at most 100 leader calls/s while serving up to 1,000/s itself |
| Entries per group | 300 to 1,000 entries/s with group commit [guess] | Same soft limit; entry under 1.5 MB against the 2 MB row limit | The merge queue needs 0.2/s and bots 0.8/s |
| Member storage | Folds plus 30 days of log; the largest repository's metadata tier (200,000 issues, 2 million comments) about 4 GB [guess] | 10 GB per DO | Older entries archive to R2 snapshots; above 8 GB the metadata tier moves to a child group (Milestone 7) |
| Clone cache reads | 33 GETs/s on one key at a moving tip | No published per-key read rate (`research/cloudflare.md`, R2) | Hot keys are written as 8 replicas (`CloneCache.replica`), read at random |
| Pack writes | One write-once key per pack per bucket | 1 write/s to the same key | Keys are ULIDs, so never contended |
| Push size | 2 GB | Request body: 100 MB Free and Pro, 200 MB Business, up to 5 GB Enterprise | Managed is Enterprise; installs on lower plans state their cap |
| Clone duration | 5 GB at 50 MB/s is about 100 s [guess] | 20-minute drain on runtime updates | 12x |
| Containers per site account | 12 `standard-4` at 2,000 developers, split over two sites, is at most 48 vCPU on one account | 1,500 vCPU per account default | 30x; managed adds cells, each with its own accounts |
| Container instances per class | 12 | `max_instances` defaults to 20 | Raised at install |
| Fan-out | (8 pushes + 30 jobs) x 3 events per developer per day is 2,700 messages/s per 100,000 developers at 10x peak | 5,000 msg/s per queue | One queue per site; more by hash |
| Workflow starts | Member replacement after a region loss: every group with a member there | 300 instance creations/s per account | 1 million groups re-protect in about an hour; durability is 2-of-3 meanwhile |
| Managed at GitHub scale | 3.35 billion pushes a month is about 1,300/s (`research/git-storage.md` finding 5) | Unlimited DO storage per account on Paid | Spread over millions of groups, so no group is near its limit; cells add site accounts |

Latency [guess, Milestone 0]:

- Commit on `Tri` is one enam to weur round trip, about 80 ms, plus two local SQLite commits.
- Push p50 is about 300 ms beyond pack transfer: token, `Authz`, commit, and report.
- Fresh reads on a held lease cost one DO request in the leader's region, plus the client's distance to it.

## Downtime

| Incident | Evidence | This design | Effect |
| --- | --- | --- | --- |
| Partition and cross-country primary promotion, 24 h | `research/uptime.md` finding 40 | No primary per database. Each chain is a three-site quorum, a minority side cannot commit, and a deposed leader stops answering `fresh` when its 9 s lease ends. There are no divergent writes to reconcile | removed |
| Hardware and power, 4 core incidents | `research/uptime.md` finding 43 | A data center loss inside a site is absorbed by Durable Object storage (3 of 5 followers) and R2's in-region replication. A whole-site loss costs one member per group | removed |
| Database version bug and major-version upgrade | `research/uptime.md` finding 44 | No database engine to choose or upgrade. Durable Object runtime updates are Cloudflare's and roll by cohort, free customers first (`research/cloudflare-uptime.md` finding 25). A bad one in one region costs one member | contained |
| Shared permissions cluster overloaded, App tokens down 12 h | `research/uptime.md` finding 44 | No shared cluster: permissions are on each owner's `Authz` group and tokens on 1,024 shards per family. Overload of one shard returns `overloaded` for that shard only | contained |
| Partial disk failure that failover could not detect | `research/uptime.md` finding 44 | Failure detection is by the protocol, not health checks. A member whose storage stalls stops acking, the leader commits with the other follower, and a stalled leader loses its lease | removed |
| Misconfigured index and online migration overloading a table | `research/uptime.md` finding 44 | No DDL anywhere. Fold schemas change by lazy refold per group, one site at a time (`DESIGN.md`, "Folds") | removed |
| Dropped column still referenced (record shape) | `research/uptime.md` finding 44 | Followers on release N `verify` every entry, so release N+1 cannot commit what N cannot read while a quorum runs N. Formats are enabled through the `Release` group only after every site runs N+1 | removed |
| 32-bit key overflow | `research/uptime.md` finding 44 | Indexes and ids are 64-bit, with an alarm at 50% | removed |
| Middleware redeploy, LB change, expired certificate, proxy rollout | `research/uptime.md` finding 45 | Releases roll one site per day, and the edge by percentage with automatic swap-back. Certificates are Cloudflare-managed at the edge. No database proxy or sharding tier exists | contained |
| Internal DNS failure cascading | `research/uptime.md` finding 46 | No internal DNS. Members are reached by Durable Object name and fixed site hostnames | removed |
| Cache TTL cut plus tenfold client reads overloaded auth | `research/uptime.md` finding 47 | Identity load is bounded by a compiled per-isolate budget, not by request rate or a TTL. The read-class cache bound is compiled | contained |
| Redis failover left no writable primary | `research/uptime.md` finding 47 | Leader election is the steady-state protocol. Every site-day deploy exercises it, because a Durable Object "will only be reset when it is assigned a different version" and each deploy reassigns one member of every group (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/with-durable-objects/) | removed |
| LB tier failed to scale; a client retry loop slowed recovery | `research/uptime.md` finding 48 | There is no LB tier to scale: the anycast front runs Workers. Retries carry budgets of 10% per isolate, and a DO `overloaded` is never retried | mitigated |
| Merge queue reverted 2,092 pull requests | `research/uptime.md` finding 49 | Tree-diff check before every `Merged`, and `RestoredTo` per repository (`DESIGN.md`) | contained |
| Search overload emptied PR and issue lists | `research/uptime.md` finding 49 | Lists come from folds on the leader. Cross-repository search is a derived index fed by Queues, and a timeout returns `incomplete_results` | removed |
| 2023-10-04 1.1.1.1 SERVFAIL | `research/cloudflare-uptime.md` finding 1 | Not a Tributary dependency. Clients that use 1.1.1.1 may fail to resolve the host | contained |
| 2023-11-02 control plane and API, about 41 h | finding 2 | No request path calls the Cloudflare API, and repository creation needs none. Deploys, new sites, and staff dashboards wait | removed |
| 2024-03-26 PDX01 power | finding 3 | As 2023-11-02 | removed |
| 2024-06-20 CDN errors on 1.4 to 2.1% of requests, 114 min | finding 4 | Front door errors reach clients for the event's duration. Nothing behind the front fails, and no data is affected | exposed |
| 2024-06-27 1.1.1.1 BGP hijack | finding 5 | Not a dependency | contained |
| 2024-11-14 Logs lost 55% | finding 6 | Audit lives in `Identity` and `Authz` entries and is copied to Basin from cursors, so log loss is an observability gap only | contained |
| 2025-02-06 R2 disabled globally, 59 min | finding 7 | Packs up to 1 MiB ride inline in entries, so pushes of that size still commit [guess: most developer pushes]. Warm git caches serve fetches. Large pushes, cold clones, and clone-cache hits fail with 503 | mitigated |
| 2025-03-21 R2 writes 100% failed, 67 min | finding 8 | As 2025-02-06 for writes. Reads mostly served | mitigated |
| 2025-06-12 storage dependency under KV: DO and D1 22% errors, Queues down, Access down, 2 h 28 min | finding 9 | No KV, D1, or Access on any user path. A commit needs the leader's write plus one follower, so at a 22% per-request error rate an attempt succeeds about 74% of the time, and three tries about 98% [inference]. Queues down only delays webhooks, because cursors hold position | mitigated |
| 2025-07-14 1.1.1.1 withdrawal | finding 10 | Not a dependency | contained |
| 2025-08-21 AWS us-east-1 interconnect congestion | finding 11 | No origin outside Cloudflare | removed |
| 2025-09-12 dashboard and API, 75 min | finding 12 | As 2023-11-02 | removed |
| 2025-11-18 global core proxy failure, about 3 h | finding 13 | Every Cloudflare-hosted surface fails. Data is unaffected. Installs with a lifeboat serve clones, fetches, and reads from it at most about 30 s behind. Writes wait | exposed |
| 2025-12-05 FL1 killswitch, 28% of HTTP, 25 min | finding 14 | Front door exposure as 2025-11-18 for affected zones. The zone enables only the features it uses (no Bot Management, no KV, no Access on user paths) [guess that this narrows exposure] | exposed |
| 2026-01-08 1.1.1.1 CNAME ordering | finding 15 | Not a dependency | contained |
| 2026-01-22 Miami route leak, 25 min | finding 16 | Reachability for some networks. Anycast moves traffic to other locations | contained |
| 2026-02-20 BYOIP withdrawal, 6 h | finding 17 | Tributary uses Cloudflare-managed addresses, including Spectrum's | removed |
| 2026-10 regional DO and R2 incidents: ENAM, WEUR 6 h 18 min, Denver | finding 20 | One member per group sits in the impaired region. Groups led from it re-elect within about 15 s, and the others never notice | removed |
| 2026-10-23 North America connectivity, about 3 h | `research/cloudflare-uptime.md` open question 6 | `Tri` has one site in North America, so its groups keep a quorum. `Na3` and `Us2w` groups lose writes and serve stale reads from their remaining member | mitigated |

**SLOs** (month of 43,200 minutes; a minute is bad when over 5% of a journey's requests fail, GitHub's definition, `research/uptime.md` finding 51):

| Journey | Target | Budget (min/month) |
| --- | --- | --- |
| Git fetch, REST, GraphQL reads | 99.99% | 4.3 |
| Push, merge, writes | 99.95% | 21.6 |
| Web pages, Actions job start, webhook first attempt | 99.9% | 43.2 |

A provider-wide event like November 18, 2025 (core traffic failing from 11:20 to 14:30, about 190 minutes) breaks every target in its month, and Tributary cannot prevent that inside Cloudflare. The annual targets are 99.95% for reads and 99.9% for writes, about 263 and 526 minutes. Both absorb one such event a year. The two front-door events of 2025 (`research/cloudflare-uptime.md` findings 13 and 14), about 215 minutes together, would have used 82% of the annual read budget [inference].

**RPO and RTO per durability class.**

| Durability class | Data | RPO | RTO | Mechanism |
| --- | --- | --- | --- | --- |
| Chains on `Tri` or `Na3` | Refs, pull requests, issues, reviews, checks, `Authz`, identity, Actions job state | 0 for loss of any one site: a data center, a region, an account, or a site's release | Writes about 10 to 15 s, reads at most 9 s [guess] | Raft over three sites; `Acked` requires two regions |
| Chains on `Tri`, continent loss | Same | 0 | Same | One site per continent |
| Chains on `*2w` residency placements | Same | 0 for one Full site, then a single copy until `ReplaceMember` finishes | About 15 s into `Degraded` | Two Full sites plus a digest witness |
| Stored packs | Pack and index files | 0 for one site (two buckets before the ack), with the third copy minutes later | As the chain: any bucket in the set serves | `DurablePack` |
| Inline packs | Packs up to 1 MiB | As the chain | As the chain | Bytes inside the entry |
| Repository, operator error | Its named refs | Any index in 35 days | Minutes | `RestoredTo` from the log and archives |
| Member corruption or bad release data | One member's SQLite | 0 (two others hold it) | Minutes | Drop the member and `ReplaceMember`; DO PITR of 30 days per member as a last resort (`research/cloudflare.md`) |
| Derived state | Folds on a new member, git caches, clone cache, search | Rebuilt | 2 s core tier, minutes for caches [guess] | Snapshots in R2 plus the log |
| Actions blobs | Logs, artifacts, caches | Lost with their site's region | Rerun | One bucket on the job's leader site |
| Delivery history | Webhook attempt records | Lost with their site; deliveries resume from cursors (at-least-once) | Seconds | Cursor in the chain's group |
| Cloudflare-wide data loss (no precedent found) | Everything | Lifeboat lag, about 30 s, for installs that run one | Rebuild sites from the lifeboat export | Lifeboat holds the log and packs off Cloudflare |

## Failure modes

Components: `chain groups` (members of every group), `pack buckets`, `git caches` (containers and clone cache, derived), `front door` (edge Worker, site Workers, Spectrum, SSH front), `event pipeline` (cursors, Queues, consumers), `actions state` (Actions groups, broker, blobs, runners), `release control` (the `Release` group, site rollouts, keys), and `lifeboat`.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| chain groups | instance | One member's object resets; in-flight requests on it fail | The DO restarts and replays its SQLite. The leader commits with the other follower; a leader reset loses its lease and a follower wins within about 10 s |
| chain groups | data-center | Members whose host DC failed | DO storage follower fencing (`research/cloudflare-uptime.md` finding 32). The quorum continues on the other two sites |
| chain groups | region | One member of every group with a member there | Two other sites keep a quorum and elect within about 15 s, then `ReplaceMember` re-protects. `*2w` groups go `Degraded` |
| chain groups | provider-wide | Every member: DO errors (June 2025) or no reachability (November 2025) | Hedged replication to both followers and three retries under a 10% budget. Members that cannot reach a quorum for 30 s enter stale mode and serve reads. Writes return 503 and nothing acknowledged is lost |
| chain groups | control-plane | None at runtime | Members are created by name on first use. Classes and site Workers exist from install |
| chain groups | bad-deploy | One site's members, because releases roll one site per day | Followers `verify` with the old release, so an undecodable or wrong-`expect` entry cannot commit. Watchdog swap-back on 503 or refusal rate |
| chain groups | bad-config | One site, for example a wrong hint or jurisdiction | Placement verification refuses misplaced members. Signed config rolls per site |
| chain groups | noisy-tenant | One group, for example a viral repository | 1,000 req/s per object bounds it to itself. Per-repository admission, follower reads, and a cap of 50 comment entries/s |
| pack buckets | instance | Single requests 5xx | Retries; ULID keys never contend |
| pack buckets | data-center | None | R2 replicates "across several data centers within a geographic region" (`research/cloudflare.md`, R2) |
| pack buckets | region | One bucket of three | Two-of-three placement means every acknowledged pack has a copy elsewhere. Pushes place into the two live buckets. The sweeper refills the third after recovery or into the replacement site |
| pack buckets | provider-wide | All buckets (February and March 2025) | Inline packs up to 1 MiB keep most pushes committing. Warm caches serve fetches. Large pushes and cold clones return 503 |
| pack buckets | control-plane | None at runtime | Buckets are pre-created per site and their location checked at install |
| pack buckets | bad-deploy | Bad packs from one site's release | `index-pack --strict` before placement; GC deletes only after 14 days unreachable and at a fixed rate |
| pack buckets | bad-config | Writes to one site's bucket fail | A readiness probe writes and reads a canary object per bucket. Placement fails over to the other two |
| pack buckets | noisy-tenant | One network's prefix | Push bytes and concurrency caps per account and network |
| git caches | instance | One container's repositories and in-flight streams | Clients retry. A new instance rehydrates from R2. Clone-cache hits never touch a container |
| git caches | data-center | Containers on that host set | As instance. Each hot network is warm on two sites |
| git caches | region | That site's containers | The other warm site serves. New caches rehydrate from surviving buckets |
| git caches | provider-wide | All containers | Clone-cache hits keep serving if R2 serves. Otherwise git is down with everything else |
| git caches | control-plane | New container images cannot roll out | Running images keep serving. Instance starts use the deployed image [inference] |
| git caches | bad-deploy | One site's git image | Site-per-day rollout; a conformance clone, fetch, and push per site gate |
| git caches | bad-config | Instance size or `max_instances` too low on one site | The other site absorbs. The limit sits in signed config with an alarm at 70% |
| git caches | noisy-tenant | One giant network's repack or clone storm | Per-network CPU caps, the clone cache, repacks one network per container at a time |
| front door | instance | Requests on one isolate | Anycast and clients retry |
| front door | data-center | That colo's traffic | Cloudflare routes to the next colo |
| front door | region | Site Workers of one site | The edge calls the other members. Leader hints in isolate memory update on `NotLeader` |
| front door | provider-wide | Everything (November 18, 2025) | Lifeboat for installs. Otherwise stated as exposed |
| front door | control-plane | No deploys or WAF changes | Runtime needs none. Rules are frozen in signed zone config |
| front door | bad-deploy | Edge version at its rollout percentage | Gradual deployment at 1%, 10%, 50%, then 100%, with automatic rollback on SLO burn. Site Workers roll per site |
| front door | bad-config | A WAF or rate rule that blocks git | Zone config is code, reviewed by two people, applied off-peak with a synthetic clone before and after |
| front door | noisy-tenant | One IP, token, or repository flooding | WAF rate rules, per-token buckets in the identity group, per-repository admission at the leader |
| event pipeline | instance | One consumer invocation | At-least-once redelivery. Cursors advance only after a delivery record |
| event pipeline | data-center | None | Queues and cursors are elsewhere |
| event pipeline | region | One site's queue and consumers | Leaders on other sites keep pushing. Cursors on the lost site resume on the new leader |
| event pipeline | provider-wide | Queues down (June 2025) | Cursors hold position. Webhooks and notifications deliver late and none are lost |
| event pipeline | control-plane | None | Queues pre-created per site |
| event pipeline | bad-deploy | Wrong or stopped deliveries from one site | Payloads render from captured entry fields. A stopped consumer only delays, and replay is by cursor rewind |
| event pipeline | bad-config | A hook owner's concurrency set too high | Per-owner concurrency caps are compiled |
| event pipeline | noisy-tenant | One hook owner with a slow endpoint | Per-owner queues shards and a delivery deadline of 10 s |
| actions state | instance | Long polls on one broker object | Runners reconnect, and leases live in the Actions group |
| actions state | data-center | As chain groups | As chain groups |
| actions state | region | Job state survives (group). Logs and artifacts in that site's bucket are lost | Jobs re-queue on lease expiry. Users rerun jobs whose logs were lost |
| actions state | provider-wide | Hosted runners and brokers | Self-hosted runners wait. Job leases extend by the outage length on recovery |
| actions state | control-plane | Sandbox images cannot update | Running images serve |
| actions state | bad-deploy | Job dispatch on one site | Pinned-runner conformance gate per site |
| actions state | bad-config | Runner capacity per org | Per-org concurrency caps |
| actions state | noisy-tenant | One org's 256-job matrix | Per-org Actions groups (16 shards) and fair queuing by runner scope |
| release control | instance | One member of the `Release` group | Quorum, as chain groups |
| release control | data-center | As chain groups | As chain groups |
| release control | region | One member | Quorum. Rollouts pause and resume from the group |
| release control | provider-wide | No rollouts | Every site keeps its running release. Members cache the pointer |
| release control | control-plane | Wrangler and the API are unavailable | Rollouts wait. Nothing at runtime reads them |
| release control | bad-deploy | A bad rollout tool | The tool can only append signed intents to the `Release` group. Each site applies one release per day under its own watchdog |
| release control | bad-config | A wrong pointer, for example enabling a format early | `formats:enable` requires every site's reported release. Two keys sign it |
| release control | noisy-tenant | None | Not tenant-facing |
| lifeboat | instance | The lifeboat host | Two lifeboat hosts per install recommended, each with its own copy |
| lifeboat | data-center | The customer's site | As instance |
| lifeboat | region | The customer's region | As instance |
| lifeboat | provider-wide | The feed stops | It serves its last state and resumes from its cursor on recovery |
| lifeboat | control-plane | None | It needs only outbound HTTPS to R2 and the edge |
| lifeboat | bad-deploy | A bad lifeboat release | Lifeboat releases trail site releases by a week and keep the previous binary |
| lifeboat | bad-config | Wrong orgs mirrored | Scope comes from an org-signed export token |
| lifeboat | noisy-tenant | One huge org saturating the feed | Feed bandwidth caps per org |

## Deployment modes

One signed artifact, used in all three modes:

- the edge and site Worker bundles (Rust to WebAssembly)
- the git, SSH, and runner container images
- web assets
- the record formats and fold schemas
- the operator CLI
- the lifeboat bundle (a workerd config plus the same images)

The vendor's existing BYOC release registry and signing transfer unchanged (`DESIGN.md`, "Deployment modes"). Deploys use Terraform for every resource it covers and Wrangler for Containers, because no Terraform container resource exists (`research/cloudflare-git.md` finding 35).

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| chain groups | Cells of three site accounts (enam, weur, apac) plus residency cells; the vendor's accounts | Three site accounts in the customer's org (recommended), or one account with three site scripts, which keeps region and release separation but loses account separation | Three workerd hosts, one site each, with `localDisk` DO storage, which is experimental and local to one instance (`research/cloudflare-git.md` finding 39). The quorum still spans hosts |
| pack buckets | One R2 bucket per site account, location checked at install | Same, in the customer's accounts | Tributary's `trib-blob` service on each host's disk, same 2-of-3 rule, served through an R2-shaped binding |
| git caches | Containers per site, `standard-4` | Containers per site, sized by tier | Docker through workerd's container engine (finding 39), on the same hosts |
| front door | Edge account with the zone (Enterprise for 5 GB pushes), Spectrum SSH, and the Data Localization Suite for EU cells | The customer's zone. Pro is the floor for Spectrum SSH. Business allows 200 MB pushes, Enterprise 5 GB | workerd edge behind the customer's load balancer. sshd listens directly. No WAF or DDoS protection |
| event pipeline | Queues per site | Same | An in-process queue on each host, fed by the same cursors |
| actions state | Actions groups, hosted runners on Sandboxes | Actions groups, Sandboxes or customer runners | Actions groups; customer runners only, because workerd "is not a hardened sandbox" |
| release control | Vendor submits signed intents; one site per day | Vendor and customer both sign intents through an account-owned token whose permissions are a subset of the granting member's (`research/cloudflare-git.md` finding 34). The runtime never calls the vendor | Customer submits intents |
| lifeboat | Optional, run by an Enterprise customer for its own orgs on its own hardware | Recommended: two workerd hosts on the customer's network | Not needed: the install already runs off Cloudflare. A second site can play the same role |

**What each mode requires of the customer.**

- **Managed:** nothing. Enterprise orgs may run a lifeboat.
- **BYOC:**
  - three Cloudflare accounts with Workers Paid ($5 each), or one;
  - a zone on Pro or higher for SSH, and Enterprise for pushes over 200 MB;
  - Containers and R2 enabled;
  - an account-owned API token for the vendor.

  The install keeps serving indefinitely with the vendor unreachable, because no runtime path calls the vendor.
- **Self-hosted:**
  - three Linux hosts in three failure domains, with Docker;
  - a load balancer;
  - SMTP;
  - an ACME or uploaded certificate.

  It gets the same quorum durability and the same wire surface. It does not get Cloudflare's edge, DDoS protection, Spectrum, Email Sending, hosted runners, or Durable Object storage's own five-way replication on each host.

## Cost

Run `python3 arena/cloudflare/candidate-3/cost.py` from the repository root. It uses `TIERS` and `ACTIVITY` from `scripts/cost_model.py` and the `cf.*` prices in `research/prices.json`.

Every per-operation constant is a [guess] until Milestone 0. The constants are requests per git operation, CPU per request, rows per entry, container sizes, and a clone-cache hit rate of 70%. Free allowances are applied per account.

The plan line uses list prices: Pro at 50 developers and Business above. Managed and 2 GB pushes need Enterprise, which has no list price and is not included. Actions runner compute is not included either, the same as in `scripts/cost_model.py`.

| Developers | This design (four accounts, `Tri`) | One DO per repository, one site | Git in Artifacts, one DO per repository |
| --- | --- | --- | --- |
| 50 | $180 | $118 | $91 |
| 300 | $947 | $756 | $821 |
| 2,000 | $5,835 | $4,386 | $5,339 |

At 2,000 developers the largest lines are:

| Line | Per month |
| --- | --- |
| Container egress for developer fetches, clone-cache misses, and repack uploads | $1,556 |
| Container memory for 12 warm `standard-4` instances | $966 |
| R2 storage (three pack copies, clone cache, Actions blobs) | $816 |
| Spectrum SSH data at $1 per GB | $619 |
| DO SQLite storage (three member copies of 700 GB) | $420 |

The quorum costs $1,449 a month over a single Durable Object per repository at 2,000 developers, 33% more. At 50 developers it costs $62 more. That buys RPO zero and about 15 s write RTO for the loss of any one region, continent, account, or site release.

**Cheapest reliable setup for 50 developers: $168 a month** (`--detail small cheap`). It runs one Cloudflare account with three site scripts and three hinted regions, which keeps quorum across regions and release days. It drops the account separation. It uses the Pro plan, with 100 MB pushes and one Spectrum SSH app, and two `standard-2` git containers on two sites.

59% of that bill is container memory. A container slept overnight would halve that line, but it would add 1 to 3 s cold starts plus rehydration to the first morning fetch (`research/cloudflare.md`, Containers). I keep them warm.

## Build order from zero

Durations are [guess]es for a team of six.

### Milestone 0

About five weeks on three throwaway site accounts plus an Enterprise trial zone, before any git code. Any fail value sends the design back.

| Measurement | Pass | Fail |
| --- | --- | --- |
| Commit p99 on `Tri`, leader enam, 1 KB entries, 100 entries/s | At most 150 ms | Over 400 ms |
| Sustained entries on one group with group commit | At least 300 entries/s, with leader CPU under 50% | Under 100 entries/s |
| Write RTO: block one site's Worker route (simulated region loss) under 100 writes/s | New leader committing within 15 s at p99 | Over 60 s |
| Linearizability: 48 hours of Porcupine-checked histories while site routes are blocked and unblocked at random and members are reset | Zero violations, zero lost acknowledged entries | Any one |
| Member placement: 1,000 members created with each hint, colo checked by `/cdn-cgi/trace` from inside the DO | At least 95% in the hinted region, and the trace's colo matches a latency probe | Under 80%, or the trace does not reflect the DO's location |
| Bucket placement: 20 buckets per hint, `location` from the get-bucket API against a latency probe | 100% agree | The API reports the hint, not the placement |
| Idle cost: 100,000 idle groups for one week | Zero duration billed | Any duration billed for idle groups |
| Cross-account member RPC over the site Worker, against a same-account binding | p50 overhead at most 20 ms | Over 60 ms |
| Push size: 2 GB push through the edge on Enterprise, teed to two buckets and a container | Completes, with Worker memory under 100 MB | Fails or needs buffering |
| Chunked push on Business | Recorded either way | None: sets each plan's stated cap |
| Container rehydration of a 5 GB network from R2 | At most 120 s | Over 600 s |
| Clone cache: 33 CI clones/s of a monorepo whose tip moves 10 times a minute | Hit rate at least 95%, time to first byte p99 at most 500 ms | Under 80% |
| SSH: Spectrum, then Tunnel, then cloudflared in three containers, with one container killed every 10 minutes for a day | At least 99.9% of sessions succeed | Under 99%, which removes SSH from Cloudflare-hosted modes and lists it as a known deviation |
| Partial DO failure: inject 22% errors on member storage calls | Commit success at least 98% within three attempts | Under 90% |

### Milestones 1 to 7

| Milestone | Built | Gate |
| --- | --- | --- |
| 1, groups and git | Raft groups with `verify`, `PackStore`, `GitCache`, receive-pack and upload-pack, the clone cache. No REST | The `git` conformance suite and `git clone` / `fetch` / `push --atomic` from unmodified git 2.4x pass with a site route blocked mid-run. Linearizability rerun at 24 hours |
| 2, folds, issues, pull requests, merge | Both fold tiers in member SQLite, the issue, pull request, review, and merge-queue model reused from `DESIGN.md` | `gh pr` and `gh issue` journeys 2 and 3 pass, and the merge p99 on a freshly elected leader is at most 2 s |
| 3, identity and authorization | `Tokens`, `Names`, `Authz`, and `Identity` groups, OAuth device flow, stateless installation tokens | Journey 4 passes over HTTPS: revoked tokens refused on the next write, 100 times out of 100 |
| 4, events, webhooks, Actions | Cursors, Queues consumers, the Actions engine, Sandboxes runners | An unmodified `actions/runner` runs a matrix, and a site is blocked mid-run with no lost job state |
| 5, release control and site-per-day rollouts | `Release` group, `verify`-gated formats, gradual edge deploys, watchdog | A deliberately unreadable format released to site A fails its own writes and commits nothing |
| 6, region loss and lifeboat | `ReplaceMember`, `Degraded`, the lifeboat feed and workerd bundle, SSH if Milestone 0 passed | Journey 5 drill: delete a site account's routes for a day. Every acknowledged write is present, and the lifeboat serves `gh pr list` with the edge blocked |
| 7, breadth and modes | BYOC through Terraform and Wrangler, self-hosted on workerd, child groups for oversized metadata, import from GHES | Self-hosted install of three hosts passes milestones 1 to 6 gates with one host powered off |

## Compared with the AWS design

| Difference | AWS design (`DESIGN.md`) | This design | Who notices | Evidence |
| --- | --- | --- | --- | --- |
| Commit point | One conditional put on an MRSC DynamoDB chain item | A Raft quorum of three Durable Objects on three sites | Maintainers: Tributary now owns a consensus protocol | `research/uptime.md` finding 29; `research/cloudflare-uptime.md` finding 40 |
| RPO zero across regions | From the store's SLA (99.999%) | From Tributary's protocol; no Cloudflare SLA covers DO | Operator | `research/cloudflare-uptime.md` finding 29 |
| Survives a continent | Managed `global` only; Paired cells stay in one continent | Every `Tri` chain | Users during an NA-wide event | finding 20 and open question 6 |
| Bad release containment | Cell then AZ waves | Site per day, and followers on the old release refuse what they cannot read | Users | `research/cloudflare-uptime.md` finding 33 |
| Provider-wide outage | AWS regions fail separately | Cloudflare fails globally; lifeboat only | Users | `research/cloudflare-uptime.md` Answer, findings 13 and 14 |
| Push latency | One intra-continent MRSC put [guess 60 ms] | One intercontinental round trip on `Tri` [guess 80 ms], intra-continent on `Na3` | Users, barely | Milestone 0 |
| Folds | SQLite on NVMe nodes, preferred tail reader per AZ | SQLite inside each member; leader lease replaces the tail probe | Maintainers: no fold cache or tail probe | `DESIGN.md`, "Folds" |
| Read-your-writes | Strong tail read per request | Leader lease, with a read-index round when it lapses | Nobody | |
| Token revocation | 5 s for writes, 60 s for reads | Immediate for writes, 10 s for reads | Security teams | `DESIGN.md`, "Identity and tokens" |
| Push size | Unbounded by the front | 100 MB, 200 MB, or 5 GB by Cloudflare plan | Users with large pushes on lower plans | `research/cloudflare.md`, Workers |
| SSH | NLB to nodes | Spectrum to Tunnel to containers, unverified | Users if Milestone 0 fails | `research/cloudflare-git.md` finding 26 |
| Largest repository | Node NVMe | 20 GB container disk, about 15 GB of packs | Monorepo owners | `research/cloudflare-git.md` finding 16 |
| Self-hosted | Same binary in the customer's account | workerd plus Docker on three hosts, with weaker per-host storage | Self-hosters | `research/cloudflare-git.md` finding 39 |
| Monthly cost, 2,000 developers | $12,432 one Region, $20,636 Paired | $5,835 on three continents (Enterprise excluded) | Operator | `research/costs.md`; `cost.py` |
| Monthly cost, 50 developers, cheapest reliable | $388 for `scrappy-paired` | $168 for `cheap` | Operator | Same |

## Tradeoffs accepted

- We accept owning a Raft implementation on Durable Objects, with leases, joint consensus, and `verify`. In exchange we get RPO zero across regions and continents on a platform that sells none. Milestone 0's linearizability run and Milestone 1's rerun are its gates.
- We accept one intercontinental round trip on every write in `Tri`, about 80 ms [guess], in exchange for surviving a continent-wide event. `Na3` trades that back for customers who prefer it.
- We accept leader leases that rest on bounded clock drift (500 ms subtracted from a 9 s lease). If drift exceeded that, a deposed leader could serve a stale read for the excess. It could never commit.
- We accept three copies of every fold and of 30 days of log, about $420 a month at 2,000 developers, so that a new leader is warm.
- We accept that pushes over 1 MiB fail during an R2-wide outage, in exchange for keeping inline packs small enough that entries stay under the 2 MB row limit.
- We accept no service during a Cloudflare-wide outage for managed users without a lifeboat, because no Cloudflare product runs outside Cloudflare's network.
- We accept a lifeboat that is read-only. Accepting writes outside the quorum would fork chains, and that is the 2018 class of failure.
- We accept that `*2w` residency placements run on one content copy after losing a site, until `ReplaceMember` finishes.
- We accept losing Actions logs and artifacts in a region loss, in exchange for not tripling their storage.
- We accept that SSH depends on an unverified Spectrum, Tunnel, and Container chain, and that it is dropped if Milestone 0 fails it.
- We accept Spectrum SSH data at $1 per GB above the plan allowance, $619 a month at 2,000 developers with 20% of git bytes on SSH [guess].
- We accept four Cloudflare accounts per managed cell or BYOC install, and cross-account HTTPS between members, in exchange for account-level isolation.

## Alternatives considered

- **One Durable Object per repository (Artifacts', dgit's, and git-on-cloudflare's shape).** It is simpler and $1,449 a month cheaper at 2,000 developers. It loses on every regional event. The object "does not currently change locations after they are created" (`research/cloudflare-uptime.md` finding 31), and no document states where it restarts after a regional loss (`research/cloudflare.md`, "Could not verify"). The 13-day status sample shows 7 DO incidents, including WEUR from 17:51 to 00:09, about 6 h 18 min (finding 20). A repository homed there would lose writes, and probably reads, for that long. Under the quorum it loses about 15 s once, and only if its leader was there. Its cross-region RPO is unstated: five followers are in "different physical data center[s]", not regions (finding 32).
- **Cloudflare Artifacts as the git store.**
  - New repositories cap at 1 GB, against the 5 GB monorepo in journey 1.
  - Push is protocol v1 only, and `filter` (partial clone) is unsupported (`research/cloudflare-git.md` findings 5 and 6).
  - Durability is "synchronously across multiple data centers" with asynchronous copies to object storage, and "a repo is a single logical instance" (finding 7). That is the single-DO failure domain again.
  - It is in beta.
  - At 2,000 developers its storage is $2,000 a month at $0.50 per GB, against $180 for three R2 copies of the same 4 TB at $0.015. Its operations add $965.
- **A write-once quorum on three R2 buckets (Disk Paxos over conditional puts, no Durable Objects on the commit path).** It is attractive because R2's `If-None-Match` is atomic (`research/cloudflare.md`, R2), and buckets in two jurisdictions are guaranteed apart. It lost on three counts:
  - Both 2025 R2 incidents failed 100% of writes globally, for 59 and 67 minutes (findings 7 and 8), and this alternative would have stopped every write.
  - With no leases, every fresh read costs two cross-region GETs. At 33 clones/s that adds about 150 ms to every CI clone [guess].
  - Three-way split votes need a round protocol on objects that cannot refuse a stale round.
- **A heterogeneous five-member quorum (three DOs and two R2 objects).** It would survive an R2-wide and a partial DO outage at once. It lost because R2 objects cannot act as Paxos acceptors without CAS on a separate promise object, and inline packs already keep most pushes alive during R2 outages.
- **D1 for metadata and identity.** There is one primary in "one location in the world", replicas "may be arbitrarily out of date", and the 10 GB cap cannot be raised (`research/cloudflare-uptime.md` finding 37; `research/cloudflare-git.md` finding 31). D1 also failed at 22% in June 2025 (finding 9).
- **Workers KV for tokens and routing.** It is eventually consistent for up to 60 s, so revocation would be 60 s for writes (finding 38). It failed about 90% of requests on 2025-06-12 and failed again on 2025-11-18 (findings 9 and 13).
- **K2 as the event log.** It is in beta with a 10 GB account cap (`research/cloudflare.md`), and the chain is already the durable log.
- **Sites as Worker scripts inside one account (the `cheap` setup) for managed.** It saves $15 a month in subscriptions, and $12 in total at 50 developers. It loses isolation from account-scoped tokens, settings, and operator or abuse actions, which Cloudflare does take: the one on 2025-02-06 was aimed at a single bucket (finding 7). It is offered for small installs only.
- **A writable lifeboat that promotes during a Cloudflare-wide outage.** Writes made there could not merge back without a stated loss when Cloudflare returns, and a 3-hour outage does not justify that. This is `DESIGN.md`'s Single `dr` promotion problem.

## Open questions and risks

- Does a subrequest from inside a Durable Object to `/cdn-cgi/trace` report the object's own colo? Placement verification rests on it. If it does not, the fallback is jurisdiction-pinned members (`eu` against `us`) for two of three members, and latency triangulation for the third [inference].
- Does the R2 get-bucket `location` field report actual placement or only the hint? Milestone 0 compares it with latency.
- Where does a Durable Object restart after a whole-region failure, and does a recovering old leader ever run beside a new instance of itself? The protocol tolerates both, but Milestone 0's fault injection cannot reproduce Cloudflare's real behavior (`research/cloudflare-uptime.md` open questions 1 and 2).
- Does DO storage still depend on a third-party provider (finding 23)? If so, a correlated failure under all sites remains possible. That is June 2025's class, which this design mitigates but does not remove.
- How far can clocks drift inside Workers and Durable Objects? Lease safety assumes under 500 ms [guess].
- Can cloudflared run reliably inside Containers that "Cloudflare does not guarantee" to keep running (`research/cloudflare-git.md` finding 17), with Spectrum reaching it through a Tunnel virtual network?
- Do the 100 MB, 200 MB, and 5 GB request body limits apply to chunked `git push` bodies (`research/cloudflare-git.md`, "Could not verify")?
- Is container traffic back to its own Durable Object billed as egress? `cost.py` assumes yes. If not, the largest line at 2,000 developers falls by about half.
- Do Cloudflare's terms allow one service to span several accounts per cell, and will Cloudflare raise Container and Workflow limits per account?
- Code Orange's progressive config and cohort rollouts (finding 25) are five months old and untested in public. If they hold, the provider-wide rows marked exposed happen less often. Nothing in this design depends on it.
- A managed lifeboat on vendor-owned hardware running workerd would serve everyone during a Cloudflare-wide outage. Whether hardware outside Cloudflare fits the operator's "only Cloudflare technologies" is for the operator to decide.
