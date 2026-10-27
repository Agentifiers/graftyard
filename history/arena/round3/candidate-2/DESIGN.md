# The whole global tier in DynamoDB global tables: an MRSC authority, MREC presence, stream relays, and deny leases

## Problem

`DESIGN.md` keeps the namespace's facts in `global`, one Aurora DSQL cluster in the North American Region set. A Frankfurt or Tokyo identity write pays a cross-continent round trip, a North American Region-set event stops identity writes everywhere, and no identity fact lives in a customer's geography. Designed for worldwide operation from day one, the global tier must commit a claim race-free from any continent, survive the permanent loss of any one continent with no acknowledged write lost, and still never let a request read a remote store.

DynamoDB global tables can do the first two, and they take away the tool that `DESIGN.md`'s freshness rule rests on: a commit-ordered log. AWS sources used below, all read on 2026-10-24:

- [GT] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html. MRSC spans exactly three Regions, as three replicas or two plus a witness, chosen at creation, with no replica added or removed later. Any three of 15 Regions may be combined across continents (also [NEW] https://aws.amazon.com/about-aws/whats-new/2026/09/dynamodb-mrsc-additional-regions/). Sydney is not among the 15. A write returns after synchronous replication to at least one other Region. Strongly consistent reads and conditional writes see the latest version. Concurrent cross-Region writes to one item fail with `ReplicatedWriteConflictException`. MRSC has no transactions and no TTL. Streams are off by default on MRSC and are enabled per replica, and "Streams records on MRSC replicas are identical for every replica, including Stream record ordering". MREC resolves conflicts by last writer wins per item, evaluates conditions against the local Region's version, exposes partial transactions in other Regions, and orders stream records per item only.
- [BP] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/bp-global-table-design.html. An MRSC Region serves reads and writes while a second replica or witness Region is reachable, and otherwise only eventually consistent reads. A Region that comes back fails strong reads and writes until it catches up. Eventually consistent reads can miss recent writes, even in the same Region. This page still says MRSC cannot span Region sets, which [GT] and [NEW] supersede.
- [ST] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Streams.html. A stream orders records per item, not across items or partitions, keeps them 24 hours, and throttles more than two readers per shard. [SR] https://docs.aws.amazon.com/amazondynamodb/latest/APIReference/API_streams_StreamRecord.html: `ApproximateCreationDateTime` is rounded down to the second.
- [RC] https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/HowItWorks.ReadConsistency.html: `Query` and `Scan` take `ConsistentRead`, GSIs never do. [CB] https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/compare-timestamps-with-clockbound.html: ClockBound gives each instance an earliest and latest bound on true time.

So a follower can never prove it has seen every commit up to a time, and stream timestamps are one second coarse. This design moves the proof to the writer: no identity write is acknowledged until every follower that may say "no" has applied it or has lost the right to say "no". Latency, Region RTTs, and costs below are [guess] unless cited.

## Usage (caller's view)

An `identity` handler in any data cell builds one `Change` and calls `write`, the only function that acknowledges an identity write:

```rust
let c = Change::new(Authority::Global, Class::Revoke, Key::Org(acme))       // fence: acme's org item
    .effect(Key::Member(acme, mira), Tombstone)?;                          // rejected unless acme fences it
let acked = write(&authority, &board, c).await?;                          // commit, then cover, at most 6 s
if !acked.lagging.is_empty() { observer.note(acked.lagging) }              // each one fails closed within 30 s
```

An edge answers from its projection and turns a miss into "no" only under a lease: `match route(&proj, &class, &who) { Lookup::Found(t) => forward(t), Lookup::Absent(_proof) => not_found(), Lookup::Behind => wait_then_503() }`. `authorize` keeps its signature and reads the cell's lease in the same `core` snapshot as the `g_*` rows.

**Journey 1: Frankfurt signup as `lena`, a PAT, and a push to `acme/tools` in North American cell `c7`.** (1) Route 53 sends the browser to an eu-central-1 front cell, which routes `IdentityWrite` to healthy cell `cE` in eu-central-1. (2) `cE` commits one conditional `PutItem` of `name#lena` to the authority's eu-west-1 replica, carrying `acct#L`, the password `secret#L`, and the index cell as effects. eu-west-1 returns after us-east-2 has it, which waits on North America (about 80 ms). The session row and the plaintext email go to `presence-eu`, the MREC table in eu-central-1, which waits on nothing remote. (3) `write` finds the change in the us-east-2, eu-west-1, and ap-northeast-1 relays' feeds and waits until every applier and publisher on the board covers it, about 0.8 s, then returns. (4) acme's owner adds lena to `acme/tools`, one `Grant` change fenced on `repo_dir(tools)`, acknowledged the same way. (5) lena creates a PAT: one conditional `UpdateItem` of `acct#L` that commits `cred#hash` as an effect, about 0.1 s plus the 0.8 s cover. (6) She pushes one second later. Her Frankfurt edge may not hold the hash yet, so it reads its publisher's head in the same Region, finds it, and routes by slot to `c7` (us-east-2), whose replica covered both changes before they were acknowledged. No `Behind` wait occurs. The push waits on North America because the repository is there. No step reads the authority.

**Journey 2: a Tokyo user renames their login while every North American Region is impaired.** The Tokyo front routes to a Tokyo cell. That cell writes to the ap-northeast-1 replica, which keeps quorum with eu-west-1 [BP] (Tokyo to Ireland is about 220 ms). Three sequential writes: lock `acct#U`, claim `name#new`, commit `acct#U` with `name_epoch + 1`, the only row a rename bumps, plus `name#old = Former { retiring }`. That takes about 0.7 s. North American appliers and publishers cannot renew their leases, so `write` stops waiting for them within 5 s and returns them in `lagging`. AP and EU fronts route the new login at once. The old login cannot be claimed until the retirement saga, which reads cards in the user's index cell, finishes after North America recovers. North American members rejoin only after they have caught up (deny lease rule below).

**Journey 3: an owner in Virginia removes a member who lives in Frankfurt.** A us-east-1 front routes to a us-east-1 cell, which writes the `Revoke` change above through the us-east-2 endpoint: one conditional `UpdateItem` on `org#acme`, which waits on eu-west-1 for about 80 ms. `write` returns `covered` once every applier, publisher, and serving edge has applied it, p99 under 2 s, at most 5 s. The member's next push through a Frankfurt front authenticates (the token is still valid) and reaches the repository's cell, whose `authorize` finds no `g_membership` row in its snapshot and refuses. The bound is the `write` return for every covered cell, and 30 s for a cell cut off from the feed. A push already past authorization when the removal applies can still commit, as in `DESIGN.md`.

**Journey 4: an anonymous request for an unknown login at a Sydney front while ap-northeast-1's replication lags.** The edge misses in its projection, reads its publisher's head (ap-southeast-2 S3, same Region), and applies it. The publisher follows all three relays, and the Tokyo relay is behind, but the us-east-2 and eu-west-1 relays are current, so its deny lease is valid and the edge answers 404 at once. If the publisher itself had fallen behind, it would stop renewing, and within 5 s the edge would answer `Behind`, wait up to 2 s, then return 503 with `Retry-After`, never 404. No step waits on another continent.

**Journey 5: Europe's Regions are lost for good.** The global authority keeps writing on us-east-2 and ap-northeast-1. Every acknowledged change was on two Regions [GT], so none is lost. The eu-west-1 relay and board bucket are gone: followers apply the other two relays, and the board's 2-of-3 quorum now needs both survivors. European members' leases lapse within 5 s, so writes stop waiting for them. Lost for good, because those facts lived only in Europe: `presence-eu` (sessions, which users recreate by signing in, and plaintext emails, which users re-verify against digests kept in the authority), the EU domain authority (resident enterprises' identity), and, per `DESIGN.md`'s cell model, the repositories of Paired cells inside Europe. Restoring a third Region uses `reseat` (Shape).

## Shape

```rust
/// Where an MRSC fact lives. Trios are three full replicas, no witness, so every continent has a stream and a
/// local endpoint. A trio cannot change after creation [GT]; `reseat` replaces the table.
pub enum Authority { Global, Domain(Domain) }
pub enum Domain { Eu, Ap }                     // resident enterprises only ("Residency")
pub struct Trio(pub [Region; 3]);
pub const GLOBAL: Trio = Trio(["us-east-2", "eu-west-1", "ap-northeast-1"]);   // us-east-1 stays out (implication 22)
pub const EU: Trio = Trio(["eu-west-1", "eu-central-1", "eu-west-3"]);
pub const AP: Trio = Trio(["ap-northeast-1", "ap-northeast-2", "ap-southeast-1"]);
/// MREC, replicas only in one continent, TTL on. Holds only facts whose loss or last-writer-wins is harmless.
/// No follower reads it. Sign-in, settings pages, and notification workers do.
pub enum PresenceRow { Session(SessionId, SessionRow), LastUsed(CredentialId, Timestamp),
                       Email(AccountId, EmailRow), SignIn(AccountId, SignInRow), Device(DeviceCode, DeviceState) }
/// Every MRSC key. One closed list. `fence_of` is the single source of which aggregate serializes each key.
pub enum Key { Name(LoginKey), Shortcode(Shortcode), Account(AccountId), Secret(AccountId), Credential(TokenHash),
    SshKey(Fingerprint), EmailDigest(Digest), Org(OrgId), Member(OrgId, UserId), Team(TeamId),
    TeamMember(TeamId, UserId), RepoName(AccountId, RepoNameKey), RepoDir(RepoId), Retired(LoginKey, RepoNameKey),
    Grant(RepoId, Grantee), App(AppId), Installation(InstallationId), InstallationRepo(InstallationId, RepoId),
    VerifyingKey(Kid), Slot(Slot), SsoAuth(CredentialRef, OrgId), SessionCut(SessionId), IdBlock(IdKind), Meta }
pub fn fence_of(k: &Key) -> Key { todo!() }   // Member, Team, TeamMember -> Org; Grant, RepoName -> RepoDir; ..
/// Stored item. `ver` grows by one on every write. Followers keep an item only if its `ver` is newer.
pub struct Item { pub key: Key, pub ver: u64, pub last: ChangeId, pub body: Option<Body>, pub pending: Option<Pending> }

pub struct ChangeId(Ulid);                     // the identity request id, so a retried write is the same change
pub enum Class { Grant, Revoke }               // Revoke also waits for edges
/// One identity write. Every effect's key must be fenced by `fence`, be one of `claims`, or be a new item created
/// with `expect: None` under a fresh id (checked in `new`).
/// The whole change is stored in the fence item, so followers receive it as one stream record.
pub struct Change { pub id: ChangeId, pub authority: Authority, pub class: Class, pub fence: Key,
    pub claims: SmallVec<[Claim; 2]>,          // keys that must be absent, Alias, or Former past retirement
    pub effects: ArrayVec<Effect, 200> }       // bounded so the fence item stays under 400 KB [guess on size]
pub struct Effect { pub key: Key, pub expect: Option<u64>, pub ver: u64, pub body: Option<Body> }   // None: tombstone
pub enum Pending { Locked(Change), Committed(Change) }
pub trait AuthorityStore {
    /// 1 Lock: conditional write of Locked(c) on the fence (no pending, ver = seen). 2 Claim: conditional put of each
    /// claim key carrying `by = c.id`. 3 Commit: conditional write of Committed(c) on the fence, the commit point.
    /// 4 Materialize, after returning: write each effect where ver = expect, then clear `pending`.
    /// With no claims besides the fence, steps 1 to 3 are one write: signup, PAT, member removal, uninstall.
    /// A change that finds a fence `pending` for over 10 s finishes it (Committed) or aborts it and releases its
    /// claims (Locked). Commit is conditional on its own Locked(c), so a slow writer and a helper cannot both win.
    /// ReplicatedWriteConflictException retries 3 times with jitter under the node's 10% retry budget.
    async fn commit(&self, c: Change) -> Result<Committed, CommitError>;
}
pub enum CommitError { Taken(Key), Retired, Stale, Frozen, Unavailable }
/// Constructed only by `write`. Commit, find the change in each live relay's segments (put it in that relay's inbox
/// if absent after 1 s), then wait until every member following `c.authority` covers it or its deny lease has
/// lapsed, and for Revoke until every serving edge, as reported by its publisher, covers it. After 30 s: 503, change
/// committed, and a retry with the same id re-drives coverage.
pub struct Acked { pub change: ChangeId, pub lagging: Vec<Follower> }
pub async fn write(a: &dyn AuthorityStore, b: &Board, c: Change) -> Result<Acked, WriteError> { todo!() }

pub struct RelayId { pub authority: Authority, pub region: Region }
pub struct SegNo(u64);
/// One active reader and one standby per replica stream, inside the two-reader limit [ST]. Follows shard lineage,
/// reads its inbox, and emits a segment every 100 ms, empty or not, to its own Region's feed bucket (If-None-Match).
pub struct Segment { pub relay: RelayId, pub seq: SegNo, pub emitted_at: Timestamp,
    pub items: Vec<Item>, pub committed: Vec<Change>, pub ids: Vec<ChangeId> }
pub async fn relay(stream: ReplicaStream, inbox: Inbox, out: FeedBucket) -> Result<Never, RelayError> { todo!() }
/// Follower side (`global-apply`, front publisher): all live relays of each followed authority, in order per relay.
/// A Committed change applies every effect in one `core` transaction or one projection segment. Items apply if newer.
pub fn apply(dst: &mut dyn Replica, s: &Segment) -> Result<(), ApplyError> { todo!() }

pub enum Member { Applier(CellId), Publisher(FrontCellId) }
pub enum Follower { Member(Member), Edge(EdgeId) }
pub struct BoardRow { pub member: Member, pub follows: SmallVec<[Authority; 2]>, pub applied: RelayMap<SegNo>,
    pub edges_applied: RelayMap<SegNo>, pub edges_behind: Vec<EdgeId>,   // publishers only: min over serving edges
    pub deny_until: Timestamp, pub grant_until: Timestamp }
/// Rows go to the board buckets in the GLOBAL trio Regions, and a compactor in each merges them every 250 ms into
/// one immutable object. A member's renewal counts once two merged boards show it. A writer reads two merged boards
/// built within PUBLISH_DELAY and keeps the newer row per member. Any two of three intersect, so no writer sees a
/// lease older than one the member uses.
pub struct Board { /* .. */ }
pub const DENY_LEASE: Duration = Duration::from_secs(5);
pub const GRANT_LEASE: Duration = Duration::from_secs(30);    // DESIGN.md's MAX_LAG, now a lease
pub const PUBLISH_DELAY: Duration = Duration::from_secs(1);
pub const FRESH: Duration = Duration::from_secs(2);
pub const BEHIND_WAIT: Duration = Duration::from_secs(2);     // then 503 with Retry-After, never 401 or 404
/// A member renews both leases each second only while, for each followed authority, some relay that emitted within
/// FRESH has been applied through a segment emitted within FRESH. A lease is usable from PUBLISH_DELAY after two merged
/// boards show it, and a member whose lease had lapsed must first apply every live relay's head read after that time.
/// The holder uses a lease while ClockBound `latest() < until`; a writer counts it lapsed once `earliest() > until` [CB].
pub struct DenyLease { from: Timestamp, until: Timestamp }
pub struct GrantLease { until: Timestamp }
pub struct NoProof(());                                       // private: only DenyLease::prove makes one
impl DenyLease { pub fn prove(&self, now: &ClockBound) -> Option<NoProof> { todo!() } }
pub enum Lookup<T> { Found(T), Absent(NoProof), Behind }
/// Edge: a projection miss becomes Absent only after the edge applies its publisher's head (same-Region S3) and
/// holds that head's deny lease. One head read in flight per edge, shared by every waiting miss.
pub fn confirm_absent(p: &mut Projection, head: &PublisherHead, now: &ClockBound) -> Lookup<()> { todo!() }
/// `global-apply` writes both leases into `core` in the transaction that applies or renews, so `authorize` reads
/// the lease and the rows it judges in one snapshot: Forbidden and Hidden need `prove`, a write-class Allow needs
/// the grant lease.
pub struct GLease { pub deny: DenyLease, pub grant: GrantLease }
/// Reads `secret#A` eventually consistently from the nearest replica and accepts it only if its `ver` is at least
/// the cell's `g_account.secret_ver`, otherwise one strong read. Writes the session to presence. Needs the grant lease.
pub async fn sign_in(req: SignIn, cell: &CellCtx) -> Result<SessionCookie, AuthError> { todo!() }
/// `Key::Meta` in each table. Writers read it strongly once a second. reseat: Copy (scan, then follow the old
/// table's relays by version), Freeze (old Meta = Frozen { to }), Drain (30 s write deadline, then relays to end),
/// Flip (new Meta = Serving), the slot-move pattern applied to the table.
pub enum Generation { Serving { table: TableId }, Frozen { to: TableId } }
pub async fn reseat(from: TableId, to: TableId) -> Result<(), ReseatError> { todo!() }
```

Invariants and decisions:

- **Claims are race-free.** A claim is a conditional put on one MRSC item, evaluated against the latest version [GT]. Two creators of `newco` cannot both succeed, and the loser gets 422 `already_exists`.
- **A multi-item change has one commit point and arrives whole.** A claim, its account row, and its first membership are one `Change` committed by one conditional write of the fence item, here `name#newco`, which carries all three after-images. Followers apply them from that one stream record in one transaction, so no cell ever sees the claim without the membership. Materialized copies follow and change nothing at followers (same `ver`). A strong reader that meets a committed but unmaterialized fence finishes it first. A rename uses lock, claim, commit on `acct#U`. A repository rename fences on `repo_dir`. Org membership, team, and policy changes serialize on `org#O`, so an admin's removal and that admin's concurrent grant cannot both commit, which the replica-only check in `DESIGN.md` could not prevent.
- **"No" holds against acknowledged writes.** `Absent` needs a `NoProof`, and `write` acknowledges only after each deny-lease holder covered the change or lost its lease. A request that arrives after a write's acknowledgment therefore never gets "no" about it. Weakening against `DESIGN.md`: between commit and acknowledgment (p99 about 1.5 s, at most 6 s), one follower may answer "yes" while another answers "no". The writer's own client has been told nothing yet, so the creator never sees a contradiction, but two other observers can. A write whose writer crashes before acknowledgment is in this window until the stream delivers it.
- **Revocation coverage counts edges.** A `Revoke` change returns `covered` only when every applier, publisher, and serving edge has applied it. Edges report their applied segment to their publisher, which posts the minimum and the edges below it. An edge that has not applied its publisher's head from 30 s ago, or whose publisher's grant lease lapsed, fails its health check and leaves rotation.
- **A cut-off cell fails write grants closed after 30 s.** It cannot read the feeds, so it stops renewing, and its grant lease in `core` expires 30 s after the last renewal. Reads continue within ACT-14's stale limit, as before.
- **Renames bump one row.** An owner rename bumps `acct#U.name_epoch`, and `check_epoch` and `NameEpoch` are unchanged.
- **No request reads a remote store.** Edges read their projection and their own Region's publisher head. Cells read `core`. Only identity writes and sign-in reach the authority, as `DESIGN.md` already allowed.
- **When the whole authority is down,** nothing new commits, relays emit empty segments, members stay fresh, and "no" stays immediate, as in `DESIGN.md`. When every relay is down, "no" also stays correct, because nothing can be acknowledged. When the board quorum is lost, every lease lapses, "no" becomes `Behind`, and no write can be acknowledged. Every board fault moves toward 503, never toward a wrong "no".
- **The store is a property of the type.** Every `Key` is an MRSC fact. `PresenceRow` is the whole MREC list. Memberships, grants, credentials, installations, and session cuts cannot be written last-writer-wins, because MREC can silently drop a removal that races another write [GT]. Display profile fields are public and live on `acct#`, so followers get them through relays.
- **Ids** come from `IdBlock` items: one conditional `ADD` leases 65,536 ids per node per kind, so no hot counter crosses continents. Domain blocks are recorded in the global table, so a front tells a domain id by its range.
- **The relays and the inbox replace the ordered log.** Per-item order comes from `ver` and from the stream [ST]. Completeness comes from `write`. The inbox covers a stalled stream or a relay that drops records. After a stream outage beyond 24 hours, a relay rebuilds with a parallel `Scan` into a snapshot segment [RC].

Dominant access patterns: an authenticated request does two to four projection lookups and one `core` snapshot read, as before. An identity write costs one to three sequential MRSC writes (each about 80 to 220 ms, because it waits on the nearest other continent) plus a cover wait of 0.5 to 1.5 s. Follower load does not depend on request rate: three relays per authority, each follower polling three feeds every 100 ms, and about 130 board rows.

## Failure modes

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| Authority (MRSC tables) | instance | Single calls fail or raise ReplicatedWriteConflictException | Bounded retries under the retry budget. A retry carries the same `ChangeId`, so it is the same change. |
| Authority | az | None; DynamoDB spans AZs | Nothing to do. |
| Authority | region | One replica: its endpoint and relay | Writers there use the next-nearest replica. Two Regions keep quorum [BP]. A returning Region fails strong reads and writes until caught up, so writers stay off it until a canary change passes there. |
| Authority | control-plane | None at runtime | Tables, per-replica streams, deletion protection, and PITR on each replica [GT: not synchronized] are created at install. Only `reseat` after a permanent loss calls a management API, and availability never waits on it. |
| Authority | bad-deploy | Wrong items service-wide, the worst global failure | `trib-authority` is the only writer and uses a fixed set of condition expressions, checked by a model checker and by a Jepsen-style harness under FIS isolation. It ships in the last wave after a week on canary cells. Body versions follow the two-release rule, and followers park unknown versions. A daily audit alarms on pending fences older than 10 minutes and on orphan claims. |
| Authority | bad-config | Writes fail in the misconfigured cells | Readiness probe commits a canary change and finds it in a relay. Signed config, last known good. |
| Authority | noisy-tenant | A signup flood or one org's SCIM sync hammering one fence | Front buckets per IP, ASN, and account. Up to 200 effects per change. Per-fence backoff on write conflicts. On-demand maximum throughput caps cost. |
| Authority | continent | Global: one Region. Domain trio: the whole domain | Global: as `region`, with no redundancy left until `reseat` (minutes of frozen identity writes, no read impact). Domain: resident identity is lost along with the residents' repositories, a stated residency cost. |
| Presence (MREC tables) | instance | Single calls fail | Retries. |
| Presence | az | None | Nothing to do. |
| Presence | region | Rows not yet replicated, seconds' worth [GT] | Writers use another replica in the continent. A lost session makes the user sign in again. A cookie alone never grants access. |
| Presence | control-plane | None | TTL and replicas set at install. |
| Presence | bad-deploy | Wrong session, email, or last-used rows | Fronts check cookies against the service key and `sessions_not_before` in the authority, so a bad row cannot widen access. Same waves. |
| Presence | bad-config | Sign-in fails in the cells that hold the bad config | Probe and last known good. |
| Presence | noisy-tenant | One account creating sessions in a loop | Per-account and per-IP sign-in buckets. |
| Presence | continent | That continent's sessions, plaintext emails, sign-in history | Users sign in again and re-verify emails against the digests kept in the authority. Stated under "Residency". |
| Relays and feed buckets | instance | One relay pauses | The standby takes the lease within 10 s. Followers keep applying the other relays. |
| Relays and feed buckets | az | None | Standby in another AZ. S3 spans AZs. |
| Relays and feed buckets | region | One of three feeds per authority | Followers apply the other two. Writers find changes there. |
| Relays and feed buckets | control-plane | None | Streams and buckets pre-created. Stream and shard discovery are Streams data APIs. |
| Relays and feed buckets | bad-deploy | Malformed or missing records from one relay | Followers park that relay alone. Writers inject into its inbox. Relays roll one Region per wave, a day apart. |
| Relays and feed buckets | bad-config | One relay | Last known good. A relay that emits nothing for 2 s stops counting as live. |
| Relays and feed buckets | noisy-tenant | A bulk change floods every feed | Effects cap per change, at most 4 MB per segment, apply capped at 2,000 rows per second per cell, so lag grows instead of `core` load. Writes wait longer, and "no" becomes `Behind`. |
| Relays and feed buckets | continent | One relay for good | Two remain. `reseat` brings three new relays. |
| Lease board (buckets, compactors) | instance | A compactor pauses | Standby. Members whose renewals stop showing lose leases, so "no" fails toward `Behind`. |
| Lease board | az | None | S3 spans AZs. Compactor standby in another AZ. |
| Lease board | region | One of three board buckets | Quorum of two remains. |
| Lease board | control-plane | None | Buckets pre-created. |
| Lease board | bad-deploy | Leases not confirmed, or rows dropped | Members use only leases they saw in two merged objects, and writers keep the newer row, so a dropped row lapses a lease (fails safe). A compactor that inflates `deny_until` only makes writers wait longer. |
| Lease board | bad-config | One compactor | Probe and last known good. |
| Lease board | noisy-tenant | None; only fleet members write rows | Rows are fixed-size, one per member per second. |
| Lease board | continent | One bucket for good | Both survivors are now needed. A second loss lapses every lease (fleet-wide `Behind`) until `reseat` moves the board. |

## Residency

A public European or Asia-Pacific account keeps its plaintext email addresses, session rows, sign-in history, and last-used stamps in its continent's `presence` table (MREC replicas only in that continent). Its login claim, account row with public profile, password and 2FA secrets, credential hashes, SSH and GPG keys, email digests, memberships, grants, and installations are in the GLOBAL trio, so they are stored in North America, Europe, and Asia-Pacific. They leave because any front must authenticate the account and any cell must authorize it without a remote read, because a claim must be unique worldwide, and because these facts must outlive any one continent. Under `DESIGN.md` they left the account's continent for North America only. Now they also stay in it.

A resident enterprise (managed users, as GitHub offers residency) gets more. Its managed users, their credentials and secrets, its orgs' memberships, teams, grants, policies, repository names, and installations live in its domain trio (EU: Ireland, Frankfurt, Paris. AP: Tokyo, Seoul, Singapore. Sydney cannot host MRSC [GT]). The same `Change` and relay code serves them, and EU data cells and fronts follow that domain. Only five things leave: the shortcode claim, org login claims carrying a domain pointer, SSH fingerprint items holding only a domain pointer (SSH carries no hostname), the slot map, and global app registrations. Domain tokens put a domain code in their first random character (173 random bits remain), so a front outside the domain forwards those requests, one cross-continent hop, to the nearest domain front cell. The cost is that a domain's identity dies with its continent. That is the residency promise, and the residents' repositories die with it.

## Deployment modes

Every mode runs the same `trib-authority`, `relay`, `apply`, `Board`, lease, and `write` code. Only the table topology, the number of relays and board buckets, and whether domains exist vary. DynamoDB replaces `DESIGN.md`'s `global` schema inside `core`, so self-hosters gain a serverless table, not a server. The authority never uses `TransactWriteItems`, even where a single-Region table would allow it, so one code path covers all modes.

| Store or protocol | Managed | BYOC (vendor-operated, customer account) | Self-hosted (customer-operated, one Region) |
| --- | --- | --- | --- |
| Authority | MRSC GLOBAL trio plus domain trios | Paired in the MRSC list: MRSC on the cell's Regions, two replicas plus witness. One Region, or Paired outside the list: a single-Region table (99.99% SLA [GT]) | Single-Region table with a stream, on demand |
| Domains | EU, AP | None; the install is its own namespace and picks its Region | None |
| Presence | MREC per continent | Paired: MREC on the two cell Regions. One Region: single-Region table with TTL | Single-Region table with TTL |
| Relays | Three per authority | One per replica with a stream: two for Paired (a witness has none [GT]), one otherwise | One, a role on the install's nodes |
| Board | Three buckets, quorum 2 | Paired: buckets in a, b, and the witness Region, quorum 2. One Region: one bucket, quorum 1 | One bucket, quorum 1 |
| Lease holders | Appliers and front publishers | The one cell's applier and its one publisher | Same, all in one process on an evaluation node |
| `reseat` | After a permanent continent loss | Converting one Region to Paired, or Paired after losing a Region | Not used |

What collapses: with one Region, the trio, the continent rows, `reseat` for loss, and the board quorum disappear. `write` still waits for one applier and one publisher, about 0.2 s, so the evaluation node exercises the same coverage code that managed runs. An install with one Region still loses identity writes with that Region, as its cell already does. Where the code cannot match: domain routing and the forwarding of domain tokens exist only in managed mode. `ReplicatedWriteConflictException` never occurs on a single-Region table.

The customer provisions nothing by hand. The OpenTofu module creates the tables with streams (`NEW_IMAGE`), deletion protection and PITR on every replica, the feed and board buckets, and a customer CMK. For Paired the customer must enable the witness Region if it is opt-in. The existing deploy role's default `PowerUserAccess` and the workload boundary, which allows everything outside `iam:*`, `organizations:*`, and `account:*` (`research/byoc-grounding.md` items 4 and 6), already cover DynamoDB and Streams, so no new grant is needed. Nodes run the ClockBound daemon from the node image.

The vendor's BYOC machinery must add four things. (1) Multi-Region targets: the target document holds one `aws.region` (items 1 and 8), so the `tributary` block names `a`, `b`, and `witness`, and the root gets one provider per Region. (2) A one-way `authority:create` stage. An MRSC table must start empty and can never gain a replica [GT], so creating it, and every `reseat`, is a barrier (items 26 and 40). `reseat` runs as a migration stage with the time-fence and marker pattern (item 49). The tables carry `prevent_destroy`, because a rollback runs older OpenTofu that would destroy newer resources (item 36). (3) Observer routes and gates for relay lag, lease holders, cover p99, and pending fences older than 10 minutes, because `components` knows only ECS and Lambda (item 19). (4) A preflight check that the chosen Regions are in the MRSC list before `Paired` is accepted.

## Changes to DESIGN.md

Deletions:
- Summary: "one multi-Region DSQL cluster, `global`, commits every name..." and the "because no follower can have read a newer fact" clause. Milestone 0's change-log sentence.
- Shape intro: "one `global` DSQL cluster for global facts". The DSQL limits table's connections row: the `global` writer and follower budget.
- Namespace and cell placement: `GlobalPart`, `GlobalStore`, the watermark comment above `Lookup`, `CLOCK_MARGIN`, `BoardRow` with `newest_read`, `Projection.applied` as a `PartMap`, the "Negative answers wait for freshness, measured against the fleet" bullet, and "Projections come from S3, not from `global`" as worded.
- Identity and tokens: `revoke` and `RevocationReceipt`, the `global` read-load and 1,800-connection paragraph, and "`global` runs read-write in us-east-2 and us-west-2 with a us-east-1 witness... cross-continent round trip".
- Cross-cell features: `LogId::Global` and the `global` half of "One log mechanism, two uses". The log stays for inter-cell use only.
- Failure modes: all seven `global` cluster rows.
- Build order: the "`global` change-log gate" paragraph.
- Deployment modes: the `global` store row and the sentence "A single-cell install runs the same applier, which copies `global` rows into `g_*` tables inside one cluster".
- Public service concerns, Residency: "Logins, account ids, memberships, grants, and credential hashes are global facts in `global`'s North American Regions".
- Tradeoffs: "We accept `global` in North American Regions...". Alternatives: "The name authority in DynamoDB". Open questions: the DSQL snapshot-time, log-head-conflict, and `CLOCK_MARGIN` items, and "Is North American residency of identity facts acceptable".

Replacements:
- Every "`global` transaction" in the Usage journeys and Namespace bullets becomes "one `Change` through `write`", fenced as in Shape. Every "`revoke`" becomes `write` with `Class::Revoke`.
- `Lookup`, `MAX_LAG`, `BEHIND_WAIT`, and the negative-answer rule become this document's `Lookup`, `NoProof`, `DenyLease`, `GrantLease`, and `confirm_absent`. `route` drops `asked_at`. `Principal.asked_at` stays for assertion lifetime.
- The applier bullet: `global-apply` follows relays, applies `Committed` changes whole, writes `GLease` into `core`, and keys pending-emit rows by `ChangeId`. Front publishers follow relays and post edge summaries.
- "Denials wait, grants race" becomes "No holds against acknowledged writes", with the weakening stated in Shape.
- Sign-in follows `sign_in`: a local eventual read, a version check against the replica, and a presence session. It needs the cell's grant lease.
- SLOs: identity writes are measured per continent with p99 acknowledgment under 2.5 s. Replica freshness becomes the share of minutes in which each member holds its deny lease (99.9%). Revocation coverage becomes `write(Revoke)` returning with empty `lagging` within 5 s (99.9%).
- Failure modes: the Cell replica `region` and `bad-deploy` rows name relays instead of `global` partitions. The Front cell `bad-config` row keeps its guards and adds the 30 s edge rule.
- Deployment modes: the table above replaces the `global` store row.
- Milestone 0's `global` gate becomes the **authority gate** (about two days). It uses the GLOBAL trio, 200 writers on three continents running signup, PAT, rename, and org-removal changes contending on 64 org fences, three relays, 130 simulated lease holders, and FIS isolation of each Region in turn [GT]. It passes when a checker finds no acknowledged change missing at any lease holder, signup acknowledgment p99 is under 2 s, `Revoke` coverage p99 is under 2 s, and writes continue through every isolation. It flips the global tier back to `DESIGN.md`'s North American DSQL `global` with an MRSC claim registry if the stream delay from commit to relay segment has p99 over 2 s, or if cross-continent commit p99 exceeds 1 s.
- Milestone 2 builds the authority, relays, board, and `write` in place of `global`'s writer and log. Its gate adds a Frankfurt `gh auth login` followed within 100 ms by `gh api user` through a Tokyo front, which must succeed with no `Behind` wait. Milestone 4's 30-minute `global` blackhole becomes an IAM deny on all three replicas, with zero failed fetches, pushes, or merges, and with 404 and 401 served at once.

Additions:
- Namespace and cell placement gains this document's Shape sketch and invariants. Identity and tokens gains `PresenceRow`, `sign_in`, and domain token routing.
- Failure modes gains the 32 rows above. Drills gain a monthly FIS isolation of one GLOBAL trio Region and a quarterly continent day that cuts one continent off from the authority, the feeds, and the boards.
- Service and crate map gains the crate `trib-authority` (Change, commit, write, Board, leases) and the `relay` and `board` roles in `worker`.
- Public service concerns, Residency, gains this document's "Residency".
- Review resolutions gains WW-1, which records this change and supersedes the `global`, watermark, and `revoke` mechanisms of NS-1, NS-ACT-1, NS-ACT-2, and NS-ACT-3, keeping their guarantees as restated here.

## Tradeoffs accepted

- We accept that every identity write, North American ones included, waits on another continent for each sequential MRSC step (one to three steps, 80 to 220 ms each) plus a cover wait of 0.5 to 1.5 s, in exchange for identity writes that survive any Region or continent and need no failover call.
- We accept that "no" is guaranteed only against acknowledged writes, so followers may disagree for up to 6 s after a commit, in exchange for freshness without a commit order that DynamoDB lacks.
- We accept that one stuck lease holder can delay every identity write by up to 6 s once, until its lease lapses, in exchange for "no" answers that never contradict an acknowledgment.
- We accept that the store holds no multi-item atomicity, so changes are fenced, committed whole in one item, and materialized afterward with help-along, in exchange for claims, account rows, and first memberships that followers see together without cross-Region transactions.
- We accept throughput per fence item of about one change per round trip (a few per second per org [guess]), batched up to 200 effects, in exchange for org changes free of write skew.
- We accept three full replicas per trio, three relays, three board buckets, and S3 request costs [guess: tens of thousands of dollars per year], in exchange for every continent having a local stream and local reads.
- We accept that `reseat` after a permanent continent loss calls the DynamoDB control plane and freezes identity writes for minutes, in exchange for not running standby tables for every loss case.
- We accept that presence rows and domain identity die with their continent, and that resident tokens used outside their domain take a cross-continent hop, in exchange for identity data that stays in its geography.
- We accept DynamoDB as a third AWS store beside DSQL and S3 in every mode, in exchange for one authority code path.

## Alternatives considered

- **Keep `DESIGN.md`'s North American DSQL `global`.** It lost because a North American Region-set event stops identity writes worldwide and identity is never resident. DSQL cannot cross continents (`review/round2-judge.md`).
- **A global commit order on DynamoDB through a sequencer item per partition.** It lost because every commit would contend on cross-continent hot items, raise `ReplicatedWriteConflictException` [GT], and cap each partition at about one append per round trip.
- **Followers read the streams directly.** That is forbidden at fleet scale by the two-reader limit per shard [ST], hence relays.
- **A strong read of the authority before every "no".** It lost because scanners and typo traffic would put the authority on the request path, the `research/uptime.md#47` pattern.
- **MREC for memberships and grants with version checks.** It lost because last writer wins per item and conditional writes evaluated per Region [GT] let a removal race a concurrent update and lose silently.
- **Two replicas plus a witness for GLOBAL.** It is cheaper, but the witness continent would have no stream, no endpoint, and no local sign-in reads [GT].
- **Per-continent DSQL homes under a thin MRSC claim registry.** It lost because a membership across continents would need a home and a cross-home saga, and the facts homed in a lost continent would go with it.

## Open questions and risks

- What are MRSC commit p50 and p99 for us-east-2, eu-west-1, and ap-northeast-1? [GT] tells us to measure them, and the authority gate does.
- What is the delay from commit to stream record on MRSC replicas at p99? Coverage latency rests on it. [unverified]
- Do the three replica streams share sequence numbers? If they do, a follower could switch relays by position instead of applying all three.
- How often does `ReplicatedWriteConflictException` fire on large org fences during SCIM syncs?
- Is PITR supported and restorable per MRSC replica? [GT] lists PITR as not synchronized, which implies per-replica settings [inference].
- How long does `reseat` take at managed scale, and how often must it be drilled?
- Do EU customers accept residency for enterprise managed users only? Can Australian residency wait until Sydney hosts MRSC?
- Is ClockBound available on every instance type the node image runs on [CB]?
- What do the feed and board S3 requests cost at 30 front cells and 100 data cells? [guess]
