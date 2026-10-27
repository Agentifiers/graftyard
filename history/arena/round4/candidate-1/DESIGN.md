# Repository logs on DynamoDB: one commit item per fork network, every other fact a view

## Summary

The bet is that each fork network is a deterministic state machine whose input log is ordered by one DynamoDB item, the network's `Head`. A push, a merge, a pull request state change, a review gate bump, or a branch-rule change commits with one conditional write on that item, which carries the two newest records inline and points to older ones. A per-network sequencer validates a batch of changes against the exact state the write is conditioned on, so many changes share one write and no transaction is needed.

Durable state lives only in DynamoDB and S3. Each cell has three tables (`repo`, `meta`, `work`). They are MRSC with two replicas and a witness in managed cells and two-Region installs, and single-Region otherwise. The code path is the same in both, because it never uses transactions, TTL, LSIs, or GSIs. Identity uses the same log type per account, org, and enterprise, in three continental MRSC tables, plus a worldwide MRSC registry for names. Pack bytes, checkpoints, archives, and projections are S3 objects.

A writer applies the views a client reads next before it acknowledges: refs, pull requests, issue and pull request lists, and timelines. Webhooks, Actions triggers, notifications, and search are consumers that read the durable log. Streams only wake them, so neither the 24-hour retention nor the two-reader limit can lose an event.

A GitHub user sees the same wire, atomic pushes up to 10,000 refs (the baseline allows 1,000), single-Region installs in any of 34 DynamoDB Regions and two-Region installs in any trio of the 15 MRSC Regions, and a revocation bound of 5 s. A maintainer runs no database cluster. There are no connections, DDL, engine versions, failover calls, or freshness boards, and four mechanisms carry every invariant: the log, newer-only views, the pump and its consumers, and projections with revocation leases. Milestone 0 measures one item's conditional-write rate on MRSC, and that test can falsify the design in a day.

## Usage (caller's view)

**What a `gh` user types** is unchanged from `DESIGN.md` ("What a `gh` user types"). Clients see GHES 3.22 because the wire layer, `trib-wire`, `trib-rest`, `trib-graphql`, and the Actions protocol are reused by reference.

**Self-hosted quickstart in a Region without Aurora DSQL.** Zurich, `eu-central-2`, has DynamoDB (https://docs.aws.amazon.com/general/latest/gr/ddb.html, read 2026-10-24) and does not appear in the DSQL Region list (`research/dynamodb.md#26`). The bootstrap, signed release, and operator role are the baseline's ("Install operations"). Only the target block changes:

```sh
tofu -chdir=bootstrap/customer apply -var product=tributary -var deploy_principal_arn=arn:aws:iam::111122223333:role/ci-deploy
platform-deploy release fetch --registry prod --release tributary-2027.03.1
cat > target.json <<'EOF'
{ "version": 1, "name": "git-zrh", "profile": "tributary-customer", "aws": { "accountId": "111122223333", "region": "eu-central-2" },
  "terraform": { "stateBucket": "platform-terraform-state-111122223333" },
  "tributary": { "hostname": "git.bank.example", "durability": { "class": "regional" },
                 "nodes": { "layout": "combined", "perAz": 1, "instanceType": "i8g.2xlarge" } } }
EOF
platform-deploy deploy --target target.json --bundle tributary-2027.03.1   # 4 tables, 3 buckets, 3 nodes, 1 NLB
tributary admin bootstrap --host git.bank.example --first-admin alice
tributary admin restore --table meta --to 2027-04-02T09:14:00Z            # journey 7, "Deployment modes"
```

A two-Region BYOC install sets `"durability": { "class": "paired", "regions": ["eu-west-1", "eu-central-1"], "witness": "eu-west-3" }`. The preflight accepts any three of the 15 MRSC Regions (`research/dynamodb.md#25`).

**Call sites inside the binary.** Every ref-tied change goes through `submit`, every metadata write goes through `write_item`, and every list goes through `list`:

```rust
let r = seq.submit(NetChange::Merge { pr, base_old, head_seen, gate_seen, commit, pack, delete_head: false }, req).await?;
views::catch_up(&t, LogId::Network(net), r.seq).await?;          // read-your-writes before the 200
let issue = meta::write_item(&t, &Key::Issue(repo, n), |old| Issue::create(old, input)).await?;   // OCC, index deltas, event
let page = meta::list::<Issue>(&t, &ListQuery::issues(repo).label("bug").state(Open).sort(Updated)).await?;
```

Journeys 1 to 4 are traced in "Commit and read paths", 5 and 6 in "Downtime", and 7 in "Deployment modes".

## Shape

**Stores.** These are all of them. Each cell's tables share the cell's durability class, and managed cells sit in their own accounts, so DynamoDB account quotas are per cell (baseline "Deployment modes", AWS accounts row).

| Store | Managed | Installs | Holds | Writers |
| --- | --- | --- | --- | --- |
| `repo` table, per cell | MRSC, two replicas plus a witness (Paired) | Paired as managed, or single-Region (Regional) with an optional MREC `dr` replica | network heads, log items, record bodies, inline packs, ref and pull request views | sequencers, `catch_up`, consumers of the `Views` group |
| `meta` table, per cell | same class as `repo` | same | issues and the issue half of pull requests, comments, reviews, reactions, labels, milestones, check runs, counters, list index entries | `write_item` only |
| `work` table, per cell | same class | same | dirty sets, cursors, pump checkpoints, leases, webhook deliveries, notifications, Actions runs, jobs, runners, cache index, operator cursors | each item has one owner role |
| `ident-na`, `ident-eu`, `ident-ap` | MRSC inside one continent | collapses to one `ident` table of the install's class | root heads and logs, root items, credentials, repository names, home-only secrets, revocation logs | identity sequencers |
| `registry` | MRSC with three full replicas: us-east-2, eu-central-1, ap-northeast-1 | absent; claims live in `ident` | login, shortcode, app-slug, SSH-fingerprint, and email-digest claims; revocation copies | `run_claim`, `revoke` |
| S3 | pack buckets (2 of 3 Regional buckets before submit in Paired), archive, control | one of each per install Region | pack bytes, checkpoints, archived log segments, clone bundles, projections, intents, status | `PackStore`, compactor, publishers, operator |
| OpenSearch, optional | per residency domain | optional | a derived search index, rebuildable from the logs | the `Search` consumer |

Nothing in any table uses `TransactWriteItems`, TTL, LSIs, or GSIs, because MRSC rejects transactions and lacks TTL and LSIs, and GSI reads are only eventually consistent (`research/dynamodb.md#6`, `#11`, `#17`, `#18`). CI rejects any call to those APIs, so the MRSC and single-Region code paths are the same calls on a different table name. The only MRSC-specific branch is a retry on `ReplicatedWriteConflictException` (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html).

### The log: one implementation, four uses

```rust
pub enum LogId { Network(NetworkId), Root(RootId), Revocations { geo: Geo, part: u8 }, Xcell { from: CellId, to: CellId } }
pub struct Seq(u64);
/// Restore epoch. A PITR cutover or a table-generation move bumps it, so caches, cursors, and webhook GUIDs from before a
/// restore can never be confused with new commits at the same seq.
pub struct Gen(u32);
/// The commit item, pk "H#<log>". At most 4 KB, so a CAS costs at most 4 WRU and a strong read 1 RRU
/// (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/read-write-operations.html).
pub struct Head { pub log: LogId, pub gen: Gen, pub seq: Seq,
    pub newest: [Option<RecordRef>; 2],            // the records at seq and seq - 1
    pub checkpoint: Option<(Seq, S3Key)>, pub archived_through: Seq,
    pub sealed: Option<Seal>,                       // Moved { to: CellId } or Deleted; a sealed head refuses every change
    pub unannounced: Option<Seq>,                   // roots only: a revocation committed here and not yet in its RV log
    pub fmt: u16 }
pub enum RecordRef { Inline(Record) /* at most 1.5 KB */, Body { id: Ulid, chunks: u8, digest: [u8; 32] } }
/// Log item: pk "L#<log>#<gen>#<seq / 1024>", sk seq, written once with attribute_not_exists. Body chunk: pk "B#<ulid>#<i>".
pub struct Record { pub gen: Gen, pub seq: Seq, pub at: Timestamp, pub fmt: u16, pub changes: Vec<Accepted> }
/// Changes carry after-images of every field a view or payload needs, captured at validation.
pub struct Accepted { pub request: RequestId, pub actor: Actor, pub change: ChangeBody }
/// Pure and deterministic. NetState for networks, RootState for identity roots, RvState for revocation partitions.
pub trait Machine { type State; type Change;
    fn validate(s: &Self::State, c: &Self::Change) -> Result<ChangeBody, Rejected>;
    fn apply(s: &mut Self::State, b: &ChangeBody); }
/// One per log per cell Region, on the rendezvous-preferred node of the log's home Region. Any node may commit, and the
/// CAS decides, so preference only saves conflicts. Requests that land in the other Region are forwarded.
pub struct Sequencer<M: Machine> { /* state at head, queue in two lanes, request-id dedupe for 10 minutes */ }
impl<M: Machine> Sequencer<M> {
    /// Batches while the previous CAS is in flight: at most 64 changes and 300 KB, the user lane before the bot lane.
    pub async fn submit(&self, c: M::Change, req: RequestId) -> Result<Receipt, Rejected> { todo!() }
}
pub struct Receipt { pub gen: Gen, pub seq: Seq, pub index: u16 }
// commit(batch) at head n:
//   1. L(n - 1) exists: it was put right after CAS n returned, while this batch filled.
//   2. Validate each change in order against state(n). A rejected change leaves the batch with its reason.
//   3. A record over 1.5 KB puts its body chunks first, overlapped with the previous CAS.
//   4. UpdateItem H SET seq = n + 1, newest = [rec, newest[0]] IF seq = n AND attribute_not_exists(sealed).
//      Lost: strong read of H, replay the gap, revalidate, retry. ReplicatedWriteConflictException: retry under the 10% budget.
//   5. Return a Receipt to each accepted change. Then put L(n).
```

Invariants the log enforces:

- **One order per log.** Every change to a network commits by a CAS on its `Head`, so seq order is commit order for every repository in the network, its forks, its pull requests, and its rules. [inference]
- **No write skew.** `validate` runs on the state at seq n, and the CAS is conditioned on seq n. A merge, a push, and a rule change that race therefore serialize, and the loser revalidates on the new state. The baseline needed `review_version` bumps and re-checks inside an optimistic transaction for the same property (`DESIGN.md` DSQL limits table).
- **Acknowledgment point.** A change is acknowledged only after its CAS returns. In Paired cells the item has then replicated to at least one other Region or the witness (HowItWorks URL above), and in Regional cells to the Region's AZs [inference: DynamoDB's documented multi-AZ storage, not re-read for this round]. `Receipt` exists only after the CAS, and every reply type takes `&Receipt`, which is the baseline's `Acked` rule.
- **Readable history.** `L(k)` exists for every `archived_through < k <= seq - 2`, and `newest` holds `seq` and `seq - 1`. A reader takes records from `newest` or from one `Query` on the log partition with `ConsistentRead`. The archiver moves log items older than both the oldest retained checkpoint and 90 days to S3 segments, verifies them, then deletes them.
- **Idempotency.** Every change carries a `RequestId`. The sequencer dedupes for 10 minutes and rebuilds the set from `newest` and the log on warm-up. Consumers key their effects by `(gen, log, seq, index)`.
- **Fence.** `Seal` is a normal CAS, so a moved or deleted network refuses every later change. That replaces the baseline's `slot_state` row.

### Network state, changes, and packs

```rust
pub struct NetState { pub refs: RefStore /* reftable per repository namespace on local NVMe */, pub repos: HashMap<RepoId, RepoLife>,
    pub prs: HashMap<PrRef, PrLog>, pub rules: HashMap<RepoId, RuleSet>, pub packs: PackSet }
pub struct PrLog { pub state: PrState, pub base: RefName, pub head: HeadSpec, pub gate: u32, pub queued: Option<QueueEntry> }
pub enum NetChange {
    Push { repo: RepoId, updates: BoundedVec<RefUpdate, 10_000>, atomic: bool, pack: Option<PackRef> },
    BotRefs { repo: RepoId, updates: Vec<RefUpdate> },     // refs/pull/*, queue refs: bot lane, last writer wins per ref
    PrOpen { pr: PrRef, base: RefName, head: HeadSpec, draft: bool }, PrSet { pr: PrRef, to: PrTransition },
    Merge { pr: PrRef, base_old: ObjectId, head_seen: ObjectId, gate_seen: u32, commit: ObjectId, pack: Option<PackRef>, delete_head: bool },
    QueueMerge { base: RefKey, base_old: ObjectId, tip: ObjectId, prs: Vec<(PrRef, ObjectId, u32)>, pack: Option<PackRef> },
    Rules { repo: RepoId, rules: RuleSet }, Packs { added: Vec<PackRef>, superseded: Vec<PackId> },
    Event(MetaEvent),   // the ordered event of a `meta` write; a review verdict or dismissal also bumps that pull request's `gate`
    Fork { repo: RepoId }, DeleteRepo { repo: RepoId }, RestoreRefs { repo: RepoId, to: Seq }, Checkpoint { seq: Seq, key: S3Key }, Seal(Seal),
}
pub struct RefUpdate { pub name: RefName, pub expect: Option<ObjectId>, pub new: Option<ObjectId> }
/// Built only by PackStore::place, after the bytes meet the cell's rule: at most 256 KB as a `P#<net>#<pack>` item in `repo`,
/// otherwise S3 PUT If-None-Match to the home bucket (Regional) or two of three Regional buckets (Paired).
pub enum PackRef { Inline(PackId), Buckets { id: PackId, set: BucketSet } }
```

- A network is the root repository and every fork, as in the baseline. All of them share one `Head`, so a pull request from `bob/widgets` into `acme/widgets` validates and commits the head ref, the base ref, and the pull request state in one record. Placement by slot, cells, and `place_fork` (including detached forks for residency) are the baseline's ("Namespace and cell placement").
- Pull request state that decides a merge is log state: open, closed, merged, draft, base, head spec, queue entry, and `gate`. Title, body, labels, assignees, and milestone are `meta` facts, because they are not tied to refs.
- The gate closes the review race. A review verdict or dismissal written in `meta` submits its `Event` before its writer acknowledges, and validating that event increments `PrLog.gate`. A merge carries the `gate_seen` its planner read, so a review acknowledged after the plan rejects the merge, and the client re-plans. A review whose event a crash delayed was never acknowledged, so no guarantee covers it until the `meta` pump submits the event. Required checks are read by the planner and stay racy, which matches GitHub and the baseline [inference].
- Packs follow the baseline's rules without change: fresh pack ids, cruft packs, a compaction lease, GC after 14 days unreachable, and S3 versioning for the backup window ("Repository storage and ref transactions"). The difference is that compaction commits `Packs` through the log, so the pack set at any seq is log state.
- The sequencer needs the network's refs, which the cache node already holds as a reftable. It also needs a small side state (pull requests, rules, packs) that checkpoints carry. A warm node becomes sequencer at once. A cold node loads the newest checkpoint from S3 and replays at most 1,000 records [guess: under 5 s for 1 million refs].

### Views and indexed items

```rust
/// Every view and index item carries the version that wrote it. A write lands only if it is newer, and a delete is a tombstone
/// that readers skip and the sweeper removes after a day. So any number of appliers converge in any order.
pub enum Version { Log { gen: Gen, seq: Seq, index: u16 }, Item(u64) }
/// Pure: the view writes of one record, from its after-images alone. Refs go to pk "R#<repo>#<hash(name) % 16>", sk name;
/// pull requests to "Q#<repo>"; timelines to "C#<parent>"; and the log-owned fields of the issue half in `meta`.
pub fn view_writes(r: &Record) -> Vec<ViewWrite> { todo!() }
/// Applies records (applied, to] for one log. The acknowledging handler calls it before it answers, and the `Views`
/// consumer calls it for everything else. It advances cursor `U#views#<log>` with a newer-only write.
pub async fn catch_up(t: &Tables, log: LogId, to: Seq) -> Result<(), StoreError> { todo!() }
pub trait Indexed { fn index_entries(&self) -> SmallVec<[IndexEntry; 16]>; fn event(&self, old: Option<&Self>) -> Option<MetaEvent>; }
/// The only writer of `meta`. (1) Conditional put on `version`, which is the commit point. (2) Index-entry deltas between the old
/// and new after-images, newer-only. (3) `submit(Event)` to the network log. (4) Return. A crash after (1) leaves `ev < version`
/// on the item, and the `meta` pump finishes (2) and (3) within seconds.
pub async fn write_item<T: Indexed>(t: &Tables, key: &Key, f: impl Fn(Option<&T>) -> Result<T, DomainError>) -> Result<T, StoreError> { todo!() }
/// Entry pk "X#<repo>#<index>", sk "<sort value>#<number>".
pub enum IndexKey { State(StateFilter, Sort), Label(LabelId, StateFilter), Assignee(UserId, StateFilter), Creator(UserId, StateFilter),
    Mentioned(UserId, StateFilter), Milestone(MilestoneId, StateFilter), PrBase(RefName, StateFilter), PrHead(HeadSpec, StateFilter) }
pub enum Sort { Created, Updated, Comments, Popularity, LongRunning }
/// One ConsistentRead Query on the most selective index for the filters, BatchGetItem with ConsistentRead of the items, drop
/// entries the item no longer lists, filter the rest in memory, and stop at 100 results or a 1,000-entry scan budget.
/// The Link header carries an opaque cursor.
pub async fn list<T: Indexed>(t: &Tables, q: &ListQuery) -> Result<Page<T>, StoreError> { todo!() }
```

- **Read-your-writes by construction.** A handler acknowledges a ref-tied change only after `catch_up` has applied its record and every earlier one. It acknowledges a `meta` write only after its index entries and event have landed. Every read path uses `ConsistentRead` on the base table and never a GSI, so any read that starts after an acknowledgment sees it, in either Region of a Paired cell, because MRSC strong reads return the latest version (HowItWorks URL).
- **Prefix per network.** `catch_up` applies every record up to the acknowledged seq. A reader therefore never sees a later change of a network without the earlier ones.
- **One version space per item.** The issue half of a pull request is one `meta` item. `write_item` writes its `meta` fields, and `catch_up` writes its log-owned fields (state, merged) with a `log_seq` guard inside the same item version. Its list entries are then derived from one after-image.
- **Counters.** Issue numbers come from `UpdateItem ADD` on `N#<repo>#number`, so a crash leaves a gap, as a deleted issue does. Stars and reactions use 16 sharded counter items, as in the baseline.
- **Check summaries** (combined status, `statusCheckRollup`) are computed on read from one `ConsistentRead` Query of `K#<repo>#<sha>`, so they need no stored view.

### Fan-out: Streams wake consumers, the log feeds them

```rust
pub enum Group { Views, Webhooks, ActionsTrigger, PrSync, Notifications, Search, Audit, Xcell }
/// The only Streams reader. One active and one standby per shard per replica Region, inside the documented limit of two readers per
/// shard (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Streams.html). It reads head images only and upserts one
/// Dirty item per subscribed group. Its checkpoint lives in `work`. After a crash it resumes with AT_TIMESTAMP one minute
/// before its checkpoint (`research/dynamodb.md#16`), and duplicates are harmless.
pub async fn pump(table: TableRef, shard: ShardId, ck: PumpCheckpoint) -> Result<Never, PumpError> { todo!() }
/// pk "D#<group>#<region>#<part>", sk log. Writer: that Region's pump, SET head = max. Deleter: the group, if head is unchanged.
pub struct Dirty { pub group: Group, pub region: Region, pub part: u8, pub log: LogId, pub head: (Gen, Seq) }
/// pk "U#<group>#<log>". Writer: the holder of the group's lease for the partition.
pub struct Cursor { pub group: Group, pub log: LogId, pub at: (Gen, Seq) }
/// For each dirty log, read records (cursor, head] from the log (never from the stream), handle each idempotently, and advance.
pub async fn consume(g: Group, part: u8, lease: Lease, h: &dyn Handler) -> Result<(), ConsumeError> { todo!() }
/// Daily, and after any pump outage over 1 hour: a parallel Scan of heads gives every log ahead of a group's cursor a Dirty item.
pub async fn reconcile(cell: CellId) -> Result<ReconcileReport, OpsError> { todo!() }
```

- Streams keep records for 24 hours and allow two readers per shard (`research/dynamodb.md#13`, `#14`). Here a stream record is only a hint that a head moved. The events are log records, which never expire before archive. A pump outage longer than 24 hours delays consumers until `reconcile` runs, and loses nothing. Consumers never read a stream, so the number of groups is not bounded by the reader limit.
- In a Paired cell each Region's pump writes its own `Dirty` items, so pumps never write the same item. A consumer's conditional delete can still meet a pump's upsert from the other Region, and it retries on `ReplicatedWriteConflictException` [inference].
- The same pump code has two other uses. On `meta` it finishes a `write_item` that a crash interrupted: an item whose `ev` is older than its `version` gets its index entries and `Event` submitted, keyed by `(item, version)` so a repeat is a no-op. On each `ident` table it writes the identity feed segments. If either stream loses records past 24 hours, the daily `reconcile` scan of `meta` items and the hourly root sweep find the gap.
- Webhook GUIDs derive from `(gen, log, seq, index, hook)`, so a redelivery keeps its GUID, and a restore never reuses one. Payloads render from the record's after-images, which is the baseline's capture rule. Delivery, redelivery, and Actions triggering are the baseline's protocols over these consumers ("Event outbox", "Actions").
- `PrSync` maintains `refs/pull/N/head` and `refs/pull/N/merge` through `BotRefs`, so bots never compete with users in a batch. The bot-lane coalescing is a graft from `arena/candidate-2/DESIGN.md` ("Repository storage").

### Identity: per-root logs, a worldwide registry, and revocation logs

```rust
pub enum Geo { Na, Eu, Ap }                       // an install has one
pub enum RootId { Account(AccountId), Org(OrgId), Enterprise(EnterpriseId) }
/// Root items (pk "O#<root>", sk "<kind>#<id>" in ident-<home>) are the root log's views, applied before acknowledgment like
/// network views. Every change commits through LogId::Root(root).
pub enum RootChange { Member { user: UserId, role: Option<OrgRole> }, TeamMember { team: TeamId, user: UserId, on: bool },
    Grant { repo: RepoId, grantee: Grantee, permission: Option<RepoPermission> }, Policy(OrgPolicyDelta),
    Credential { hash: [u8; 32], meta: Option<CredentialMeta> }, SshKey { fp: Fingerprint, on: bool },
    RepoName { name: RepoNameKey, entry: Option<RepoEntry> }, Visibility { repo: RepoId, to: Visibility },
    Installation(InstallationDelta), Suspend(bool), Profile(ProfileDelta) }
/// Derived, never chosen by the caller: any removal, narrowing, suspension, or public-to-private change is Revoke.
pub enum Class { Grant, Revoke }
/// Entry in LogId::Revocations { geo, part }, 16 partitions per geography in ident-<geo>. A Revoke is acknowledged only after its
/// entry commits there and, in managed, after a copy item commits in `registry`, the second-continent copy that `reconstitute` reads.
pub struct RvEntry { pub root: RootId, pub through: Seq }
/// Per follower (a front cell's publisher or a cell's applier), per geography it follows: the start time of the newest sync that
/// read every RV head of that geography strongly and applied every listed root up to `through`.
pub struct RvLease { pub synced_from: GeoMap<Timestamp> }
pub const WRITE_FRESH: Duration = Duration::from_secs(5);
pub const READ_FRESH: Duration = Duration::from_secs(60);
/// Node memory, built from an S3 snapshot plus segments the follower writes each second (the baseline's projection mechanism).
/// Fronts: names, routes, credentials, suspension, for every root. Cells: the owner roots of the networks the cell hosts.
pub struct Projection { pub roots: RootMap, pub lease: RvLease, pub generation: u64 }
pub enum Decision { Allow, Deny(DenyReason), Verify(SmallVec<[RootId; 2]>), Stale }   // Stale renders 503 with Retry-After
/// Pure, no I/O. Allow needs the deciding roots' geographies within WRITE_FRESH for writes or READ_FRESH for reads, by
/// ClockBound. A Deny or NotFound becomes Verify unless the root was verified at its head within the last second.
pub fn authorize(p: &Principal, a: Action, on: &ResourceRef, proj: &Projection, now: &ClockBound) -> Decision { todo!() }
/// One strong GetItem per root head in its home table, coalesced per node, then the missing records, then authorize again.
pub async fn verify(roots: &[RootId], proj: &ProjectionHandle) -> Result<(), VerifyError> { todo!() }
/// Stored tokens keep GitHub's prefix, 30 characters, and CRC32. The first character names the minting geography and the next
/// three the minting minute mod 238,328, so a front can tell a token too new for its projection from an unknown one.
pub const MINT_WINDOW: Duration = Duration::from_secs(120);
```

- **Authenticate at the front with the principal's root, authorize in the cell with the owner's root.** A push to `acme/payments` depends on facts in `acme`'s root (members, teams, grants, policies, visibility, installations) and on the pusher's credential and suspension. A cell's projection therefore holds only the roots that own its networks, and its size grows with what the cell hosts. The baseline replicated the whole authorization graph into every cell (`DESIGN.md` "Identity and tokens").
- **Grants never wait, denials verify.** A new grant commits to the root log and returns. A cell that has not yet applied it answers `Verify`, reads the root head strongly, applies the gap, and allows. A denial is therefore never wrong about a grant acknowledged before the request [inference]. Verification is coalesced per root per node, so its load scales with denied roots, not with requests.
- **Revocations are provably fresh.** `revoke` commits the root change and sets `unannounced` in the same CAS, appends the `RvEntry`, writes the registry copy, and returns. The next commit on that root first makes sure the entry exists. Allows require a lease younger than `WRITE_FRESH`, so no write decision made more than 5 s after a revocation returns can miss it. This replaces the baseline's commit-ordered DSQL logs and freshness boards, which `research/dynamodb.md#13` rules out for DynamoDB, because a stream gives no cross-item watermark (`review/round3-judge.md`, fact checks).
- **Claims** use the baseline's `run_claim` saga. A pending item goes into the home `ident` table, a conditional put on the claim goes into `registry` (the commit point), and activation creates the root's head. Names resolve at fronts from the projection. A miss verifies with one strong read of `login#<key>` in the nearest `registry` replica, coalesced per key and capped per IP, and the cap's overflow gets 503, never a wrong 404.
- **Feeds.** A pump on each `ident` table writes segments to its Region's feed bucket every 250 ms, and followers tail them. Correctness never rests on a feed. Revocations ride RV logs, and grants are verified on deny. An hourly sweep compares hosted roots' heads with the projection.
- **Continent loss.** One Region of a continental trio is covered by MRSC. Losing a whole trio (`research/uptime.md#38`) leaves that geography's identity writes stopped until a human runs `reconstitute`. It seals the geography in `registry` and rebuilds its roots from front projections, the registry's revocation copies, and the escrow of home-only rows, as in the baseline ("Geographies, the claim registry, and the authorities"). There are no writer leases. A healed trio rejoins only through `rejoin`, which records its post-seal commits as stated loss.

### Dominant access patterns traced

- **Fetch or clone.** One strong `GetItem` of `H#<net>` (1 RRU), joined only by reads issued after the request arrived, so each request sees a head no older than its arrival. Then zero to two log reads and upstream `git upload-pack` from the NVMe cache. A node issues at most one head read per network at a time, so head reads per second are bounded by nodes divided by read latency, not by clients. In a Regional cell, if the head read fails or exceeds 200 ms, the node serves its cache at its last proven head for a signed limit (default 2 hours) and pushes get 503, which is the baseline's ACT-14 rule kept with its weekly drill. A Paired cell moves traffic to the peer Region instead.
- **Push.** Pack placement, then `submit`, then one CAS shared by the batch, then `catch_up` (one round of parallel newer-only puts on ref items), then `report-status`.
- **Pull request page.** `GetItem` on `Q#<repo>` and `I#<repo>`, plus one Query each on `C#<pr>` and `K#<repo>#<head sha>`, all `ConsistentRead`. The head SHA comes from the ref view of the head branch, as in the baseline.
- **List.** One index Query plus one `BatchGetItem` per page.
- **Authorization.** Memory only, plus a strong read of a root head on `Verify`.
- **Consumers.** One Query of a `D#` partition, then log Queries per dirty log.

## Commit and read paths

**Push, up to 10,000 refs atomically (journey 1).** A developer runs `git push --atomic origin a b c` to `acme/mono` in Paired cell `c7` (us-east-2 and us-west-2, witness us-east-1). In the same minute the merge queue merges 10 pull requests, bots move 50 refs, and CI clones 2,000 times.

1. The front authenticates from its projection, routes `acme/mono` to network N in `c7`, and forwards to a node in N's rendezvous set in the caller's AZ (baseline "Namespace and cell placement"). Stores: the front's node memory.
2. `info/refs`: a coalesced strong `GetItem` of `H#N` in the local `repo` replica. MRSC strong reads need cross-Region communication (HowItWorks URL), so this costs about one round trip to the nearest quorum member [inference]. The node replays missing records from `newest` or the log and advances its reftable. Stores: `repo` (us-east-2), NVMe.
3. `receive-pack`: upstream `git index-pack --strict` into quarantine, then a connectivity check against the cache. `authorize` runs in memory with `acme`'s lease under 5 s. Rules are evaluated at the cached seq to reject early.
4. Placement. A pack of at most 256 KB becomes a `P#` item in `repo` (MRSC, two Regions). A larger pack goes to S3 with `If-None-Match: *` in two of the three Regional buckets.
5. `submit(Push { atomic: true, 3 updates })` to N's sequencer in us-east-2. The batch might hold a queued `QueueMerge`, the developer's push, and coalesced `BotRefs`. Validation checks each `expect` against state n. An atomic push is all or nothing, and a stale ref rejects only its own change with `fetch first`. One `UpdateItem` on `H#N` commits seq n + 1.
6. The receiving node calls `catch_up(N, n + 1)`, which puts three ref view items newer-only, then sends `report-status` `ok` for each ref. Durable at "done": the record inside `H#N` in two Regions (or one Region plus the witness), and the pack in DynamoDB or two buckets. Not yet done: `L(n)`, the dirty items, webhooks, Actions runs, and `refs/pull/*` updates, all of which follow from the durable record.
7. Next read: `git fetch` in either Region sees the three branches, because its head read starts after the CAS. `GET /repos/acme/mono/branches/a` reads the ref item with `ConsistentRead`. CI clones read the same coalesced head. Full clones fetch the newest bundle from S3 through `bundle-uri`, which the compactor writes per checkpoint (`research/git-storage.md` implication 11).

At about one change per second, this workload uses roughly 1% of one head's CAS rate ("Scale").

**Same-repository merge.** The planner reads the pull request view, `gate`, reviews, and required checks with `ConsistentRead`, and the protection rules from the node's state. It asks the cache node for `git merge-tree` and `commit-tree`, checks that the result's tree diff equals the pull request's diff (`research/uptime.md#49`), places the pack, and submits `Merge`. The sequencer checks that the pull request is open, that the head ref equals `head_seen`, that the base equals `base_old`, that `gate` equals `gate_seen`, and that the rules at this seq allow the merge. One record then moves the base ref, marks the pull request merged with its final head, and optionally deletes the head branch. GitHub's texts `Head branch was modified` and `Base branch was modified` map from the rejection reasons, and neither retries into success.

**Fork merge.** This path is the same, because the fork's head branch is in N's state. No saga and no second store are involved.

**Merge queue.** Queue entries are log state (`PrLog.queued`). The queue builder creates `gh-readonly-queue/*` refs through `BotRefs`, and CI runs on them. It verifies that the group's tip tree equals the base plus each pull request's diff, then submits one `QueueMerge`. That change fast-forwards the base and marks every pull request in the group merged, or fails whole if the base, any head, or any gate moved.

**Branch-rule changes** are `Rules` changes, ordered against every push and merge. Org rulesets live in the org's root and reach cells through the projection. Any tightening is `Class::Revoke`, so it binds within 5 s.

**Journey 2.** A maintainer merges `bob:fix` into `acme/widgets` while a reviewer submits "request changes" and a check run completes. The author then runs `gh pr view` and `gh pr list` at once.

1. Review: `write_item` puts the review in `C#<pr>` in `meta` (MRSC), then submits its `Event` to N, which bumps the pull request's gate, then returns. Check run: `write_item` updates `K#<repo>#<sha>`, an Actions completion written from `work`.
2. Merge: if the review's event committed before the merge's CAS, validation fails on `gate_seen`. The maintainer gets a 405 whose message says the review state changed and asks for a new attempt [inference: GitHub's exact text unverified], and a retry plans against the new review. If the merge committed first, the review attaches to a merged pull request, which GitHub allows [inference]. A check completing after the plan does not block the merge.
3. `catch_up` puts `Q#<repo>` (merged), the `meta` issue half (closed, guarded by `log_seq`), the list entries (removed from the open-state indexes, added to the closed ones), and the `merged` timeline item, then returns 200. Stores: `repo` and `meta` in us-east-2 and replicated to us-west-2, S3 or `P#` for the merge pack.
4. `gh pr view` (GraphQL `pullRequest`) reads `Q`, `I`, `C#<pr>`, and `K` with `ConsistentRead`, and sees merged, the review, and the check. `gh pr list` queries `X#<repo>#State(Open, Created)`, which no longer lists the pull request. Both hold in either Region for every write acknowledged before the reads started.

**Lists with GitHub's filters and sorts.** `GET /repos/{o}/{r}/pulls` takes `state`, `head`, `base`, `sort` (created, updated, popularity, long-running), and `direction`. `GET /repos/{o}/{r}/issues` takes `milestone`, `state`, `assignee`, `creator`, `mentioned`, `labels`, `sort` (created, updated, comments), `direction`, and `since`. Each has an index entry kind. `list` picks the most selective index (an assignee, creator, mentioned, or milestone entry, then the first label, then state with the sort), applies `since` as a range condition on the updated sort, and filters the rest in memory. A label AND over two common labels may scan up to the budget and return a short page with a `Link` to continue. GitHub's API also paginates short pages under heavy filters [guess]. The issues endpoint lists pull requests too, because their issue halves carry index entries.

**Journey 3.** `POST /repos/o/r/issues` with `labels: [bug]` runs `UpdateItem ADD` on `N#<repo>#number` (returning 812), a conditional put of `I#<repo>#812`, newer-only puts of its entries (`State(Open, Created)`, `State(Open, Updated)`, `State(Open, Comments)`, `Label(bug, Open)`, `Creator(u, Open)`), and `submit(Event)` to N, then 201. Every write goes to `meta` or `repo` in the cell's Regions. `gh issue list --label bug` queries `X#<repo>#Label(bug, Open)` with `ConsistentRead` and gets 812 first by creation time. `gh api repos/o/r/issues?sort=updated` queries `X#<repo>#State(Open, Updated)` descending and lists 812 first. Webhooks, notifications, and search see it after the `Event` record, typically within 1 s [guess].

**Authorization and revocation (journey 4).** The `acme` org (home `Na`) removes `mira`.

1. The identity handler in an NA cell submits `RootChange::Member { mira, None }` to `LogId::Root(acme)` in `ident-na` (us-east-2 and us-west-2, witness ca-central-1). The CAS sets `unannounced`.
2. In parallel it submits `RvEntry { acme, through }` to `Revocations { Na, hash(acme) % 16 }` in `ident-na`, and puts the revocation copy in `registry`, which waits on one other continent. Then the handler returns 204.
3. Every follower worldwide strong-reads the 16 NA RV heads each second, fetches `acme`'s records, and publishes a segment that nodes apply within a second.
4. `mira`'s next push authenticates at the front (her token is still valid), reaches the cell, and gets `Deny` from memory. The deny verifies against `acme`'s head and is confirmed, so she gets `403` with GitHub's text.

The stated bound is that any write-class decision made more than 5 s after step 2 returns denies, and the typical figure is 1 to 2 s [guess]. A long upload authorized before the removal is re-authorized when it is submitted to the sequencer. A cell that cannot complete an NA sync for 5 s answers `Stale` (503) for writes to NA-owned repositories and keeps serving reads for 60 s. Token revocation and suspension bind at the front by the same rule, because the front's projection holds credentials. Installation tokens remain the baseline's stateless JWS with a per-installation `not_before` that `revoke` bumps.

**What each read guarantees after a write.**

| Read | Guarantee |
| --- | --- |
| git ls-refs, fetch, clone | Every push and merge acknowledged before the request arrived (strong head read per request) |
| REST and GraphQL refs, pull requests, issues, comments, reviews, check runs, lists, timelines | Every write acknowledged before the request arrived, in any Region of the cell, as a prefix per network |
| Authorization | Grants: any acknowledged before the request (verify on deny). Revocations: any acknowledged 5 s before a write decision or 60 s before a read decision |
| Names and new tokens at the front | Any acknowledged before the request (verify on a miss inside `MINT_WINDOW` or for a name) |
| Webhooks, Actions triggers, notifications, search, stars, feeds | Eventually, p99 target 2 s [guess]. No read-your-writes |

## Scale

All numbers are [guess] until Milestone 0 measures them, except the cited AWS limits.

- **Per-network write ceiling.** One CAS is in flight per head. A conditional write costs about 5 to 10 ms single-Region and 15 to 60 ms on MRSC, depending on the distance to the nearest quorum member [guess]. That gives 100 to 200 batches per second single-Region and 17 to 67 on MRSC. At 64 changes per batch, one network carries about 1,000 to 4,000 changes per second on MRSC and 6,000 to 12,000 single-Region, limited first by CAS latency. The second limit is the partition maximum of 1,000 write units per second (`research/dynamodb.md#8`): a head of at most 4 KB costs 4 WRU per CAS, so 250 CAS per second. Cursor reports 120 pushes per second for an S3 CAS chain (`research/git-storage.md#30`), and Spokes takes about a third of a second per ref update (`#4`).
- **Ref updates.** A record body holds about 2,000 ref updates per 300 KB chunk, and an atomic push may span chunks. Ref views spread over 16 partitions per repository, about 16,000 view writes per second [inference], and a hot repository's shard count can rise through a view rebuild.
- **Clones.** Head reads per network are at most nodes divided by read latency. A 9-node rendezvous set at 10 ms is 900 strong reads per second, or 900 RRU, which is under the 3,000 per partition (`research/dynamodb.md#8`). Pack serving is node CPU, so the rendezvous set widens from 3 to 9 nodes above 64 concurrent clones, and full clones go to S3 bundles (at least 5,500 GETs per second per prefix, `research/git-storage.md#54`). The journey 1 load (33 clones per second, about 1 write per second) is under 5% of every limit.
- **Busiest repository.** Taking the lowest of the limits above, one network on MRSC carries about 1,000 pushes and merges per second, 16,000 ref updates per second, and clones limited by nodes and S3 [guess].
- **Fleet.** 3.35 billion pushes a month (`research/git-storage.md#5`) is about 1,290 per second on average, so about 4,000 at a 3x peak [guess]. Spread over millions of independent heads in about 200 cells, that is about 20 CAS per second per cell, plus about 30 WRU per push for logs, views, and dirty work, plus inline pack bytes. Each cell table stays far under the 40,000-unit default quota (`research/dynamodb.md#9`). On-demand tables absorb double their previous peak at once (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/on-demand-capacity-mode.html), and new cells start with warm throughput set to their load-tested level (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/warm-throughput.html).
- **Fan-out.** One dirty item per group per commit, about 8 x 1,290 = 10,000 writes per second fleet-wide on average. A viral repository's notifications are bounded per thread and coalesced, and they go through `Xcell` logs to index cells, as in the baseline's cross-cell features.
- **Identity.** About 300 followers in managed [guess] each read 48 RV heads per second, about 14,400 strong reads per second, roughly 300 per head, against a 3,000 partition limit. Identity reads per request are zero.

| Hot key | Load bound | Spread |
| --- | --- | --- |
| `H#<net>` | CAS at most 1 per round trip, reads at most nodes divided by latency | group commit, read coalescing, bot lane |
| `L#<net>#<gen>#<seq/1024>` | one put per CAS, at most 1.5 KB | bodies over 1.5 KB get random `B#` keys |
| `R#<repo>#<shard>` | ref view writes | 16 shards, raisable |
| `X#<repo>#State(Open, Updated)` | one move per pull request or issue update | per index kind, per state |
| `N#<repo>#number` | issue and pull request creation | a gap per crash is acceptable |
| `H#<org root>` | membership and grant changes of one org | group commit, so a 10,000-row SCIM sync is about 160 CAS |
| RV heads | revocations of one geography | 16 partitions, group commit |
| `login#<key>` | claims for one name | one item per name |
| Star and reaction counters | viral repositories | 16 shards |
| `D#<group>#<region>#<part>` | dirty marks | partitions per group (64 in managed) |

## Downtime

| GitHub incident | Evidence | This design | Effect |
| --- | --- | --- | --- |
| Database overload: tenfold reads on the auth database, expensive writes on a shared permissions cluster | `research/uptime.md#47`, `#44` | No request reads identity storage. Fronts and cells authorize from node memory, and only denials verify, coalesced per root. Identity writes are per-root logs in per-continent tables, so one org's bulk change contends only on its own head. DynamoDB has no shared cluster CPU or pool to exhaust, and throttling is per partition | removed |
| Schema change: dropped column, online migration at peak, misconfigured index, 32-bit key overflow | `#44` | No DDL exists. Record and item formats follow the two-release rule: release N reads format k + 1 and N + 1 writes it, behind a signed flag that needs every node at N. CI derives each release's reader set and fails if a release stops writing a field N - 1 reads. New access patterns are new views rebuilt from the log. Ids are 64-bit | removed |
| Engine upgrade: major-version upgrade under load, a version bug on a custom cluster | `#44` | DynamoDB exposes no engine version for us to upgrade, and AWS stages its own deploys [inference]. A regional DynamoDB fault leaves a Paired cell's other Region writable, as global table replicas outside us-east-1 stayed writable in October 2025 (`#34`). Regional cells and installs depend on one Region's DynamoDB | contained |
| Failover and partition: cross-site promotion, no writable primary after failover, undetected partial disk failure | `#40`, `#47`, `#44` | No primary and no failover operation. MRSC quorum writes, sequencer preference is not ownership, and caches are disposable | removed |
| Connection storms and congestive recovery | `#33`, `#27` | DynamoDB is per-request HTTPS with no connection quota (unlike DSQL's 10,000 connections and 100 new per second). Retry budgets of 10% and admission per network, account, and IP | removed |
| Config and deploy: LB changes, an expired certificate, a proxy rollout, a sharding-middleware redeploy | `#45` | Cells, AZ waves, signed config with last known good, certificate alarms at 30 days, all reused from `DESIGN.md` "Change safety" | contained |
| Capacity: an LB tier failed to scale at a traffic peak, client retry loops | `#48` | Static node capacity at 66% per AZ, admission control, warm throughput per cell, on-demand doubling, and per-cell accounts for DynamoDB quotas | mitigated |
| Search and DNS dependency: overloaded Elasticsearch, a data center's DNS | `#49`, `#46` | Search is a derived view off every core path. Nodes keep the last-known-good addresses of DynamoDB and S3 endpoints. A Region's DNS fault moves Paired cells to the peer | mitigated |
| Merge logic bugs: wrong squash merges in 2,092 pull requests | `#49` | The planner refuses a merge whose tree diff differs from the pull request's. The log keeps every ref value, so `RestoreRefs` repairs any branch exactly per repository | contained |

**RPO and RTO per durability class.**

| Class | Data | RPO | RTO |
| --- | --- | --- | --- |
| Paired (MRSC, packs in 2 of 3 buckets) | pushes, merges, pull request state, `meta`, `work`, identity | 0 for a Region loss | under 2 minutes for traffic, by health checks, with no control-plane call [inference: three failed 30 s checks] |
| Regional with `dr` (MREC replica) | the same | MREC replication delay, "usually a few seconds" (HowItWorks URL), and S3 replication for bucket packs | 15 to 60 minutes: a customer-signed `DrPromote` intent |
| Regional without `dr` | the same | 0 for an AZ loss. For a Region loss, the newest cross-Region backup copy (daily by default) | restore time, hours [guess] |
| Any class, operator mistake | tables | PITR to the second within 35 days (`research/dynamodb.md#19`). Ref mistakes are repaired from the log with no restore | minutes for `RestoreRefs`, hours for a table restore [guess] |
| Identity (managed) | root logs and claims | 0 for one Region of a trio or of the registry. For a whole continent, the feed lag for shared facts | one Region: seconds. A continent: until `reconstitute` |

**Journey 5: the Region with the newest writes is lost for good mid-push.**

Managed cell `c7` (us-east-2 and us-west-2, witness us-east-1): us-east-2 vanishes while a push is between steps 4 and 6.

- If the CAS had not returned, the client sees a broken connection and nothing was acknowledged. A retry through us-west-2 (fronts drop us-east-2 on its deep health check) finds the refs either unchanged or already at the new values, if the CAS replicated before the loss. git then reports success or "up to date". No outcome contradicts an acknowledgment.
- If the CAS returned, the record was on us-west-2 or the witness before success (HowItWorks URL). us-west-2 plus the witness keep quorum, so strong reads and writes continue (`research/dynamodb.md#18`, https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/bp-global-table-design.html). The pack was in a `P#` item (MRSC) or in two buckets.
- us-west-2's warm cache nodes become sequencers at their next submit. RPO is 0, and RTO is the front health-check interval.
- Re-protection: MRSC cannot add a replica (`research/dynamodb.md#18`), so the vendor creates a new table set on a new trio. It moves networks one at a time with `Seal { Moved }`, a copy of the checkpoint and the log tail, and a new head at the same seq. Each network freezes for seconds [guess]. Until that finishes, a second Region loss in the trio stops writes.

Two-Region BYOC install (eu-west-1 and eu-central-1, witness eu-west-3): the same steps, carried out by the in-account operator role with no vendor session. The customer's next step is the same table-set move, submitted as a signed intent.

**Journey 6: a tenfold identity read storm, then an unreadable record shape.**

- A bad client release multiplies requests tenfold. Fronts authenticate and cells authorize from node memory, so the storm reaches node CPU and admission control, not a store. Denied requests verify at most once per root per node per second. A front answers an expired or revoked token from its projection without a read. The February 2026 failure, a cache TTL cut that multiplied database reads (`research/uptime.md#47`), has no counterpart, because no TTL governs any identity read.
- A release starts writing `Record.fmt = 7`, which release N - 1 cannot decode. The two-release rule forbids that write until a signed flag says every node and launch template is at N, and CI fails the build if N - 1's decoder lacks format 7. If a bug still ships it, N - 1 nodes refuse the record and stop at it instead of misreading it. Swap-back to N - 1 is safe once the flag is off. Records are immutable, so recovery is to deploy a decoder or to append a corrective record, never to rewrite history. Items follow the same rule, and attributes are only added.

## Failure modes

The stateful components are the cell tables, the identity tables, S3, the git cache node (rebuildable state), pumps and consumers (checkpoints and cursors in `work`), projections (S3 plus node disk), and the search tier. The operator role is the baseline's, with its cursors in `work` and its recovery leases in the control bucket.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| repo table | instance | A request to one storage node fails or slows | SDK retry under the 10% budget. A lost CAS reply is resolved by the next strong head read. Every change is idempotent by request id |
| repo table | az | None for data. DynamoDB spans AZs | Nodes in healthy AZs keep serving. No connection pools to rebuild |
| repo table | region | Paired: one Region's endpoint. Regional: the cell | Paired: journey 5, RPO 0. Regional with `dr`: `DrPromote` to the MREC replica. Without `dr`: restore from the cross-Region backup copy |
| repo table | control-plane | None at runtime | Tables are created at install. No request or `DrPromote` calls a management API. PITR restore and re-protection do |
| repo table | bad-deploy | One cell, canary first. A bad `validate` rejects changes, and a bad view writer writes bad views | `validate` runs only in the sequencer and is differential-tested against the previous release on replayed logs before rollout. Views are rebuilt from the log. Per-AZ waves with automatic swap-back on rejection rate or CAS p99 |
| repo table | bad-config | One cell | Signed config with last known good. Table names and endpoints change only with the customer-only key (baseline) |
| repo table | noisy-tenant | One network's head and partitions | Per-network admission (about 100 changes per second per actor, concurrent push caps), bot-lane coalescing, and per-partition throttling that stays on that network's keys. A slot above 20% of cell load moves to its own cell |
| meta and work tables | instance | One request fails | Retries. `write_item` is OCC and idempotent per request, and job transitions are CAS on version |
| meta and work tables | az | None for data | As `repo` |
| meta and work tables | region | As `repo` for the same class | As `repo`. Actions leases survive in MRSC, and runners reconnect to the surviving Region |
| meta and work tables | control-plane | None at runtime | As `repo` |
| meta and work tables | bad-deploy | One cell's issues, lists, or Actions | Index entries are rebuilt from items by the `meta` pump. Item formats follow the two-release rule. Waves and swap-back |
| meta and work tables | bad-config | One cell | Signed config with last known good |
| meta and work tables | noisy-tenant | One repository's counters, one org's Actions queue | Sharded counters, concurrent-job limits per org and repository, and `work` in its own table so Actions churn never throttles `repo` partitions |
| identity tables | instance | One identity write or verify fails | Retries. Claims are idempotent by saga id, and root changes by request id |
| identity tables | az | None | Nothing to do |
| identity tables | region | One Region of a continental trio or of `registry` | MRSC keeps quorum on the other two. A returning Region rejects strong reads until it catches up, so followers read another replica |
| identity tables | control-plane | None at runtime | Created once. `reconstitute` and a new trio are planned work |
| identity tables | bad-deploy | Identity writes for one geography's roots. Pushes, merges, and reads continue on projections | Identity sequencers roll one geography per wave, AP first. RV and root `validate` are differential-tested on replayed logs |
| identity tables | bad-config | Identity writes from the cells that carry it | Signed config, with the identity handler rolling in cell waves |
| identity tables | noisy-tenant | One org's SCIM storm, a signup flood | Group commit per root, write buckets per account, IP, and ASN, claim limits at the front, and per-root heads so one org never slows another |
| S3 buckets | instance | Single requests return 5xx or SlowDown | Retries with jitter. Keys spread by network id |
| S3 buckets | az | None. S3 Standard spans at least three AZs (`research/git-storage.md#54`) | Nothing to do |
| S3 buckets | region | Paired: one of three pack buckets. Regional: large pushes and cold cache fills | Paired: 2-of-3 placement continues. Packs up to 256 KB go inline in `repo` either way, and warm caches keep serving |
| S3 buckets | control-plane | None at runtime | Buckets are pre-created, and presigned URLs are computed locally |
| S3 buckets | bad-deploy | Deleted or bad packs in one cell | `index-pack --strict` before placement, a bucket policy that requires `If-None-Match`, GC only after 14 days unreachable, versioning for 35 days, GC in the last wave |
| S3 buckets | bad-config | One cell's placement fails before acknowledgment | A readiness probe writes and reads a canary object in every bucket |
| S3 buckets | noisy-tenant | One network's prefix | Push bytes per second per account and network |
| git cache node | instance | In-flight requests on that node. A sequencer move costs one strong read | Rendezvous sets keep each network warm on one node per AZ, and any warm node sequences. NLB removal capped at 33% |
| git cache node | az | One third of nodes | 66% static sizing, zonal shift, weekly autoshift practice (`research/uptime.md#2`, `#20`) |
| git cache node | region | Paired: that Region's fleet | The peer fleet is sized for full load and already replays every log |
| git cache node | control-plane | Dead nodes are not replaced while EC2 launch is impaired | N + 1 static capacity. No recovery step launches anything |
| git cache node | bad-deploy | One AZ of one cell | Binary hot swap with swap-back. Git versions pass a differential pack test |
| git cache node | bad-config | One AZ of one cell | Per-AZ config with abort on cache-miss rate or upload-pack latency |
| git cache node | noisy-tenant | One network's rendezvous set | Per-repository upload-pack concurrency of 64, widened sets, bundle offload, cgroups per network |
| pumps and consumers | instance | Wakeups for one shard or one group partition pause for a lease timeout | The standby pump takes the shard within 10 s. Consumer leases move, and cursors make the work resumable |
| pumps and consumers | az | Leases held in that AZ move | As `instance` |
| pumps and consumers | region | Paired: that Region's pumps | The other Region's pumps write their own `Dirty` items for every head |
| pumps and consumers | control-plane | None | Streams and DynamoDB data APIs only |
| pumps and consumers | bad-deploy | One group in one cell stops or mishandles records | Groups are independent and parked per log after 10 failures. Effects are idempotent by `(gen, log, seq, index)`, so a fixed release replays from the cursor |
| pumps and consumers | bad-config | One cell's groups | Signed config with last known good. `reconcile` restores any missed wakeup |
| pumps and consumers | noisy-tenant | A viral repository's fan-out | Per-thread notification caps, per-hook-owner delivery concurrency, and fair share across logs per partition |
| projections | instance | One node reloads its projection | The node boots from the newest S3 snapshot plus segments |
| projections | az | None | S3 holds them |
| projections | region | Paired: that Region's publisher | The peer Region's follower publishes, and the RV lease is per follower |
| projections | control-plane | None | S3 data APIs only |
| projections | bad-deploy | A publisher writes bad segments for one cell or front cell | Edges refuse segments that move more than 5 slots or remove more than 1,000 names without a signed record (baseline `apply_segment`). Waves move one front cell at a time |
| projections | bad-config | A follower that cannot sync RV | Its lease lapses, so writes for the affected geography get 503 after 5 s and reads after 60 s, never a wrong allow |
| projections | noisy-tenant | A random-name or random-token flood | Verify reads coalesced per key and capped per IP and per front node, then 503. Known revoked or expired tokens never verify |
| search tier | instance | Search queries slow down | Multi-AZ domains. Only search endpoints depend on it |
| search tier | az | Reduced search capacity | Multi-AZ domains |
| search tier | region | That residency domain's search | `incomplete_results: true` for the missing domain |
| search tier | control-plane | None at runtime | Domains are pre-created. A rebuild is a new index replayed from the logs |
| search tier | bad-deploy | Wrong results in one domain | A new index and an alias flip (`research/byoc-grounding.md#41`) |
| search tier | bad-config | One domain | Alias flip back |
| search tier | noisy-tenant | A botnet driving query cost (`research/uptime.md#49`) | Per-user and per-IP query cost limits at the front |

## Deployment modes

One signed release and one operator role, as in the baseline ("Deployment modes", "Install operations"). The table states what varies.

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| repo table | One per cell in the cell's account. Paired (MRSC, two replicas and a witness) by default | One. Paired (two Regions plus a witness from the MRSC 15) by default, or Regional with optional MREC `dr` | One. Regional by default, Paired optional |
| meta and work tables | One each per cell, same class as `repo` | Same as `repo` | Same as `repo` |
| identity tables | `registry` (three continents) plus `ident-na`, `ident-eu`, `ident-ap`, in identity accounts | One `ident` table of the install's class holding roots, claims, and 16 RV partitions. No registry, no geography | Same as BYOC |
| S3 buckets | Pack buckets in three Regions per Paired cell, archive, control | Paired: three pack buckets. Regional: one, plus a `dr` bucket with replication if set | Same as BYOC |
| git cache node | The `git` role per cell Region, three AZs at 66% | Combined roles on 3 to 9 nodes per Region | Combined roles on 3 nodes, or 1 for evaluation |
| pumps and consumers | `worker` role per cell, one pump pair per shard per replica Region | `worker` on the install's nodes | Same as BYOC |
| projections | Front cells and cells, each with its own follower and control bucket | One follower writes one projection for the install's edges and authorization | Same as BYOC |
| search tier | OpenSearch per residency domain | Optional OpenSearch in the home Region | Same as BYOC |

**What a self-hoster operates.** Four DynamoDB tables, three buckets, nodes, and an NLB. There is no database server, no cluster, no connection quota, and no engine to upgrade. The preflight checks DynamoDB, S3, and the instance type in each install Region. For Paired it checks that the trio is in the MRSC 15. It checks no DSQL.

**Backups and point-in-time restore.** Every table has PITR with 35 days and deletion protection, and the deploy policy denies `DeleteTable` and `DisableKey` as the baseline's denies stateful deletes. S3 versioning keeps noncurrent pack versions for 35 days. PITR always restores to a new table (`research/dynamodb.md#19`). A restored global-table replica becomes an independent table (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Point-in-time-recovery.html), and a restore does not copy stream, TTL, PITR, or deletion-protection settings (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/pointintimerecovery_restores.html). So `tributary admin restore` runs as two customer-signed intents, as in the baseline. The first restores the chosen tables to time T and reports their names. The second re-enables streams, PITR, and deletion protection, bumps `Gen` in a new config generation, and points nodes at the new tables. Nodes drop caches and projections when `Gen` changes. For a Paired install, a restored table is single-Region, and an MRSC table can be created only empty (`research/dynamodb.md#18`). The restore therefore copies items into a new empty MRSC table set with the same seal-and-move used for re-protection [inference]. Cross-Region restore is available for a lost home Region (same restores URL). A daily AWS Backup copy goes to a second Region by default [inference: AWS Backup copies DynamoDB backups across Regions; Milestone 0 confirms].

**Journey 7: a single-Region self-host where DSQL is not offered.** Zurich (`eu-central-2`), class Regional.

1. Install: the quickstart in "Usage" creates `repo`, `meta`, `work`, and `ident` as single-Region on-demand tables with streams, PITR, and deletion protection, three buckets with versioning, three i8g nodes, and an NLB. Every store is in `eu-central-2`.
2. Mistake: an admin's script calls the API and deletes 40 repositories and force-pushes `main` in 12 others. Deletes and force pushes are log records, so `tributary admin repo-restore` submits `DeleteRepo` reversals and `RestoreRefs { to }` computed from the log. That needs no table restore, and objects survive under GC's 14-day rule.
3. Mistake: a bad maintenance script overwrites 200,000 `meta` items. `tributary admin restore --table meta --to T` restores `meta` alone to T, because `meta` does not order against refs and views rebuild from the log. The cutover resets the `Views` cursors of the cell to each network's newest checkpoint, so `catch_up` rewrites the log-owned fields of pull request items to the log's current state. Writes to `meta` after T are lost and reported, and their `Event` records stay in the log as the list of what was lost. A whole-install restore to T uses one T for all four tables, and the cutover bumps `Gen`.
4. Afterwards the next read sees state at T for restored tables. A cache whose `Gen` is older drops itself, and webhook GUIDs after the restore differ from before because they include `Gen`.

**Regions each mode can run in.** Managed Paired cells and two-Region installs use any three of the 15 MRSC Regions (`research/dynamodb.md#25`), cross-continent sets included. Regional cells and installs run in any of the 34 commercial DynamoDB Regions (`#24`), with `dr` in any second DynamoDB Region [inference: MREC Region list not re-read]. That covers Sydney, Sao Paulo, Zurich, Cape Town, and Jakarta, which have no DSQL multi-Region or no DSQL at all (`#26`, `#27`). Managed identity uses the three continental trios and the registry trio named above.

## Cost

`cost.py` in this directory imports `TIERS`, `ACTIVITY`, prices, and every non-database line from `scripts/cost_model.py`. It drops every DSQL line and the DSQL interface endpoints, because DynamoDB and S3 use free gateway endpoints. It adds DynamoDB writes (about 111 WRU per push, mostly the 40 KB inline pack written and later deleted; 35 per CI job; 17 per metadata write), reads, storage at 1.3 times the baseline's row volume, PITR on every replica, Streams reads, and S3 requests without freshness boards. Paired bills rWRU in both replica Regions at the WRU price (`research/dynamodb.md#28`). Baseline figures are `cost_model.model(tier, topo, partitions=16)` with GFS backups, the two fixes the rubric names. Run `python3 arena/round4/candidate-1/cost.py`:

| Developers | Topology | Baseline after fixes | This design | DynamoDB lines | Difference |
| --- | --- | --- | --- | --- | --- |
| 50 | single-Region (regional) | $1,433 | $1,330 | $68 | -$103 |
| 50 | two-Region (paired) | $2,485 | $2,336 | $95 | -$149 |
| 300 | single-Region | $3,103 | $2,664 | $130 | -$439 |
| 300 | two-Region | $5,062 | $4,600 | $209 | -$462 |
| 2,000 | single-Region | $16,384 | $12,929 | $633 | -$3,455 |
| 2,000 | two-Region | $24,879 | $21,543 | $1,145 | -$3,335 |

The store is under 6% of every total. EC2 nodes, unchanged from the baseline, are 47 to 65%. The savings come from DSQL polling and lease renewals (the outbox is gone), DSQL backups (PITR is cheaper per GB), and the interface endpoints. Every unit count is a guess that Milestone 0 measures. Doubling every DynamoDB unit moves no total by more than 6%. The managed identity layer adds about 14,400 strong RV reads per second [guess], about $4,700 a month at the read price plus cross-Region transfer, and no freshness boards (the baseline estimates $27,000 a month for those).

## Build order from zero

**Milestone 0: falsify the foundation in a day.** A driver with no git code runs the `commit` loop against one item on a single-Region table in us-east-2 and on MRSC with us-east-2 and us-west-2 replicas and a us-east-1 witness, then on a cross-continent trio.

| Measurement | Pass | Fail |
| --- | --- | --- |
| Sustained CAS per second on one 4 KB head, one writer, MRSC | at least 40, p99 at most 80 ms | under 20, or p99 over 200 ms |
| Same, single-Region | at least 100, p99 at most 25 ms | under 50 |
| Changes per second with group commit (64 per batch), MRSC | at least 1,000 | under 400 |
| Throttles on one head at 125 CAS per second plus 1,800 strong reads per second, 1 hour | 0 | any sustained throttling under 80 CAS per second |
| Strong `GetItem` p99 on the head, MRSC | at most 30 ms | over 100 ms |
| `ReplicatedWriteConflictException` rate with forwarded sequencing, writers in both Regions | under 0.1% | over 1% |
| Push acknowledgment p99 including pack item and `catch_up`, MRSC | at most 250 ms | over 600 ms |
| Pump wakeup lag p99 (CAS to `Dirty` item), standby takeover | at most 1 s, takeover at most 10 s | over 5 s |
| RV sync from two other continents, 16 heads per geography, every second | complete in at most 1 s at p99.9 | over 2.5 s, which breaks the 5 s bound |
| PITR restore of a 100 GB table | at most 4 h | over 12 h |

If CAS throughput or the throttle test fails, a busy repository cannot ride one item, and the design must shard the network log (the baseline's DSQL rows or `arena/candidate-2`'s S3 log become the base). If the RV test fails, the revocation bound rises to the measured figure plus 2 s, or the identity layer returns to the baseline's DSQL authorities.

**Milestone 1: the log under unmodified git.** `Log`, `Sequencer`, `NetState`, `PackStore`, Rust receive-pack, and upstream upload-pack on one Regional cell and one Paired cell. Gate: 64 unmodified git 2.51 clients pushing `--atomic` and non-atomic, contending on `main` and on distinct branches, plus clones, while a checker proves linearizability from the log and kills sequencer nodes and an AZ. Zero lost acknowledged pushes.

**Milestone 2: views, `meta`, read-your-writes.** `catch_up`, `write_item`, `list`, the review gate, merges, fork merges, and the merge queue. Gate: the cli/cli acceptance suite for `issue` and `pr`, plus a harness that runs journeys 2 and 3 in a loop with reads from the other Region and asserts every read-your-writes row of the guarantee table.

**Milestone 3: identity in an install.** One `ident` table, root logs, RV, projections, verify on deny, OAuth, and tokens. Gate: GCM, `gh auth login`, and a removal loop asserting that no write is allowed more than 5 s after `revoke` returns.

**Milestone 4: fan-out.** Pumps, `Dirty`, consumers, `reconcile`, webhooks, and Actions on the v2 flow (baseline Milestone 5 gate). Gate: Argo CD and Jenkins discover pushes and pull requests, and a 30-hour pump outage loses no event.

**Milestone 5: Paired and Region loss.** Gate: a staging Region is cut off at the network layer under continuous pushes and merges. Zero lost acknowledged writes, traffic recovered by health checks alone, then a table-set move to a new trio.

**Milestone 6: one artifact in three modes.** Baseline Milestone 6 gates with these substitutions: install in `eu-central-2`, PITR restore of `meta` and of the whole install under load, and repository restore from the log.

**Milestone 7: managed worldwide.** `registry`, three geographies, front cells, slots, network moves by `Seal`, and `reconstitute` drills.

## Changes to DESIGN.md

**Deletions.**

- Title and Summary: every statement that DSQL holds refs, packs, pull requests, issues, the outbox, or the authorities.
- "Shape": the two DSQL clusters per cell, the whole "Why Aurora DSQL" table of limits, `run_tx`, and the 3,000-row and 1,000-ref caps.
- "Namespace and cell placement": `SlotState` and its fence rows, `Lookup::Behind`, `clock_margin`, `BoardRow`, `MEMBER_LEASE`, `BoardLease`, `MAX_LAG`, and `absent_allowed`.
- "Geographies, the claim registry, and the authorities": the three DSQL authorities, writer leases, `GeoView`, `Quiet`, `Blind`, and the follower watermark rules.
- "Repository storage and ref transactions": `ref`, `ref_shard`, `ref_tombstone`, `pack_inline`, `ShardVersions`, and `refresh`'s 40-row read.
- "Event outbox": every type and the 256 partitions. "Cross-cell features": the DSQL `Append` head rows.
- Failure modes: every DSQL `core`, DSQL `aux`, Geo authority, Cell replica, Freshness boards, and Inter-cell log row.
- "Install operations": AWS Backup for DSQL, `dr-apply`, CDC, and the `ref_log` reconcile.
- Build order: the DSQL Milestone 0 load gate, failure gate, and authority gate.
- Tradeoffs and Open questions: every DSQL item.

**Replacements.**

- Shape: the five stores of this document's store table replace `core`, `aux`, the authorities, and the boards. `Head`, `Sequencer`, and `Receipt` replace `commit_push` and `Acked`. `view_writes`, `catch_up`, and `write_item` replace `Change<T>` and the outbox.
- Pull request model: `PrLog` in the log plus the issue half in `meta`, with the review gate (`PrLog.gate`, bumped by review events) replacing `review_version`.
- Identity: per-root logs, RV logs, `RvLease`, and `verify` replace authorities, watermarks, and coverage waits. The revocation bound changes from 5 s coverage to the 5 s `WRITE_FRESH`. Front and cell projections keep `apply_segment` and `NameEpoch`.
- Bounded stale fetches while `core` is down become unnecessary in Paired cells and are kept in Regional cells: caches serve fetches for the signed limit when the head read fails.
- Durability classes: Paired is MRSC; Regional is single-Region with optional MREC `dr`. Restore is PITR plus `Gen`. Region lists follow "Deployment modes".
- Downtime and Failure modes: this document's tables. Cost: `cost.py`. Milestone 0: this document's table.

**Additions.**

- The bot lane, `RestoreRefs`, atomic pushes up to 10,000 refs, `MINT_WINDOW` token encoding, `reconcile`, table-set moves for re-protection, and CI bans on transactions, TTL, LSIs, and GSIs.

## Tradeoffs accepted

- We accept one CAS chain per fork network, about 1,000 changes per second on MRSC [guess], in exchange for one order over refs, pull requests, rules, and gates with no transaction. A network that outgrows it cannot be split without giving up one-record fork merges.
- We accept a sequencer per network that is a performance preference, not an owner. Two nodes may race, and the loser revalidates, in exchange for no leases on the commit path.
- We accept that a push acknowledgment waits for `catch_up` (one more MRSC round trip) in exchange for read-your-writes on every list and page with no read-side watermark.
- We accept that required checks can complete after a merge is planned, as on GitHub, while reviews cannot race a merge because their events bump the gate.
- We accept a 5 s revocation bound and that a follower cut off from a geography's RV heads fails writes for that geography's repositories after 5 s and reads after 60 s. In exchange, no request reads an identity store and a tenfold read storm reaches no database.
- We accept two writes per `meta` change (the item, then its `Event` record) and a gap-repair pump for crashes, in exchange for one durable, ordered event source per network.
- We accept no automatic fence for a cut-off continent. `reconstitute` waits for a human to declare the loss, so a lost continent's identity writes stay stopped longer than in the baseline.
- We accept that MRSC cannot add a replica, so re-protecting after a permanent Region loss is a table-set move over days.
- We accept that inline packs cost $0.25 per GB-month in DynamoDB until compaction moves them to S3, in exchange for small pushes that are RPO 0 with one store.
- We accept DynamoDB as the only database, tied to AWS, in exchange for running in 34 Regions with no database to operate.

## Alternatives considered

- **The baseline: refs as DSQL rows.** It gives full SQL and transactions. It lost on downtime: one engine under every cell with unstaged upgrades (`DESIGN.md` "Tradeoffs accepted"), connection quotas, DDL migrations, 20 Regions, and freshness boards. Its 40 shard rows per repository serialize under OCC, where group commit batches.
- **An S3 CAS manifest per repository with metadata elsewhere** (`arena/candidate-2/DESIGN.md`, `research/git-storage.md#29`-`#31`). S3 has no cross-Region CAS, so RPO 0 needs a second mechanism, and pull request state sits in another store. It lost on Region loss and on its per-repository ceiling near 120 pushes per second.
- **One DynamoDB item per ref with per-ref conditional writes.** It gives GitHub's 2026 per-ref ordering (`research/git-storage.md#6`). It lost because an atomic multi-ref push or a merge needs several items, and MRSC has no transactions (`research/dynamodb.md#6`).
- **`TransactWriteItems` on single-Region tables, items on MRSC.** That is two code paths whose invariants differ by mode. It lost on testability.
- **DynamoDB Streams as the log.** Streams keep records 24 hours, order only per item, and allow two readers per shard (`research/dynamodb.md#13`, `#14`). It lost because the log must outlive every consumer outage.
- **Kinesis Data Streams for fan-out.** It has longer retention and more readers, but it is another store with per-shard hot keys. `Dirty` items plus the durable log need no new service.
- **One commit item per repository instead of per network.** Fork merges would span two items, with no transaction to join them. It lost on fork merge atomicity.
- **The whole identity tier in one worldwide MRSC table** (`arena/round3/candidate-2/DESIGN.md`). Every identity write would wait on another continent (`review/round3-judge.md`). Continental trios keep writes inside a continent, and only claims and revocation copies cross oceans.
- **Leased single-writer sequencers.** Leases would add a protocol to the commit path. The CAS alone is correct.

## Open questions and risks

- MRSC conditional-write and strong-read latency per trio are unpublished. AWS says only that both need cross-Region communication (HowItWorks URL). Does a nearby witness serve as the quorum partner for writes from its neighbor? Every per-network number rests on Milestone 0.
- Does adaptive capacity hold one 4 KB item at 125 CAS per second plus 1,800 strong reads per second without throttling? `research/dynamodb.md#8` caps one item at the partition maximum.
- What is DynamoDB Streams' lag on MRSC replicas, and does `AT_TIMESTAMP` behave on shards split after the timestamp?
- How long does a PITR restore of a 1 TB table take, and does AWS Backup's cross-Region copy cover the four tables in every Region an install uses?
- How large do sequencer side states and checkpoints grow for networks with 100,000 forks, and how fast does a cold node warm?
- Is a 5 s revocation bound acceptable to enterprise buyers, and how often does a cross-ocean RV sync miss its 1 s target?
- Is a scan budget of 1,000 entries enough for multi-label issue filters on repositories with 200,000 issues, or does GitHub return fuller pages than `list` does?
- A regional DynamoDB fault still stops Regional cells and installs whole, as the October 2025 event stopped us-east-1 (`research/uptime.md#32`). Only Paired contains it.
- Does the MREC `dr` replica's last-writer-wins resolution need a `Gen` bump on every head at promotion to keep a returning home Region's unreplicated tail from overwriting newer heads? This design assumes promotion bumps `Gen` and that readers ignore older-`Gen` records [inference].
