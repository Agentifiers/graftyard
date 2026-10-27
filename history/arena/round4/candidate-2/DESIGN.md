# Home-Region item collections on DynamoDB, with a synchronous per-cell journal for Region loss

## Summary

The bet: every repository and every account lives in item collections of single-Region DynamoDB tables in its home Region, every invariant is one `TransactWriteItems` of at most 100 items and 4 MB (`research/dynamodb.md#1`), and the one write path that needs more (atomic pushes above 44 refs) uses intents behind a single commit item. Region loss is covered by a synchronous per-cell journal in an MRSC table: a prepare record before the home transaction and a commit mark after it, applied continuously to a standby copy in a peer Region. No commit passes through a single per-repository item or an MRSC item per repository, and no log orders a repository.

Stores: DynamoDB (cell tables in the home Region, the journal and standby in Paired cells, plus an MRSC claims table and an MREC directory in managed), S3 for pack bytes, NVMe caches on nodes. No Aurora DSQL, no Aurora, no self-run database.

A GitHub user notices nothing on the wire. Paired pushes and merges cost two extra MRSC round trips. After a home Region dies, the cell answers 503 for about one to two minutes and then serves every acknowledged write from the peer. Atomic pushes go up to 5,000 refs.

A Tributary maintainer notices: no SQL, no DDL, no connection pools, no engine upgrades, one `commit(Write)` call for every authoritative change, point-in-time restore of a single repository, and self-hosted installs in all 34 commercial DynamoDB Regions instead of DSQL's 20 (`research/dynamodb.md#24`, `#26`). The price is a journal protocol, an intent protocol, a list-index scheme, and full standby capacity in the peer.

## Usage (caller's view)

**GitHub user.** Every command in `DESIGN.md` "Usage" works unchanged against `tributary.dev` or an install host. What a user can observe:

- `git push --atomic` with more than 44 refs takes the intent path, about 1 s for 1,000 refs and at most 5,000 refs [guess on the cap]. A concurrent push to one of those refs waits up to 2 s and then gets `failed to lock` for that ref.
- When a cell's home Region is lost, its writes return 503 with `Retry-After` for about 1 to 2 minutes [inference, gated in Milestone 0]. Then every acknowledged push, merge, comment, and grant is there. A push that never printed `ok` must be pushed again.
- `gh issue list --label bug`, `gh pr list`, and `gh pr view` read their own writes from any front. `/search/*` stays eventually consistent, as on GitHub.

**Maintainer.** Three call sites define the interface. Everything else is built from them.

```rust
// 1. A merge in the api role. One call writes the prepare record, the home transaction, and the mark, then acks.
let plan = git.plan_merge(&pr, method).await?;                       // tree-diff check already passed (research/uptime.md#49)
let w = Write::new(req.id)
    .check(pr.head_guard(), Expect::At(plan.head_v))                // fork's head ref, or refs/pull/N/head when detached
    .put(Ref::advance(&base, plan.commit)?, Expect::At(plan.base_v))
    .put(pr.merged(actor, &plan)?, Expect::At(pr.v))                // review_version lives in pr.v
    .check(Key::Checks(repo, plan.head, CheckPart::Summary), Expect::At(plan.checks_v))
    .lists(ListMove::of(&pr, State::Merged))
    .emit(Event::pull_request_closed(&pr, &plan)).emit(Event::push(&base, &plan));
match cell.commit(w).await {
    Ok(acked) => render_merged(acked),
    Err(CommitError::Stale(k)) => github_text(k),                   // "Head branch was modified..." and the others
    Err(e) => retry_or_503(e),
}
// 2. A strongly consistent list page.
let page = cell.list(repo, ListDim::Label("bug".into()), ListSort::Created, Dir::Desc, State::Open, cursor).await?;
// 3. A fetch: refresh the node's cache against the 40 head items, then run upstream upload-pack locally.
let delta = cell.refresh(repo, &cache.heads).await?;
```

**Operator.** The install machinery of `DESIGN.md` ("Usage", "Install operations") is reused. The target's `tributary` block names `regions { home }` or `regions { home, peer, witness }`, and the CLI gains:

```sh
tributary cell status                                     # home, epoch, lease age, journal and standby lag
tributary cell switchover --to eu-central-1               # planned home change, RPO 0, about 10 s of 503
tributary cell hold --until 2027-03-01T12:00Z             # signed: suspend automatic promotion
tributary admin restore --repo acme/payments --to 2027-03-01T09:58:00Z   # one repository, point in time
tributary admin restore --install --to 2027-03-01T09:58:00Z              # every table, point in time
```

## Shape

Sources used below, read on 2026-10-24: [TX] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/transaction-apis.html, [GT] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html, [LSI] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/LSI.html, [AC] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/burst-adaptive-capacity.html, [ST] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Streams.html, [RW] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/read-write-operations.html, [WT] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/warm-throughput.html, [PITR] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/PointInTimeRecovery_Howitworks.html, [EXP] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/S3DataExport.HowItWorks.html, [KDS] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/kds.html, [CB] https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/compare-timestamps-with-clockbound.html. `dyn#N`, `up#N`, and `gs#N` are findings in `research/dynamodb.md`, `research/uptime.md`, and `research/git-storage.md`.

A **cell** is one AWS account with a home Region, and in Paired form a peer Region and an MRSC witness Region. Cells share no table. Slots, `DbId`, `RESERVED`, `NameEpoch`, the S3 slot map, and the front cells come from `DESIGN.md` "Namespace and cell placement" unchanged, except that a slot maps to a cell whose home can change.

| Store | Form | Holds | Only writer |
| --- | --- | --- | --- |
| Cell tables (`repo`, `refs`) | Single-Region DynamoDB in the home Region, on demand, PITR, deletion protection | Every authoritative fact of the repositories and accounts homed in the cell | `CellStore::commit` |
| `eph` table | Single-Region, TTL, no PITR | Runner sessions, Actions cache index, delivery attempts, rate counters, node leases | The roles, directly |
| Journal | MRSC per Paired cell: home and peer replicas plus a witness | Prepare records, commit marks, the `HomeLease` | `commit`, the lease keeper, the promoter |
| Standby tables | Single-Region copies of `repo` and `refs` in the peer | After-images of committed writes | The standby applier |
| Claims table (managed) | MRSC with replicas in us-east-2, eu-central-1, ap-northeast-1 | Logins, SSH key fingerprints, enterprise shortcodes | The claim saga |
| Directory (managed) | MREC with a replica in every front Region | A routing and credential projection, never authoritative | Each cell's `DirPublish` consumer |
| S3 pack buckets | Home and peer buckets, plus a witness-Region bucket used only while one is down | Pack bytes above 32 KiB, bundles, LFS | The git role |

```rust
// ---------- Placement. Ids, slots, the slot map, and NameEpoch are DESIGN.md's.
pub struct CellId(u16);
pub struct Epoch(u32);                        // +1 on every change of a cell's home; stamped in every item and journal record
pub enum Pairing { Single { home: Region }, Paired { home: Region, peer: Region, witness: Region } }
/// A network's home is its root owner's cell. A fork joins its network's cell unless residency forbids it.
pub enum ForkPlacement { WithNetwork(CellId), Detached(CellId) }

// ---------- Items. One enum holds every key, so every access pattern is visible here and nowhere else.
pub enum Table { Repo, Refs, Eph }            // only Refs has an LSI (on wv), so only ref shards are pinned to a partition [LSI]
pub enum Key {
    RepoMeta(RepoId),                         // pk R#{repo}  sk META: name epoch, visibility, network, default branch, frozen
    Rule(RepoId, RuleId), Grant(RepoId, Grantee), Hook(RepoId, HookId),   // pk R#{repo}  sk RULE#, GRANT#, HOOK#
    Counter(RepoId, CounterKind),             // pk R#{repo}  sk CTR#issue: the issue and PR number allocator
    Lease(RepoId, LeaseKind),                 // pk R#{repo}  sk LOCK#bigpush, MQ#{branch}, MAINT
    BigPush(RepoId, PushId),                  // pk R#{repo}  sk BP#{push}: the single commit item of an intent push
    Head(RepoId, Shard),                      // pk RH#{repo} sk {shard}: 40 per repository, 32 user and 8 refs/pull/*
    Ref(RepoId, Shard, RefName),              // Refs table, pk RF#{repo}#{shard} sk {name}, LSI on wv
    Push(RepoId, PushId),                     // pk P#{repo}#{day}: (ref, old, new) list, actor, pack ids. The reflog
    Pack(NetworkId, PackId), PackInline(NetworkId, PackId),   // pk K#{network}; inline only when <= 32 KiB
    Issue(RepoId, IssueNumber), Child(RepoId, IssueNumber, ChildId),  // pk IS#{repo}#{n}: issue or PR, comments, reviews, timeline
    List(RepoId, ListDim, ListEntryKey),      // pk IX#{repo}#{dim} sk {kind}{state}#{sort}#{value}#{n}; labels, base projected
    ListCount(RepoId, ListDim, State, u8),    // 8 counter shards per (dim, state), summed on read
    PrHead(RepoId, RefName, RepoId, IssueNumber),   // pk PH#{head repo}#{branch}: open PRs this branch heads
    Checks(RepoId, ObjectId, CheckPart),      // pk CK#{repo}#{sha}: Summary (bumped by every conclusion), suites, runs
    Run(RepoId, RunId, RunPart),              // pk RUN#{repo}#{run}: the run and its jobs
    Account(AccountId, AccountPart),          // pk A#{account}: profile, secret, emails, SSH keys, token metadata, org index
    Org(AccountId, OrgPart),                  // pk O#{org}: META, M#{user}, TEAM#{t}, TM#{t}#{user}, INST#{i}, RULESET#{r}
    Token(TokenHash),                         // pk T#{hash}: account, scopes, expiry, installation
    RepoName(AccountId, RepoNameKey),         // pk N#{owner} sk {name}: Live { repo, cell, epoch } or Alias { repo, epoch }
    Login(LoginKey),                          // pk L#{key}: installs only; managed claims live in the claims table
    Event(EventId), Pending(Consumer, u8, EventId),   // pk EV#{id}; pk Q#{consumer}#{0..16} sk {event id}
    TxMarker(TxId),                           // pk TX#{id}, TTL 1 day, holds the item count: present exactly when the tx committed
}
pub enum ListDim { All, Label(LabelName), Assignee(AccountId), Author(AccountId), Milestone(u32) }
/// n = max(previous n + 1, ClockBound now in microseconds) [CB], so a key re-created after its tombstone's TTL still sorts newer.
pub struct Version { pub n: u64, pub tx: TxId, pub epoch: Epoch }
/// Deletes of authoritative keys are tombstones (rec = None) expired by TTL, so a re-create names the delete it follows.
/// `fmt` follows the two-release rule: release N reads formats <= k+1 and writes k until the cell's FormatGate allows k+1.
pub struct Item<R> { pub key: Key, pub rec: Option<R>, pub v: Version, pub fmt: u8 }

// ---------- One write. The only way to change a cell table.
pub struct TxId(Ulid, u8);                    // request id and attempt: a retry is idempotent, a re-planned attempt is new
pub enum Expect { Absent, At(Version), Fresh }   // Fresh: a new key under a newly minted id
pub enum Op { Put(Key, Record, Expect), Tombstone(Key, Expect), Check(Key, Expect) }
pub(crate) enum Durability { Sync, Lazy }     // Lazy: only pending-row deletes, journaled in 100 ms batches
/// The builder adds the TX marker, the Event item, and one Pending row per subscriber of each event (DESIGN.md's
/// SUBSCRIPTIONS table), and refuses above 100 items or 4 MB [TX] with TooBig. An event without its change cannot compile.
pub struct Write { id: TxId, ops: ArrayVec<Op, 100>, events: SmallVec<[Event; 4]>, durability: Durability }
/// Only `commit` constructs this. report_status, API 2xx, and merge results take &Acked.
pub struct Acked { pub tx: TxId, pub epoch: Epoch, _sealed: Sealed }
pub enum CommitError { Stale(Key), Locked(Key), TooBig, NotHome { home: Region, epoch: Epoch }, Unavailable }
pub trait CellStore {
    /// Single: one TransactWriteItems with ClientRequestToken = id [TX]; Acked when it returns.
    /// Paired: (1) PutItem Prepared into the journal, which returns once a second Region holds it [GT];
    /// (2) the same TransactWriteItems in the home Region; (3) PutItem Mark{Committed|Aborted};
    /// (4) Acked only if HomeLease.until > ClockBound latest() [CB]. Packs above 32 KiB are in two buckets before (1).
    async fn commit(&self, w: Write) -> Result<Acked, CommitError>;
    async fn get(&self, k: &Key) -> Result<Option<Item<Record>>, StoreError>;                       // ConsistentRead, home only
    async fn query(&self, pk: Pk, sk: SkRange, page: PageReq) -> Result<Page<Item<Record>>, StoreError>;   // ConsistentRead
}

// ---------- Journal and home lease (Paired cells). MRSC has no transactions, TTL, or LSIs [GT]; none is used.
pub enum JournalKey { Record(CellId, u8 /* 64 partitions */, TxId), Mark(TxId), Lease(CellId), Closed(CellId, Epoch) }
/// The replayable transaction: its ops with their conditions, and `reads`, the tx stamps its conditions named.
/// Above 350 KB the body spills to S3 in two Regions and the record keeps its key and hash.
pub struct Prepared { pub tx: TxId, pub epoch: Epoch, pub ops: Vec<Op>, pub reads: Vec<(Key, TxId)>, pub spill: Option<S3Key> }
pub enum Mark { Committed, Aborted }
pub struct HomeLease { pub cell: CellId, pub epoch: Epoch, pub home: Region, pub state: LeaseState, pub until: Timestamp }
pub enum LeaseState { Serving, Promoting { from: Region }, Held { until: Timestamp } }
/// Renews every second with 8 s validity, conditional on (epoch, home), and only while this Region's canary write to
/// `repo`, `refs`, and its pack bucket succeeded in the last 2 s. Nodes serve nothing, reads included, without a live lease.
pub async fn keep_lease(cell: CellId, me: Region) -> Result<Never, LeaseError> { todo!() }
/// Pure. Applies every Committed record, plus every Prepared record that an applied record's `reads` names, transitively.
/// Drops the rest. No acknowledged write is dropped, because ack follows the mark, and no applied write depends on a dropped one.
pub fn resolve(records: &[(Prepared, Option<Mark>)]) -> Vec<TxId> { todo!() }
/// Runs in the peer. Waits until ClockBound earliest() > lease.until + HOLD (30 s), CASes the lease to Promoting at epoch e+1,
/// Queries every epoch-e record and mark (ConsistentRead), resolves, applies, writes Closed(e), publishes the cell map to every
/// front Region's control bucket, and CASes the lease to Serving in the peer. Uses only data-plane calls.
pub async fn promote(cell: CellId, to: Region) -> Result<Epoch, PromoteError> { todo!() }
/// One reader per stream shard of the peer's journal replica, a spare on standby, so never more than two [ST]. Applies a
/// record when its mark arrives, each item a conditional PutItem that wins only if newer by (epoch, n). An hourly re-Query of
/// the last two hours makes the 24 h stream retention [ST] irrelevant. Once the lease leaves epoch e it applies no epoch-e
/// record: the promoter applies the resolved set itself, and the rejoin rolls back keys of every epoch-e record left unapplied.
pub async fn apply_standby(cell: CellId) -> Result<Never, ApplyError> { todo!() }

// ---------- Refs
pub struct Shard(u8);                         // hash(name) % 32, or 32 + hash(name) % 8 for refs/pull/*
/// Readers use `target` until they see the intent push Committed through a head's `open`, then overlay its update list.
pub struct RefRec { pub target: ObjectId, pub wv: u64, pub intent: Option<RefIntent> }   // wv: the head value that wrote it
pub struct RefIntent { pub push: PushId, pub new: Option<ObjectId> }
pub struct HeadRec { pub hv: u64, pub open: Option<PushId> }   // every Write touching the shard CASes hv to hv + 1
pub struct Heads([u64; 40]);
pub enum BigPushState { Preparing { deadline: Timestamp }, Committed, Aborted }   // the update list is in the item or S3
/// Query RH#{repo} (40 items, one 4 KB read unit), Query the LSI for wv > have per changed shard, resolve any open
/// intent push through its BigPush item, re-Query the heads, and repeat until two head reads match (at most 5 rounds).
pub async fn refresh(cell: &dyn CellStore, repo: RepoId, have: &Heads) -> Result<RefDelta, StoreError> { todo!() }

// ---------- Managed only
pub enum ClaimKey { Login(LoginKey), SshKey(Fingerprint), Shortcode(Shortcode) }
pub enum Claim { Pending { account: AccountId, cell: CellId, saga: TxId, since: Timestamp },
                 Active { account: AccountId, cell: CellId, ver: u64 }, Alias { account: AccountId, ver: u64 } }
/// Single-item writes by one writer per item, so MREC can neither expose a partial transaction nor resolve a conflict here.
pub enum DirKey { Login(LoginKey), RepoName(AccountId, RepoNameKey), Token(TokenHash), SshKey(Fingerprint),
                  Installation(InstallationId), VerifyingKey(Kid), Heartbeat(CellId) }
pub struct DirItem { pub key: DirKey, pub val: DirVal, pub cell: CellId, pub epoch: Epoch, pub ver: u64 }
/// There is no Absent: a miss asks the owning cell or the claims table, so the directory never says "no".
pub enum Lookup<T> { Found(T), AskCell(CellId), AskClaims }
pub enum AuthClass { Read, Write }
/// A directory credential is usable when its cell's Heartbeat item in this Region's replica is younger than 10 s for Write
/// and 30 s for Read, and its epoch is the cell's current epoch.
/// `DirPublish` writes Heartbeat(cell) each second while its oldest pending row is under 2 s old.
pub fn usable(item: &DirItem, hb: &DirItem, class: AuthClass, now: &ClockBound) -> bool { todo!() }
```

Invariants and where each lives:

- **One writer per fact.** Only `CellStore::commit` writes `repo` and `refs`. Every write to an existing key is a versioned compare-and-set (`Expect::At`), so each item has one total order of versions, and every `Prepared` record is a deterministic list of after-images [inference]. A `Prepared` record left without a mark for 10 s is resolved at home: its `TX` marker present means Committed; otherwise the resolver replays the transaction with the same `ClientRequestToken`, which within 10 minutes either reports the original's success or runs it now, and the result decides the mark [TX].
- **Acknowledgment point.** Single: the home `TransactWriteItems` returned, so the write is in three AZs [inference: DynamoDB stores data across AZs]. Paired: the commit mark is in the journal, which an MRSC write returns only after another Region holds [GT], and the home lease was live at that instant. In both, a pack above 32 KiB is in S3 Standard first, in two Regions when Paired (`gs#54`).
- **Atomicity.** One `Write` is one `TransactWriteItems`, serializable against `GetItem` and other transactions [TX]. Above 100 items only two shapes exist: the intent push (refs) and idempotent saga steps (bulk operations, transfers), as in `DESIGN.md`.
- **No partial transaction is ever served.** At home, reads are `GetItem`, `Query`, or `TransactGetItems` against committed data. A `Query` is read-committed, not a snapshot [TX], which `refresh` and the list assembler handle below. The standby is not read until promotion has applied whole records. After a PITR restore, partial transactions can exist [TX: "it can contain only some of the changes made during a recent transaction"], and the restore reconcile removes them by `Version.tx` grouping ("Deployment modes").
- **No GSI answers a client.** GSIs are eventually consistent (`dyn#11`). Lists use `List` items written in the same transaction. The one LSI, on `refs`, is strongly readable and caps each ref shard's collection at 10 GB in one partition [LSI], which ref shards never approach [inference].
- **Events cannot be lost or orphaned.** The `Event` item and its `Pending` rows commit with the change. Consumers delete only their own rows. Fan-out never reads a DynamoDB stream.

Dominant access patterns traced through the keys:

- *Fetch or clone:* `Query RH#{repo}` with `ConsistentRead`, 40 items of about 60 bytes, one read unit [RW]. Equal to the cache's heads: upstream `git upload-pack` runs on the NVMe cache with no further reads. A changed shard: one `Query` of the `wv` LSI for `wv > have`. Auth: the front's assertion plus `Query R#{repo}` (rules, grants) and `GetItem O#{org} M#{user}`.
- *Push of up to 44 refs:* the reads above plus one `Write` of n refs, up to n heads, the push record, two pack items, the TX marker, one Event and six Pending rows, so n + n + 11 <= 100 gives 44.
- *PR page:* `GetItem IS#{repo}#{n}` (PR), `Query IS#{repo}#{n}` first page of children, `Query CK#{repo}#{sha}`, and `GetItem` of the base and head refs.
- *List page:* one `Query IX#{repo}#{dim}` in sort-key order, plus `ListCount` shards when `totalCount` is asked.
- *Route:* the front's in-memory cache, then the local directory replica, then the claims table or the owning cell on a miss. No front reads a cell table to route.

## Commit and read paths

**Push of up to 44 refs (one `Write`).** The front authenticates the token and signs a `Principal` assertion as in `DESIGN.md`, then forwards to the git role on the repository's rendezvous node in the home Region. Upstream `git index-pack --strict` and the connectivity check run in quarantine on the cache (`DESIGN.md` journey 1). Rules come from `Query R#{repo}` and org rulesets from `Query O#{org}` at the org's cell, all strongly consistent. Packs of at most 32 KiB go inline as `PackInline`; larger packs are PUT with `If-None-Match: *` to the home and peer buckets in parallel (the witness-Region bucket replaces whichever fails). Then `commit`: each ref is `Put(Ref, Expect::At(v))` with `target == expect` checked by the version, each touched head CAS `hv -> hv + 1`, plus `Push`, `Pack`, `PackInline`, `Event`, `Pending`, and `TX`. `Stale(Ref)` becomes `fetch first` or `stale info`, `Locked(Ref)` becomes `failed to lock`. `report-status` takes `&Acked`. The `pr-sync` consumer moves `refs/pull/N/head` later, coalesced, in the 8 pull shards, so bots never conflict with people (graft kept from `DESIGN.md`).

**Atomic push above 44 refs (intent push).** (1) Take `LOCK#bigpush` with a 60 s lease, so one intent push runs per repository. (2) One `Write`: `BigPush` = `Preparing { deadline: now + 60 s }` with the update list (S3 when above 350 KB), and `open = push` on every touched head with `hv + 1`. (3) Chunks of at most 80 refs, each one `Write`: `Put(Ref { intent: Some(..) }, Expect::At(v))` plus `Check(BigPush, Expect::At(preparing_v))`. An intent locks the ref and leaves `target` unchanged. A ref whose version moved fails the chunk, and the push aborts. (4) The commit `Write`: `BigPush` = `Committed` plus heads `hv + 1`, `Push`, packs, `Event`, `Pending`, `TX`, at most 52 items. Acked here. (5) After the ack, finalize chunks set `target = new`, clear the intent, and bump `hv` with `wv`, and the last one clears `open` and the lock. Readers: while any head shows `open = P`, `refresh` reads `BigPush` P once per round. Preparing or Aborted: every stored `target` is still the old value, so it is used. Committed: P's update list is overlaid on the stored targets, which gives the same answer whether or not a ref is finalized yet. The flip to Committed and the head bump are one transaction, so a `refresh` round sees the whole push or none of it. A small push that meets an intent waits up to 2 s, finalizes a ref of a decided push itself, aborts a `Preparing` push past its deadline by CAS, or returns `failed to lock`. A writer that dies after the ack leaves a `Committed` push that readers already resolve and that the maintenance sweeper finalizes. Every step is journaled, so the standby resolves the same way.

**Non-atomic push above 44 refs.** Chunks of 44 refs, each its own `Write`, with per-ref status, which git's non-atomic semantics allow. This is git's default push mode [inference: `--atomic` is opt-in].

**Same-repository merge.** The api role plans from strong reads (PR item, head and base refs, reviews, rules, `Checks` summary), asks the git role for the merge commit, which verifies that the tree diff equals the PR's diff (`up#49`, kept from `DESIGN.md`), and places the pack. One `Write` (Usage, call site 1): `Check` the head ref at the planned version, CAS the base ref, CAS the PR item (state open, `review_version` inside `v`), `Check` the `Checks` summary version, move the PR's list entries, add timeline children, bump heads, add the push record, events, and pending rows, about 34 items. A review submitted or dismissed concurrently bumps the PR's version, and a check run that concludes bumps the summary, so whichever commits second sees `Stale` and re-plans. The re-plan merges if protection still passes and otherwise returns GitHub's text. Merged state and the base ref cannot disagree, because they are one transaction.

**Fork merge.** A fork lives in its network's cell (`ForkPlacement::WithNetwork`), so the fork's head ref is another item collection in the same `refs` table, and the merge `Write` above `Check`s it directly. That makes cross-owner merges as atomic as same-repository ones, with the fork's objects already in the network's pack set. A fork that residency forces elsewhere (`Detached`, `DESIGN.md` "Namespace and cell placement") is guarded by `refs/pull/N/head` in the base repository instead. `pr-sync` copies the fork's new objects into the base network's packs and moves that ref within seconds. A detached fork's push that lands in those seconds is not in the merge, and `final_head` records what was merged. `pr.head_guard()` is the only place that chooses between the two keys.

**Merge queue.** One owner per base branch under `MQ#{branch}` builds groups, tests them in CI, and commits a group of up to 3 PRs per `Write` (each PR costs about 17 items in list moves, timeline, and events). A larger group commits as consecutive `Write`s whose intermediate base tips are prefixes of the tested group [inference]. The owner serializes, so queued merges never fight over `main`.

**Branch-rule changes.** A rule is one `Rule` item, and a ruleset one `RULESET` item in the org's collection, each a versioned `Write`. Pushes and merges read rules strongly at authorization time but do not `Check` the rule items, because every push in a repository would then conflict with every other [TX: concurrent transactions on one item conflict]. A rule change applies to every push and merge whose authorization starts after the change returns. One already past authorization can commit under the old rule, a window of tens of milliseconds [inference], as in `DESIGN.md`.

**List endpoints.** `IX#{repo}#all` holds every issue and PR in every state, sorted by created, updated, and comments, with the state in the sort key. `IX#{repo}#{label|assignee|author|milestone}` holds open items only, sorted by created and updated. Each entry projects labels, assignees, the PR base, and the PR head, and each `Write` that changes an issue moves its entries (delete plus put) in the same transaction.

| Request | Answered by | Consistency |
| --- | --- | --- |
| `GET /repos/{o}/{r}/issues` with `state`, `sort=created|updated|comments`, `direction`, `since` | `Query IX#all`, `since` as a range on the updated key | Strong |
| The same with `labels`, `assignee`, `creator`, or `milestone`, state open | `Query IX#{dim}` on the most selective dimension, the other filters checked on projected attributes | Strong |
| The same with those filters and state closed or all | `Query IX#all` with the filters applied, at most 1 MB read per page, the page shortened with a `next` link | Strong |
| `mentioned`, free text, `/search/issues` | OpenSearch, fed by the `SearchFeed` consumer | Eventual, p99 under 5 s [guess] |
| `GET /repos/{o}/{r}/pulls` with `state`, `sort`, `head`, `base` | `Query IX#all` (kind PR), `head` through `PH#{repo}#{branch}`, `base` on the projected attribute | Strong |
| GraphQL `issues`, `pullRequests` with `labels`, `states`, `orderBy`, `totalCount` | The same items, plus the 8 `ListCount` shards | Strong |

Pagination uses an opaque cursor in the `Link` header. A bare `?page=N` reads from the start, capped at page 100, as GitHub caps results [inference]. Because `Query` is read-committed [TX], an item updated while a page is read can appear twice or not at all, and the assembler drops duplicates by number, as any cursor list under concurrent edits does.

**Worldwide identity layer.** Every account and org has a home cell, chosen at signup by the caller's continent as in `DESIGN.md`, and its identity is item collections in that cell: `A#{account}` (profile, password and 2FA secrets, emails, SSH keys, token metadata, an index of its memberships), `O#{org}` (members, teams, team members, installations, rulesets), `T#{hash}` per token, and `N#{owner}` for repository names. These are journaled like refs, so an identity write has RPO 0 and fails over with its cell. Only three facts need one answer on Earth: logins, SSH key fingerprints, and enterprise shortcodes. Each is one item in the managed claims table, MRSC across three continents, decided by a conditional `PutItem` (`Pending`), then the home cell's `Write` that creates the account or key, then a conditional update to `Active`; a sweeper finishes or releases a `Pending` claim after 10 minutes by reading the home cell. Repository names are unique per owner, so `N#{owner}` decides them inside the owner's cell with `Expect::Absent` in the same `Write` that creates the repository when both are in one cell, and in a three-step saga (reserve, create, activate) when the network's cell differs. A rename is the same shape: claim the new login, alias the old one, bump `NameEpoch` (`DESIGN.md` renames and retirement rules kept). The directory is a read-optimized projection for fronts: one MREC table, one replica per front Region, written only by each cell's `DirPublish` consumer from its pending rows, one item per fact. A front never answers "no" from it: an unknown login asks the claims table with a strongly consistent read, an unknown repository name or token asks the owning cell, which the login item or the token itself names. No request reads a shared identity store, and no store holds a geography's identity except its cells. A Region outage stops identity writes only for accounts homed in that Region's cells, for the 1 to 2 minutes until their peers serve.

**Event fan-out and Streams' limits.** Fan-out never reads a DynamoDB stream. The `Write` that changes data inserts one `Event` item and one `Pending` row per subscribed consumer, so an event exists exactly when its change committed and no stream reader, retention window, or per-item stream order is involved. Streams appear once per Paired cell: the standby applier reads the journal's stream in the peer with one reader per shard and a spare, inside the two-reader limit, and its hourly re-Query of the last two hours covers the 24 h retention and a lost checkpoint [ST]. Consumers own partitions by rendezvous over node leases, poll their own partitions every second with eventually consistent `Query`s (about 80 read units per second per cell), and get an in-memory hint from the committing node for low latency. Delivery is at least once with the `X-GitHub-Delivery` GUID derived from the event id, as in `DESIGN.md`.

**Authorization.** The front verifies the credential and asserts the principal (`DESIGN.md` "Identity and tokens", token prefixes and CRC kept, with the issuing cell encoded in the random part). In managed, the front checks the token in the local directory replica, and on a miss asks the issuing cell, so a brand-new token works at once everywhere. The serving cell decides: write-class operations by a token issued in a cell of the same Region re-read `T#{hash}` strongly, and tokens from other Regions are trusted from the directory only while `usable(.., Write, ..)` holds, else the cell asks the issuing cell across Regions. Membership, teams, and grants are always read strongly where they live: `R#{repo}` grants in the repository's cell, `O#{org}` in the org's cell, which is in the repository's Region for every network an org roots. A fork owned by an org homed in another Region lives in its network's cell, so its write-class authorization pays one cross-Region read of the org's membership and fails with 503 while that org's cell is unreachable.

**Revocation.** Removing a member, a team member, or a grant is one `Write` in the owning cell, and every authorization reads that item strongly, so the bound is zero: every request whose authorization starts after the removal returns is refused. Revoking a token or uninstalling an app is one `Write` in its cell, which denies it at once in that Region. `DirPublish` updates the directory, and `revoke` returns after a strong read of the item in every front Region's replica shows the change, or after 5 s naming the lagging Regions. A lagging Region stops trusting that cell's directory items for write-class requests once its heartbeat is 10 s old and for reads at 30 s, so the bound is the coverage wait, at most 5 s, and 10 s for writes in a Region whose replication stalled. This rests on MREC delivering a cell's heartbeat no earlier than its prior writes, which AWS does not document, so Milestone 0 measures it [inference].

**Read guarantees after a write.**

| Read | Path | Guarantee |
| --- | --- | --- |
| git fetch, clone, ls-remote | `refresh` against home `refs` | Every acked push. A fetch never sees part of an intent push. A fetch that races a single-`Write` push touching several shards converges in the next `refresh` round, and after 5 rounds on a repository committing over 100 times per second it may serve per-shard-consistent refs once [inference] |
| REST and GraphQL object reads | `GetItem` and `Query` with `ConsistentRead` in the home Region, through any front | Read-your-writes for every acked write |
| Lists on strong dimensions | `Query IX#..` | Read-your-writes |
| `/search`, `mentioned` | OpenSearch | Eventual |
| Routing (managed) | Front cache, directory replica, claims or cell on a miss | A stale positive is caught by the cell's `NameEpoch` check (`DESIGN.md`), and a miss is never final |
| Standby | Not served | Read only by the promoter |

**Journey 1: an atomic push of three branches to a busy monorepo.** Stores and Regions: the front in the pusher's Region (directory replica), the cell's home Region (`repo`, `refs`, pack bucket), the peer's pack bucket, and the journal (home replica, synchronously copied to the peer or the witness). Three refs make one `Write` of about 20 items. The merge queue commits `main` in its own `Write`s, so it conflicts with the push only if one of the three branches is `main`, in which case the push gets `fetch first`. Bot updates go to the 8 pull shards through `pr-sync`. CI's 2,000 clones a minute (33 per second) each read 40 heads in one read unit, coalesced per node in 100 ms windows. Durable at `ok`: refs, push record, and events in the home Region's three AZs, the prepare record and commit mark in two Regions, and pack bytes in two Regions. Next read: every fetch, from any node, sees all three new tips or, while a fetch races the commit, none of them until its next round.

**Journey 2: a fork PR merged while a review and a check run land.** The fork is in the network's cell, so the merge `Write` checks the fork's head ref, the PR version, and the check summary in the base cell's home Region. If the review commits first, the merge's PR check fails and the merge re-plans: an approval merges, a request for changes returns GitHub's protection text. If the check run's conclusion commits first, the summary check fails the same way. If the merge commits first, the review becomes a comment on a merged PR, and the check conclusion is recorded against the merged head, as on GitHub [inference]. Durable at the 200: everything in the one transaction, plus the journal in Paired cells. `gh pr view` and `gh pr list` run at once, each routed by the front to the home Region: `pr view` reads the PR item strongly and shows `MERGED`, and `pr list` reads `IX#{repo}#all` open PRs strongly, from which the merge removed the entry in the same transaction.

**Journey 3: create an issue labeled `bug`, then list.** One `Write` in the home Region: `CTR#issue` CAS, the `Issue` item, three `IX#all` entries, two `IX#label:bug` entries, two `IX#author` entries, the `ListCount` shards, the event and pending rows, about 16 items. `gh issue list --label bug` runs GraphQL `issues(labels: ["bug"], states: OPEN)` against `IX#{repo}#label:bug`, and `gh api repos/o/r/issues?sort=updated` against `IX#{repo}#all` by updated key, both strongly consistent in the home Region, so both show the new issue. Neither touches search.

**Journey 4: an org owner removes a member, whose next push must fail.** The removal tombstones `O#{acme} M#{mira}` (and her `TM#` items) in one `Write` in acme's cell. Mira's next push authenticates (her token is valid) and reaches acme/payments's cell in acme's home Region, where the strong read of `M#{mira}` finds the tombstone and refuses with `Permission to acme/payments.git denied to mira.` The stated bound is zero for every push whose authorization starts after the removal's 200. A push already past authorization can still commit, as in `DESIGN.md`. If acme instead revoked mira's token, the bound is the `revoke` coverage wait, at most 5 s, or 10 s where replication stalled.

**Journey 5: the Region with a repository's newest writes is lost for good, mid-push.** Managed, cell `c7` with home us-east-2, peer us-west-2, witness us-east-1. The push in flight is in one of three states. Before its prepare record: the client sees a broken connection and nothing exists. Prepared, not marked: the client never saw `ok`, and `resolve` applies it only if a marked write read from it. Marked: the client may or may not have seen `ok`, and the write is applied. The home lease expires 8 s after the last renewal, because us-east-2 cannot renew it, and every node in us-east-2 that is still alive stops serving reads and writes. After the 30 s hold, the promoter in us-west-2 CASes the lease to epoch e+1 (the journal keeps quorum with the witness [GT]), resolves epoch e from strongly consistent reads, applies the remainder (the applier normally trails by about a second), writes `Closed(e)`, publishes the cell map, and serves. The pack is in the us-west-2 bucket because it was there before the prepare record. RTO is about 45 to 70 s plus front propagation [inference]. Writes that us-east-2 committed without a mark, and that nothing acked depends on, are gone, which the client sees as an unacknowledged push. In a two-Region BYOC install (home eu-west-1, peer eu-central-1, witness eu-west-3, all in MRSC's 15 Regions, `dyn#25`) the same promoter runs in the install's own operator role, and Route 53 health checks against each Region's `/health`, which passes only where the lease is held, move clients without editing DNS (`up#15`). If us-east-2 later returns, its nodes see epoch e+1 and refuse traffic. Writers never begin a `Write` with under 2 s of lease left, and every home transaction wrote its prepare record first, so the epoch-e records that were not applied (dropped by `resolve`, or arriving after `Closed(e)`) name every key us-east-2 may hold that us-west-2 does not. The rejoin copies those keys from us-west-2, then us-east-2's applier follows the journal as the new standby.

**Journey 6: a release multiplies identity reads tenfold, and another writes a shape the previous release cannot read.** Reads: a client release that multiplies requests is shed per token and per IP at the front before any table (`DESIGN.md` rate limits), and what passes hits the front's in-memory positive cache, whose entries the serving cell validates by epoch, so no TTL exists to cut (`up#47`). Misses go to the local directory replica or the cell tables, whose warm throughput is preset to 10 times the measured peak, which DynamoDB serves instantly on demand [WT]. A server release that multiplies reads is held to a per-node token bucket on DynamoDB calls, rolls one front cell and one data cell first, and rolls back automatically on throttle metrics. The blast radius is one canary cell; nothing reads a shared identity database. Shapes: every item carries `fmt`. A release may write format k+1 only after the cell's `FormatGate` item allows it, and the operator raises the gate only after every node and both standby appliers report a release that reads k+1. Decoders accept absent fields with defaults for one release, so a dropped field is first no longer read, then no longer written. CI decodes a corpus written by N+1 with N's decoder and the reverse. A bug that writes an unknown format anyway makes old readers return 503 for that item, never a wrong answer.

**Journey 7: a single-Region self-hosted install where DSQL is absent, an operator mistake, and a point-in-time restore.** Region: af-south-1 or ap-southeast-3, both DynamoDB Regions that DSQL lacks (`dyn#24`, `#26`). Install: the bootstrap and `platform-deploy deploy` create `repo`, `refs`, and `eph` with PITR (35 days, `dyn#19`) and deletion protection, versioned pack buckets, and three nodes. There is no journal, standby, claims table, or directory: claims are `L#{key}` items and fronts read the cell tables. Mistake: an admin script force-pushes 40 repositories and deletes 300 issues at 10:00. Restore: `tributary admin restore --repo acme/payments --to 09:58` restores `repo` and `refs` to scratch tables at 09:58 (PITR always restores to a new table [PITR]), freezes the repository (`frozen` in `RepoMeta`), finds every transaction group that the restore cut by grouping items written in the last two minutes by `Version.tx` and comparing with each `TX` marker's item count, takes those items' prior images from a second scratch restore one second before the group's first write, copies the repository's collections (`R#`, `RH#`, `RF#*`, `P#`, `IS#*`, `IX#*`, `CK#*`, `RUN#*`) back as `Write`s whose versions and head values exceed the current ones, so caches refresh, and unfreezes. Packs are there because GC keeps unreachable objects 14 days and bucket versioning keeps deletes for 35 days (`DESIGN.md` "Repository storage"). `--install` restores every table, bumps a `restore_epoch` that makes nodes drop caches, and replays the revocation records that `revoke` writes to the control bucket (the `DESIGN.md` re-walk's N2 fix). Restore calls the DynamoDB control plane, which delays only restores. What is not undone: webhooks already delivered.

## Scale

Per-partition limits are 3,000 read and 1,000 write units per second, and a hot item can be isolated but not exceed them [AC]. A transactional write costs 2 units per KB per item (`dyn#3`). Numbers below are [inference] from those facts unless labeled.

| Busiest single repository | Ceiling | Limiter | Spread |
| --- | --- | --- | --- |
| Ref-update `Write`s (pushes, bot updates) | About 330 per second guaranteed, up to about 2,000 if adaptive capacity isolates head items [guess] | `RH#{repo}` takes 3 units per `Write` (1.5 heads at 2 units); then CAS contention per head, about 50 per second each [guess] | 40 heads; `repo` has no LSI, so the collection can split by sort key [AC]; admission default 100 commits per second per repository (`DESIGN.md` graft) |
| Updates to one ref (`main`) | About 18 per second Paired, 60 Single [guess] | Version CAS at commit latency, about 55 ms Paired | The merge queue owner serializes and commits groups of 3, so about 50 PRs per second |
| Bot updates to `refs/pull/*` | About 400 per second | 8 pull heads | `pr-sync` coalesces last-writer-wins behind user pushes |
| Atomic push size | 5,000 refs [guess cap], about 1 s per 1,000 | Sequential 80-ref chunks | One intent push per repository at a time |
| Clones and fetches | Unbounded by the store | 3,000 strong head reads per second per partition, coalesced to one read per node per 100 ms | Rendezvous sets widen from 3 to 9 nodes; bundle-uri and S3 serve clone bytes (`gs` implication 11) |
| Issue and PR writes | About 100 to 150 per second | `IX#{repo}#all` takes 3 entries per write, and its updated key is monotonic, which blocks a split [AC] | Per-dimension collections; `ListCount` sharded 8 ways |
| Issue numbers | About 50 per second [guess] | `CTR#issue` CAS | None needed at GitHub's rates |

Journey 1's load (3-ref atomic push, 0.17 merges, 0.83 bot updates, and 33 clones per second) sits below every ceiling by 10 times or more.

**Fleet.** 3.35 billion pushes a month (`gs#5`) is about 1,290 per second; with a peak of 3 times average [guess], 3,900 per second. At about 38 write units per push in the home Region (`cost_ddb.py`), one table at its default 40,000 write units per second (`dyn#9`, adjustable) carries about 1,000 pushes per second, so push volume alone needs 4 cells at peak. Cells are sized by load test and blast radius instead: at most 2% of the fleet's repositories per cell gives 50 or more cells per wave plan [inference]. Nothing is fleet-wide on the write path: the claims table sees signups, renames, and SSH key additions only, and the directory is written once per identity change.

**Fan-out.** Each `Write` carries one `Pending` row per subscribed consumer (at most 8), in 16 partitions per consumer per cell, each owned by one node chosen by rendezvous over live `NodeLease` items in `eph`. Webhooks expand to hooks after the commit, write delivery attempts to `eph` and payloads to S3, and deliver with per-hook-owner shuffle shards (`DESIGN.md` "Event outbox", minus its SQL). A pending row costs 2 write units to insert in a transaction and 1 to delete, and a partition takes at most 1,000 write units per second [AC], so 16 partitions carry about 5,000 events per second per consumer per cell [inference]. Actions: `ActionsTrigger` writes runs and jobs in `Write`s of at most 90 jobs, and larger runs commit in chunks under a `Creating` marker (`DESIGN.md`). DynamoDB Streams carries no fan-out: their two-reader limit and 24 h retention [ST] bind only the standby applier, which uses one reader.

**Hot keys.**

| Key | Load | Spread |
| --- | --- | --- |
| `RH#{repo}` heads | Every ref `Write` and every freshness read | 40 items, node coalescing, per-repository admission |
| `LOCK#bigpush`, `MQ#{branch}` | One owner each | Leases, by design |
| `CTR#issue` | Issue and PR creation | CAS; one item per repository |
| `IX#{repo}#all` | Every issue and PR change | Per-dimension collections carry the filtered reads |
| `Q#{consumer}#{p}` | Every event | 16 partitions per consumer per cell |
| Journal `Record(cell, p, ..)` | Every Paired write | 64 partitions per cell |
| `Lease(cell)` | One renewal per second | One item per cell |
| Directory `Login` of a famous account | Reads | Front cache; a replica per Region; partition limit 3,000 strong or 6,000 eventual reads per second [AC] |

## Downtime

| GitHub incident | Evidence | This design | Effect |
| --- | --- | --- | --- |
| Database overload | Auth and user database overloaded by 10 times the reads (`up#47`); writes on a shared permissions cluster broke App tokens for 12 h (`up#44`) | No shared identity or permissions database: membership and grants live in their owner's cell, tokens in their issuing cell, routing in a per-Region directory replica that is never authoritative. Load spreads over item collections and per-partition limits [AC], and one collection's throttling stops only that repository or account | contained |
| Schema change | Dropped column (`up#44`, Aug 2025); online migration at peak (May 2026); misconfigured index | No DDL. Shapes change through `fmt` and the `FormatGate` two-release rule. A new access pattern is new items written by new code and backfilled per collection. No GSI is on a client path | removed |
| Engine upgrade | Datastore major-version upgrade (`up#44`, Jan 2026); version bug on a custom cluster (May 2023) | DynamoDB has no customer-run engine or version [inference]. An AWS-side regression in the home Region fails the canary, the lease lapses, and the peer takes over | removed |
| Failover and partition | 2018: a 43 s partition, cross-country promotion, 954 unreplicated writes, 24 h (`up#40`); undetected partial disk failure (`up#44`) | A home serves only under a live lease and promotion waits for its expiry plus 30 s, so two homes never serve. Every acked write is in the journal in two Regions, so promotion loses none and no fail-back reconcile of acked writes exists. Detection is a failed canary write, not a heartbeat | contained |
| Connection storms | Load-balancing tier failed to scale and a client retry loop slowed recovery (`up#48`) | DynamoDB is per-request HTTPS with no connection quota to exhaust [inference]. Retry budgets of 10% per node, deadlines, and admission per tenant stay from `DESIGN.md` | mitigated |
| Config and deploy | Sharding middleware redeploy, internal LB change, expired certificate (`up#45`) | Cells, per-AZ waves, signed config with last-known-good, certificate expiry alarms from `DESIGN.md`. A bad release reaches one canary cell's tables, and per-repository PITR undoes its damage | contained |
| Capacity | August 17, 2026 peak, 7 h 47 min (`up#48`); 15 capacity incidents (`up#42`) | Static N+1 nodes, warm throughput preset to 10 times peak [WT], per-tenant buckets, and a peer Region already sized for full load | mitigated |
| Search and DNS dependency | Elasticsearch overload emptied PR and issue lists (`up#49`); DNS failures cascading (`up#46`); DynamoDB's own DNS record emptied in us-east-1 (`up#32`) | Lists read `IX` items, never search. Endpoints are cached with last-known-good. A Region whose DynamoDB endpoint fails cannot renew leases, and its cells move to their peers | contained |
| Merge logic bugs | Merge queue squash reverted changes in 2,092 PRs (`up#49`) | Tree-diff verification before the ref moves (`DESIGN.md`), groups committed as prefixes, and per-repository PITR to repair a branch | mitigated |
| Hardware and power | Zone power loss in use1-az4 (`up#39`); mec1-az2 data lost for good (`up#37`) | DynamoDB and S3 span AZs; nodes run at 66% per AZ; journal plus standby for the Region case | contained |

RPO and RTO per durability class. RTO targets are [inference] until Milestone 0 measures them.

| Class | Contents | AZ loss | Region loss, managed and Paired installs | Region loss, single-Region install |
| --- | --- | --- | --- | --- |
| Journaled | Refs, intents, push records, packs metadata and inline packs, PRs, issues, comments, reviews, checks, runs and jobs, rules, grants, identity, events and pending rows | RPO 0, RTO 0 | RPO 0 for acknowledged writes. Committed but unacknowledged writes are kept only when an acked write read them. RTO 1 to 2 minutes, automatic | RPO the newest cross-Region backup copy or incremental export (daily by default, [EXP] allows another Region's bucket), RTO hours, with a new install in a new Region |
| Lazy | Deletes of pending rows | RPO 0 | Up to about 1 s, seen as duplicate deliveries with the same GUID | Not separate |
| Pack bytes | S3 objects above 32 KiB | RPO 0 | RPO 0: two Regions before the prepare record | Optional cross-Region replication, RPO 15 min [unverified RTC SLA] |
| Claims (managed) | Logins, SSH fingerprints, shortcodes | RPO 0 | RPO 0, RTO 0 while two of three Regions live [GT] | In cell tables |
| Derived | Directory, search index, caches | Unaffected | Rebuilt: the promoter re-stamps the cell's directory items at e+1, and items at e are ignored | Rebuilt |
| Ephemeral | Runner sessions, Actions cache index, delivery attempt logs, rate counters | Unaffected | Lost: runners reconnect, caches miss, attempt history restarts | Lost |

What MREC would have done to the journaled class, and why it holds only derived data here: MREC applies a transaction's items to other Regions one by one, so a replica can show a merged PR whose base ref has not moved [TX], resolves concurrent writes per item by last writer wins [GT], and may combine several changes into one replicated write [GT]. A partitioned old home's unreplicated writes would arrive after promotion and overwrite every item the new home had not rewritten. The directory has one writer per item and only single-item writes, so none of these can happen there.

## Failure modes

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| Cell tables | instance | Requests on a failed storage node see 5xx or `TransactionCanceledException` | SDK retries under a 10% budget; `ClientRequestToken` and `TxId` make a retried `Write` idempotent for 10 minutes [TX] |
| Cell tables | az | None for data; DynamoDB spans AZs [inference] | Nothing to do; nodes in the AZ are zonally shifted (`up#20`) |
| Cell tables | region | Every repository and account homed in the cell's Region | Paired: lease lapses, promotion to the peer, RPO 0, RTO 1 to 2 minutes. Single: restore from the newest cross-Region copy |
| Cell tables | control-plane | None at runtime | Tables, warm throughput, PITR, and TTL are set at install. Runtime and promotion call only data-plane APIs. Restore calls the control plane and waits for it |
| Cell tables | bad-deploy | One canary cell first; a release can write wrong items | `fmt` gate, waves, automatic abort on error and throttle metrics, per-repository or whole-install PITR restore |
| Cell tables | bad-config | One cell | Signed config with last-known-good (`DESIGN.md`); readiness probe writes a canary item to each table |
| Cell tables | noisy-tenant | One repository's or account's collections | Per-partition limits isolate collections [AC]; per-repository and per-account token buckets keep any tenant under 10% of a table's throughput; a slot above 20% of a cell moves (`DESIGN.md`) |
| Journal | instance | One prepare or mark call fails | Retried with the same item key; a writer that dies leaves a `Prepared` record that the home resolver replays with the same `ClientRequestToken`, which reports or completes the outcome |
| Journal | az | None | MRSC replicas span AZs [inference] |
| Journal | region | One of three Regions | Writes need two of three [GT]: losing the peer or witness keeps the home writing; losing the home lets peer and witness promote |
| Journal | control-plane | None at runtime | Created empty with its trio at install [GT]. After a peer is lost for good, the cell keeps journaling to home and witness, which keeps writes and RPO 0 for the home's own failures but leaves no readable second copy; a new journal and peer are planned work, and status says so |
| Journal | bad-deploy | Wrong or missing prepare records corrupt the standby, not the home | The standby verifier compares a 1% sample of home and standby items hourly and alarms; switchover is blocked while it disagrees |
| Journal | bad-config | Wrong trio or table | Preflight checks the trio is in MRSC's 15 Regions (`dyn#25`); canary prepare and mark at readiness |
| Journal | noisy-tenant | A cell's write burst | 64 partitions per cell; tenants are capped before `commit` |
| Standby tables | instance | Applier node dies | A spare takes over through its `NodeLease` and resumes from its checkpoint or `AT_TIMESTAMP` (`dyn#16`) |
| Standby tables | az | None | DynamoDB spans AZs |
| Standby tables | region | The peer is lost, so the cell has no Region-loss protection | Home keeps serving with journal quorum home plus witness; status turns red; re-pairing to a new peer is planned work |
| Standby tables | control-plane | None at runtime | Created at install with PITR |
| Standby tables | bad-deploy | Applier bug writes wrong after-images | Hourly verifier; the journal still holds the records, so a fixed applier re-applies from a time |
| Standby tables | bad-config | Applier pointed at the wrong table | Applier refuses a table whose `CellId` marker differs |
| Standby tables | noisy-tenant | Applier lag | Lag alarm at 10 s; switchover and the 30 s hold wait for it |
| S3 pack buckets | instance | Single requests fail | Retries; keys spread by network id |
| S3 pack buckets | az | None (`gs#54`) | Nothing to do |
| S3 pack buckets | region | One of home, peer, witness buckets | Two of three still take every pack; reads use the pack item's location set |
| S3 pack buckets | control-plane | None at runtime | Pre-created; presigned URLs computed locally |
| S3 pack buckets | bad-deploy | Wrong deletes | Policy requires `If-None-Match` on pack keys (`up#31`); GC keeps 14 days; versioning keeps 35 days |
| S3 pack buckets | bad-config | Writes fail before ack | Readiness canary object per bucket |
| S3 pack buckets | noisy-tenant | One network's prefix | Push bytes and concurrency caps per account and network |
| Git cache node | instance | Its in-flight requests | Rendezvous sets; NLB removes it with a 33% velocity cap (`up#10`) |
| Git cache node | az | A third of nodes | 66% per AZ, zonal shift and autoshift practice (`up#20`, `#21`) |
| Git cache node | region | The home's fleet | The peer's fleet is warm through `CacheHint` and serves after promotion |
| Git cache node | control-plane | No replacement while EC2 launch fails | N+1 static capacity (`up#2`) |
| Git cache node | bad-deploy | One AZ of one cell | Hot swap with swap-back (`DESIGN.md`) |
| Git cache node | bad-config | One AZ of one cell | Per-AZ config with abort |
| Git cache node | noisy-tenant | One repository's rendezvous set | Upload-pack concurrency 64 per repository, cgroups |
| Claims table | instance | One claim call fails | Retries; `ReplicatedWriteConflictException` retried [GT]; sagas idempotent |
| Claims table | az | None | Nothing to do |
| Claims table | region | One continent's replica | The other two keep quorum [GT]; only claims notice, with a longer round trip |
| Claims table | control-plane | None at runtime | Created once; a new trio is a copy, planned work |
| Claims table | bad-deploy | Wrong claims | Conditional writes only; a daily scan compares `Active` claims with their cells |
| Claims table | bad-config | Claims fail where misconfigured | Readiness claims and releases a canary key per replica |
| Claims table | noisy-tenant | Signup or rename storm | Per-IP, per-ASN, per-account claim limits at the front |
| Directory | instance | One lookup fails | Retry, then ask the owning cell |
| Directory | az | None | Nothing to do |
| Directory | region | That Region's fronts lose their replica | Fronts there ask cells directly, slower; other Regions are unaffected |
| Directory | control-plane | None at runtime | Adding a front Region's replica is planned work |
| Directory | bad-deploy | Wrong items | Never authoritative: cells check epochs, misses ask cells, and `DirPublish` re-derives every item weekly |
| Directory | bad-config | Fronts read the wrong table | Readiness reads a canary item per cell |
| Directory | noisy-tenant | A scanner's misses | Misses go to cells and claims under per-IP admission; hits come from the front cache |

## Deployment modes

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Cell tables | Many cells, each one vendor account, Paired by default | One cell in the customer's dedicated account; Paired by default | One cell; Single by default, Paired optional |
| Journal | One MRSC table per Paired cell, trio inside MRSC's 15 Regions | One, when Paired; outside the 15, a single-Region journal in the peer, with peer loss stopping writes until a customer-signed `solo` intent | Same as BYOC, only when Paired |
| Standby tables | In each Paired cell's peer | In the peer, when Paired | Same as BYOC |
| S3 pack buckets | Home, peer, and witness-Region buckets | Same when Paired; one versioned bucket when Single | Same as BYOC |
| Git cache node | `git` role per cell Region, three AZs at 66% | Combined roles on 3 to 9 nodes per Region | Combined roles on 3 nodes, or 1 for evaluation |
| Claims table | MRSC in us-east-2, eu-central-1, ap-northeast-1 | `L#{key}` items in the cell tables | Same as BYOC |
| Directory | MREC with a replica per front Region | Not deployed: fronts read the cell tables | Same as BYOC |

The release, signing, operator role, intents, status documents, and vendor-dependency table of `DESIGN.md` "Deployment modes" and "Install operations" carry over. Changes: the bootstrap's deploy policy denies `dynamodb:DeleteTable`, `UpdateTable` on deletion protection, and data reads on the cell tables, instead of DSQL; the operator role keeps its intent journal and recovery lease in the control bucket with `If-Match`, so promotion, restore, and config apply never depend on a broken table (the re-walk's N1 fix). Promotion runs in the peer's operator role from the journal, which the home's failure cannot take down. A self-hoster operates three DynamoDB tables, one versioned bucket, and three nodes, and has no database server, cluster, engine version, connection quota, or schema migration to manage [inference].

**Backups and point-in-time restore.** PITR on every cell and standby table, 35 days (`dyn#19`); weekly on-demand backups kept 4 weeks in a vault in a separate account; for Single installs a daily incremental export to a bucket in another Region [EXP]. Restores are per repository or per install (journey 7), always into new tables [PITR], reconciled for cut transactions by `Version.tx`, then copied or flipped by signed config.

**Regions.** Managed: Paired cells in MRSC's 15 Regions (`dyn#25`), fronts and directory replicas in any DynamoDB Region. BYOC: any Paired trio inside the 15, or any two DynamoDB Regions with the single-Region journal, or Single anywhere. Self-hosted: Single in any of the 34 commercial DynamoDB Regions (`dyn#24`). An install no longer needs DSQL's 20 Regions or a DSQL Region set.

## Cost

Monthly AWS list cost, from `cost_ddb.py` in this directory, which imports `scripts/cost_model.py` unchanged for prices, `TIERS`, `ACTIVITY`, and every non-database line. It removes the DSQL lines, the DSQL interface endpoints, and the freshness-board S3 requests, and adds DynamoDB writes and reads (on-demand, $0.625 and $0.125 per million), storage ($0.25 per GB-month), PITR ($0.20), weekly backups ($0.10), and for two Regions the MRSC journal in replicated write units in both replicas (written then deleted), the standby apply, and its storage (`dyn` section 7). The baseline column is `cost_model.py` after the rubric's two fixes (GFS backups and 16 outbox partitions). Per-operation unit counts are [guess] until Milestone 0.

| Developers | Install | This design per month | DynamoDB lines | Baseline after fixes |
| --- | --- | --- | --- | --- |
| 50 | single-Region | $1,298 | $52 | $1,433 |
| 50 | two-Region | $2,285 | $80 | $2,485 |
| 300 | single-Region | $2,681 | $163 | $3,103 |
| 300 | two-Region | $4,618 | $293 | $5,062 |
| 2,000 | single-Region | $13,351 | $1,074 | $16,384 |
| 2,000 | two-Region | $21,891 | $1,986 | $24,879 |

Arithmetic for 2,000 developers, two Regions: about 486,000 pushes a month at 38 home write units, 19 KB of journal after-images, and 19 KB of standby apply; 1.8 million Actions jobs at 25 transitions of 4 units; 152 million API reads at 5 units. The journal costs $418 (about 670 million replicated write units), backups $364, storage $460, PITR $364, home writes $169, reads $137. Nodes ($12,026) dominate in every row, as in the baseline (`python3 cost_ddb.py --detail large two`). The savings at 2,000 developers come mostly from backups: the baseline's GFS scheme keeps 35 full DSQL copies plus cross-Region copies ($3,640 a month in `cost_model.py`), where PITR bills once by table size (`dyn#20`). DynamoDB costs more than the baseline's DSQL requests ($91 a month), so the two designs' request costs are not the difference.

## Build order from zero

**Milestone 0: falsify the foundation in two weeks on scratch accounts.** No git code. A Rust driver issues exactly `commit`, `refresh`, and `promote` against real tables in us-east-2, us-west-2, and us-east-1 (witness), and eu-west-1, eu-central-1, and eu-west-3.

| Measurement | Pass | Fail, and what changes |
| --- | --- | --- |
| Ref `Write`s on one repository, 40 heads, single-ref pushes spread over shards | At least 300 per second sustained, conflict retries under 5%, p99 commit under 60 ms Single and 150 ms Paired | Under 100 per second, or Paired p99 over 300 ms: the per-repository ceiling is wrong, and heads split into 4 collections before anything else is built |
| Serial CAS on one ref | At least 15 per second Paired | Under 5: merge groups grow and the merge-queue claim is restated |
| MRSC journal `PutItem` p99 in both trios, and a `ConsistentRead` `Query` of a journal partition | Under 40 ms; the Query works and returns every record | Over 100 ms, or strong Query unsupported: the promoter enumerates records by `GetItem` from the marks, and latency claims are restated |
| Promotion under 200 `Write`s per second with a linearizability checker, home cut by network ACL, 20 runs | Zero acked writes missing or reordered; RTO p99 under 120 s | Any acked write missing: the foundation is wrong and the baseline stands |
| Standby applier at 2,000 records per second | Lag p99 under 2 s | Over 30 s |
| MREC heartbeat ordering over 7 days, 3 Regions | No replica shows a heartbeat written 10 s after an item while lacking that item | Any case: write-class credential checks go to the issuing cell across Regions |
| `refresh` at 10 commits per second on one repository | p99 at most 3 rounds | Over 10% beyond 5 rounds |
| Intent push of 1,000 refs beside 50 concurrent small pushes | p99 under 3 s, and no reader ever sees part of it | Any partial read |
| Ten times read spike on a pre-warmed directory and cell table for 10 minutes | Throttled requests under 0.1% | Over 1% |
| Units per push, merge, and API read | Within 1.5 times `cost_ddb.py`'s `OPS` | Cost section restated |

**Milestone 1: one Single cell serves git.** `trib-store` with `CellStore`, `Write`, items, `refresh`, the git role from `DESIGN.md`, and packs. Gate: stock git 2.39 and 2.54 push and fetch through three nodes for 24 hours under node kills with a linearizability checker, zero acked pushes lost.

**Milestone 2: Paired.** Journal, lease, standby applier, resolver, promotion, switchover, rejoin. Gate: weekly switchover and monthly promotion drills with git traffic, the Milestone 0 promotion gate repeated with real pushes.

**Milestone 3: pull requests and lists.** Issues, PRs, reviews, checks, list items, merges, fork merges, merge queue, intent pushes. Gate: `gh`, Desktop, and VS Code oracles (`DESIGN.md` "Wire compatibility") plus a read-your-writes checker that runs journeys 2 and 3 a million times.

**Milestone 4: events and Actions.** Pending rows, consumers, webhooks, Actions. Gate: `actions/runner` conformance and, after a promotion, every event delivered at least once with stable GUIDs.

**Milestone 5: installs.** Single self-hosted in a Region without DSQL, two-Region BYOC, per-repository and whole-install restore with the cut-transaction reconcile, upgrades across a format change. Gate: journeys 5 and 7 as drills in the install machinery.

**Milestone 6: managed identity.** Claims, directory, fronts, token encoding, coverage waits. Gate: journeys 4 and 6 as drills, and the 10 times read spike on a canary front cell.

**Milestone 7: fleet.** Placement, slot moves between cells, cell pairs across Regions, wave plans.

## Changes to DESIGN.md

**Deletions.**
- Summary: the DSQL `core` and `aux` clusters, the geo authorities, bounded stale fetches, and the DSQL Milestone 0 gates.
- Shape: "Two clusters per cell" and "Why Aurora DSQL" with its limits table.
- Namespace and cell placement: `PartClass`, `PartKey`, watermark `Lookup`, `MAX_LAG`, `BoardRow`, `MEMBER_LEASE`, `BoardLease`, `GeoView`, `absent_allowed`, `depends_on`, `auth-apply`, the `g_*` replica, and publishers that follow three logs.
- Geographies, the claim registry, and the authorities: the three DSQL authorities, writer leases, escrow, `reconstitute`, `Handoff`, and change logs.
- Repository storage and ref transactions: the SQL `commit_push`, `ref_shard`, `ref_tombstone`, 1 MiB `pack_inline` in DSQL, bounded stale fetches, Regional `dr` through CDC and `dr-apply`, and the `ref_log` reconcile.
- Event outbox: the SQL tables, 256 partitions, and per-partition `ConsumerLease` rows.
- Availability, Deployment modes, Install operations: every DSQL, authority, board, `dr`, and ARC routing-control row.

**Replacements.**
- Summary and Shape: this design's Summary and Shape; `Durability::{Regional, Paired}` becomes `Pairing::{Single, Paired}`.
- Repository storage: `refs` items, 40 heads, `refresh`, the intent push, inline packs at 32 KiB, `Acked` from `CellStore::commit`.
- Identity and tokens: tokens, memberships, and grants in their home cell's collections, the claims table, the directory, and the issuing cell encoded in tokens.
- Issues and pull requests: item collections, list items, and `merge` as one `Write`, with the check summary as a condition.
- Event outbox: `Event` and `Pending` items, consumers placed by `NodeLease` rendezvous.
- Availability, Deployment modes, Install operations, Build order: this design's Downtime, Failure modes, Deployment modes, and Build order.

**Additions.** The journal, home lease, promotion, switchover, `hold`, and rejoin; the standby applier and verifier; per-repository point-in-time restore; the `FormatGate`; Single self-hosting in all 34 DynamoDB Regions. Kept by reference: wire compatibility, upstream git and NVMe caches, Actions, the web app, slots, `NameEpoch`, `RESERVED`, front cells, waves, signing, and the operator role.

## Tradeoffs accepted

- We accept two MRSC writes per acknowledged write in Paired cells, about 30 to 60 ms [guess], in exchange for RPO 0 with no per-repository item, no MREC conflict, and no repository log.
- We accept that a Paired cell stops reads and writes while its journal table is impaired for longer than the 8 s lease plus 30 s hold, a dependency on a 99.999% service (`dyn#22`), with a customer-signed `solo` intent as the only way to keep writing without it.
- We accept that a write committed at a lost home but never acknowledged can vanish even if a reader saw it there, unless an acknowledged write read from it.
- We accept full node capacity and a second copy of every table in the peer.
- We accept the intent protocol for atomic pushes above 44 refs, one at a time per repository.
- We accept that a `Query` is not a snapshot, so `refresh` loops and the list assembler dedups, and a fetch racing a multi-shard single `Write` on a repository committing over 100 times per second may serve per-shard-consistent refs once.
- We accept list filters that are strong only on the fixed dimensions, with `mentioned` and free text on the eventual search tier.
- We accept a cross-Region membership read for writes to a fork owned by an org homed in another Region, and a 10 s directory fence for tokens issued in other Regions.
- We accept that forks live in their network's home Region, so a Tokyo user's fork of a US project is homed in the US.
- We accept no ad hoc SQL: analytics run on exports to S3 [EXP].

## Alternatives considered

- **MREC for the authoritative tables, with a fenced per-repository home epoch.** Every Region holds a warm writable replica and AWS replicates. It lost because MREC exposes partial transactions at replicas [TX], resolves conflicts by last writer wins [GT], and has RPO "usually a few seconds" [GT]: a promoted Region needs receipts to reach RPO 0 anyway, and a partitioned old home's queued writes would overwrite untouched items after promotion, which no epoch on the item can stop because LWW compares timestamps.
- **Packs and per-repository manifests in S3 in two Regions.** It lost because a manifest per repository is a per-repository commit log with one CAS chain (`gs#29`-`#31`, about 120 pushes per second, `gs#30`), and S3 has no cross-Region conditional write, so RPO 0 needs a second mechanism.
- **The baseline, DSQL rows plus DSQL geo authorities** (`DESIGN.md`). It keeps snapshots and SQL. It lost on downtime and reach: a connection-quota and engine-upgrade surface on every git request (`up#27`), 20 Regions and no cross-continent clusters (`dyn#26`, `#27`), and the board, lease, and log machinery its worldwide layer needs.
- **Kinesis Data Streams for DynamoDB as the event bus.** It has longer retention and fan-out [KDS], but records "might appear in a different order" and "more than once" [KDS], and it is one more store. Pending rows give at-least-once with no new store.

## Open questions and risks

- Does MRSC support `Query` with `ConsistentRead` across a partition, and at what latency? [GT] promises strong reads of items; the promoter's enumeration depends on it. Milestone 0 tests it, with a `GetItem` fallback design named.
- Does MREC ever deliver a later item before an earlier one from the same writer by more than seconds? The cross-Region write-class token fence assumes not [inference].
- What is the real CAS ceiling of one head item, and does adaptive capacity isolate head items fast enough under a burst? [AC] gives no timing.
- Can a `TransactWriteItems` commit long after its client gave up? The resolver relies on replaying with the same `ClientRequestToken` within its 10-minute window [TX] to learn or force the outcome.
- How long do PITR restores of 100 GB to 1 TB tables take? AWS states no bound, so journey 7's RTO is [unverified].
- Is the journal's delete cost worth replacing with weekly journal-table generations, which trade replicated deletes for a planned control-plane step?
- Residency: per-cell Regions satisfy data residency for repositories and accounts, but the managed directory replicates logins, repository names, and token hashes to every front Region, which a residency customer may reject.
