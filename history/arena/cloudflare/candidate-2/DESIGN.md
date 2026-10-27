# Tributary on Cloudflare, managed first: Artifacts holds the git, a Durable Object per repository decides, and upstream git runs only where Artifacts cannot

## Summary

The bet: Cloudflare already runs most of a code host, so Tributary should write only the policy layer. Artifacts (beta) stores the objects of every repository under 1 GB. One SQLite-backed Durable Object per repository is the commit point for refs, pull requests, issues, checks, and events. D1 holds the global directory of logins, tokens, keys, and routes. Queues, Workflows, R2, and Email do the rest. Custom code is the Rust front Worker (git wire, REST, GraphQL), the Durable Object classes, and one confined git engine: upstream git in Containers, which serves repositories over the Artifacts cap and computes merges, diffs, and blame for all.

Tributary never trusts Artifacts alone. Every byte it writes into Artifacts is also written to an R2 pack log in another region before the client is told "done" (while R2 itself is failing, within minutes after), in the format the Containers engine serves, so a repository can leave Artifacts (it grew past 1 GB, Artifacts changed, or a region died) without a copy step.

A GitHub user notices little: the same `git`, `gh`, and Octokit flows, partial clone served by the Containers engine instead of Artifacts, revocation that binds writes at once and reads within 40 s, and HTTPS pushes capped by the zone plan (5 GB on Enterprise). A maintainer notices much less to run than in `DESIGN.md`: no fold engine, no chain protocol, no node fleet, no database to patch; instead a closed beta engine whose limits Milestone 0 must measure, cross-region RPO of seconds for metadata, and every global Cloudflare outage as Tributary's outage.

## Usage (caller's view)

**Managed.** Nothing is provisioned per customer. A user signs up at `tributary.dev`; the commit point is one `INSERT` of the login row in its D1 directory shard, which fails on the unique key if the name is taken. An org owner picks a residency (`us`, `eu`, or none), which fixes the jurisdiction of the org's Durable Objects, Artifacts namespace, and R2 buckets at creation (`research/cloudflare-git.md#40`, `#41`).

**BYOC.** The customer creates a Cloudflare account on Workers Paid with a zone for its host, and grants the vendor an account-owned API token limited to Workers, Durable Objects, D1, R2, Queues, Workflows, Containers, Artifacts, and DNS edit (`research/cloudflare-git.md#34`).

```sh
# Vendor CI, against the customer's account. Terraform owns D1, R2, Queues, DNS, Spectrum, the Tunnel, and the
# Artifacts namespaces; Wrangler owns Workers, Durable Object migrations, and Containers, which have no Terraform
# resource (research/cloudflare-git.md#35, #36).
tributary release verify tributary-2027.03.1.tar.zst            # Ed25519 signature over the content digest
tributary install plan  --account $CF_ACCOUNT --zone git.example.com --jurisdiction eu --plan business
tributary install apply --account $CF_ACCOUNT --zone git.example.com     # terraform apply, then wrangler deploy
tributary admin bootstrap --first-admin alice                         # prints a one-time sign-in link
```

**Self-hosted** means the same bundle, applied by the customer's own CI into its own Cloudflare account with no vendor token. A deployment on the customer's own hardware is not offered ("Deployment modes").

**What a `gh` user types** is unchanged from `DESIGN.md` ("Usage", "What a `gh` user types"): `gh auth login --hostname tributary.dev`, `git clone`, `gh repo fork`, `git push`, `gh pr create`. gh treats the host as GHES because of its name (`research/clients.md#1`).

**A maintainer** writes policy against two interfaces and never sees which engine holds a repository:

```rust
// POST /{o}/{r}.git/git-receive-pack, in the front Worker
let admitted = repo.admit(&principal, PushRequest::parse(&mut body).await?).await?;   // authz, rules, old values
let staged = engines.for_tier(admitted.tier()).stage(&admitted, body).await?;          // Artifacts or Large, plus pack log
let acked = repo.commit(admitted, staged).await?;                                      // one SQLite transaction
report_status(&mut out, &acked).await?;                                                // takes &Acked: no "ok" before commit
// GET /repos/{o}/{r}/issues?labels=bug&sort=updated
let page = repo.issues(&principal, IssueQuery::from_rest(&params)?).await?;            // SQL inside the Repo DO
```

## Stack

| Product | Status | Used for |
| --- | --- | --- |
| Workers | GA | Front Worker in Rust: git smart HTTP, REST, GraphQL, LFS batch, Actions service endpoints, web app static assets; Gradual Deployments for every release |
| Durable Objects (SQLite-backed) | GA | Repo DO per repository (refs, pull requests, issues, checks, outbox), Owner DO per user or org (members, teams, repository names), Run DO per Actions run, Pool DO per runner pool |
| D1 | GA (read replication beta) | Directory shards: logins, accounts, token hashes, SSH fingerprints, emails, routes, id blocks; Sessions API for replica reads |
| Artifacts | beta | Git objects for every repository under the cap, follower copy of its refs, single-object reads for the web and API |
| R2 | GA | Pack log (second-region copy of every write), Large-tier packs in two buckets, clone cache, metadata journal, LFS, release assets, Actions cache, artifacts, logs |
| Queues | GA | Event fan-out from Repo DO outboxes: webhooks, Actions triggers, PR head sync, notifications, search, audit |
| Workflows | GA | Graduation to the Large tier, pack log consolidation, region recovery, imports, transfers, deletions |
| Containers | GA | Confined upstream git: Large-tier hosts, the git compute pool (merges, diffs, blame, partial clone), SSH fronts, hosted Actions runners |
| Spectrum | GA | Public port 22 for git over SSH |
| Cloudflare Tunnel | GA | Spectrum origin: `cloudflared` replicas inside the SSH front containers |
| DNS | GA | Authoritative zone for the host and isolation subdomains |
| WAF and rate limiting rules | GA | Anonymous and per-token limits, abuse shedding before the Worker runs |
| Cron Triggers | GA | Release health gate, intent sweeps, clone cache trimming |
| Email Sending (Email Service) | beta | Notifications and password resets |
| Email Routing | GA | Reply-by-email for issues and pull requests |
| Secrets Store | beta | Signing keys for installation tokens, OIDC, webhook HMAC wrapping key |
| Workers Analytics Engine | GA | Per-repository traffic counters behind the traffic API, release health metrics |
| Workers Logs | GA | Request and error logs, 7 days |
| Basin Pipelines | GA | Audit events from the outbox into Iceberg on R2 |
| Basin Catalog | GA | Iceberg tables for the audit log |
| Basin SQL | GA | Audit log search and exports (analytics only, never on a commit path) |
| Access | GA | Staff console SSO |
| Data Localization Suite | GA | Managed EU orgs that also need TLS terminated only in the EU (Enterprise add-on) |

Not used: Workers KV, because it is eventually consistent for up to 60 s and its storage failure took Durable Objects, D1, and Queues with it in June 2025 (`research/cloudflare-uptime.md#9`, `#38`); K2, because it is beta with 20 streams per account (`research/cloudflare.md`, K2); Hyperdrive, because no external database exists.

## Shape

Four kinds of state, each with one owner. The Repo DO owns everything scoped to a repository. The Owner DO owns everything scoped to a user or org. D1 owns everything that must be unique across owners. Artifacts or a Large host owns git objects, and R2 owns every copy Tributary must control. No record has two writers.

```rust
// ---------- Placement and tenancy. D1 directory, sharded by key hash into `dir-00`..`dir-15` (one shard per BYOC install). ----------
pub struct AccountId(i64); pub struct RepoId(i64); pub struct NetworkId(RepoId);   // a network id is its root repository's id
/// Every GitHub-visible id comes from a 4,096-id block leased from `idblocks` in shard 0, so ids stay below 2^53
/// (research/wire-census.md#28) with no allocator on the request path.
pub struct DbId(i64);
pub enum LocationHint { Wnam, Enam, Weur, Eeur, Apac, Oc }        // DO and R2 hints (research/cloudflare.md, DO and R2 rows)
pub enum Jurisdiction { None, Us, Eu }                              // fixed at creation for DO, R2, D1, and Artifacts namespaces
pub enum DirRow {
    Login   { key: LoginKey, account: AccountId, kind: AccountKind, v: u64 },           // UNIQUE(key): the claim
    Account { id: AccountId, login: Login, home: LocationHint, jurisdiction: Jurisdiction, state: AccountState },
    Token   { hash: [u8; 32], account: AccountId, kind: TokenKind, scopes: Scopes, expires: Timestamp, revoked: Option<Timestamp> },
    SshKey  { fingerprint: [u8; 32], account: AccountId, key_id: DbId },                // UNIQUE: one key, one account
    Route   { owner: AccountId, name: RepoNameKey, repo: RepoId, state: RouteState },   // shard by owner id, so renames move nothing
    RepoById { repo: RepoId, owner: AccountId, generation: u32, hint: LocationHint },   // `/repositories/{id}` and node ids
}
pub enum RouteState { Live, Alias { to: RepoNameKey }, Deleted { retain_until: Timestamp } }
/// A Repo DO's name carries a generation, so region recovery creates a new object instead of waiting for a lost one.
pub fn repo_do_name(repo: RepoId, generation: u32) -> String { format!("repo:{}:g{}", repo.0, generation) }
/// Artifacts namespaces are sharded so no namespace nears 2,000 control requests per 10 s (Artifacts limits page).
pub struct ArtifactsAddr { namespace: Box<str> /* "t-{jurisdiction}-{repo % 256:02x}" */, name: Box<str> /* "r{repo}" */ }

// ---------- Repository storage and ref transactions. Repo DO tables. ----------
pub struct Oid([u8; 20]);                                          // SHA-1 only at launch, as on github.com (research/wire-census.md#44)
pub enum Tier { Artifacts(ArtifactsAddr), Large(NetworkId), Graduating { from: ArtifactsAddr, to: NetworkId } }
pub struct RefRow { name: RefName, oid: Oid, peeled: Option<Oid>, seq: u64 }      // the only ref truth Tributary serves
pub struct RefUpdate { pub name: RefName, pub old: Option<Oid>, pub new: Option<Oid> }
pub struct PushIntent { id: PushId /* ULID */, updates: BoundedVec<RefUpdate, 10_000>, atomic: bool, actor: Principal,
                        state: IntentState, packlog: Option<PackLogKey>, created: Timestamp }
pub enum IntentState { Admitted, Staged { staging_prefix: RefName }, Committed { seq: u64 }, Rejected(Vec<RefOutcome>), Applied, Abandoned }
/// Built only by `GitEngine::stage`, after the engine reported every staging ref ok and the pack log write
/// returned (or failed fast, which sets `durability: EngineOnly` and raises an alarm).
pub struct Staged { push: PushId, peeled: Vec<(Oid, Oid)>, packlog: PackLogOutcome, _seal: Seal }
pub enum PackLogOutcome { Written(PackLogKey), EngineOnly }
/// Built only by `RepoDo::commit`, after the SQLite transaction confirmed (output gate, 3 of 5 followers).
pub struct Acked { pub seq: u64, pub outcomes: Vec<(RefName, RefOutcome)>, _seal: Seal }
pub enum RefOutcome { Ok, Ng(RejectReason) }
pub enum RejectReason { Stale, NonFastForward, Protected(RuleViolation), Hidden, TooManyRefs }

/// The one interface over both engines. Callers pick an engine by `Tier` and never branch on it again.
pub trait GitEngine {
    /// Artifacts: rewrite every update to create `refs/tributary/q/{push}/{i}`, stream the client's pack to the Artifacts
    /// remote, and tee the same bytes to the pack log. Large: quarantine receive-pack on the network's host, which places
    /// the pack in two R2 buckets. Neither moves a visible ref.
    async fn stage(&self, push: &Admitted, body: PackStream) -> Result<Staged, StageError>;
    /// Make the engine's own refs equal the committed ones and drop staging refs. Idempotent; retried by the DO alarm.
    async fn apply(&self, updates: &[RefUpdate], drop_staging: &[PushId]) -> Result<(), ApplyError>;
    /// Serve `fetch` for wants the Repo DO already resolved. Refs are never advertised by an engine.
    async fn upload_pack(&self, req: FetchRequest) -> Result<ByteStream, FetchError>;
    /// One object by id: Artifacts `readCommit`/`readTree`/`readBlob`, or `cat-file` on the Large host.
    async fn read(&self, oid: Oid, kind: ObjectKind) -> Result<Option<Object>, ReadError>;
}
/// Upstream git in a Container: merge-tree, commit-tree, replay, log, diff, blame, archive, grep, and filtered fetches.
/// For an Artifacts-tier repository it keeps a mirror fetched from Artifacts; for a Large network it is the host.
pub enum ComputeJob { Merge { base: Oid, head: Oid, method: MergeMethod, message: String }, Mergeable { base: Oid, head: Oid },
                      Log { from: Oid, page: Page }, Compare { base: Oid, head: Oid }, Diff { base: Oid, head: Oid, path: Option<Path> },
                      Blame { at: Oid, path: Path }, Archive { at: Oid, format: ArchiveFormat }, Fetch(FetchRequest) }
pub enum ComputeOut { NewObjects { tip: Oid, pack: PackStream }, Answer(Bytes), Conflict(Vec<Path>), Stream(ByteStream) }

pub struct PackLogKey(Box<str>);  // "pl/{repo}/{push-ulid}": the push's updates in a header, then the pack bytes
/// Size bookkeeping: Artifacts reports no repository size (REST `RepoInfo` has none), so the DO sums staged pack bytes.
pub struct SizeBook { staged_bytes: u64, consolidated_bytes: u64, storage_limit_errors: u32 }
/// Parents and generation numbers of every commit reachable from a ref, so ancestry checks, compare counts, and
/// merge bases walk SQL rows. Filled at commit for protected refs and by the apply alarm for the rest (10,000 a run).
pub struct CommitNode { oid: Oid, parents: SmallVec<[Oid; 2]>, generation: u32 }
pub const GRADUATE_AT: u64 = 768 << 20;                            // 75% of the 1 GB cap

// ---------- Identity and tokens. Owner DO tables, plus the D1 rows above. ----------
pub enum TokenKind { Pat, FineGrained, OAuth, UserToServer, Installation, Refresh }   // ghp_ github_pat_ gho_ ghu_ ghs_ ghr_
pub struct Member { user: AccountId, role: OrgRole }                // Owner DO: one row per member
pub struct Team { id: DbId, slug: Box<str>, parent: Option<DbId> }
pub struct TeamGrant { team: DbId, repo: RepoId, permission: Permission }
pub struct RepoName { key: RepoNameKey, repo: RepoId, state: RouteState }  // the claim; the D1 Route row is its read model
pub struct AuthzEpoch(u64);                                         // bumped by every membership, team, or grant change
impl OwnerDo {
    /// Strong: the answer reflects every change committed before the call started. Writes always call this.
    async fn permission(&self, user: AccountId, repo: RepoId, visibility: Visibility) -> (Permission, AuthzEpoch);
}

// ---------- Issues and pull requests. Repo DO tables; issue numbers are allocated in the same transaction. ----------
pub struct Issue { id: DbId, number: u32, kind: IssueKind, title: Box<str>, body: Body, state: IssueState,
                   author: AccountId, created: Timestamp, updated: Timestamp }        // indexes: (state, updated), (label, number)
pub enum Body { Inline(Box<str>) /* <= 64 KiB */, R2(Digest) }
pub struct PullRequest { number: u32, base: RefName, head: HeadSpec, head_sha: Oid, state: PrState, merge_ref: Option<Oid> }
pub struct HeadSpec { repo: RepoId, branch: RefName }              // a fork's head is copied in as refs/pull/N/head
pub struct Review { pr: u32, reviewer: AccountId, verdict: Verdict, commit: Oid }
pub struct CheckRun { id: DbId, head_sha: Oid, name: Box<str>, app: DbId, status: CheckStatus, conclusion: Option<Conclusion> }
pub struct Mergeable { base: Oid, head: Oid, result: MergeResult }  // keyed by the SHA pair, so it never needs invalidation
pub struct MergeQueueEntry { pr: u32, group_tip: Oid, parent_tip: Oid }  // parent_tip is the previous entry's group_tip or main

// ---------- Actions. Run DO per run, Pool DO per runner pool. ----------
pub enum JobState { Pending { needs: SmallVec<[DbId; 4]> }, Queued { labels: LabelSet }, Assigned { runner: DbId, lease_until: Timestamp },
                    InProgress { runner: DbId, lease_until: Timestamp }, Completed { conclusion: Conclusion } }

// ---------- Event outbox. A row in the same transaction as the change it describes. ----------
pub struct OutboxRow { seq: u64, event: EventKind, payload: Bytes, enqueued: bool }
pub enum Consumer { Webhooks, ActionsTrigger, PrSync, Notify, Search, Audit, Journal, CloneCacheWarm }
```

Invariants the structure enforces:

- **The Repo DO is the commit point.** A ref is visible only through `RefRow`, written only by `RepoDo::commit` in one SQLite transaction with the intent's new state and the outbox rows. The DO's output gate holds the response until the write is confirmed by 3 of 5 followers in different data centers (`research/cloudflare-uptime.md#32`). Artifacts refs are a follower copy that `apply` brings level after the acknowledgement; the staging refs keep the new objects reachable in the meantime.
- **Nothing is committed that is not durable in two places.** `commit` takes a `Staged`, which only `stage` builds, after the engine (Artifacts replicates synchronously across data centers, `research/cloudflare.md`, Artifacts) and the pack log in a second-region R2 bucket both hold the bytes. If R2 fails fast, the push still commits on the engine copy and the DO records `EngineOnly`, which the consolidation workflow repairs within minutes; this keeps R2 outages off the push path (`research/cloudflare-uptime.md#7`, `#8`) at the price of a stated RPO window.
- **Engines never advertise refs.** Every `info/refs`, `ls-refs`, and v0 advertisement is built from `RefRow`, so hidden staging refs never leak and both tiers show one capability set. A `fetch` goes to Artifacts only if every argument is in the set Artifacts supports; anything else (for example `filter`, `wait-for-done`) goes to the compute pool, so the advertisement can stay identical to github.com's: `ls-refs=unborn`, `fetch=shallow wait-for-done filter`, `server-option`, `object-format=sha1` (`research/wire-census.md#41`).
- **Push stays on protocol v0/v1, which is all git speaks.** Upstream git downgrades every non-upload-pack service to v0 (`connect.c`: "if (version == protocol_v2 && service != GIT_CONNECT_UPLOAD_PACK) version = protocol_v0", https://github.com/git/git/blob/master/connect.c), and `transport.c` dies with "support for protocol v2 not implemented yet" for a v2 push. Artifacts' v1-only receive-pack (`research/cloudflare-git.md#5`) is therefore invisible to unmodified clients [inference from both]. Tributary advertises github.com's receive-pack set: `report-status report-status-v2 delete-refs side-band-64k ofs-delta atomic object-format=sha1 quiet push-options` (`research/wire-census.md#44`).
- **One engine interface, one write path.** Merges, PR head copies, imports, and recovery all produce a `PackStream` that goes through `stage`, so the pack log and size bookkeeping never miss a writer.
- **Repositories leave Artifacts without a copy step.** The pack log is a list of packs plus their ref updates, which is the Large tier's own storage format, so graduation and recovery read it directly. The `Consolidate` workflow runs weekly per active repository, and within minutes after any `EngineOnly` commit: it streams an incremental fetch from Artifacts (haves are the last consolidated tips) into one new pack log object that carries the ref snapshot at its `seq`, then deletes the raw push packs it covers.
- **Merge queue tips only fast-forward.** A queue entry's `parent_tip` must equal the previous entry's tip, and main moves only to a queue tip that descends from main's current value, checked in the commit transaction. This is the structural guard against GitHub's April 2026 group merges that reverted earlier changes (`research/uptime.md#49`).
- **Authorization for writes is never cached.** `admit` and `merge` call `OwnerDo::permission` and re-read the token on the D1 primary, so a removal or revocation binds the next write after its 204. Reads may use a cached token or allow for 30 s per isolate. A token lookup that misses the cache reads a D1 replica, which "may be arbitrarily out of date" (`research/cloudflare-uptime.md#37`), so a Cron Trigger measures each shard's replica lag every minute and, above 10 s, the Worker sends token lookups to the primary until it recovers.
- **One fork network per tier.** Forks of an Artifacts-tier repository are Artifacts forks (the binding's `fork()`); forks of a Large-tier repository join its network on the same host and share its packs, as in `DESIGN.md` ("Namespace and cell placement"). A pull request between tiers works because `PrSync` moves objects through `stage`.
- **Reused from `DESIGN.md` by section, unchanged except for storage names:** "Wire compatibility" (tiers, oracles, and the Octokit contract run), "Identity and tokens" (token kinds, prefixes, and formats), "Public service concerns" (rate limits, abuse, takedowns, enterprise managed users), "Actions" (workflow engine, runner protocol with `use_v2_flow: true`, toolkit pins, cache, artifacts, logs, OIDC) with R2 presigned URLs on the S3 API domain in place of object storage (`research/cloudflare-git.md#30`) and hosted runners as Containers with rootful Docker and no KVM (`#27`, `#28`), and "Frontend and native app" with the React app served as Workers static assets (https://developers.cloudflare.com/workers/static-assets/).

Dominant access patterns, traced:

- **Fetch or clone** (the largest volume): Worker resolves token and route from the isolate cache or a D1 replica; one Repo DO call returns refs and the read token; `fetch` goes to the R2 clone cache (CI, no `have` lines) or the engine. Two D1 rows on a miss, one DO request, one Artifacts or R2 request.
- **Push**: four DO calls (`advertise`, `admit` with one Owner DO call, `commit`), one Artifacts receive-pack plus one later `apply`, one R2 multipart upload, about 12 SQLite rows.
- **Issue and PR lists**: one DO call that runs SQL over indexed tables in the repository's own SQLite. No index is eventually consistent, so `gh issue list --label bug` after `gh issue create` always finds the issue.
- **File and tree views**: DO resolves the ref, then `GitEngine::read` by object id, cached per isolate and in R2 by id because objects are immutable.
- **Commit lists, compare, diffs, blame, merges**: the compute pool, with answers cached in R2 under the SHA pair.

## Commit and read paths

**Push to an Artifacts-tier repository.** (1) `GET info/refs?service=git-receive-pack`: the Worker authenticates, and the Repo DO builds the advertisement from `RefRow`. (2) `POST git-receive-pack`: the Worker parses the command block and push options, then `admit` checks permission (Owner DO, strong), ref-name and protection rules that need no objects, and old values against `RefRow`, and records `PushIntent::Admitted`. (3) The Worker streams the rewritten command block (each update becomes a create of `refs/tributary/q/{push}/{i}`) and the client's unmodified pack to the Artifacts remote, `https://<ACCOUNT_ID>.artifacts.cloudflare.net/git/<namespace>/<repo>.git` with a bearer write token the DO holds (https://developers.cloudflare.com/artifacts/api/git-protocol/), and tees the bytes into an R2 multipart upload in the pack log bucket. Thin packs work because their bases are objects the repository already has. (4) `commit` takes the refs lock, which is never held across a pack stream, re-checks old values, checks fast-forward for refs that forbid force pushes by walking `readCommit` from the new tip until it meets a `CommitNode`, and commits. A walk that passes 2,000 commits asks the compute pool for `git merge-base --is-ancestor` instead. (5) The Worker writes `report-status`. (6) An alarm calls `apply`, which sends Artifacts one atomic receive-pack with an empty pack that moves the real refs and deletes the staging refs, batching every commit since the last apply. Staged intents that never commit are abandoned after 10 minutes and their staging refs deleted.

**Push to a Large-tier repository.** The Worker streams the body to the network's host container through its Durable Object. Upstream `git receive-pack` quarantines and indexes the pack (`index-pack --strict`); its `pre-receive` hook places the quarantine pack and index in two R2 buckets with `If-None-Match: *` and calls `admit` and `commit` with the facts git computed (old values, fast-forward), and receive-pack updates the host's local refs only after the hook returns success. If the host dies after the commit, the client's retry finds its refs already moved, and the next host start rebuilds from R2 and `RefRow`.

**Fetch and clone.** v2 `ls-refs` is answered by the DO, filtered by `ref-prefix`, with peeled tags from `RefRow.peeled` (filled at staging from Artifacts' own `ls-refs` with `peel`). v0 and v1 clients get the same refs in the classic advertisement, and their `want` and `have` rounds are forwarded unchanged. `fetch` without `have` lines and without `filter` is looked up in the R2 clone cache under a key of the repository, the sorted wants, and every argument that changes the bytes; a hit is streamed only if every want is a current tip or a tip in the last 24 hours that no force push withdrew, so a removed secret cannot be fetched from cache. On a miss the DO grants one fill lease per key; the filler tees the engine's response to the client and to R2, and concurrent requests for the key wait up to 30 s for it, then go to the engine. Incremental fetches always go to the engine.

**Merge.** `PUT /repos/o/r/pulls/17/merge` reaches the Repo DO, which checks permission, required reviews at the head SHA, and required checks for the head SHA in its own tables, then asks the compute pool for `Merge { base, head }`. The pool's mirror fetches the missing objects from Artifacts, runs `git merge-tree --write-tree` and `git commit-tree` (or `git replay` for rebase), and returns a pack of the new objects, which goes through `stage`. `commit` then re-verifies that base, head, reviews, and checks are unchanged and moves the base ref, marks the PR merged, and writes the outbox rows in one transaction. A changed base returns 409 "Base branch was modified", as GitHub does.

**Graduation past the cap.** When `SizeBook` passes `GRADUATE_AT`, the `Graduate` workflow consolidates the pack log into one bitmap pack on a Large host (fetching anything missing from Artifacts), places it in two R2 buckets, sets `Tier::Graduating`, and then, under the refs lock for at most a few seconds, applies the last pushes and flips to `Tier::Large`. Pushes that arrive during the flip wait on the lock. A single push that would cross the cap, or that Artifacts refuses with a storage limit error, is already whole in the pack log, so the DO graduates the repository inline while the client waits, sending `remote:` progress on the side band, and then commits the push on the Large tier. The old Artifacts repository is kept read-only for 7 days, then deleted. Large networks above 8 GB of packs [guess: 20 GB container disk holds the packs and a repack's temporary copy] are refused at push with a message that names the limit.

**Journeys.** Managed example: org `acme` has home hint `enam`, so its Owner DO and Repo DOs sit in eastern North America; its pack log bucket has hint `wnam`; its Large-tier buckets have hints `enam` and `wnam`. Location hints are best effort (`research/cloudflare.md`, DO and R2 rows).

1. *Atomic three-branch push to busy `acme/mono` (600 MB, Artifacts tier) during 10 merges a minute, 50 bot ref moves a minute, and 2,000 CI clones a minute; then a 5 GB clone and 2 GB push on `acme/assets` (Large tier).* Cloudflare DNS answers with anycast addresses, the developer's TLS terminates in the nearest Cloudflare data center, and the WAF rate limiting rules run before the front Worker, which resolves the token and route from its isolate cache or a D1 replica, and re-reads the token on the D1 primary because a push is write-class. The Repo DO in `enam` admits the push after a strong Owner DO call; the Worker streams the pack to Artifacts and the `wnam` pack log; the DO commits all three refs in one transaction and returns `ok` for each. Durable at "done": the objects in Artifacts (synchronous multi-data-center replication) and in R2 `wnam`; the three refs, the intent, and three `push` outbox rows in the Repo DO (3 of 5 followers). The merges and bot moves serialize on the same refs lock, each holding it for one transaction, so the push waits at most a few milliseconds for them [inference]. After the acknowledgement the outbox drains through Queues to the webhook, `ActionsTrigger` (which creates Run DOs), and `PrSync` consumers. CI clones read refs from the DO (about 33 a second) and get packs from the `enam` clone cache in R2; each new tip costs Artifacts one fill, about 60 a minute, far below 2,000 requests per 10 s (`research/cloudflare-git.md#6`). The next fetch anywhere sees the three refs because `ls-refs` reads the committed `RefRow`. The 5 GB clone streams from the `acme/assets` host container through its DO and the Worker, with bitmaps and `filter` available; a stream that outlives a Workers runtime update by 20 minutes can be cut and git retries (`research/cloudflare-git.md#22`). The 2 GB push needs a managed Enterprise zone, whose body limit is up to 5 GB (`research/cloudflare-git.md#23`); the host places the pack in R2 `enam` and `wnam` before the commit, so "done" means two regions.
2. *Fork pull request merged while a review and a required check land; then `gh pr view` and `gh pr list` at once.* `bob/widgets` (Bob's hint `weur`) is an Artifacts fork of `acme/widgets`. When the PR opened and on every push to its head, the `PrSync` consumer piped a v2 `fetch` from the fork's Artifacts repository (wants the head, haves `acme/widgets` tips) into `stage` on `acme/widgets`, and its DO committed `refs/pull/17/head`; the compute pool built `refs/pull/17/merge`. The review (`POST .../reviews`) and the check run (`POST .../check-runs` from the CI app) are transactions in `acme/widgets`' Repo DO, so each lands before the merge's commit, and the merge's re-verification sees it, or after, as a review of a merged PR. Durable at the merge's 200: the merge commit's objects in Artifacts and the pack log, and the base ref, PR state, and outbox in the Repo DO. Both gh calls are GraphQL answered by the same Repo DO, so both show `MERGED` with the merge commit; author logins come from D1 replicas and can trail a rename by seconds.
3. *`gh issue create --label bug`, `gh issue list --label bug`, `gh api repos/o/r/issues?sort=updated`.* Create is one Repo DO transaction that allocates the number, writes the issue and label rows, and the outbox row. Both lists run SQL in the same DO, so the issue is in both, first by `updated`. `gh search issues` reads the owner's D1 FTS5 shard, fed by the `Search` consumer, and can miss the issue for a few seconds.
4. *An org owner removes a member and revokes a token; the member's next push over HTTPS and over SSH.* The removal commits in `acme`'s Owner DO and bumps `AuthzEpoch`; the 204 follows the commit. The revocation sets `revoked` on the token row in the D1 primary. Mira's HTTPS push with the revoked token fails at the Worker's D1 primary re-read with 401; with another token it reaches `admit`, whose Owner DO call returns no permission, and git prints GitHub's "Permission to acme/r.git denied to mira." The SSH path is Spectrum to a `cloudflared` replica in an SSH front container, which matches her key's fingerprint in D1 and forwards with a signed assertion to the same `admit`, so she gets the same denial. Bound for writes: zero after the 204. Bound for reads: 40 s, the 30 s isolate cache plus at most 10 s of D1 replica lag before lookups move to the primary.
5. *The data center, then the region, holding the newest writes is lost mid-push.* Data center: confirmed DO writes survive on their followers and the object restarts elsewhere after the followers fence the old host (`research/cloudflare-uptime.md#32`); Artifacts replicates synchronously across data centers. The in-flight push breaks the client's connection; its intent is `Admitted` or `Staged`, never `Committed`, so the sweep abandons it and git's retry succeeds because the old values still match. Region: no Cloudflare page says where a DO restarts or whether its followers span regions (`research/cloudflare-uptime.md`, open questions 1 and 2). Tributary waits 30 minutes for the platform, then an operator-signed `RepoRecover` workflow (never automatic) creates `repo:{id}:g2` with hint `wnam`, rebuilds metadata from the R2 journal (RPO: journal lag, seconds [inference]), rebuilds git from the pack log (RPO zero for every push whose pack log write succeeded), and replays newer pack log records in order when their old values still match. Recovered repositories serve from the Large tier because Artifacts imports only public HTTPS remotes (https://developers.cloudflare.com/artifacts/guides/import-repositories/); a `Reseed` workflow can push them back later. RTO: minutes per repository, hours for a region [guess], bounded by the default 1,500 container vCPU.
6. *A global outage like November 18, 2025, and a control-plane outage like November 2, 2023.* On November 18 the core proxy that every Worker request passes through returned 5xx for about 3 hours (`research/cloudflare-uptime.md#13`). Every Tributary request goes through it, and the SSH front reaches the Worker over HTTPS through it too, so a developer can commit locally and nothing else: no fetch, push, API, web, or Actions. Outbox rows and queued events wait and deliver after recovery. On November 2, 2023 the dashboard and API failed for about 40 hours while "traffic through our network was not impacted" (`research/cloudflare-uptime.md#2`). Tributary's runtime calls no Cloudflare API: Artifacts, D1, R2, Queues, and Workflows are reached through bindings, and Artifacts tokens are minted with the binding (https://developers.cloudflare.com/artifacts/api/workers-binding/), so git, the API, the web, merges, and Actions keep working. Releases, Terraform, DNS changes, new BYOC installs, and log delivery stop. Whether a sleeping container can start without the control plane is undocumented, so Large-tier hosts are kept warm while a control-plane incident is declared [inference].
7. *A release multiplies identity reads tenfold; another writes a record the previous release cannot read.* Identity reads are D1 directory lookups and Owner DO `permission` calls. Every release ships through Gradual Deployments at 1%, 10%, 50%, then 100% (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/), and a Cron Trigger compares D1 rows read per request and Owner DO calls per request between versions from Analytics Engine and rolls back above 2x [guess]. Each isolate also carries compiled budgets for directory misses and Owner DO calls; past them it serves only cached principals and answers others 503, so a bad 1% cohort cannot push an Owner DO past its 1,000 requests a second (`research/cloudflare.md`, DO row). A new record shape follows expand then contract: a release that reads it ships first, and writing it is enabled by a `formats` row in D1 only after the reader has run at 100% for a soak. This matters more here than in `DESIGN.md` because a gradual deployment pins each Durable Object to one version and resets it when its version changes (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/with-durable-objects/), so a rollback would otherwise put old code on objects holding new rows. Repair is roll forward, with DO point-in-time recovery for 30 days (`research/cloudflare.md`, DO row).
8. *BYOC and fully self-hosted.* BYOC gets everything managed has except the global namespace, in the customer's account and jurisdiction, with pushes limited by its zone plan (100 MB on Free and Pro, 200 MB on Business, 5 GB on Enterprise) and SSH only on Pro or above. Fully self-hosted on the customer's hardware is not possible: Artifacts is closed, and workerd has no D1 binding and only experimental local Durable Object storage (`research/cloudflare-git.md#38`, `#39`). That customer gets `tributary export`: bare repositories from the pack log and JSON lines from the journal, which it can load into GHES or into `DESIGN.md`'s design.

## Scale

| Resource | Limit (`research/cloudflare.md` unless noted) | This design at the busiest point | Headroom or plan |
| --- | --- | --- | --- |
| Artifacts git requests per repository | 2,000 per 10 s | Journey 1 monorepo: about 60 clone fills, 120 push requests, and 300 incremental fetches a minute, about 8 a second [guess] | 25x; above 50% the DO routes misses to the compute pool's mirror |
| Artifacts control requests per namespace | 2,000 per 10 s | Creates, forks, token mints (write tokens live 7 days and rotate at half life) | 256 namespaces per jurisdiction |
| Artifacts storage per repository | 1 GB (new repos) | Graduates at 768 MB | Large tier to 8 GB [guess] |
| Artifacts storage per account | 1 TB, raisable | Managed and the 2,000-developer install (2 TB of Artifacts-tier data at the cost model's 50% large share) exceed it | Raise on request, or one account per 1 TB as a managed shard |
| Repo DO requests | ~1,000 req/s soft | Monorepo: 33 `ls-refs` a second plus API reads | Repositories with more than 10,000 refs publish each ref snapshot to R2 keyed by `seq`, so `ls-refs` costs the DO one small call |
| Repo DO storage | 10 GB | Largest GitHub-sized issue histories, about 2 GB [guess]; bodies over 64 KiB go to R2 | 5x |
| Owner DO requests | ~1,000 req/s soft | One call per write in the org; reads cached 30 s per isolate | An org above 300 writes a second gets team and grant facts mirrored into its Repo DOs' read caches [inference] |
| D1 database | 10 GB, single-threaded | Directory at managed scale: 150 million accounts at about 1 KB [guess] is 150 GB | 16 shards at launch, more by splitting the hash range |
| D1 per account | 1 TB, 50,000 databases | Directory plus per-owner search shards | Search shards move to a second account first |
| Workers request body | 100 MB Free/Pro, 200 MB Business, up to 5 GB Enterprise | Large pushes | SSH path stages to R2 through the S3 API, which has no Worker body limit |
| Workers memory | 128 MB per isolate | Pack streams never buffer beyond one 8 MiB multipart part | Fixed by design; no repository size changes it |
| Queues | 5,000 msg/s per queue | 16 event queues sharded by repository id | 80,000 msg/s per account before adding queues |
| Workflows | 300 instance creations per second per account | Graduations, consolidations, recoveries only, never per push | A region recovery of 100,000 repositories starts in under 6 minutes |
| Containers | 1,500 vCPU per account, 20 GB disk per instance | Large hosts (standard-4) and compute pool | Region recovery is bounded by this; raise before launch |
| R2 | 1 write/s per key | Every key is a ULID or content digest | No key is written twice |

Per-repository write throughput is bounded by the refs lock: one SQLite transaction plus, for protected refs, a `readCommit` walk over the new commits. At 5 ms a commit [guess] that is about 200 ref commits a second per repository, before batching. Milestone 0 measures it.

## Downtime

| Incident | Evidence | This design | Effect |
| --- | --- | --- | --- |
| Cross-region primary promotion with unreplicated writes (GitHub, October 2018) | `research/uptime.md#40` | No automatic promotion exists. Region recovery is an operator-signed workflow that creates a new DO generation from the R2 journal and pack log, and the old generation's id is retired in D1 first | removed |
| Database engine version bug on a custom-configured cluster | `research/uptime.md#44` | Tributary runs no database server, but Cloudflare upgrades the SQLite engine under every DO and D1 at its own pace; a regression there hits every repository | exposed |
| Expensive writes on a shared permissions cluster | `research/uptime.md#44` | Permissions live in one Owner DO per org, so one org's write storm stays in its object | contained |
| Partial disk failure that failover missed | `research/uptime.md#44` | Disk and failover belong to Cloudflare's DO and Artifacts replication; Tributary sees only latency and errors, and admission sheds per object | mitigated |
| Misconfigured index or online schema migration at peak | `research/uptime.md#44` | Each Repo DO migrates its own small SQLite on first access after a release, so a migration touches one repository at a time and only in the gradual cohort | contained |
| Dropped column still read by the previous release | `research/uptime.md#44` | Expand then contract with the `formats` row; contraction waits for 100% plus soak | contained |
| Major-version datastore upgrade under load | `research/uptime.md#44` | No Tributary-run datastore to upgrade; Cloudflare's own upgrades are outside Tributary's control | mitigated |
| 32-bit key overflow blocking new review threads | `research/uptime.md#44` | Ids are 53-bit block-leased integers; per-repository numbers are 32-bit and per repository | removed |
| Cleanup job throttled on the wrong signal | `research/uptime.md#44` | GC and consolidation run as Workflows per repository with a fixed step rate, and touch only the repository they run for | contained |
| Config or deploy fault pushed to every pod at once | `research/uptime.md#43`, `#45` | Gradual Deployments with an automatic 2x error and identity-read gate; each DO is pinned to one version per deployment | contained |
| Expired internal TLS certificate failing all git | `research/uptime.md#45` | Edge certificates are Cloudflare-managed; the only Tributary-held credentials are Artifacts tokens, rotated at half life with a 7-day TTL and an alarm at 2 days | mitigated |
| Auth service memory leak crashing pods under load | `research/uptime.md#45` | Workers isolates are capped at 128 MB and recycled by the runtime, so a leak costs requests in one isolate, and the release gate catches the error rate | mitigated |
| Load balancer or proxy change that broke allow lists or sent auth to a wrong host | `research/uptime.md#45` | Tributary owns no load balancer or proxy config; every binding and hostname is in the signed release and ships through the gradual gate | contained |
| Network config deployed to the wrong environment | `research/uptime.md#45` | Staging and production are separate Cloudflare accounts, and the deploy tool takes the account id only from the signed target | contained |
| Automated review suspending a service account | `research/uptime.md#45` | The runtime uses bindings, not API tokens; the deploy token is separate and only deploys | removed |
| Internal DNS failure cascading into git | `research/uptime.md#46` | Workers reach DOs, D1, R2, Queues, and Artifacts through bindings and Cloudflare-internal hostnames; Tributary runs no DNS of its own | mitigated |
| Cache TTL cut plus tenfold reads overloading auth (February 2026) | `research/uptime.md#47` | Directory reads go to D1 replicas, writes re-read the primary, isolate budgets cap misses, and the release gate compares identity reads per request | contained |
| Automatic Redis failover with no writable primary | `research/uptime.md#47` | No cache or queue with primary failover in Tributary's design; Queues and DOs fail over inside Cloudflare | mitigated |
| Load-balancing tier failed to scale; client retry loop (August 2026) | `research/uptime.md#48` | No load balancer to scale; WAF rate limiting rules and per-isolate retry budgets shed misbehaving clients | mitigated |
| Merge queue group merges that reverted earlier changes | `research/uptime.md#49` | `parent_tip` chaining and the fast-forward check in the commit transaction | contained |
| Search cluster as a single point of failure | `research/uptime.md#49` | Lists are SQL in Repo DOs; search is a separate consumer whose outage empties only the search box | removed |
| Hardware or power loss in one data center | `research/uptime.md#43` | DO followers in 5 data centers, Artifacts synchronous replication | mitigated |
| 1.1.1.1 lookup failures, October 2023 | `research/cloudflare-uptime.md#1` | No Tributary path resolves through 1.1.1.1 [inference] | removed |
| Control plane and analytics, November 2023 | `research/cloudflare-uptime.md#2` | Runtime calls no Cloudflare API; deploys and log delivery stop (journey 6) | contained |
| PDX01 power loss, March 2024 | `research/cloudflare-uptime.md#3` | Same as November 2023, seven minutes | contained |
| CDN errors from a DDoS rule, June 2024 | `research/cloudflare-uptime.md#4` | Every Tributary request passes the same proxy | exposed |
| 1.1.1.1 BGP hijack, June 2024 | `research/cloudflare-uptime.md#5` | Not on any Tributary path [inference] | removed |
| Logs lost, November 2024 | `research/cloudflare-uptime.md#6` | The audit log comes from DO outboxes through Queues and Basin Pipelines, not Logpush; only operator logs are lost | contained |
| R2 disabled by an abuse action, February 2025 | `research/cloudflare-uptime.md#7` | Artifacts-tier pushes commit as `EngineOnly`; fetches skip the clone cache; Large-tier pushes and cold hosts fail; LFS, releases, and Actions storage fail | mitigated |
| R2 credentials misdeployed, March 2025 | `research/cloudflare-uptime.md#8` | Same as February 2025 | mitigated |
| Third-party storage under KV, DO, D1, Queues, June 2025 | `research/cloudflare-uptime.md#9` | DO and D1 errors at 22%, Queues down, and Artifacts keeps its auth tokens in KV (`research/cloudflare-git.md#4`), so git and writes fail broadly | exposed |
| 1.1.1.1 withdrawal, July 2025 | `research/cloudflare-uptime.md#10` | Not on any Tributary path [inference] | removed |
| Single customer overloading an interconnect, August 2025 | `research/cloudflare-uptime.md#11` | Tributary uses no external cloud interconnect | removed |
| Dashboard and API, September 2025 | `research/cloudflare-uptime.md#12` | As November 2023 | contained |
| Global proxy panic from a feature file, November 18, 2025 | `research/cloudflare-uptime.md#13` | Everything down for about 3 hours (journey 6) | exposed |
| FL1 proxy, 28% of traffic, December 2025 | `research/cloudflare-uptime.md#14` | Requests that land on the affected proxy fail | exposed |
| 1.1.1.1 record order, January 2026 | `research/cloudflare-uptime.md#15` | Not on any Tributary path [inference] | removed |
| Miami route leak, January 2026 | `research/cloudflare-uptime.md#16` | Users routed through the leak fail; others reach other data centers | mitigated |
| BYOIP withdrawal, February 2026 | `research/cloudflare-uptime.md#17` | Managed and BYOC use Cloudflare-owned addresses | removed |
| Regional DO and R2 error bursts, October 2026 status sample | `research/cloudflare-uptime.md#20` | Repositories whose DOs live in the affected region fail; others are untouched | contained |

RPO and RTO per durability class. "Region" means the region holding the DO or Artifacts copy is lost for good.

| Class | Data center loss | Region loss | Logical corruption |
| --- | --- | --- | --- |
| Git objects, Artifacts tier | RPO 0, RTO Cloudflare's (undocumented) | RPO 0 when the pack log write succeeded, else consolidation lag (minutes); RTO minutes per repository via `RepoRecover` | Pack log objects are write-once; refs restore from the journal or DO point-in-time recovery |
| Git objects, Large tier | RPO 0, RTO a container start plus rehydration (Milestone 0) | RPO 0 (two buckets before commit); RTO as above | Cruft packs kept 14 days, superseded packs 35 days |
| Refs, PRs, issues, checks (Repo DO) | RPO 0 (3 of 5 followers), RTO undocumented | RPO = journal lag (seconds) [inference]; refs of committed pushes are restored from pack log records | DO point-in-time recovery, 30 days |
| Memberships, teams, names (Owner DO) | RPO 0 | RPO = journal lag; removals replayed before grants | DO point-in-time recovery, 30 days |
| Directory (D1) | RPO 0 [inference: D1 is built on DO storage] | RPO = 1 minute, a Cron Trigger drains each shard's `journal` table to R2 | D1 Time Travel, 30 days, 10 restores per 10 minutes |
| Actions logs, cache, artifacts, LFS, releases (R2) | RPO 0 | Lost with the bucket's region; Tributary does not copy them across regions | Deletes wait 7 days behind a tombstone row before the object is removed |

SLOs per month, measured at the Worker and by external probes. They sit below `DESIGN.md`'s because Durable Objects, D1, and Artifacts carry no SLA (`research/cloudflare-uptime.md#29`) and Cloudflare's global incidents in 2025 alone exceed a 99.99% budget [inference from findings 9 and 13].

| Journey | Target | Error budget (min/month) |
| --- | --- | --- |
| git fetch and clone | 99.95% | 21.6 |
| git push acknowledged; PR merge; REST and GraphQL writes | 99.9% | 43.2 |
| REST and GraphQL reads; web page load | 99.95% | 21.6 |
| Actions job start within 5 minutes; webhook first attempt within 60 s | 99.9% | 43.2 |
| Acknowledged push durability | 100% | 0, checked by a daily restore of a sample of repositories from the pack log alone, with `git fsck` and a ref comparison |

## Failure modes

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| Artifacts repository | instance | One repository's git requests while Cloudflare moves it | Synchronous multi-data-center replication (`research/cloudflare.md`, Artifacts); clone cache still serves CI hits; pushes retry |
| Artifacts repository | data-center | Repositories whose instance was there, for the restart | Same as instance |
| Artifacts repository | region | Repositories placed there, possibly for good | `RepoRecover` from the pack log onto the Large tier, operator-signed after 30 minutes |
| Artifacts repository | provider-wide | Every Artifacts-tier repository's pushes and cache misses | Clone cache serves CI hits; the compute pool's mirrors serve fetches for warm repositories [inference]; a sustained product failure triggers bulk graduation from the pack log |
| Artifacts repository | control-plane | None at runtime; repository creation by REST is not used | Creates, forks, and tokens go through the binding |
| Artifacts repository | bad-deploy | A Cloudflare engine release that breaks a git feature, for every repository | The nightly conformance suite runs unmodified git against a canary namespace; the pack log keeps an independent copy |
| Artifacts repository | bad-config | A namespace created with the wrong jurisdiction | Jurisdiction is chosen by the install plan and checked by preflight; it cannot change later (`research/cloudflare-git.md#41`) |
| Artifacts repository | noisy-tenant | One repository near 2,000 requests per 10 s | Clone cache, per-repository admission in the DO, and overflow of misses to the compute mirror |
| Repo DO | instance | One repository | Restart after follower fencing; Workers retry idempotent calls once; intents make pushes idempotent |
| Repo DO | data-center | Repositories whose DOs were there | Followers in 5 data centers (`research/cloudflare-uptime.md#32`) |
| Repo DO | region | Repositories homed in the region | New generation from the journal and pack log (journey 5) |
| Repo DO | provider-wide | Every repository (June 2025 class) | None inside Cloudflare; outbox and intents resume afterwards |
| Repo DO | control-plane | None; DOs run without the API | Deploys wait |
| Repo DO | bad-deploy | The gradual cohort's repositories | Version pinning per DO, the release gate, `formats` row, N-1 RPC compatibility both ways |
| Repo DO | bad-config | A wrong rule or setting in one repository | Settings are DO rows with an audit trail; PITR for 30 days |
| Repo DO | noisy-tenant | One hot repository | ~1,000 req/s soft limit per object is the wall; ref snapshots in R2, cached object reads, and WAF limits keep traffic off it |
| Owner DO | instance | One org's writes and uncached reads | Restart after fencing; cached allows serve reads for 30 s |
| Owner DO | data-center | Orgs homed there | Followers in 5 data centers |
| Owner DO | region | Orgs homed there, and writes to all their repositories | New generation from the journal; removals replay before grants |
| Owner DO | provider-wide | Every org | As Repo DO |
| Owner DO | control-plane | None | As Repo DO |
| Owner DO | bad-deploy | Orgs in the cohort | As Repo DO |
| Owner DO | bad-config | One org's policy | Audit rows and PITR |
| Owner DO | noisy-tenant | One huge org | Isolate budgets, per-org write admission, mirrored grant facts above 300 writes a second |
| Run DO | instance | One run's job dispatch | Leases expire and jobs re-queue; runners re-poll |
| Run DO | data-center | Runs homed there | Followers in 5 data centers |
| Run DO | region | Runs homed there | Runs are re-triggered from the Repo DO outbox by `ActionsTrigger` after recovery |
| Run DO | provider-wide | All Actions | None; runs resume |
| Run DO | control-plane | New hosted runner containers may not start [inference] | Self-hosted runners unaffected; warm hosted pool kept |
| Run DO | bad-deploy | Cohort's runs | Version pinning; runner protocol pinned per release (`DESIGN.md`, "Actions") |
| Run DO | bad-config | One workflow's misconfiguration | Validated by the parser at trigger time |
| Run DO | noisy-tenant | One repository's matrix explosion | Per-owner concurrency caps in the Pool DO |
| Directory D1 | instance | One shard's primary writes for a restart | Reads served by replicas and isolate caches |
| Directory D1 | data-center | One shard's primary | D1 storage replication [inference] |
| Directory D1 | region | Shards whose primary was there: signups, token creation, write-class token re-reads for 1/16 of keys | Replicas keep reads; recovery restores the shard from Time Travel or the R2 journal into a new database |
| Directory D1 | provider-wide | All token checks after isolate caches expire | Positive cache entries are honored for reads up to 10 minutes during a declared D1 outage, writes fail closed |
| Directory D1 | control-plane | None at runtime | Shard splits wait |
| Directory D1 | bad-deploy | Queries from the cohort | Release gate on rows read per request |
| Directory D1 | bad-config | A wrong shard map | The shard map is compiled into the release and checked against a manifest row at startup |
| Directory D1 | noisy-tenant | Token-guessing traffic | WAF rate limiting before the Worker; negative cache 5 s |
| R2 buckets | instance | None visible | R2 replicates within the region (`research/cloudflare.md`, R2) |
| R2 buckets | data-center | None visible | Same |
| R2 buckets | region | Pack log copies of repositories whose engine survives; one of two Large-tier copies | The engine copy remains; consolidation rewrites the pack log into another region's bucket |
| R2 buckets | provider-wide | Pack log, clone cache, Large tier, LFS, Actions storage (February and March 2025 class) | `EngineOnly` commits for the Artifacts tier; warm Large hosts serve fetches from local disk |
| R2 buckets | control-plane | None; bindings and presigned URLs keep working | Nothing to contain; bucket and lifecycle changes wait |
| R2 buckets | bad-deploy | A release that writes wrong keys | Keys are ULIDs or digests with `If-None-Match`; nothing overwrites |
| R2 buckets | bad-config | A lifecycle rule deleting live data | Lifecycle rules are in the signed release and preflight-checked; pack log objects are referenced by DO rows before any delete |
| R2 buckets | noisy-tenant | A hot clone cache object | Isolate cache in front [inference: Workers Cache API]; per-key concurrency measured at Milestone 0 |
| Git containers | instance | One Large network or one compute shard | DO restarts the container; Large hosts rehydrate from R2; host restarts happen at any time (`research/cloudflare-git.md#17`) |
| Git containers | data-center | Containers there | Placement can move on restart (`research/cloudflare.md`, Containers) |
| Git containers | region | Large networks hosted there | Rehydrate in another region from the surviving bucket |
| Git containers | provider-wide | Large tier, merges, diffs, blame, partial clone, hosted runners | Artifacts-tier pushes and fetches continue; merges return 503 |
| Git containers | control-plane | Cold starts may fail [inference] | Keep hosts warm during declared incidents |
| Git containers | bad-deploy | Image with a broken git | Image pins a git version; the conformance suite gates the image |
| Git containers | bad-config | Wrong instance size | Instance types are in the release, checked by preflight against account limits |
| Git containers | noisy-tenant | One network's repack | One network per host process group; repack scheduled by Workflow off peak |
| Event queues | instance | Delayed events | At-least-once delivery; consumers are idempotent by outbox seq |
| Event queues | data-center | Delayed events | Same |
| Event queues | region | Delayed events | Outbox rows stay `enqueued = false` until acknowledged and are re-sent |
| Event queues | provider-wide | Webhooks, Actions triggers, PR head sync stop (June 2025) | Outboxes hold events; delivery resumes in order per repository |
| Event queues | control-plane | None | Nothing to contain; new queue creation waits |
| Event queues | bad-deploy | A consumer that crashes | Messages retry up to 100 times then go to a dead-letter queue (`research/cloudflare.md`, Queues) |
| Event queues | bad-config | A queue bound to the wrong consumer | Bindings are in the signed release |
| Event queues | noisy-tenant | One repository's event storm | 16 queues by repository id; per-repository outbox drain rate |

## Deployment modes

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Artifacts repository | Vendor accounts, one per 1 TB of Artifacts data, 256 namespaces per jurisdiction | Customer account, `us`, `eu`, or unrestricted namespaces | Customer account, customer-operated; not available on customer hardware |
| Repo DO | Vendor accounts, hint from the owner's home | Customer account, jurisdiction from the plan | Same as BYOC, customer-operated |
| Owner DO | Vendor accounts | Customer account | Same as BYOC |
| Run DO | Vendor accounts; hosted runners in vendor containers | Customer account; hosted runners optional, self-hosted runners common | Same as BYOC |
| Directory D1 | 16 shards, one global namespace | One shard, the install's own namespace | One shard |
| R2 buckets | Pack log, packs, journal in two regions per jurisdiction | Same in the customer account; EU installs use two `eu` buckets with different hints | Same |
| Git containers | Vendor accounts, raised vCPU limit | Customer account, default 1,500 vCPU | Same |
| Event queues | 16 queues per account | 16 queues | 16 queues |

What each mode requires of the customer. Managed: nothing. BYOC: a Cloudflare account on Workers Paid with Artifacts and Containers enabled, a zone on Pro or above for SSH, Business for 200 MB pushes or Enterprise for 5 GB, an account-owned API token for the vendor, and acceptance that Artifacts, Secrets Store, and Email Sending are beta. Self-hosted: the same account, plus the customer runs `tributary install apply` and upgrades from its own CI, with the vendor's signed bundle. The FRAME's reading of self-hosted as "the customer's own infrastructure" cannot be met with Cloudflare products: Artifacts has no self-hosted form and workerd lacks D1 and replicated Durable Object storage (`research/cloudflare-git.md#38`, `#39`). The vendor's release registry, signing, and wave planning reuse the shapes in `research/byoc-grounding.md` (implication 1) with R2 in place of the registry bucket and an offline Ed25519 key in place of the cloud key service, because Cloudflare has no customer-managed signing key product in the research table.

## Cost

Monthly list prices for one company's install, from `python3 arena/cloudflare/candidate-2/cost.py` with `TIERS` and `ACTIVITY` from `scripts/cost_model.py` and `cf.*` prices from `research/prices.json`. Every coefficient of this design's request shape in the script is a guess. Excluded, as in `research/costs.md`: vendor fee, CI runner compute, support, tax. Enterprise is contract-priced, so the Business plan is priced.

| Developers | Monthly list cost | Per developer | Largest line |
| --- | --- | --- | --- |
| 50 | $438 | $9 | Zone plan, Business ($250) |
| 300 | $1,647 | $5 | Large-tier hosts, 19 x standard-4 warm 220 h a month ($653) |
| 2,000 | $14,911 | $7 | Large-tier hosts, 250 x standard-4 warm 220 h a month ($8,593) |

At 2,000 developers the rest is Spectrum SSH data ($1,563, at $1/GB with half of developer bytes over SSH), Artifacts storage ($1,100 at $0.50/GB-month), Artifacts operations ($831), R2 ($771 with operations), the compute pool ($365), and logs ($318). The 20 GB container disk is what makes the Large tier expensive: the cost model's 2 TB of repositories above 1 GiB need 250 hosts.

| Case (`--sensitivity`) | 50 | 300 | 2,000 |
| --- | --- | --- | --- |
| As designed | $438 | $1,647 | $14,911 |
| Own engine for every repository: no Artifacts, all in the Large tier | $480 | $2,136 | $22,091 |
| Artifacts holds every repository (a 10 GB cap), no Large tier | $426 | $1,190 | $7,671 |
| Clone cache hit rate 50% instead of 80% | $440 | $1,686 | $15,388 |
| SSH carries 20% of developer bytes instead of 50% | $415 | $1,505 | $13,967 |
| Large hosts warm all month instead of 220 h | $598 | $3,161 | $34,832 |

Cheapest reliable setup for 50 developers (`--cheapest`). Every row keeps the pack log, the two-bucket Large tier, and the SLOs above.

| Config | Configuration | Per month | Per developer |
| --- | --- | --- | --- |
| `as-designed` | Business zone, SSH, pack log, one compute container | $438 | $9 |
| `pro` | Pro zone: SSH with 5 GB included, 100 MB HTTPS push limit | $218 | $4 |
| `https-only` | Free zone, no SSH, 100 MB HTTPS push limit | $142 | $3 |

`pro` is the recommendation: $218 a month against $226 for `DESIGN.md`'s `scrappy` profile (`research/costs.md`), with no fixed load balancer, NAT, or key charges, and two regions of git durability instead of a cold recovery region.

## Build order from zero

**Milestone 0: measurements that would make this design wrong.** Each runs against a real account before any product code. A fail changes the named decision.

| Measurement | Pass | Fail, and what changes |
| --- | --- | --- |
| Artifacts accepts stage-then-apply: create staging refs with a pack, then an atomic empty-pack receive-pack that moves 3 real refs and deletes the staging refs | 1,000 of 1,000 runs correct, `atomic` advertised | Any wrong result: the Artifacts tier drops staging and runs rules from a pack parser in the Worker, or shrinks to read-mostly repositories |
| Push latency, 1 MB pack, admit to `ok`, same region | p99 below 800 ms | p99 above 2 s: pushes go to the Large tier and Artifacts holds only forks and small repositories |
| Fetch throughput from Artifacts, 1 GB full clone | Below 60 s | Above 180 s: graduation threshold drops to 256 MB |
| Sustained git requests on one Artifacts repository | 150 a second for 10 minutes with no `rateLimited` events | `rateLimited` below 100 a second: busy repositories graduate by request rate as well as size |
| Fork storage billing, 500 MB repository forked 10 times | Billed storage grows under 500 MB | Grows by 5 GB: forks are created lazily on first push and read from the upstream repository until then |
| Artifacts `fetch` arguments: `shallow`, `deepen`, `include-tag`, `thin-pack`, `ofs-delta`, `wait-for-done` | Each either works or is rejected cleanly | Silent wrong packs: every fetch with that argument routes to the compute pool |
| Repo DO commit, same-region Owner DO call | p99 below 20 ms and 10 ms | Above 100 ms: permission facts cache in the Repo DO with epoch push from the Owner DO |
| SSH through Spectrum to `cloudflared` in a container, through a container restart | Clone and push succeed with at most one retried connection | Fails: SSH is not offered in Cloudflare-only modes, a stated deviation |
| Large host cold start plus 3 GB rehydration from R2 | Below 60 s | Above 180 s: Large hosts stay warm all month (the `--sensitivity` row) |
| One R2 object read by 100 concurrent clones of 500 MB | Each above 20 MB/s | Below 5 MB/s: the clone cache fans out into per-region copies |

**Milestone 1: Artifacts-tier git.** Front Worker, Directory D1, Repo DO refs and intents, `stage`, `commit`, `apply`, pack log, clone cache. Gate: `git clone`, `git push --atomic`, `git fetch --filter=blob:none`, and `GIT_TRACE_PACKET` advertisement checks with git 2.39 and 2.51 (`DESIGN.md`, "Wire compatibility", `git-http` and `git-protocol-v2` rows); a linearizability harness with 50 concurrent pushers on one repository finds no lost or reordered ref; a restore from the pack log alone matches every ref.

**Milestone 2: pull requests, issues, merges.** Repo DO tables, compute pool, `PrSync`. Gate: journeys 2 and 3 with unmodified `gh`; the Octokit contract runs for issues, pulls, and checks.

**Milestone 3: orgs and authorization.** Owner DO, teams, tokens, revocation. Gate: journey 4 over HTTPS; revocation probes every 5 minutes for a week show zero writes accepted after a 204.

**Milestone 4: events and Actions.** Queues, webhooks, Run DO, Pool DO, runner protocol, hosted runners on Containers. Gate: the pinned `actions/runner` runs a matrix workflow with cache and artifacts.

**Milestone 5: Large tier and SSH.** Large hosts, graduation, SSH fronts. Gate: journey 1 in full, including a push that crosses the cap and graduates inline; journey 4 over SSH.

**Milestone 6: modes and recovery.** BYOC and self-hosted install, upgrade, and `RepoRecover`. Gate: a fresh account installs from the bundle; a recovery drill rebuilds 100 repositories into a new generation from R2 alone.

**Milestone 7: breadth.** Remaining REST and GraphQL surfaces by `DESIGN.md`'s tiers ("Wire compatibility"), LFS, releases, search.

## Compared with the AWS design

| Matters to | AWS design (`DESIGN.md`) | This design | Evidence |
| --- | --- | --- | --- |
| Users: push durability across regions | RPO 0 in Paired cells (MRSC put) | RPO 0 for git objects while R2 is healthy; refs and metadata RPO seconds | `DESIGN.md` Summary; this design's RPO table |
| Users: global outage exposure | Regional failures stay regional; AWS's worst event stayed in us-east-1 | Every Cloudflare global proxy or storage incident is a full outage | `research/cloudflare-uptime.md` implication 1 |
| Users: SLO | 99.99% fetch and reads, 99.95% writes | 99.95% fetch and reads, 99.9% writes | `DESIGN.md` "SLOs"; `research/cloudflare-uptime.md#29` |
| Users: latency | Cell-region round trips | TLS and auth at the nearest data center; writes cross to the repository's region | [inference] |
| Users: push size over HTTPS | Not limited by a proxy | 100 MB to 5 GB by zone plan; SSH unlimited | `research/cloudflare-git.md#23` |
| Users: revocation | Writes 5 s, reads 60 s | Writes 0 s after the 204, reads 40 s | Journey 4 |
| Users: partial clone | Everywhere | Everywhere, through the compute pool for Artifacts-tier repositories | Shape invariants |
| Maintainers: code to own | Fold engine, chains, mirrors, heartbeats, front cells, operator role, node fleet | Front Worker, four DO classes, the Large-tier host, the compute pool, SSH front | Shape |
| Maintainers: storage engine | Upstream git on NVMe nodes for every repository | Closed Artifacts engine for most repositories; upstream git only for the Large tier and compute | `research/cloudflare-git.md#14` |
| Maintainers: deploys | Node rollouts, AMIs, the operator role's waves | Gradual Deployments with DO version pinning | Journey 7 |
| Operator: cost at 50 / 300 / 2,000 developers | $1,287 / $2,573 / $12,432 one Region | $438 / $1,647 / $14,911 (`pro` 50-developer setup $218) | `research/costs.md`; Cost |
| Operator: modes | Managed, BYOC, self-hosted in any of 34 Regions | Managed, BYOC; self-hosted only as customer-operated Cloudflare; residency `us` or `eu` only | `research/cloudflare-git.md#39` to `#42` |
| Operator: vendor risk | GA services with SLAs | Artifacts, Secrets Store, Email Sending, D1 read replication in beta | `research/cloudflare.md` statuses |

## Tradeoffs accepted

- We accept a closed beta engine holding most repositories in exchange for writing no git server for them, and we pay for that with a pack log in R2 that duplicates every write and can rebuild any repository without Artifacts.
- We accept two git engines behind one interface. The Large tier is upstream git in Containers, confined to repositories over 768 MB, merges, diffs, blame, and partial clone, because Artifacts has a 1 GB cap and no `filter` (`research/cloudflare-git.md#5`, `#6`).
- We accept a push that costs Artifacts two receive-packs (stage, then a batched apply) and one R2 upload, in exchange for rules and ref commits that Tributary decides and an acknowledgement that never depends on Artifacts' ref update.
- We accept cross-region RPO of seconds for refs and metadata, because Cloudflare offers no synchronous cross-region store (`research/cloudflare-uptime.md#40`) and writing a second DO synchronously would put a second object's failure on every write.
- We accept that a pack log write that fails fast does not block the push, so R2's two 2025 outages would not have stopped pushes, at the price of minutes of single-copy git data during such an outage.
- We accept every Cloudflare-wide incident as a full Tributary outage, with no second provider, as the operator asked for Cloudflare only.
- We accept no automatic region failover, so a lost region costs the time an operator takes to sign `RepoRecover` plus the rebuild, in exchange for never running two generations of a repository at once.
- We accept HTTPS push limits from the zone plan and SSH that depends on an unverified Spectrum-to-Tunnel-to-container path.
- We accept a Large tier capped at about 8 GB per network by the 20 GB container disk, and its cost, which is most of the 2,000-developer bill.
- We accept no fully self-hosted mode.
- We accept reads that honor a cached allow or token for up to 40 s after a removal or revocation, in exchange for no directory or Owner DO call on most reads.
- We accept these wire deviations beyond `DESIGN.md`'s "Known deviations from GitHub": HTTPS pushes limited by the zone plan, SSH only where Milestone 0's Spectrum test passes, reads revoked within 40 s, and `refs/pull/N/head` for fork pull requests a few seconds after the fork push.

## Alternatives considered

- **Run our own engine for every repository** (Repo DO refs, R2 packs, upstream git in Containers for all; `DESIGN.md`'s storage model moved onto Cloudflare). It removes the beta dependency and the two-engine split. It lost on cost and code: every repository needs a container with provisioned memory and a 20 GB disk to serve it, which costs $22,091 a month at 2,000 developers against $14,911 (the `--sensitivity` row), and it is the code this direction exists to avoid. The pack log keeps it available as the exit.
- **Let clients push straight to Artifacts and react to its events.** Artifacts emits `pushed` events (https://developers.cloudflare.com/artifacts/guides/event-subscriptions/), but they arrive after the ref moved, so branch protection, required reviews, and atomic rejection could only be undone, not prevented, and push events need one subscription per repository.
- **A WASM git server in each Repo DO** (dgit, git-on-cloudflare). It stores client deltas, computes none, has no bitmaps, and fits 128 MB and 5 minutes of CPU (`research/cloudflare-git.md#9`, `#14`, `#15`). Artifacts is the same idea, run by Cloudflare.
- **Split a large repository across several Artifacts repositories and stitch the packs.** OFS deltas are relative, so packs concatenate, but thin pushes would miss bases held by the other part, and negotiation across parts is new protocol code.
- **D1 for per-repository metadata.** 50,000 databases per account cannot hold one per repository, and a shared database brings back GitHub's shared-cluster incidents (`research/uptime.md#44`).
- **Workers KV as the token cache.** Up to 60 s stale, and its storage outage in June 2025 took DOs and D1 down too (`research/cloudflare-uptime.md#9`).
- **One Workflow instance per Actions run.** 300 creations a second per account caps triggers, and job leases need sub-second state that Workflow steps price per step.

## Open questions and risks

- Artifacts is beta, closed, and changed its per-repository cap from 10 GB to 1 GB a month ago (Artifacts changelog, 2026-09-29). A further cut lowers `GRADUATE_AT` and grows the Large tier and its cost; a raise for this account shrinks both.
- Artifacts' behavior beyond its docs (atomic, empty-pack updates, thin packs, wants reachable only from staging refs, memory limits on large pushes, `MEMORY_LIMIT` errors) is unmeasured (`research/cloudflare-git.md`, could not verify). Milestone 0 decides.
- What counts as a billed Artifacts operation (binding reads, `ls-refs`) is unstated on the pricing page; the cost script assumes every git request and binding read counts.
- Where an Artifacts repository is placed, and whether it sits near the Repo DO that created it, is undocumented; the pack log bucket's region is chosen against the DO's hint, not Artifacts'.
- Durable Object behavior on region loss (restart location, RTO, whether followers cross regions) is undocumented (`research/cloudflare-uptime.md`, open questions 1 and 2).
- Artifacts keeps its auth tokens in KV (`research/cloudflare-git.md#4`), so a KV outage can stop git even though Tributary avoids KV.
- The SSH path through Spectrum, Tunnel, and a container is unverified end to end (`research/cloudflare.md`, could not verify).
- Whether bytes a container returns through its Durable Object count as container egress is unstated; the cost script assumes they do.
- Managed scale needs Artifacts' 1 TB account cap raised by orders of magnitude, or many vendor accounts, each with its own Containers, D1, and Queues limits.
- D1 read replication, Secrets Store, and Email Sending are beta; a regression in D1 replicas falls back to the primary, which is one location per shard.
- Rust on Workers through workers-rs, including Durable Objects in Rust, is assumed workable for a service this size [inference].
