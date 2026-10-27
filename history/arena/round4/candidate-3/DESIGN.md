# Every commit is one conditional put: per-aggregate chains in DynamoDB, bytes in S3, folds on NVMe

## Summary

The bet: every acknowledged change to a repository, a fork network, or an owner is exactly one DynamoDB conditional put of the next sequence number of that aggregate's chain, made only after every byte it names is already in S3. Every read is a node-local fold of chains, proven fresh by one strongly consistent tail read that started after the request arrived. No database transaction, SQL server, connection pool, outbox table, change-log protocol, request-path lease, or clock bound sits under git, pull requests, issues, or authorization.

Stores. DynamoDB holds the chains and small mutable items: MRSC tables (two replica Regions plus a witness) in Paired cells, one worldwide MRSC `global` table across three continents in managed, and plain single-Region tables in single-Region installs, in any of 34 Regions. S3 holds packs, spilled entries, fold snapshots, the chain archive, and control. SQLite files on node NVMe are derived per-aggregate query indexes. OpenSearch serves only `/search`. Aurora DSQL, Aurora PostgreSQL, MemoryDB, and Kinesis are rejected ("Alternatives considered").

What a GitHub user notices against `DESIGN.md`: the wire is unchanged. Repository-scoped lists (`gh issue list`, `gh pr list`, `gh pr view`) read their own writes in any Region. Atomic pushes go to 10,000 refs, not 1,000. A removed member's next request fails at once, not after a coverage wait. Token revocation takes up to 60 s, not 5 s. There is no 2-hour stale-fetch mode: a cell that cannot reach a DynamoDB quorum stops serving that cell. In managed, signup, login renames, and token creation wait one cross-continent round trip, and no continent's loss loses or stalls them.

What a maintainer notices: no DSQL clusters, `core` and `aux` split, connection budgets, geo authorities, claim sagas, freshness boards, writer leases, ClockBound, escrow, `reconstitute`, outbox pending rows, or inter-cell logs. One commit primitive (`commit`), one freshness primitive (`fresh`), one event path (chain, stream trigger, per-consumer cursor). The new build risk is the fold engine (SQLite per aggregate, snapshots, refold), which Milestone 0 gates. Installs run in 14 Regions the baseline cannot reach, and restore by DynamoDB point-in-time recovery.

## Usage (caller's view)

**Operator.** The baseline's bootstrap, signed release, `platform-deploy`, operator role, intents, and status documents are reused unchanged (`DESIGN.md` "Usage", "Install operations"). Only the target's durability block and what `apply:infra` creates change:

```sh
# Single-Region self-hosted install in Zurich, where Aurora DSQL is not offered.
cat > target.json <<'EOF'
{ "version": 1, "name": "git-prod", "profile": "tributary-customer", "aws": { "accountId": "111122223333", "region": "eu-central-2" },
  "tributary": { "hostname": "git.example.ch", "durability": { "class": "single", "pitrDays": 35 },
                 "nodes": { "layout": "combined", "perAz": 1, "instanceType": "i8g.2xlarge" } } }
EOF
platform-deploy deploy --target target.json --bundle tributary-2027.03.1
# Creates 5 on-demand DynamoDB tables (log, checks, actions, inbox, global) with PITR, deletion protection, Streams,
# warm throughput, and a maximum throughput; 3 versioned buckets; 3 nodes. No database server and no cluster.
# Two-Region BYOC instead: "durability": { "class": "paired", "regions": ["eu-central-1", "eu-west-1"], "witness": "eu-west-3" }
```

**A `gh` user** types exactly what `DESIGN.md` "Usage" shows. Nothing is configured beyond the hostname.

**A maintainer** writes every state change as a pure `decide` over a fresh fold, and every read against a fresh fold:

```rust
// receive-pack, after `index-pack --strict` and the connectivity check against the fresh fold
let acked = commit(&cell, ChainId::Repo(r), Push { updates, pack: packs.place(net, staged).await?, actor, atomic: true },
                   req.arrived).await?;                         // decide re-runs authorize on fresh owner and repo folds
report_status(&acked);                                          // takes &Acked, so "ok" cannot precede the put
// GET /repos/{o}/{r}/issues?labels=bug&sort=updated
let view = folds.fresh(ChainId::Repo(r), req.arrived).await?;  // one strong tail read, shared by concurrent requests
let page = view.issues(&IssueQuery::from_rest(&params)?, cursor)?;   // SQL over the node's SQLite fold
```

**Journeys.** Managed cell `c1` is Paired in us-east-2 and us-west-2 with a us-east-1 witness. "Durable" means what exists when the client is told done.

1. *Atomic three-branch push to busy `acme/mono` while the merge queue merges 10 PRs a minute, bots move 50 refs a minute, and CI clones 2,000 times a minute.* The edge in us-east-2 finds the token and the route `(acme, mono) -> c1` in its cache. A `c1` git node proves the `acme` owner chain and the `mono` chain fresh with two strong tail reads on `log` (us-east-2, quorum with us-east-1). A pack over 16 KiB is PUT with `If-None-Match` to the us-east-2, us-west-2, and us-east-1 buckets, and two answers suffice. `commit` appends one `Refs` entry with all three updates. Queue merges and bot updates append their own entries on the same chain, batched by the chain's preferred node, and a lost race re-runs `decide`, so a ref nobody else touched rebases and a contended one reports `fetch first`. Durable: the entry in at least two of three Regions (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html) and the pack in two buckets. Next read: any node in either Region proves its fold fresh first, so every clone that starts after the "ok" sees all three refs, and a clone already running sees all three or none. The 33 clones a second cost at most one strong read per node per in-flight read, and bytes come from NVMe.
2. *A fork PR merges while a review and a check run land, then `gh pr view` and `gh pr list` run at once.* PR 17 lives in `acme/widgets`'s chain in `c7`, and the head `bob/widgets:fix` is a chain in the same network and cell. The review is a `Review` entry on `acme/widgets`'s chain, so it lands either before the merge, and the merge's `decide` sees it, or after, as a review of a merged PR. The check run is a conditional update in `checks` (pk repo and head SHA), which the merge reads strongly. The merge appends one `Merged` entry carrying the base update, the head SHA, the verified tree-diff digest, and the inline pack. Durable: that entry in two Regions. Both `gh` calls route to `c7`, prove the chain fresh, and show `MERGED`.
3. *`gh issue create --label bug`, then `gh issue list --label bug`, then `gh api repos/o/r/issues?sort=updated`.* Create appends one `IssueOpened` entry whose number comes from the fold at the seq it was decided on, so numbering and labels commit together. Both lists prove the chain fresh and run SQL over the SQLite fold, so the new issue is in both, first by `updated`. No index is eventually consistent.
4. *An owner removes a member.* `DELETE /orgs/acme/members/mira` appends `Member { user: mira, role: None }` to `acme`'s owner chain in its home cell and returns 204 when the put returns. Every authorization reads a fresh owner fold, and every write re-proves the owner chain just before its put, so mira's next push fails. Bound: zero after the 204 for any request that authorizes after it, plus at most one put round trip (about 20 ms [guess]) for a push already deciding. Her token stays valid, as on GitHub.
5. *The Region holding the newest writes is lost mid-push.* Managed `c1` loses us-east-2. An acknowledged entry is in us-west-2 or the witness, so us-west-2 still reads it strongly and keeps writing with the witness (V2globaltables_HowItWorks, above). A push whose put had not returned gets a broken connection, and its retry is idempotent because the entry records the request id and git re-sends expected old values. Route 53 health checks drop us-east-2's edges, which forward to us-west-2 ingress: no human, no control-plane call, RTO about 2 minutes [guess], RPO zero. A two-Region BYOC install (eu-central-1, eu-west-1, witness eu-west-3, a pack bucket in each) behaves the same. Losing a replica Region and the witness together stops writes and strong reads until one returns.
6. *A bad release multiplies identity reads tenfold, and another writes a record shape the previous release cannot read.* Identity reads are edge cache misses on `global` and owner-chain tail reads in cells. Misses are capped per node by a compiled constant, owner reads are coalesced per node, and each table carries a maximum read throughput (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/on-demand-capacity-mode.html), so the canary wave's nodes answer 503 for uncached principals, abort on the 5xx gate, and swap back, while other cells and older nodes see no added load. If the tenfold reads are instead real traffic from a client release, as in February 2026 (`research/uptime.md#47`), the principals repeat, so they hit edge caches and coalesced folds, store load stays flat, and admission by OAuth client id sheds only that client. A new entry format is written only after a signed `formats` config lists it, and the operator role sets that only when every node reports a release that reads it. If one escapes, nodes on the old release refuse to apply that chain, answer 503 for it, and alarm. Rollback is refused for that format and the fix rolls forward, contained to chains the canary cell wrote.
7. *Self-hosted in eu-central-2 (no DSQL), operator mistake, point-in-time restore.* DynamoDB is offered in 34 Regions and DSQL in 20, and eu-central-2 is one of the 14 that only DynamoDB covers [measured from https://docs.aws.amazon.com/general/latest/gr/ddb.html and .../dsql.html]. The install's preflight checks DynamoDB, the Streams interface endpoint, S3, and the instance type in three AZs [unverified that i8g is offered in eu-central-2; the target may name another NVMe type]. An admin script deletes the org `acme`. `tributary admin restore --to <t>`, signed with the customer-only key, restores all five tables to the same second `t` into new tables (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/PointInTimeRecovery_Howitworks.html), restores pack versions the restored chains name from noncurrent S3 versions, then a second intent repoints table names and bumps `restore_epoch`, and nodes drop every fold and refold from snapshots at or below each restored head. Consumer cursors above a restored head are clamped to it. Durable: every commit before `t`. Next read: `acme` exists with its state at `t`.

## Shape

A **cell** is one table set, one bucket set, and one node fleet that share nothing with other cells. Managed has many cells. An install is one cell. Each store is chosen for one job:

| Store | Holds | Chosen because |
| --- | --- | --- |
| DynamoDB `log` | Every chain entry for 30 days, checkpoint pointers, id blocks | One conditional put is linearizable and RPO zero across Regions with no failover step (`research/uptime.md#29`); request API with no connection quota [inference]; 34 Regions [measured] |
| DynamoDB `checks`, `actions`, `inbox` | High-churn single-item state: check runs, Actions jobs, consumer cursors, deliveries, notifications | Separate tables give separate throughput caps, so Actions or webhook load cannot throttle `log` (doc above) |
| DynamoDB `global` | Logins, token hashes, SSH fingerprints, installations, slot map | MRSC spans any three of 15 Regions on different continents (`research/dynamodb.md#25`) |
| S3 | Packs, spilled bodies, fold snapshots, archived entries, bundles, control | $0.023 against $0.25 per GB-month (`research/pricing.md`, `research/dynamodb.md` section 7); conditional create (`research/uptime.md#31`) |
| SQLite on NVMe | One fold per chain: the query index for every list, filter, and sort | Derived and rebuildable; a bad plan or migration touches one aggregate on one node [inference] |
| OpenSearch | Full-text search only | Off every core path (`research/uptime.md#49`) |

```rust
// ---- Placement ----
pub struct CellId(u16); pub struct Slot(u16);            // 65,536 slots; the global table maps each to a cell
/// Paired: every table is MRSC in two replica Regions plus a witness, packs land in two of three Regional buckets before
/// a commit. Single: plain tables in one Region; `dr` adds an MREC replica that takes writes only after a signed promotion.
pub enum Durability { Paired { a: Region, b: Region, witness: Region }, Single { home: Region, dr: Option<Region> } }
pub struct DbId(i64);                                     // (seq << 16) | slot, below 2^53 (research/wire-census.md#28)
pub struct RepoId(DbId); pub struct NetworkId(DbId); pub struct AccountId(DbId);

// ---- DynamoDB `log`, one per cell. pk = ChainId, sk = LogSk. No GSI, no LSI, no TTL, no transactions. ----
/// One gap-free chain per repository, per fork network, and per owner (user, org, enterprise). An owner chain lives in
/// the owner's home cell, chosen at signup. A new network takes a slot in its owner's home cell, and forks join it.
pub enum ChainId { Repo(RepoId), Network(NetworkId), Owner(AccountId) }
pub struct Seq(u64);
pub enum LogSk { Entry(Seq) /* "e#" + 20 digits */, Checkpoint /* "c" */, IdBlock(IdKind) /* "i#" + kind */ }
/// Put once with `attribute_not_exists(pk)`. That put is the commit point for every event inside.
pub struct EntryItem { pub chain: ChainId, pub seq: Seq, pub prev: Digest, pub format: Format, pub at: Timestamp,
    pub requests: SmallVec<[RequestId; 4]>,               // a retried command finds its own id in the tail
    pub body: Body }
pub enum Body { Inline(Vec<Event>) /* item <= 350 KB */, Spilled { key: S3Key, digest: Digest } }
pub struct CheckpointItem { pub upto: Seq, pub snapshot: S3Key, pub archived_below: Seq, pub fold_digest: Digest, pub release: ReleaseDigest }
pub enum Event { Repo(RepoEvent), Network(NetworkEvent), Owner(OwnerEvent) }
pub enum RepoEvent {
    Genesis { owner: AccountId, network: NetworkId, name: RepoName, fork_of: Option<(RepoId, Seq)> },
    Refs { push: PushId, actor: Actor, updates: Vec<RefUpdate>, pack: Option<PackRef> },   // one atomic push or bot batch
    PrOpened { n: IssueNumber, head: HeadSpec, base: RefName, capture: PrCapture },
    Merged { n: IssueNumber, base: RefUpdate, head: ObjectId, method: MergeMethod, also: Vec<RefUpdate>,
             pack: Option<PackRef>, verified: TreeDiffDigest, engine: EngineVersion },
    PrClosed { n: IssueNumber, final_head: ObjectId }, IssueOpened { n: IssueNumber, capture: IssueCapture },
    Comment(CommentEvent), Review(ReviewEvent), Labels(LabelEvent), Queue(QueueEvent), Rule(BranchRule),
    Settings(RepoSettings), Collaborator { who: AccountId, permission: Option<RepoPermission> },
    Sealed { to: CellId, generation: u64 },               // the move fence: no entry ever follows it
}
pub enum NetworkEvent { ForkAdded(RepoId), Compacted { consumed: Vec<PackId>, produced: Vec<PackRef> }, Sealed { to: CellId, generation: u64 } }
pub enum OwnerEvent { Created(AccountCapture), Member { user: AccountId, role: Option<OrgRole> }, Team(TeamEvent),
    Grant { repo: RepoId, grantee: Grantee, permission: Option<RepoPermission> },
    RepoName { key: RepoNameKey, entry: Option<RepoEntry> },   // Live { repo, epoch } | Alias { repo, epoch }, as in DESIGN.md
    Installation { id: InstallationId, state: InstallState, not_before: Timestamp }, Policy(OrgPolicy),
    Credential(CredentialEvent), Sso(SsoEvent), Sealed { to: CellId, generation: u64 } }

// ---- The two primitives ----
/// The only write path for chained state. Proves the chain and `deps` fresh, runs the pure `decide`, appends the
/// result at head + 1, and on ConditionalCheckFailed or ReplicatedWriteConflict refolds and reruns `decide`, at most 3
/// times under a per-node retry budget of 10% of calls. Concurrent commands for one chain on one node share one item.
pub async fn commit<C: Command>(cell: &Cell, chain: ChainId, cmd: C, arrived: Instant) -> Result<Committed<C::Out>, CommitError<C::Err>>;
pub trait Command { type Out; type Err;
    fn deps(&self) -> SmallVec<[ChainId; 2]>;            // re-proven fresh just before the put (the owner chain)
    fn decide(&self, fold: &Fresh, deps: &[Fresh]) -> Result<(SmallVec<[Event; 4]>, Self::Out), Self::Err>; }  // pure
pub struct Committed<T> { pub seq: Seq, pub out: T }
/// A fold that includes every entry acknowledged before `arrived`, because its strong tail read started after it.
/// Only `FoldCache::fresh` builds one, and `authorize`, `decide`, and every list read accept nothing else.
pub struct Fresh<'a> { view: FoldView<'a>, proven_after: Instant }
impl FoldCache {
    /// Single flight per chain per node: joins an in-flight tail read only if that read started after `arrived`.
    pub async fn fresh(&self, chain: ChainId, arrived: Instant) -> Result<Fresh<'_>, StoreError>;
}
pub fn authorize(p: &Principal, a: Action, owner: &Fresh, repo: &Fresh) -> Decision;   // pure; Decision as DESIGN.md

// ---- Packs ----
pub enum PackRef { Inline(Bytes) /* <= 16 KiB */, Stored { id: PackId, checksum: [u8; 20], bytes: u64, copies: BucketSet } }
/// Only `PackStore::place` builds it: inline, or PUT If-None-Match to 1 bucket (Single) or 2 of 3 (Paired).
pub struct DurablePack { pack: PackRef, _sealed: Sealed }
pub struct Acked { pub seq: Seq, pub per_ref: Vec<(RefName, RefOutcome)> }   // report_status(&Acked)

// ---- DynamoDB `checks`, `actions`, `inbox` (same Regions as `log`) ----
/// checks: pk (RepoId, ObjectId), sk CheckRunId; every write conditional on `version`; merges read it strongly.
pub struct CheckRunItem { pub suite: SuiteId, pub name: Box<str>, pub app: AppId, pub status: CheckStatus, pub conclusion: Option<Conclusion>, pub version: u64 }
/// actions: Run and Job items by id with the baseline's job state machine; queue index pk ("q", scope, label_set_hash,
/// shard 0..16), sk (since, JobId), deleted when the job leaves Queued.
pub enum InboxKey {
    Dirty { consumer: Consumer, bucket: u8, chain: ChainId },   // max_seq; dispatchers raise it, the consumer deletes it
    Cursor { consumer: Consumer, chain: ChainId },              // last seq the consumer finished
    StreamPos { table: Table, region: Region, shard: ShardId },
    Delivery { hook: HookId, guid: Uuid }, Notification { user: AccountId, thread: ThreadId },
    UserIndex { user: AccountId, kind: IndexKind, at: Timestamp, item: ItemRef }, Card { repo: RepoId }, Stars { repo: RepoId, shard: u8 } }
pub enum Consumer { Webhooks, ActionsTrigger, PrSync, Notify, Cards, Search, Audit }

// ---- DynamoDB `global`: managed is one MRSC table in us-east-2, eu-central-1, ap-northeast-1 ----
pub enum GlobalKey { Login(LoginKey), Token([u8; 32]), Ssh(Fingerprint), Email(EmailDigest), Account(AccountId),
                     App(AppId), Installation(InstallationId), Slot(Slot) }
pub enum LoginItem { Live { account: AccountId, kind: AccountKind, home: CellId, v: u64 }, Former { account: AccountId, v: u64 }, Reserved }
pub struct TokenItem { pub account: AccountId, pub kind: TokenKind, pub scopes: ScopeSet, pub expires: Option<Timestamp>, pub revoked: Option<Timestamp> }
pub struct AccountItem { pub home: CellId, pub suspended: bool, pub sessions_not_before: Timestamp }
pub struct SlotItem { pub cell: CellId, pub generation: u64 }
/// Edge read-through cache. Positive entries live 60 s. Token misses are cached 10 minutes, because a 177-bit secret is
/// never guessed before it exists; login misses 2 s. A miss reads the local replica, then strongly before saying "no".
pub const MISS_BUDGET_PER_NODE: u32 = 200;                // reads per second, compiled, never config

// ---- S3 and node folds ----
/// Every PUT carries If-None-Match: * and the bucket policy denies any that does not (research/uptime.md#31).
pub enum S3Key { Pack { network: NetworkId, id: PackId }, Spill { chain: ChainId, seq: Seq }, Snapshot { chain: ChainId, upto: Seq },
                 Archive { chain: ChainId, from: Seq, to: Seq }, Bundle { network: NetworkId, id: BundleId } }
/// One SQLite file per chain on NVMe, plus a bare cache repository with a reftable per network. Derived only.
pub struct Fold { pub chain: ChainId, pub applied: Seq, pub schema: FoldSchema, db: rusqlite::Connection }
pub fn apply(fold: &mut Fold, e: &EntryItem) -> Result<(), FoldError>;   // the only writer; refuses a wrong `prev` digest
```

Invariants the structure enforces:

- **Acknowledgment point.** A write is acknowledged when, and only when, its entry's put returned. Bytes it names are inline or already in S3 (`DurablePack`), and `report_status` takes `&Acked`. In Paired cells the put returned after a second Region had it (`research/uptime.md#29`).
- **Gap-free, linearizable chains.** Only `commit` puts entries, and it puts seq N+1 only from a fold at N; a put of any seq at or below the head fails its condition, so seq order is commit order. `apply` refuses an entry whose seq is not `applied + 1` or whose `prev` digest does not match, and alarms. Two commands on one chain serialize, and every input on that chain is in the fold `decide` saw, so a merge cannot race a review, a rule change, or a push (no write skew, unlike Repeatable Read in `research/uptime.md#27`).
- **An event exists if and only if its change committed**, because the entry is both. Consumers read chains, never a separate outbox.
- **One writer per fold** (`apply`), **one owner per fact** (the chain whose events set it). Pull request state, open, closed, or merged, lives only in its repository chain.
- **Bounded store calls.** `trib-store` exposes GetItem, BatchGetItem, conditional PutItem and UpdateItem, DeleteItem, and single-partition Query with a limit. CI fails on a transaction API, a secondary index, or a Scan anywhere outside the operator's `Remark` and restore steps.
- **The seal is the move fence.** After `Sealed`, `commit` refuses the chain and redirects. A writer that decided before the seal loses its put to it.
- **Multi-chain operations have one commit point and finish on first touch.** A fork's commit point is the `RepoName` entry in the fork owner's chain. Whoever first touches the fork appends `ForkAdded` to the network chain, then `Genesis` (both idempotent by repository id). A transfer's commit point is the repository chain's `Settings { owner }` entry, then the two owners' `RepoName` entries. A signup's commit point is the `Login` claim in `global`, which carries the account record, and the owner chain's `Created` is written from it on first touch. The `Audit` consumer also finishes any that nobody touched within a minute.
- **Events reach consumers through markers, not the stream.** In each replica Region one dispatcher reads that replica's stream, one reader per shard under the two-reader limit (`research/dynamodb.md#14`), and raises `Dirty.max_seq` idempotently. MRSC streams are identical on every replica (`#15`), so both Regions' dispatchers run at once and either alone suffices. A consumer deletes its marker only with a condition that `max_seq` is at or below the seq it finished. Markers last until their consumer finishes, so a consumer outage of any length loses nothing. The 24-hour stream retention (`#13`) binds only the dispatchers, which alarm at 5 minutes of lag; past 24 hours, the operator's `Remark { since }` command scans `log` once and raises markers for every chain with newer entries, and a weekly drill runs it.
- **Packs follow `DESIGN.md`'s retention rules.** Compaction runs per network under a conditional `maint` item, places its packs first, then appends `Compacted`. GC deletes only objects unreachable from every member chain's fresh fold for 14 days, and versioning keeps deletes 35 days.

Dominant access patterns traced through the keys:

- *Fetch or clone.* Edge cache: token and route. Cell: `fresh(Owner)` and `fresh(Repo)`, two strong Queries `pk = chain, sk > "e#K"`, 1 RCU each when empty [inference: 4 KB strong read unit]. If entries arrived, `apply` them, write changed refs into the reftable, pull missing packs. Then upstream `git upload-pack` on NVMe.
- *Push.* The same reads, S3 PUTs for a stored pack, one PutItem.
- *Issue or PR list, PR page.* `fresh(Repo)`, SQL on the fold; a PR page adds one strong Query on `checks` pk `(repo, head)`.
- *Authorization.* Inside the two fresh folds, no other read.
- *`/user/repos`, `/orgs/{o}/repos`.* The user's owner fold, each org's owner fold (parallel, cross-cell when needed), and a BatchGetItem of `Card` items, which lag pushes by up to 60 s.
- *Webhook.* Stream record, `Dirty` marker, consumer Query from `Cursor`, render from the entry's captured fields, `Delivery` item, POST.

## Commit and read paths

**Push, up to 1,000 refs atomically.** Receive-pack quarantines and indexes the pack, checks connectivity against the fresh fold, and places it: at most 16 KiB inline in the entry, else S3. `commit` runs `Push::decide`: each update's expected old value must equal the fold (else `Stale`, git's `fetch first`), rules come from the fold's `Rule` rows and the owner's rulesets, `AllOrNothing` rejects everything on one failure and `PerRef` marks only that ref. 1,000 refs of names up to 900 bytes is at most about 940 KB, so a body above 350 KB spills to S3 (2 of 3 buckets in Paired) before the put, keeping items under DynamoDB's 400 KB (`research/dynamodb.md#7`). Admission caps a push at 10,000 refs [guess at GitHub's own limit], and a push over 1,000 refs takes the chain's put alone instead of sharing a batch. A lost race re-runs `decide` on the new fold: refs no intervening entry touched rebase, touched ones become `Stale`. Each chain has a preferred node per Region and a preferred Region, both by rendezvous hash, and while both Regions are healthy the other Region forwards a busy chain's commands there, which avoids `ReplicatedWriteConflictException` (V2globaltables_HowItWorks). Correctness never depends on that preference: any node in either Region may put.

**Same-repository merge.** The plan reads the fresh fold (PR, head ref, base ref, reviews, rule) and the head's check runs, then asks the git role for `merge-tree`, `commit-tree`, and the tree-diff check that the result equals base plus the PR's own diff (`research/uptime.md#49`, implication 19). `Merge::decide` re-checks on the fold it is given: head moved gives `Head branch was modified. Review and try the merge again.`, base moved gives `Base branch was modified...`, a new review or rule gives the protection error. It emits one `Merged` entry, with head-branch deletion in `also`. A 409 is never retried into success, because a rerun of `decide` re-runs protection.

**Fork merge.** The head is `bob/widgets`'s chain, in the same network and cell. The plan proves it fresh and pins the head SHA; GitHub's `sha` parameter, when given, must match it. The `Merged` entry goes on the base chain alone. A push to the fork that lands in the one put round trip between plan and commit is not merged, and the PR closes merged at the reviewed head. `DESIGN.md` instead compare-and-sets the fork's head row in the merge transaction; this design accepts the narrower guarantee that the merged commit is exactly the reviewed head.

**Merge queue.** `Queue` events on the base chain record entry, group building, and temporary `gh-readonly-queue/*` refs. When a group's checks pass, one entry carries a `Merged` per PR and the base fast-forward, each PR's tree diff verified separately, so a group lands whole or not at all.

**Branch-rule changes.** A repository rule is a `Rule` entry on the same chain as pushes, so a push decided before it loses its put and re-decides under it. An org ruleset is an owner-chain entry, and every push re-proves the owner chain just before its put.

**List endpoints.** Each fold has tables `issue(number, kind, state, author, milestone, created, updated, comments, locked)`, `issue_label`, `issue_assignee`, `issue_mention`, `pull(number, head, base, draft, ...)`, `review`, `comment`, `ref`, each with indexes per GitHub filter. `GET /repos/{o}/{r}/issues` maps `milestone`, `state`, `assignee`, `creator`, `mentioned`, `labels` (all must match), `since`, `sort` (created, updated, comments), and `direction` to one indexed SQLite query. Pulls add `head`, `base`, and `sort=popularity|long-running`. GraphQL `filterBy` and `orderBy` use the same queries with keyset cursors. Cross-repository lists (`/issues`, `/user/repos`, notifications) read `inbox` and owner folds and lag by seconds. `/search/*` and `gh pr status` read OpenSearch and lag by seconds, as GitHub's search does [inference].

**Authorization.** The edge authenticates: token hash from its cache or `global`, installation JWS against cell verifying keys, session cookie against `sessions_not_before`. It routes by `login -> home cell` from `global` and caches `(owner, repo) -> (cell, repo, epoch)` from the home cell's answer, and the serving cell rejects a stale epoch so the edge re-resolves, as `DESIGN.md`'s `NameEpoch` does. Sign-in routes to the home cell, which checks the password hash and second factor in the owner fold and signs a session cookie. The serving node runs `authorize` on the fresh owner fold (membership, teams, grants, base permission, SSO, installations) and the fresh repository fold (collaborators, visibility). An org-owned fork placed in another owner's network reads its owner chain across cells, the one cross-cell read on the write path. A huge owner's read-class proofs coalesce over a window of `max(20 ms, nodes / 1,000 s)` so its partition sees at most about 1,000 strong reads a second [inference from `research/dynamodb.md#8`], while writes always re-prove.

**Revocation.** Membership, team, grant, visibility, ruleset, collaborator, and installation changes are owner or repository chain entries, so they bind every request that authorizes after the acknowledgment, plus one put round trip for a write already deciding, and at most the coalescing window (20 ms, or under 1 s for an owner served by 1,000 nodes) for reads. Token revocation, account suspension, and sign-out-everywhere change `global` items and bind within the 60 s edge cache. App uninstall bumps the installation's `not_before` in the owner chain, so its stateless tokens fail at once.

| After this write is acknowledged | The next read | Sees it |
| --- | --- | --- |
| Push, merge, issue, comment, label, review, rule (repository chain) | Any fetch or repository-scoped API read, any node, any Region | Always: linearizable per chain |
| Membership, team, grant, rename, installation (owner chain) | Any authorization or owner-scoped read | Always, within the coalescing window for reads |
| Check run (`checks`) | A merge, `gh pr checks` | Always: strong read |
| Token creation, login claim (`global`) | Any edge | Always: a miss reads strongly |
| Token revocation, suspension (`global`) | Any edge | Within 60 s |
| Cards, cross-repo lists, notifications, search | Any | Within seconds, cards within 60 s |

## Scale

**Busiest single repository.** A chain's tail lives in one partition, which delivers at most 1,000 write and 3,000 read units a second (`research/dynamodb.md#8`) [inference that monotonically increasing sort keys stay in one partition].
- *Writes:* at about 1 KB per non-inline push entry [guess], the chain carries about 1,000 entries a second, bytes-bound. Unbatched, it carries 1 / L, about 50 a second at L = 20 ms [guess], so group commit is what reaches the bytes bound. Inline packs count against it, so a chain above 30% of its write budget stops inlining. GitHub's Spokes took about a third of a second per update under a repository lock (`research/git-storage.md#4`), and Continuity reports up to 120 pushes a second on S3 Standard (`#30`). Per-repository admission defaults to 200 pushes a second, raisable to 800 [guess].
- *Merges:* each merge on one base branch needs the previous one's result, so about 1 / (plan + put), roughly 4 a second, 270 a minute at a 200 ms plan [guess]; merge-queue groups multiply that by group size. Journey 1 needs 10 a minute.
- *Bot ref updates:* `PrSync` coalesces `refs/pull/N/*` per PR last-writer-wins into one `Refs` entry per batch, below user entries in the batch order. 50 a minute is under 1% of the chain.
- *Clones:* freshness costs at most one strong read per serving node per read round trip, and the serving set is capped at 32 nodes, so at most about 1,600 reads a second against the 3,000 limit. Bytes come from NVMe, 32 nodes at about 300 MB/s [guess] serve about 120 shallow 80 MB clones a second (7,200 a minute), and full clones go to S3 bundles through bundle-uri (`research/git-storage.md#42`) at 5,500 GETs a second per prefix (`#54`). Journey 1's 2,000 a minute fits on 9 nodes.
- *Comments and reactions* share the chain under a per-repository cap of 50 entries a second, so a viral issue cannot starve pushes. Stars and check runs are not on the chain.

**Fleet.** 3.35 billion pushes a month (`research/git-storage.md#5`) is 1,275 a second on average; at 3 times for peak [guess], 3,800 a second. At about 16 write units per push including consumers (`cost.py`), that is about 61,000 a second, or 122,000 replicated units in Paired cells, spread over cells. At 200 cells that is about 610 a second per cell against the default 40,000 per table (`research/dynamodb.md#9`). Nothing is serialized fleet-wide: `global` sees claims, token writes, and slot flips only.

**Fan-out.** Each stream record raises one `Dirty` marker per subscribed consumer (about 4), so consumers progress independently and a stuck consumer holds only its own markers, kept until it finishes, with no 24-hour loss. Webhook deliveries are shuffle-sharded per hook owner as in `DESIGN.md`. Actions: journey 1's 2,000 jobs a minute are job items hashed by id, and the busiest check partition, one SHA with a 256-job matrix, writes about 13 units a second.

| Hot key | Load | Limit | Spread |
| --- | --- | --- | --- |
| `log` repository chain | Pushes, merges, issues, comments | 1,000 WCU/s, 3,000 RCU/s per partition | Group commit, preferred writer Region per chain, 32-node serving cap, per-repository and comment caps, stars and checks off-chain |
| `log` owner chain of a huge org | SCIM bursts; every authorization in its repositories | Same | Writes batch; read proofs coalesce per node over a window sized to the serving set |
| `checks` (repo, SHA) | Matrix check updates | Same | 16-way suite sharding when a SHA exceeds 256 runs |
| `actions` queue index | Queued jobs per runner scope | Same | 16 shards per (scope, label set) |
| `inbox` dirty buckets | One write per record per consumer | Same | 256 buckets per consumer by chain hash |
| `inbox` stars, notifications | Viral repository or thread | Same | 16 star shards; per-user partitions with a per-thread cap |
| `global` login and token items | Edge misses | Same | Hash keys; positive cache 60 s; per-node miss budget |
| S3 pack prefix | Pushes to one network | 3,500 PUT/s per prefix (`research/git-storage.md#54`) | Hashed prefix per network |

## Downtime

| GitHub incident | Evidence | This design | Effect |
| --- | --- | --- | --- |
| Database overload | Shared permissions cluster, App tokens down 12 h (GHAR May 2023); auth database overloaded by a TTL cut and 10x reads (Feb 2026) (`research/uptime.md#44`, `#47`) | No shared query engine: every call is one item or one partition. Permissions live in each owner's chain in its home cell. Partitions throttle alone (`research/dynamodb.md#8`), each table has a maximum throughput, and per-node budgets bound misses, so load follows fleet size, not request rate or TTL | contained |
| Schema change on the store | Misconfigured index (Sep 2023), migration overloading a table at peak (May 2026), 32-bit key overflow (May 2026) (`#44`) | No DDL, index build, or online migration exists on any table. Ids are 64-bit with a 50% overflow alarm. Fold schemas change by lazy refold per aggregate, canary cell first | removed |
| Record shape the old release cannot read | Dropped column still referenced (GHAR Aug 2025) (`#44`) | Records are versioned with optional fields; CI decodes release N+1's corpus with release N; a format is written only after the signed `formats` config says every node reads it (journey 6) | contained |
| Engine upgrade | Version bug on a custom cluster (May 2023); major-version upgrade under load (Jan 2026) (`#44`) | Neither DynamoDB nor S3 exposes an engine version to choose or upgrade [inference]. SQLite is pinned in the binary and touches only derived folds, rolling in waves. AWS-side DynamoDB deploys remain a risk ("Open questions") | removed |
| Failover and partition | 43 s partition, cross-country promotion, 24 h outage (2018); Redis failover with no writable primary (Mar 2026); undetected partial disk (Sep 2023) (`#40`, `#44`, `#47`) | No primary and no promotion: each put waits for a second Region (`#29`), nodes hold no canonical state, and evacuation is health checks only | removed |
| Connection storms | Pool exhaustion classes; DSQL's 100 new connections a second (`#27`) | DynamoDB and S3 are request APIs with no per-table connection quota [inference]; retries are budgeted at 10% | removed |
| Config and deploy | Sharding-middleware redeploy, LB changes, expired certificate, memcached proxy (`#45`) | Cells with their own tables and fleets, per-AZ waves, signed config with last known good, certificate alarms (`DESIGN.md` "Change safety"). No database proxy or sharding tier exists to misconfigure | contained |
| Capacity | LB tier failed to scale, a retry loop slowed recovery (Aug 2026) (`#48`) | Static N+1 compute at 66% per AZ, warm throughput set at install to the fleet's budget (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/warm-throughput.html), admission per repository, owner, IP, and client id, retry budgets | contained |
| Search dependency | Elasticsearch overload left PR and issue lists empty (Apr 2026) (`#49`) | Lists come from folds. OpenSearch serves only `/search`, and a timeout yields `incomplete_results` | removed |
| DNS dependency | A site's DNS down 19 h (Oct 2024), DNS cascade (Apr 2026) (`#46`); DynamoDB's own DNS record emptied (Oct 2025) (`#32`) | Peer endpoints cached with last known good. A Regional DynamoDB DNS failure leaves the cell's other Region serving, as global table replicas did in Oct 2025 (`#34`) | mitigated |
| Merge logic bugs | Squash merge queue reverted 2,092 PRs (Apr 2026) (`#49`) | The tree-diff check refuses a merge whose result is not base plus the PR's diff. Each `Merged` entry records `engine` and the pre-merge base, and the archive keeps every entry, so repair is one tool run over the chains | contained |

**What keeps serving while each store is impaired.** `log` impaired in one Region: the other Region serves everything. `log` without quorum: that cell stops; other cells serve. `checks` or `actions`: merges that need checks wait and Actions pauses; git, other merges, and lists serve. `inbox`: webhooks, notifications, cards, and Actions triggers lag with their markers kept; core serves. `global`: cached tokens and routes serve, new sign-ins and claims fail in the affected continent. S3: inline pushes, merges, issues, and warm folds serve; large pushes and cold folds fail. OpenSearch: only `/search`.

| Durability class | Data | RPO | RTO | Mechanism |
| --- | --- | --- | --- | --- |
| Paired cell | Pushes, merges, metadata, checks, Actions state | 0 | About 2 minutes for a Region, health checks only [guess] | MRSC put plus 2-of-3 buckets; no control-plane call |
| Managed `global` | Logins, tokens, installations, slot map | 0 | 0 on the two other continents | MRSC across three continents (`research/dynamodb.md#25`) |
| Single, operator error | Everything | Any second in 35 days (`research/dynamodb.md#19`) | Restore time per table plus refold [unverified duration] | PITR into new tables, S3 noncurrent versions |
| Single with `dr`, Region loss | Entries and inline packs; stored packs | Seconds (`research/uptime.md#30`); up to 15 minutes [unverified RTC SLA] | About 60 minutes, nodes launch | MREC replica, S3 replication, promotion truncates each chain at its first gap or first missing pack |
| Single without `dr`, Region loss | Everything | The newest cross-Region backup copy [unverified for DynamoDB] | Hours | Restore |

## Failure modes

Every row is per cell unless it names `global` in managed. "Instance" for a managed store means a storage or request node behind its endpoint.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| log table | instance | Single puts or reads fail or throttle | Two retries with jitter under the 10% budget; a retried put finds its request id in the tail |
| log table | az | None for data [inference: DynamoDB spans AZs]; a third of nodes | Nodes in other AZs at 66% |
| log table | region | Paired: one replica Region. Single: the install | Paired: the other replica and witness keep quorum (`research/uptime.md#29`) and edges forward there. Single: wait, `dr promote`, or restore |
| log table | control-plane | None at runtime | Tables, Streams, PITR, warm and maximum throughput set at install; restore is the only control-plane step |
| log table | bad-deploy | Wrong but well-formed entries in canary chains | `decide` is pure and property-tested against the previous release; merge verification; compensating entries; auto-abort on put error or conflict rate |
| log table | bad-config | One cell, for example a maximum throughput set too low | Signed config waves, a readiness probe that appends to a canary chain, last known good |
| log table | noisy-tenant | One repository's or owner's partition | Partition limits isolate it (`research/dynamodb.md#8`); per-repository admission and comment caps |
| work tables | instance | Single Actions, check, or delivery writes fail | Retries; every write conditional on a version or idempotent by key |
| work tables | az | None for data | As `log` |
| work tables | region | Paired: one replica Region. Single: with the install | As `log` |
| work tables | control-plane | None at runtime | Created at install |
| work tables | bad-deploy | Actions dispatch, check updates, deliveries, notifications in one cell | Separate tables and throughput caps keep git, lists, and merges without required checks serving; markers wait |
| work tables | bad-config | One cell's Actions and webhooks | Signed config waves |
| work tables | noisy-tenant | One org's queue or one hook owner | Queue shards, per-org job caps, per-hook-owner delivery concurrency, per-table maximum throughput |
| global table | instance | Single misses fail | Edges keep cached entries; retry |
| global table | az | None for data | Nothing to do |
| global table | region | Managed: one continent's replica. Install: as `log` | Edges read the nearest other replica; claims need two of three Regions |
| global table | control-plane | None at runtime | Created once, empty, as MRSC requires (`research/dynamodb.md#18`) |
| global table | bad-deploy | Wrong claims or token items, service-wide in managed | Only `trib-global` writes, always conditional on a version; claim code ships in the last wave; a daily audit compares logins with owner chains |
| global table | bad-config | A cache setting that raises misses | Cache bounds and the miss budget are compiled; table maximum read throughput |
| global table | noisy-tenant | Signup storm, token scanner | Per-IP and per-ASN limits, 10-minute negative token cache, miss budget |
| pack and snapshot buckets | instance | Single requests 5xx or SlowDown | Retries; hashed prefixes |
| pack and snapshot buckets | az | None (three AZs, `research/git-storage.md#54`) | Nothing to do |
| pack and snapshot buckets | region | Paired: one of three buckets. Single: large pushes and cold folds | Paired: 2-of-3 still acknowledges. Single: inline pushes and warm folds serve |
| pack and snapshot buckets | control-plane | None at runtime | Pre-created; presigned URLs computed locally |
| pack and snapshot buckets | bad-deploy | Bad packs or snapshots in one cell | `index-pack --strict` before upload; snapshots carry release and fold digest, and a release marked bad has its snapshots ignored and refolded from the archive; GC only after 14 days unreachable; versioning |
| pack and snapshot buckets | bad-config | Writes fail before acknowledgment | Readiness probe writes and reads a canary object per bucket |
| pack and snapshot buckets | noisy-tenant | One network's prefix | Push bytes and concurrency caps per account and network |
| node folds and caches | instance | Requests in flight | Folds rebuild elsewhere from snapshot plus tail; each chain stays warm on one node per AZ |
| node folds and caches | az | A third of nodes | Static 66% sizing, zonal shift and weekly autoshift practice (`research/uptime.md#20`, `#21`) |
| node folds and caches | region | Paired: that Region's fleet | The other Region's fleet is sized for full load and keeps its own warm folds |
| node folds and caches | control-plane | Dead nodes not replaced while EC2 launch is impaired | N+1 static capacity; no recovery step launches |
| node folds and caches | bad-deploy | Wrong list answers in one AZ of the canary cell | Folds are derived; a fold audit refolds sampled chains from genesis on each release and daily and compares digests; swap back and refold |
| node folds and caches | bad-config | One AZ of one cell | Per-AZ rollout with abort on fold lag or list p99 |
| node folds and caches | noisy-tenant | One giant aggregate | Per-fold disk caps, widened serving set, per-chain CPU cgroups |
| search domain | instance | Search slower | Multi-AZ domain |
| search domain | az | Search capacity drops a third | Three data nodes across AZs |
| search domain | region | Search in that Region | `incomplete_results`; lists unaffected |
| search domain | control-plane | None at runtime | Pre-provisioned |
| search domain | bad-deploy | Search results wrong | Re-index from chains |
| search domain | bad-config | Search only | Signed config |
| search domain | noisy-tenant | A search botnet | Per-caller search limits; only `/search` sheds |
| control bucket and operator | instance | One stage step | Lease and cursor in the bucket with `If-Match`; another node resumes (`DESIGN.md` "Install operations") |
| control bucket and operator | az | None | Nothing to do |
| control bucket and operator | region | Paired: that Region's operator | The other Region's bucket and operator take the lease |
| control bucket and operator | control-plane | Rollouts that launch nodes wait | Serving is unaffected |
| control bucket and operator | bad-deploy | One rollout | Operator runs the previous release until after the soak; watchdog swap-back |
| control bucket and operator | bad-config | One install or cell | Signed config, last known good, customer-only key for destructive intents |
| control bucket and operator | noisy-tenant | One install's intents | Managed operators run per cell |

## Deployment modes

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| log table | One MRSC table per cell, two replica Regions plus a witness in one continent, for example us-east-2 and us-west-2 with us-east-1 | Paired by default: any two of the 15 MRSC Regions plus a witness. Single on request | Single by default in any of 34 Regions, Paired optional |
| work tables | `checks`, `actions`, `inbox` in the same Regions and mode as `log` | Same | Same |
| global table | One MRSC table in us-east-2, eu-central-1, ap-northeast-1 | A table in the install's own Regions, same mode as `log` | Same as BYOC |
| pack and snapshot buckets | Three Regional buckets per Paired cell, 2-of-3 | Paired: one per Region including the witness Region. Single: one, plus one in `dr` | Same as BYOC |
| node folds and caches | Role groups per cell Region, i8g NVMe, three AZs at 66% | Combined roles on 3 to 9 nodes per Region | Combined roles on 3 nodes, or 1 for evaluation |
| search domain | OpenSearch per residency domain | Optional, home Region | Optional |
| control bucket and operator | Per cell, vendor-only submitter | Per install Region, vendor and customer keys | Per install Region, customer keys |

**Backups and point-in-time restore.** Every table has PITR with 35 days at per-second granularity, restore always into a new table (`research/dynamodb.md#19`), deletion protection, and a 35-day system backup if deleted anyway (PITR page above). PITR is a per-replica setting on global tables (V2globaltables_HowItWorks, settings list) [unverified that MRSC replicas accept it]. Buckets keep noncurrent versions 35 days. Entries older than 30 days move to immutable `Archive` objects and are deleted from `log`, so S3 holds full history and every fold can be rebuilt from genesis. Restore is the two-intent flow of `DESIGN.md` "Install operations", with the table restores in place of the cluster restores. Journey 7 traces it.

**Regions per mode.** Managed cells run in the 15 MRSC Regions (`research/dynamodb.md#25`), with no replica in us-east-1 (`research/uptime.md` implication 22). BYOC Paired needs two of those 15 plus a witness; Sydney, Melbourne, and Calgary customers run Single with `dr`. Single runs in all 34 DynamoDB Regions, and `dr` in any other, because MREC accepts any Region (V2globaltables_HowItWorks). The baseline runs only in DSQL's 20 Regions, and Paired only inside one continent's set (`research/dynamodb.md#26`, `#27`).

## Cost

`cost.py` in this directory imports `TIERS`, `ACTIVITY`, and prices from `scripts/cost_model.py` and keeps every non-database line identical, so the difference is the foundation. Request units per operation are [guess]es stated in the script. Baseline totals are `scripts/cost_model.py` with its default backup retention (hourly for a day, daily for a week, weekly for a month) and 16 outbox partitions instead of 256, its two known overcharges fixed so the comparison is not flattered. On-demand list prices, us-east-2 home, us-west-2 peer, us-east-1 witness.

| Developers | Topology | This design per month | DynamoDB lines | Baseline, fixed | Difference |
| --- | --- | --- | --- | --- | --- |
| 50 | Single-Region | $1,287 | $19 | $1,433 | $-146 |
| 50 | Two-Region (Paired) | $2,292 | $38 | $2,485 | $-193 |
| 300 | Single-Region | $2,573 | $33 | $3,103 | $-530 |
| 300 | Two-Region (Paired) | $4,466 | $60 | $5,062 | $-596 |
| 2,000 | Single-Region | $12,432 | $127 | $16,384 | $-3,952 |
| 2,000 | Two-Region (Paired) | $20,636 | $214 | $24,879 | $-4,243 |

Arithmetic for one line, 300 developers Paired, DynamoDB writes: pushes 300 x 8 x 30.4 = 72,960 at 16 units (1 entry + 0.5 x 6 inline + 4 consumers x 3), metadata writes 182,400 at 13, jobs 273,600 at 21, about 9.3 million units, times 2 replica Regions at $0.625 per million rWRU (`research/dynamodb.md#28`) = $12. EC2 is the largest line in every total, 48% to 67% of this design's (`cost.py --detail`). The baseline's extra cost is its DSQL backups as full copies ($3,640 at 2,000 developers Paired), DSQL storage, board and polling requests, and DSQL interface endpoints. This design's DynamoDB and S3 traffic rides free gateway endpoints (https://docs.aws.amazon.com/vpc/latest/privatelink/gateway-endpoints.html), and only DynamoDB Streams needs an interface endpoint (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/privatelink-streams.html), $44 a month in Paired. Replication between MRSC Regions has no transfer charge (`research/dynamodb.md#29`). The request-unit guesses would have to be about 6 times too low at 50 developers, and about 20 times too low at 2,000, before this design cost more than the baseline.

## Build order from zero

**Milestone 0: measurements that would make this foundation wrong.** About three days on a scratch account, before any git code. Trios: NA (us-east-2, us-west-2, witness us-east-1), EU (eu-central-1, eu-west-1, witness eu-west-3), and the three-continent `global` trio.
- *Append.* One chain, 1 KB entries, writers in both replica Regions with group commit, at 10, 50, 100, 500 entries a second. Pass: p99 put at most 60 ms from the preferred Region at 100 a second, at least 500 entries a second sustained, conflict retries under 5%. Fail: p99 over 150 ms, or under 200 a second.
- *Tail read.* Strong single-partition Query. Pass: p99 at most 30 ms within a continent and at least 2,500 a second on one key without throttling. Fail: p99 over 80 ms or under 1,500 a second.
- *Correctness under faults.* 48 hours of appends with AWS FIS isolating each Region in turn (V2globaltables_HowItWorks, "Fault injection testing"). Knossos per chain. Pass: zero lost acknowledged entries, zero gaps, zero duplicate seqs. Fail: any one.
- *Streams.* Enabled on both MRSC replicas. Pass: identical record sets, trigger lag p99 at most 2 s. Fail: divergence or p99 over 10 s.
- *Fold engine.* A synthetic repository of 200,000 issues and 2 million comments. Pass: apply at least 5,000 entries a second per node, `labels=bug&sort=updated` p99 at most 50 ms, cold load of a 2 GB snapshot from S3 at most 30 s. Fail: list p99 over 200 ms or cold load over 120 s.
- *Global and restore.* Pass: claim p99 at most 300 ms across continents with claims continuing through each isolation; a 100 GB PITR restore in at most 4 hours [guess]. Ask AWS whether MRSC replicas support PITR and how DynamoDB deploys roll out across Regions.
- *Flip.* If append, tail read, or correctness fails, `DESIGN.md` stays the base and only this design's `global` table grafts in place of the geo authorities. If the fold engine fails, `DESIGN.md` stays the base, because this design has no other list engine.

**Milestone 1: chains and git.** One Paired cell, `commit`, `fresh`, `PackStore`, Rust receive-pack, upstream upload-pack, snapshots and archive. Gate: 64 unmodified git clients doing atomic and non-atomic pushes and fetches on one repository while nodes die and an AZ drops; Knossos per ref, Elle per push; zero lost acknowledged pushes; at least 200 pushes a second to one repository; push p99 under 1 s.

**Milestone 2: folds, issues, pull requests, merge.** Gate: journeys 2 and 3 through unmodified `gh`; the cli/cli `issue` and `pr` acceptance suite; a merge-queue group with a planted reverting squash refused by the tree-diff check.

**Milestone 3: owner chains, authorization, `global`, edges.** Gate: journey 4 with a push in flight; `gh auth login` and an immediate `gh api user` through another continent's edge; journey 6's tenfold read test against the miss budget and table maximum.

**Milestone 4: dispatcher, consumers, webhooks, Actions.** Gate: Argo CD and Jenkins react to pushes; a consumer stopped for 30 hours resumes with zero lost events; `actions/runner` passes `DESIGN.md` Milestone 5's gate.

**Milestone 5: many cells, slot moves, Region loss.** Gate: a slot moves under load with each chain's freeze under 1 s; journey 5 in managed and in a two-Region BYOC install with zero lost acknowledged writes.

**Milestone 6: installs.** Gate: journey 7 in eu-central-2 under load; an N to N+4 upgrade; journey 6's format gate; BYOC through the vendor principal; an air-gapped install whose endpoint list includes the Streams interface endpoint.

**Milestone 7: breadth**, as `DESIGN.md` Milestone 7, plus cross-cell repository transfer.

## Changes to DESIGN.md

**Deletions.**
- Summary: the DSQL `core` and `aux` bullets, the claim registry and geo authority bullet, the stale-fetch bullet, and the Milestone 0 flip text.
- Shape: "Two clusters per cell", "Why Aurora DSQL, defended against its limits" and its table, `slot_state`, and `move_slot`'s freeze, drain, and 30 s horizon.
- Namespace and cell placement: `Projection`, `apply_segment`, publishers, `Lookup`, `BEHIND_WAIT`, `MAX_LAG`, `BoardRow`, `BoardLease`, `MEMBER_LEASE`, `GeoView`, `absent_allowed`, `depends_on`, `clock_margin`.
- Geographies, the claim registry, and the authorities: the whole section (`Geo`, `Registry`, `run_claim`, `Authority`, writer leases, escrow, `RecoveryClass`, `Owned<T>`, `reconstitute`, `rejoin`, `RegistryGeneration`).
- Repository storage: the `ref`, `ref_shard`, `ref_tombstone`, `pack`, and `pack_inline` tables, `refresh`, bounded stale fetches, `dr-apply` and DSQL CDC.
- Identity and tokens: the `g_*` replica, `revoke` coverage, `RevocationReceipt`, and the geography character in tokens.
- Event outbox and Cross-cell features: `outbox`, `outbox_pending`, `ConsumerLease`, polling `dispatch`, `LogId`, `follow`, `XcellEvent`, inter-cell partitions.
- Actions: the `aux` placement and `CheckRelay`.
- Availability, Deployment modes, Install operations, Build order, Tradeoffs: every DSQL, registry, authority, replica, board, CDC, and AWS Backup row or paragraph.

**Replacements.**
- Summary, Problem, and Shape: this document's Summary and Shape.
- Repository storage and ref transactions: chains, `commit`, `Push`, `PackStore` with a 16 KiB inline limit and S3 spill.
- Issues and pull requests: chain events and SQLite folds; `merge` as one `Merged` entry; fork merges pin the reviewed head.
- Identity and tokens: owner chains for the authorization graph, `global` for what an edge resolves, the edge read-through cache; installation tokens signed by the owner's home cell and checked against the owner fold.
- Event outbox: the chain is the log; Streams trigger `Dirty` markers; per-consumer `Cursor`s.
- Namespace and cell placement: owner home cells, network cells, the slot map in `global`, moves fenced by `Sealed`.
- Availability: SLO rows for freshness, revocation coverage, and name claims become a token-cache bound (60 s) and a fold-lag alarm; failure modes and Deployment modes tables as above.
- Install operations: PITR restore, MREC `dr`, and the gap-truncating promotion replace AWS Backup, `dr-apply`, and the `ref_log` reconcile.
- Build order: this Milestone 0 to 7.

**Additions.** Crates `trib-chain` (`commit`, `fresh`, `apply`, seal), `trib-fold` (SQLite schemas, snapshots, archive, fold audit), and `trib-global` (claims, edge cache). The signed `formats` config. Compiled miss and read budgets. The CI lint that rejects Scan, transactions, and secondary indexes.

**Kept by reference.** Wire compatibility and known deviations (plus 10,000-ref atomic pushes), upstream git as the object engine, the Actions protocol and job state machine, the web app, cells and waves, change safety for deploys and config, the operator role, intents, status documents, release signing, and the BYOC machinery.

## Tradeoffs accepted

- We accept building and owning a fold engine, SQLite per chain with snapshots, archive, refold, and audit, in exchange for list endpoints with no shared query engine and no index that is eventually consistent. It is the largest new build risk, and Milestone 0 gates it.
- We accept no stale-read mode: a cell whose `log` has no quorum stops, in exchange for never serving a fold that is not proven fresh. AWS warns that fallback modes rot (`research/uptime.md#5`). Git read availability rests on the global tables SLA of 99.999% (`research/dynamodb.md#22`).
- We accept one strong read per request per chain, coalesced per node, costing one cross-Region round trip within the cell's continent [inference from V2globaltables_HowItWorks], in exchange for read-your-writes everywhere without a replica protocol.
- We accept that a fork merge merges the reviewed head even if the fork moved within the put round trip, in exchange for merges that touch one chain.
- We accept token revocation and account suspension within 60 s, in exchange for edges that read `global` only on misses and never on a TTL that a config change can shorten into an overload.
- We accept one cross-continent round trip on every managed signup, login rename, token creation, and SSH key addition [guess: 95 to 225 ms], in exchange for RPO zero on the namespace with no reconstitution after a continent loss.
- We accept that every repository-scoped write in one repository serializes on one chain, capped near 1,000 entries a second, in exchange for every input to a decision living in one fold.
- We accept multi-chain operations (fork, transfer, signup) that finish on first touch, in exchange for having no transaction across chains.
- We accept that cross-repository lists, cards, notifications, and search lag by seconds.
- We accept a witness Region that holds table data (V2globaltables_HowItWorks), which a residency customer must accept as one of its three Regions.
- We accept that DynamoDB is the single database under every cell, so an AWS-side DynamoDB regression in both Regions of a cell stops it. Cells span two Regions and managed cells avoid us-east-1 replicas.
- We accept that BYOC Paired runs only in the 15 MRSC Regions.

## Alternatives considered

- **The baseline: everything in Aurora DSQL** (`DESIGN.md`). It keeps every invariant inside one transaction, which is its strength. It loses on downtime and reach, with numbers. DSQL is offered in 20 Regions against DynamoDB's 34 [measured], so 14 Regions, Zurich included, cannot install it (journey 7). Multi-Region DSQL cannot cross continents (`research/dynamodb.md#27`), which forces three geo authorities, a claim registry, boards, writer leases, ClockBound, escrow, and `reconstitute`, about a dozen novel protocols (`DESIGN.md` "Geographies, the claim registry, and the authorities") whose bugs reach every identity write; one MRSC table replaces them. This design's whole list of distributed protocols is five, each with a gate: chain append, marker consumption, first-touch completion, seal-and-copy moves, and gap-truncating `dr` promotion. Each cluster allows 10,000 connections and 100 new a second (`research/uptime.md#27`), so an AZ loss that drops a third of a cell's 4,000 `core` connections needs about 13 s of reconnects at full rate, which is why the baseline budgets pools per role. Its git path depends on a query planner, frozen tables, and OCC on 40 shard rows, and Repeatable Read lets write skew through unless every input bumps `review_version`. DSQL reached GA on 2025-05-27 (`research/uptime.md#26`), and the baseline itself flips if AWS cannot stage engine upgrades per cluster. Atomic pushes stop at 1,000 refs because of the 3,000-row limit. Its 2,000-developer Paired install costs $24,879 against $20,636 here.
- **All DynamoDB, as round 3 and the operator's question framed it** (`arena/round3/candidate-2/DESIGN.md`, `arena/candidate-2/DESIGN.md`). It uses DynamoDB for queries, events, and freshness proofs, where it is the wrong tool. GSIs are eventually consistent (`research/dynamodb.md#11`), so `gh issue list --label bug` right after create can miss the issue (journey 3). Twenty GSIs a table (`#10`) cannot index GitHub's issue filters times three sorts and two directions, and a filter expression reads every item first, so `labels=bug` on a 200,000-issue repository costs on the order of 50,000 strong read units a page [inference]. Streams keep 24 hours and allow two readers a shard (`#13`, `#14`), so streams as the outbox lose events after a day-long consumer outage. Per-item stream order cannot prove a follower fresh, so round 3 needed deny leases, coverage waits up to 6 s, and ClockBound (`review/round3-judge.md`). Storing bytes there costs $0.25 against S3's $0.023 per GB-month with a 400 KB item cap (`#7`). This design keeps DynamoDB for commit points and tails only.
- **Aurora PostgreSQL, with Global Database for Regions.** Unplanned cross-Region failover loses seconds and is an RDS control-plane call that may not fence the old primary (`research/uptime.md#24`, `#25`). In-Region failover takes up to 60 s (`#23`). It brings back every GitHub database class at once: engine upgrades, DDL, connection storms, and expensive queries on a shared server (`#44`), the 2,500 incident-minutes of `#43`.
- **MemoryDB for queues, rate limits, or sessions.** Its shards fail over primary to replica, the class of GitHub's March 2026 Redis failover with no writable primary (`#47`). Single-item DynamoDB writes cover each use.
- **Hybrid: refs in DynamoDB, metadata in DSQL.** A merge then spans two stores and needs a saga, the metadata store keeps the database incident classes, and installs run two databases.
- **S3 conditional writes as the ref register (Continuity, walgit).** S3 has no synchronous cross-Region compare-and-swap (`research/git-storage.md` implication 6), so RPO zero needs a second mechanism, and AWS documents no hot-key CAS rate (open question 3).
- **Kinesis Data Streams for events.** It would add a store to every install for retention the per-consumer cursors already give, since the chain itself is the durable log.
- **OpenSearch as the list engine.** It is GitHub's April 2026 failure (`research/uptime.md#49`).

## Open questions and risks

- MRSC put and strong-read latency for each trio are unpublished (`research/uptime.md` open question 3). Every latency number here is a guess until Milestone 0.
- Does a strong Query that returns nothing cost one read unit, and do MRSC strong reads cost the same units as single-Region ones? `cost.py` assumes yes [unverified].
- Do monotonically increasing sort keys keep a chain's tail in one partition, so its ceiling is exactly 1,000 write units a second (`research/dynamodb.md#8`)? If adaptive capacity splits further, the ceiling rises.
- How does AWS deploy DynamoDB changes across Regions and cells? A simultaneous regression in both Regions of every cell is the one failure no bulkhead here covers, as the DSQL engine upgrade is for the baseline.
- Do MRSC replicas support PITR and AWS Backup cross-Region copy, and how long does a 100 GB PITR restore take? [unverified]
- Fold engine scale: how large does the biggest repository's fold grow (comments included), how long does its cold load take, and is SQLite's single-writer model enough at 1,000 entries a second on one fold?
- Does the 60 s token-revocation bound satisfy security teams used to GitHub's revocation, and should write-class actions re-read the token strongly at the cost of a cross-continent dependency?
- Is merging the reviewed head of a fork, not the fork's newest head, invisible to every client oracle?
- Is a witness Region holding data acceptable to residency customers, and does a witness in an opt-in Region need that Region enabled?
- How large does an edge's cache need to be at managed scale for misses to stay under the budget, and what is the cost of `global` reads at GitHub's request rate?
- Can the slot move copy a giant network's chains and snapshots fast enough that its seal-to-serve gap stays under 1 s per chain?
