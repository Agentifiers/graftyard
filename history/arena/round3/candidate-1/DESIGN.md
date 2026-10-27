# Geo-homed identity authorities under a worldwide claim registry

## Problem

`DESIGN.md` commits every global fact in one Aurora DSQL cluster, `global`, in the North American Region set ("Identity and tokens", access patterns). Three costs follow. Every identity write from Europe or Asia-Pacific pays a cross-continent round trip, a North American Region-set event stops identity writes worldwide ("Tradeoffs accepted", the `global` bullet), and no identity fact is ever stored in its owner's geography ("Public service concerns", Residency). The operator now allows DynamoDB global tables (`FRAME.md`, "Operator decision, 2026-10-24, later"). This design treats worldwide operation as a day-one requirement.

Five AWS facts decide the shape. Each was read on 2026-10-24.

1. DSQL multi-Region clusters live inside one of three Region sets: North America (us-east-1, us-east-2, us-west-2, ca-central-1, ca-west-1), Europe (eu-central-1, eu-west-1, eu-west-2, eu-west-3, eu-south-2, eu-north-1), and Asia-Pacific (ap-east-1, ap-south-1, ap-northeast-1, ap-northeast-2, ap-northeast-3, ap-southeast-1). Sydney and Melbourne run DSQL single-Region only (https://docs.aws.amazon.com/aurora-dsql/latest/userguide/what-is-aurora-dsql.html).
2. An MRSC global table spans exactly three of 15 Regions, in any combination, including one per continent. A write returns after it replicates synchronously to at least one other Region. Conditional writes evaluate against the latest version. MRSC has no transaction APIs, and replicas cannot be added or removed later (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html; https://aws.amazon.com/about-aws/whats-new/2026/09/dynamodb-mrsc-additional-regions/). A Region keeps serving reads and writes while one other Region of the three is reachable (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/bp-global-table-design.html, "Key facts about MRSC"). That best-practices page still lists the older single-continent Region sets, so it disagrees with the two newer sources. This design follows the newer ones.
3. DynamoDB Streams orders records per item only, not across items or partitions. MRSC replica streams are identical in every Region, ordering included. MREC replica streams can order different items differently (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Streams.html; HowItWorks, "Streams"). No DynamoDB stream can say "you have seen every write committed before T", which is the watermark that `Lookup::Behind` rests on (`DESIGN.md`, `Lookup`).
4. MREC resolves concurrent writes to one item by last writer wins on an internal timestamp, evaluates conditions against the local Region's copy, exposes partial transactions in other Regions, accepts any number of Regions, and publishes no SLA on replication delay (HowItWorks; best-practices page, "Key facts about MREC"; `research/uptime.md#30`).
5. A Region and its data can be lost for good, and in 2026 two Regions of one geography were lost together (`research/uptime.md#37`, `#38`).

So a global fact is one of two kinds. A key that every owner on Earth competes for (a login, an enterprise shortcode, an app slug, an SSH key fingerprint, a verified email digest) needs one answer worldwide. It is a single item, so a conditional write in MRSC is its commit point. Every other global fact belongs to one aggregate root, an account or an org. Such facts need multi-row transactions and the gap-free, commit-ordered change log that freshness rests on, and only DSQL provides those. DSQL cannot cross a continent, so each continent group gets its own authority, and each authority owns exactly the roots homed there. Reads keep the base design's rule: every request reads local replicas, never an authority and never the registry.

## Usage (caller's view)

A user picks a home geography at signup. The default is the continent of the front cell that served the page. South America maps to `Na`, Africa and the Middle East to `Eu`, and Oceania and India to `Ap`. An org's creator picks the org's home. Nothing on the wire changes. Tokens keep GitHub's prefixes and length.

| Store | Regions | What a write waits on |
| --- | --- | --- |
| Claim registry `trib-claims` (DynamoDB MRSC, three replicas) | us-east-2, eu-central-1, ap-northeast-1 | The nearest other replica, always on another continent |
| Authority `auth-na` (DSQL multi-Region) | us-east-2, us-west-2, witness us-east-1 | North America only |
| Authority `auth-eu` | eu-central-1, eu-west-1, witness eu-west-3 | Europe only |
| Authority `auth-ap` | ap-northeast-1, ap-southeast-1, witness ap-northeast-2 | Asia-Pacific only |
| Freshness board `trib-board` (DynamoDB MREC) | Every Region with a data cell or front cell | Nothing remote; a local write |

The registry sees only signup, org and enterprise creation, login renames, SSH key changes, email verification, app creation, and slot flips. Token creation and revocation, sign-in, memberships, teams, grants, installations, and repository create, rename, and delete each touch only the home authority of their root. A repository transfer between owners homed apart, a rehome, and an account deletion with memberships in other geographies are sagas over two authorities. Round-trip times below are [guess]: Frankfurt to Ohio 95 ms, Tokyo to Ohio 140 ms, Tokyo to Frankfurt 225 ms, Sydney to Singapore 95 ms.

**Journey 1. `lena` signs up in Frankfurt, creates a token, forks `acme/widgets` (network in NA cell `c7`), and pushes within one second.**

1. `POST /join` reaches a Frankfurt front cell (eu-central-1) through Route 53 latency records. The edge classifies it as `IdentityWrite` with home `Eu` and forwards to an EU data cell `c31` (eu-central-1 and eu-west-1). The edge reads only its projection, which shows `lena` is not reserved.
2. Pending. `c31`'s `identity` handler runs one `auth-eu` transaction through the eu-central-1 endpoint, committed with eu-west-1 and the eu-west-3 witness. It inserts `account(A, Pending)` with `A` from `auth-eu`'s sequence. No log record names `lena` yet.
3. Claim. A conditional `PutItem` on `login#lena` (absent, or `Alias`) goes to the eu-central-1 replica, which replicates to us-east-2 before it returns. This is the journey's only wait on another continent's store, one round trip of about 95 ms.
4. Activate. One `auth-eu` transaction sets `A` active and appends `name(lena) = Live { A, v: 1 }` to A's shared partition of the EU log. It writes the password hash, email address, and 2FA rows to home-only tables. Every follower in the world applies the name within the cross-continent apply lag (p99 under 1.5 s [guess]).
5. Token. `POST /settings/tokens` carries a session cookie that names `Eu`, so the front sends it to `c31`, which commits the credential in one `auth-eu` transaction. The token is `ghp_` plus one character from the EU class, 29 random characters, and the CRC. Nothing leaves Europe.
6. Fork. The front routes `POST /repos/acme/widgets/forks` to `c7` (us-east-2 and us-west-2). `c7` authorizes read from its replica, `place_fork` returns slot 7, and `c7` allocates `R2` from slot 7's `id_block` in its `core`. `writer_geo` is `Eu`, the fork owner's home, so `c7` calls `c31`'s `identity` handler over PrivateLink. `c31` commits `repo_name(A, widgets) = Live { R2 }` and `repo_dir(R2)` in `auth-eu`, where the owner's unique key decides any collision. `c7` then creates R2's rows in its `core` and answers 202. The waits on another continent are this one call, not a store read, and they are on the write path only.
7. Push. `git push https://tributary.dev/lena/widgets.git` reaches the Frankfurt front. The edge checks the prefix and CRC, reads `Eu` from the token, and looks up the hash in its projection. If the hash has not arrived, `Behind` depends only on EU partitions, which the Frankfurt publisher tails inside Europe in 250 ms segments, so the wait stays under 1 s. `route` resolves `lena` to A, `(A, widgets)` to R2, and slot 7 to `c7`. The edge forwards to `c7` in North America, where the repository lives. `c7`'s snapshot reads `g_repo_dir(R2)` and `g_name(lena)`. `c7`'s EU applier pulls `auth-eu` across the Atlantic every 100 ms, and a missing row is `CellBehind`, which waits up to 2 s. Then `authorize` (owner) and `commit_push` in `c7`'s `core` run as in `DESIGN.md` Journey 1. No step reads `auth-eu` or the registry synchronously.

**Journey 2. `kenji` (home `Ap`) renames to `kenji-dev` in Tokyo while the North American Region set is impaired.**

1. The Tokyo front cell routes the `IdentityWrite` to AP cell `c40` (ap-northeast-1 and ap-southeast-1).
2. Pending. One `auth-ap` transaction (Tokyo endpoint, committed with Singapore and the Seoul witness) marks the rename pending.
3. Claim. `PutItem login#kenji-dev` on the ap-northeast-1 replica. us-east-2 is impaired, so MRSC completes with eu-central-1 (fact 2), one Tokyo to Frankfurt round trip of about 225 ms. Retired items for kenji's popular repositories go under `login#kenji` as parallel conditional puts, each with the same wait. `c40` computes them from the cards it holds.
4. Activate. One `auth-ap` transaction writes `name(kenji-dev) = Live { K, v }` and the account's new login.
5. Alias. A conditional update turns `login#kenji` from `Active { v: u }` into `Alias { v: u + 1 }`. One more `auth-ap` transaction writes `name(kenji) = Former { K, v: u + 1 }`. The handler returns once every serving edge and data cell has applied the commit, or after 5 s. North American members are named lagging in the receipt.
6. Steps 3 and 5 wait on Europe. Nothing waits on North America. `auth-ap`'s writer leases renew through ap-northeast-1 and eu-central-1 (fact 2). North American cells cut off from `auth-ap` fail AP-dependent write grants closed after 30 s, as `DESIGN.md` already requires of any cut-off cell.

**Journey 3. An owner in Virginia removes `fritz` (home `Eu`) from `acme` (home `Na`); fritz then pushes to `acme/api` through a Frankfurt front.**

1. `DELETE /orgs/acme/memberships/fritz` reaches a us-east-1 front cell, which routes it to acme's index cell `c7`. `writer_geo(acme)` is `Na`. One `auth-na` transaction tombstones `membership(acme, fritz)` and fritz's team rows at version + 1 and appends them to a NA revocation partition.
2. `revoke` reads the board at its local us-east-2 replica and returns `covered` once every data cell and every serving edge, in every Region, has applied that NA partition past the commit. EU cells and Frankfurt edges get there through their NA appliers and publishers, which pull `auth-na` across the Atlantic. Expected 1 to 1.5 s [guess], capped at 5 s.
3. fritz's push through the Frankfurt front authenticates from the projection (his EU token is still valid) and goes to `c7`. `authorize` reads `g_membership`, `g_team_member`, `g_repo_grant`, and `g_org_policy` in `c7`'s snapshot, finds no write permission, and git prints `Permission to acme/api.git denied to fritz.`
4. Bound. The push fails at once if `c7` has applied the removal, and within 5 s of the owner's response when `revoke` returned `covered`. In every case it fails within 30 s: a cell that lags NA partitions behind NA-continent board members, or that sees no fresh NA-continent member at all, fails NA-dependent write grants closed after `MAX_LAG`. If `acme/api` lived in an EU cell, that cell's NA applier gives the same bound. No step reads a remote store. The owner's request waits on other continents' followers through the board, not on their stores.

**Journey 4. An anonymous `GET /api/v3/users/zq-nobody` reaches a Sydney front cell while one Region's replication lags.**

1. The Sydney front cell (ap-southeast-2) has no projection entry. An unknown login depends on the shared partitions of all three authorities. The Sydney publisher tails `auth-ap` through Singapore, `auth-na` through us-west-2, and `auth-eu` through eu-central-1. The edge reads the board from the ap-southeast-2 replica and the ap-southeast-1 replica every second.
2. If the lag is MREC replication into ap-southeast-2, the Singapore read is fresh, so the decision is unchanged: 404 at once. If both replicas show no fresh row from some authority's continent for 5 s, that geography is `Blind`, the answer is `Behind`, and the edge returns 503 with `Retry-After` after 2 s, never 404.
3. If the lag is one authority endpoint, say eu-west-1, and the publisher reads through it, its EU watermark trails `asked_at` while EU-continent members on the board have read newer commits. The answer is `Behind`. The publisher moves to the other endpoint after 1 s without progress, so the edge answers 404 or 503 within 2 s.
4. If the lag is some other cell's applier, this edge is unaffected and that cell fails closed on its own. No step reads the registry or an authority, and nothing waits on another continent except the publisher's ongoing pulls.

**Journey 5. Europe is lost for good.**

1. At t0 every European Region is gone: `auth-eu`, the registry's eu-central-1 replica, EU data cells, EU front cells, and their repositories. Paired cells stay inside one DSQL set, so EU-hosted repositories are lost as `DESIGN.md`'s cell tier already implies. This design does not change that.
2. The registry keeps quorum on us-east-2 and ap-northeast-1 (fact 2). Claims continue, and each now waits on one transpacific round trip of about 140 ms.
3. EU writer leases stop renewing. Until every EU lease has been expired for 30 s plus `CLOCK_MARGIN`, which is at most 40 s after the last renewal, survivors treat EU partitions as `Blind`. Unknown-login lookups get 503 after 2 s, and EU-dependent write grants fail closed once 30 s pass, so the overlap is at most about 10 s. After that, EU partitions are `Quiet`. Survivors serve EU-homed facts as of the most advanced survivor's view. EU-homed users authenticate and push to NA and AP repositories as before.
4. Identity writes for EU-homed roots return 503. A signed config change sends new signups that would default to `Eu` to `Na`.
5. Reconstitution, run by the operator over hours. `reconstitute(Eu, into: Na)` picks the surviving data cell with the highest EU watermarks on the board, takes every EU-owned replica row at its version plus the EU escrow rows held in `auth-na`, and writes them into `auth-na` as `Live { Na, version + 1 }`. Appliers keep the highest version, so nothing reverts. A registry update with a version check moves each EU-homed claim's `home` to `Na`. Ids do not change.
6. RPO. Registry claims lose nothing. EU identity commits that no survivor had applied are lost, about the cross-continent apply lag (p99 1.5 s [guess]). Escrowed secrets lose the escrow lag. Resident EU enterprises keep nothing outside Europe except public names ("Residency").
7. The registry now has two Regions. One more registry Region loss would stop claims and writer leases, and with them every identity write. The vendor rebuilds the registry on a new trio (`RegistryGeneration`).

## Shape

```rust
/// A continent group. Each has exactly one DSQL Region set (fact 1).
pub enum Geo { Na, Eu, Ap }
/// Every account and org has one home. `Open`: the authorization graph replicates worldwide, and home-only rows
/// are escrowed to `escrow` (None in single-geography installs). `Resident`: identity residency, offered only to
/// managed-user enterprises. Everything except public names goes on resident partitions that only followers in
/// `geo` apply.
pub enum HomeClass { Open { escrow: Option<Geo> }, Resident }
pub struct Home { pub geo: Geo, pub class: HomeClass, pub generation: u32 }   // bumped by each rehome
/// The aggregate root that owns a fact: the org for org-scoped rows, the repository owner for repository rows,
/// the target account for installations, otherwise the account. The root's home picks the authority.
pub enum Root { Account(AccountId), Org(OrgId) }
pub fn root_of(fact: &FactRef) -> Root { todo!() }
/// Global ids (accounts, teams, credentials, apps, installations) are seq * 4 + origin. Each authority allocates
/// its own, so the three never collide. The origin is fixed and the home can move. Repository ids are unchanged.
pub struct GlobalSeq { pub origin: Geo, pub seq: u64 }
```

```rust
/// The facts that need one answer worldwide. One MRSC item each, keyed "<kind>#<key>".
pub enum ClaimKey { Login(LoginKey), Shortcode(Shortcode), AppSlug(LoginKey), SshKey(Fingerprint),
                    Email(EmailDigest), Slot(Slot), Lease { geo: Geo, cell: CellId } }
/// Every write is a conditional put or update on `v`, so `v` totally orders one item's history worldwide.
pub struct ClaimVersion(u64);
pub enum ClaimState {
    Active { holder: Holder, home: Geo, saga: Ulid, v: ClaimVersion },
    Alias { owner: AccountId, home: Geo, v: ClaimVersion },        // a renamed login, claimable by anyone
    Reserved { reason: ReserveReason, v: ClaimVersion },
}
pub enum Holder { Account(AccountId), App(AppId), UserKey(AccountId), DeployKey(RepoId),
                  Slot { cell: CellId, generation: u64 }, Writer { valid_until: Timestamp } }
/// Under the login's partition key, sort key "retired#<repo>". Written before the login can become Alias.
pub struct Retired { pub owner: LoginKey, pub repo: RepoNameKey }
/// One implementation over the DynamoDB API in every mode ("Deployment modes"). Only `trib-claims` holds a client.
/// No request reads it. Followers read only the lease items.
pub trait Registry {
    /// Put if absent, or if the login is Alias. A repeat with the same saga id returns the existing claim.
    async fn claim(&self, key: &ClaimKey, holder: Holder, home: Geo, saga: Ulid) -> Result<Claimed, ClaimError>;
    /// Conditional on `seen`: Active to Alias, release, slot generation bump, home change, lease renewal.
    async fn transition(&self, key: &ClaimKey, seen: ClaimVersion, to: Transition) -> Result<ClaimVersion, ClaimError>;
    async fn put_retired(&self, r: &Retired, saga: Ulid) -> Result<(), ClaimError>;
    /// Strongly consistent. Sagas, the sweeper, and lease readers only.
    async fn get(&self, key: &ClaimKey) -> Result<Option<(ClaimState, Vec<Retired>)>, RegistryError>;
}
pub struct Claimed { pub v: ClaimVersion, pub retired: Vec<Retired> }
pub enum ClaimError { Taken, Retired, Stale, Unavailable }
/// Pending, claim, activate (round-2 candidate 2). The claim is the commit point. `pending` and `activate` are
/// transactions in the root's home authority. `activate` appends the record that publishes the claim with its
/// `v` and copies `retired` into the authority, so repository creation never reads the registry. The sweeper
/// finishes or undoes any saga older than 10 minutes by reading the claim.
pub async fn run_claim<T>(a: &Authority, r: &dyn Registry, saga: ClaimSaga,
    pending: impl Fn(&mut AuthTx<'_>) -> Result<(), DomainError>,
    activate: impl Fn(&mut AuthTx<'_>, &Claimed) -> Result<T, DomainError>) -> Result<T, ClaimError> { todo!() }
/// After a replica Region is lost for good: a new empty MRSC table on a new trio, filled by a copier whose puts
/// are conditional on version. During the copy, sagas claim in the old table and then mirror to the new one at
/// the same version. The flip is a signed config generation that pauses claims for seconds.
pub struct RegistryGeneration { pub n: u32, pub regions: [Region; 3] }
```

```rust
/// One DSQL multi-Region cluster per Geo. It keeps DESIGN.md's GlobalStore contract, scoped to one geography: one
/// writer, an after-image appended to the geography's log in the same transaction, and a `version` on every row.
/// It can be constructed only in a cell of its own geography, so writer connections never cross a continent.
/// AuthTx refuses a row whose root is not homed here, which the HomeView in the same snapshot decides.
pub struct Authority { geo: Geo /* pools */ }
impl Authority {
    pub async fn run_tx<T, F>(&self, ctx: TxCtx, f: F) -> Result<Committed<T>, StoreError>
    where F: Fn(&mut AuthTx<'_>) -> Result<T, DomainError>;   // refuses to open with under 2 s of writer lease left
}
/// Rows the shared log never carries. They stay in the home authority and, for Open homes, its escrow partner.
pub enum HomeOnly { PasswordHash, TotpSeed, RecoveryCodes, WebAuthn, Session, EmailAddress, OAuthClientSecret,
                    IdpConfig, ScimToken, SignInHistory }
/// Per geography: 64 shared, 8 revocation, 16 resident, and 8 escrow partitions [guess]. The escrow table is
/// readable only by the partner geography's `escrow-apply` database role.
pub enum PartClass { Shared, Revocation, Resident, Escrow }
pub struct PartKey { pub geo: Geo, pub part: u16 }
pub enum LogId { Authority(Geo), Xcell(CellId) }   // replaces LogId::Global
/// A row has one owning geography at a time. A rehome or cross-geography transfer writes Handoff at version v,
/// and the new owner writes v + 1 only after its own applier has applied the Handoff. Appliers keep the highest
/// version, so a row never reverts, whichever log they read first.
pub enum Owned<T> { Live { geo: Geo, version: u64, row: T }, Handoff { to: Geo, version: u64 }, Tombstone { version: u64 } }
/// `g_name` merges across geographies by ClaimVersion, because two homes can publish the same key over time.
pub struct NameRecord { pub key: LoginKey, pub v: ClaimVersion, pub state: NameClaim, pub home: Geo }
pub enum Lookup<T> { Found(T), Absent, Behind { parts: SmallVec<[PartKey; 4]> } }
/// What a "no" depends on. An unknown login: the shared partitions of all three geographies. An unknown token:
/// the shared and revocation partitions of the token's own geography. A route to a Resident owner outside its
/// geography: never answered there, because the edge forwards to a front cell in that geography.
pub fn depends_on(q: &NegQuery) -> PartSet { todo!() }
/// The serving cell's route check. The owner part compares the login's ClaimVersion in `g_name`. The repository
/// part compares `g_repo_dir.epoch` as before. A rename or a reclaim in any geography changes one name row.
pub fn check_route(owner: (&LoginKey, ClaimVersion), repo: NameEpoch, names: &GName, dir: &GRepoDir) -> EpochCheck { todo!() }
/// Where an identity write runs: a cell in the home of the root it writes. Signup and org creation use the chosen
/// home, a fork uses the fork owner's home, and a membership change uses the org's home.
pub fn writer_geo(op: &IdentityOp, p: &Projection) -> Lookup<Geo> { todo!() }
```

```rust
/// GitHub's stored-token shape (research/wire-census.md#29): a prefix, 30 base62 characters, and a CRC32 in 6.
/// The first of the 30 comes from the minting geography's 20-character class, and the other 29 are random
/// (about 177 bits). Session cookies, OAuth and device codes, and refresh tokens carry the geography the same way.
/// The minting geography's log keeps a record for the token's whole life (Live or Handoff), so "no such token"
/// never depends on another geography.
pub struct OpaqueToken { kind: TokenKind, geo: Geo, secret: [u8; 29], crc: [u8; 6] }
```

```rust
/// `trib-board`: DynamoDB MREC with a replica in every Region that has a data cell or front cell. One item per
/// member, written only by that member each second, so last-writer-wins never chooses between writers, and no
/// transaction is used. Soft state, rebuilt within a second if lost. Edges report their applied segment to their
/// publisher every 250 ms, and an edge with no acknowledgment for 2 s fails its own health check.
pub enum BoardRow {
    Cell  { cell: CellId, region: Region, applied: PartMap<Timestamp>, newest_read: PartMap<Timestamp>, at: Timestamp },
    Front { front: FrontCellId, region: Region, applied: PartMap<Timestamp>, newest_read: PartMap<Timestamp>,
            segments: SegmentMarks, edges: Vec<(NodeId, u64)>, at: Timestamp },
}
/// Each member reads its own Region's replica and the nearest other replica on its continent every second and keeps
/// the newer row per member. Both reads carry production traffic, so neither is a fallback (research/uptime.md
/// implication 8). For geography G: Live if some member in G's continent wrote within BOARD_BLIND; Quiet once every
/// writer lease of G expired more than run_tx's 30 s deadline plus CLOCK_MARGIN ago; otherwise Blind.
pub enum GeoView { Live { newest: PartMap<Timestamp> }, Quiet { since: Timestamp }, Blind }
pub const BOARD_BLIND: Duration = Duration::from_secs(5);
pub fn geo_views(rows: &[BoardRow], leases: &[WriterLease], now: Timestamp) -> [GeoView; 3] { todo!() }
/// `Absent` for a part of G: watermark at or after asked_at + CLOCK_MARGIN, or G Live and no member newer, or G Quiet
/// and no surviving member newer. Blind is Behind. An edge lagging G past MAX_LAG fails its health check only if
/// another front cell in its Region is current for G (the fault is its own). Otherwise it stays in rotation and
/// answers 503 to principals homed in G. A data cell fails closed only the write grants that depend on G.
pub fn absent_allowed(part: PartKey, mine: Timestamp, view: &GeoView, asked_at: Timestamp) -> bool { todo!() }
/// Read by every data cell's applier set and front publisher once a second (one strongly consistent Query).
pub struct WriterLease { pub geo: Geo, pub cell: CellId, pub valid_until: Timestamp }
```

```rust
/// Partner-geography copy of home-only rows for Open homes: Eu to Na, Na to Eu, Ap to Na.
pub async fn escrow_apply(src: &dyn Tail<HomeOnlyEntry>, dst: &Authority) -> Result<(), FollowError> { todo!() }
/// Operator-run after a geography is declared lost and Quiet. Rebuilds its roots in `into` from the most advanced
/// survivor's replica and the escrow, then appends each row at version + 1.
pub async fn reconstitute(lost: Geo, into: Geo, source: CellId) -> Result<ReconstituteReport, OpsError> { todo!() }
```

Invariants and decisions:

- **One owner per fact.** `root_of` and the root's `Home` pick the authority. No transaction spans authorities, and writers connect only within their own continent. Almost every identity write touches one authority. The exceptions are the claim sagas, which add the registry, and the transfer and rehome sagas, which use `Owned::Handoff`.
- **The registry commits, the authorities publish.** No follower reads the registry for facts. A claim becomes visible only when `activate` appends it, with its `ClaimVersion`, to the home log. So `Lookup::Absent` keeps resting on commit-ordered DSQL logs with snapshot watermarks, never on a DynamoDB stream (fact 3). A claimed but unactivated login is taken for claimers and invisible to readers. The user has not been told that it succeeded.
- **Repository names need no registry.** They are unique per owner, so the owner's authority decides them with a unique key, as `DESIGN.md` does today.
- **Slot flips** become a registry update conditional on the generation, then an activation record in the target cell's geography log. A stale slot map stays safe because of the slot fence (`DESIGN.md`, `move_slot`).
- **Writer leases fence a cut-off continent.** MRSC needs two of three continents for a lease renewal. A geography on the minority side of a continent cut stops committing within 10 s plus the 30 s transaction deadline. Survivors can then treat its partitions as final (`Quiet`) without a human, and they never answer "no" over a commit they could not see.
- **Every data cell and front publisher follows all three logs** with one lease, cursor, and watermark per geography, plus the resident partitions of its own geography only. Each authority's follower load is fixed by fleet size: about 130 followers polling every 100 ms, roughly 1,300 queries per second per authority [guess], the same as `global` today.

## Failure modes

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| Claim registry | instance | Single claim, transition, or lease calls fail | SDK retries under the 10% retry budget. Claims are idempotent by saga id, and leases renew every 2 s with 10 s validity. |
| Claim registry | az | None; DynamoDB spans AZs | Nothing to do. |
| Claim registry | region | One replica Region. Callers in that continent lose their local endpoint | They call the nearest surviving replica. The other two keep quorum (fact 2). Requests never read the registry, so only claims and lease renewals notice. |
| Claim registry | control-plane | None at runtime | Created at install. A rebuild after a permanent loss calls CreateTable, which is planned work and not recovery. |
| Claim registry | bad-deploy | Wrong claims or lost leases service-wide | `trib-claims` is the only writer, and every write is conditional on a version. Claim code ships in the last wave. A daily audit compares every `Active` claim with its authority row. A lease bug stops identity writes, never reads, and alarms within 10 s. PITR is on [unverified for MRSC]. |
| Claim registry | bad-config | Claims fail where the config is wrong | The readiness probe claims and releases a canary key through each replica. Signed config with last known good. |
| Claim registry | noisy-tenant | A signup or rename storm | Per-IP, per-ASN, and per-account claim limits at the front, plus the table's on-demand maximum write throughput. Lease items stay under 1 write per second each. |
| Claim registry | continent | One replica's continent. Claims and leases continue on two replicas with one round trip between the survivors | One more Region loss stops claims and leases, and so every identity write. The vendor builds a `RegistryGeneration` on a new trio within days. |
| Geo authority | instance | That geography's in-flight transactions see errors or 40001 | Bounded retries. Writes carry the request id. |
| Geo authority | az | None for data | Writers reconnect inside their budgets. |
| Geo authority | region | One endpoint of that geography | Writers and followers use the other endpoint. The witness keeps quorum with no failover call (`research/uptime.md#26`). |
| Geo authority | control-plane | None at runtime | Created once per geography. IAM auth tokens are signed locally. |
| Geo authority | bad-deploy | Identity writes for the roots homed in one geography, about a third of accounts, not all | Authorities migrate one per release wave, AP first, each after two weeks on its own CDC-fed shadow. Writers run a fixed prepared-statement set. The `identity` handler rolls with the cell waves. |
| Geo authority | bad-config | Same as bad-deploy | Writer config rides the signed cell waves. |
| Geo authority | noisy-tenant | One geography's write capacity, for example an org-wide membership saga | The base design's write buckets, 1,000-row saga steps, and revocations first, now per geography, so a European storm cannot slow Asia. |
| Geo authority | continent | Identity writes for that geography's roots until reconstitution. Reads, pushes, and merges continue elsewhere on replicated facts | Leases expire and the geography turns `Quiet` within 40 s. `reconstitute` uses the most advanced survivor's replica and the escrow. RPO equals the apply lag for shared facts and the escrow lag for home-only rows. Resident roots are not recoverable outside the continent. |
| Freshness board | instance | One member's write fails | It writes again next second. Missing one second stays inside `BOARD_BLIND`. |
| Freshness board | az | None | DynamoDB spans AZs. |
| Freshness board | region | That Region's replica | Members already read the nearest same-continent replica every second. Members whose writes cannot land go stale on the board. `revoke` names them lagging, and they see their own blindness and fail closed after 30 s. |
| Freshness board | control-plane | None at runtime | Replicas are created with each Region's first cell. Adding a Region is part of the Region build, not recovery. |
| Freshness board | bad-deploy | A member that overstates `applied` can make `revoke` report `covered` falsely | Rows are written by `trib-authority` code that takes `applied` from the cursor committed with the apply. Every minute a canary revocation is probed through every front cell, and a 200 for the revoked canary token alarms and blocks the wave. |
| Freshness board | bad-config | A member that reads the wrong table or Region | Readiness requires a fresh read of its own row through both replicas. |
| Freshness board | noisy-tenant | None from tenants. Load is fixed at about 130 rows per second | 16 partition keys by member hash. Cost is an open question. |
| Freshness board | continent | That continent's members disappear | Survivors see its geography `Blind`, then `Quiet` once leases lapse. Rows older than `MAX_LAG` no longer count as serving members for `revoke`. |

## Residency

What an `Open` European account or org keeps only in Europe: password hashes, TOTP seeds, recovery codes, WebAuthn credentials, session rows, plaintext email addresses and their verification state, OAuth client secrets, IdP configuration and SCIM tokens, sign-in history with IP addresses, and the identity audit log. These never enter the shared log, so no follower outside Europe holds them. Sign-in therefore runs in a European cell. A European user traveling in Tokyo pays one forward from the Tokyo front to a European cell at sign-in, and only then. One exception is the escrow. An `Open` home's home-only rows are copied to a partner geography (Europe to North America) so that a continent loss does not wipe millions of passwords and email addresses. `Open` makes no residency promise, and the escrow is the price of recovering such accounts.

What leaves Europe for an `Open` home, and why. The registry's three Regions hold the login, the account id and home, SSH key fingerprints, HMAC digests of verified emails, app slugs, and retired pairs, because each needs one answer worldwide. Every data cell's replica holds the account's public profile, `sessions_not_before`, suspension, memberships, team memberships, repository grants, org policies without secrets, installations, repository names, SSH and GPG public keys, and email digests. Any cell must authorize any user in the same snapshot as the refs it guards. Every front projection holds names, credential hashes with scopes and expiry, fingerprints, and installation `not_before`, because any front must authenticate without a remote read.

What a `Resident` home keeps in Europe. Identity residency is offered to managed-user enterprises, as GitHub ties data residency to managed users [unverified]. Their orgs, managed users, credential hashes, memberships, teams, grants, installations, and repository names go on resident partitions that only EU followers apply, and they have no escrow. Only the shortcode, the org logins, and the managed users' logins leave Europe, as registry items and shared name records, because they are public names in one namespace. A front outside Europe that resolves a resident owner, or reads a resident token's class, forwards to a European front cell. Their repositories live only in European cells already. A cell outside Europe treats a resident principal only as an authenticated reader of public content, which is all a managed user may do outside its enterprise [inference]. The enterprise's IdP is the root of recovery, and a European continent loss loses these accounts with their repositories. Network residency for ordinary orgs stays as in `DESIGN.md`.

## Deployment modes

Every new store keeps one implementation in all modes. Only its Region count and replica settings change, through the install's `tributary` target block. An install has one geography, the continent of its home Region, so it runs one authority, and its namespace never meets the managed one.

| Element | Managed | BYOC (vendor-run, customer account, one or three Regions) | Self-hosted (customer-run, one Region, possibly one evaluation node) |
| --- | --- | --- | --- |
| `Geo` and `Home` | Three geographies, `Open` or `Resident` | One geography. Every home is `Open { escrow: None }` | Same as BYOC |
| Authority | `auth-na`, `auth-eu`, `auth-ap` clusters | An `authority` schema in the cell's `core`, as `DESIGN.md`'s `global` schema today. Paired cells make it multi-Region | Same schema in a single-Region `core` |
| Claim registry | MRSC across three continents | Paired: MRSC on the cell's two Regions plus the witness Region, when all three are among MRSC's 15 Regions. Otherwise, and for Regional: a single-Region table with an MREC replica in the `dr` Region, written only in the active Region | Single-Region table. Regional with `dr`: as BYOC Regional |
| Freshness board | MREC in every serving Region | MREC on the cell's Regions, or a single-Region table | Single-Region table. One evaluation node writes one row |
| Writer leases | Per writer cell, read by every follower | One cell's lease. Always renewable while the registry is | Same |
| Escrow and `reconstitute` | On | Off. The authority's own Paired durability protects home-only rows | Off |
| Cross-geography sagas | Live | Never reached | Never reached |
| Token geography class | Three classes | The install's one class | Same |

What collapses. MRSC needs exactly three Regions, so a one-Region install uses a plain table. The claim saga still runs pending, claim, activate against it, and the code cannot tell the difference. A Regional install's registry has the same non-zero cross-Region RPO as its cells. After a `dr` promotion, the mandatory `ref_log` reconcile also runs the claim audit. The audit finds claims with no authority row, written in the old Region and replicated, and authority rows whose claim never replicated, and the sweeper resolves each by the saga's pending row. Escrow, reconstitution, and cross-geography sagas cannot run with one geography. CI therefore runs a two-geography install that keeps two authority schemas in one DSQL cluster, so those paths stay tested. The `Quiet` rule degenerates to the base design's rule, because every member sits in the authority's continent.

What the customer provisions or grants. BYOC Paired customers enable three Regions that are in both the DSQL Region set and the MRSC list. ca-west-1 and ap-east-1 are in DSQL's sets but not in MRSC's 15 Regions (fact 1, fact 2). An install that uses either falls back to the Regional registry form, and target validation says so. DynamoDB data operations need nothing beyond the existing workload boundary, which allows everything outside `iam:*`, `organizations:*`, and `account:*` (`research/byoc-grounding.md#6`). Global-table replication needs DynamoDB's replication service-linked role [unverified name], and the boundary forbids IAM writes. So the customer bootstrap must create that role once, or permit only `iam:CreateServiceLinkedRole` for `replication.dynamodb.amazonaws.com`. A self-hoster needs no new grant beyond running the same bootstrap with their own CI as the principal (`#23`).

What the BYOC machinery must add. (1) The target document carries one `aws.region` (`#8`, `#1`). The `tributary` block gains `regions { home, peer, witness }`, which DSQL, the registry, and the board share. (2) An MRSC table must start empty and cannot change Regions (fact 2). So `create:registry` is a create-once stage. Any release that changes registry Regions is a `RegistryGeneration` copy marked as a rollback barrier (`#26`, `#34`), and rollback never destroys the table, because OpenTofu sets `prevent_destroy` on it. (3) The tables take a `trib-` prefix, because installs own unprefixed account singletons (`#2`, `#6`). (4) `components` knows only ECS and Lambda (`#19`). A new observer route, `GET /v1/system/observer/identity`, reports authority lag per geography, board freshness, lease state, the sweeper backlog, and claim-audit orphans. Gates add `identity-fresh` next to `components-healthy` (`#31`). (5) Registry and board items take only additive attributes, so they never add a migration barrier (`#40`).

## Changes to DESIGN.md

Deletions:

- Summary: the clause "one multi-Region DSQL cluster, `global`, commits every name, account, credential, and authorization fact", and the Milestone 0 bullet's last sentence about switching to the thin claim registry.
- "Namespace and cell placement": `GlobalPart`, `GlobalStore`, `BoardRow`, the S3-board sentences in the `MAX_LAG` doc comment, the second sentence of the `NameEpoch` doc, `Target::Repo.owner_epoch`, and the owner clause of `check_epoch`.
- "Identity and tokens", access patterns: the sentences on us-east-2, us-west-2, and the us-east-1 witness, and "Identity writes from Europe and Asia pay one cross-continent round trip".
- "Public service concerns", Residency: the last two sentences (identity facts in North America, and BYOC as the only identity-resident offer).
- "Failure modes": the seven `global` cluster rows.
- "Deployment modes": the `global` store row.
- Milestone 0: the change-log gate's last two sentences (the flip to round-2 candidate 2).
- "Tradeoffs accepted": the bullet beginning "We accept `global` in North American Regions".
- "Alternatives considered": "The name authority in DynamoDB".
- "Open questions and risks": the North American residency question, and the sentence on the us-east-1 witness.

Replacements:

- Summary and "Problem": `global` becomes the claim registry plus three geo authorities. The identity-write bullet scopes outages to one geography for writes and to the registry for claims.
- "Namespace and cell placement": the definitions paragraph, with `PartKey`, `Authority`, `Lookup`, `BoardRow`, `GeoView`, `absent_allowed`, `check_route` replacing `check_epoch`, `RouteClass::IdentityWrite` routed by `writer_geo`, and the `move_slot` Flip step (registry update conditional on generation, then activation). Bullets: "Claims are registry items with sagas, and repository names are authority unique keys", "Negative answers" per geography with `Blind` and `Quiet`, `NameEpoch` (owner check by `ClaimVersion`), "Renames", "Retirement" (registry items copied on reclaim), the applier bullet (three followers), and the front-cell health rule (per-geography refusal).
- "Identity and tokens": `OpaqueToken`, `revoke(a: &Authority, ..)`, the ownership bullet (authorities by root home), and the sign-in bullet (sign-in runs in a cell of the home geography). The index cell is chosen inside the home geography.
- `DESIGN.md` journeys 2 and 4: the fork's `global` transaction runs in the fork owner's authority through `writer_geo`, and the org race uses `run_claim`.
- "Cross-cell features": `LogId::Global` becomes `LogId::Authority(Geo)`, and the log bullet describes three authority logs.
- "SLOs": identity writes per geography at 99.95%, a new claim row at 99.9% measured by a synthetic claim and release from each continent every minute, and replica freshness per geography (p99 apply lag under 1 s within a continent and under 1.5 s across, plus 1 s at fronts [guess]).
- "Failure modes": Cell replica rows (three appliers, and a poison record halts one geography's partition) and the Front cell `bad-config` and `region` rows (the per-geography refusal rule).
- "Service and crate map": `trib-global` becomes `trib-authority` (Authority, logs, `Lookup`, board, leases, `revoke`, `auth-apply`, `escrow-apply`, `front-publish`), and AWS services add DynamoDB, on the identity-write and follower paths only.
- "Deployment modes": the `global` row becomes this document's table.
- "Build order": Milestone 0's change-log gate runs on all three DSQL Region sets with followers on other continents, plus an MRSC claim-latency and quorum-loss test on the chosen trio using AWS FIS Region isolation (HowItWorks, "Fault injection testing"). Milestone 2 gets one authority plus the registry, and Milestone 4 gets a second geography and the continent-cut drill.

Additions:

- Shape: a subsection "Geographies, the claim registry, and the authorities" with this document's sketches and invariants.
- "Failure modes": this document's 24 rows.
- "Drills": a monthly FIS isolation of one registry Region. A quarterly continent-cut drill blocks a canary geography's authority from every other continent and checks `Blind`, fail-closed, and `Quiet`. A yearly `reconstitute` rehearsal into a shadow authority.
- "Public service concerns": this document's "Residency" text.
- "Review resolutions": an `NS-2` entry summarizing this redesign.
- Tradeoffs, alternatives, and open questions below.

## Tradeoffs accepted

- Every login claim, SSH key change, email verification, and app creation waits on one cross-continent round trip (95 to 225 ms [guess]), even for a user next to a replica. In exchange, no single continent's loss stops claims, and claim races stay decided by one conditional write.
- An unknown login now depends on three logs, two of them across oceans, so a cross-continent stall turns some 404s into a 2 s wait or a 503. Tokens carry their geography so that unknown tokens depend on one log.
- Identity writes in a geography depend on its authority and on the registry's quorum through writer leases. A registry that loses two Regions stops identity writes everywhere. In exchange, a continent cut fences itself in 40 s with no human step.
- Each geography's identity facts have RPO equal to the cross-continent apply lag on a continent loss, and RTO in hours for its identity writes. Today a North American continent loss loses `global` with no recovery path at all.
- `Open` homes escrow secrets to another continent, which is a residency cost. `Resident` homes have no escrow and lose identity with their continent.
- Followers follow three logs, the board needs a DynamoDB replica in every serving Region, and installs gain DynamoDB tables. In exchange there is no S3 cross-Region board read per member per second.
- Cross-geography transfers and rehomes are sagas with a `Handoff` window.

## Alternatives considered

- **Everything in DynamoDB global tables.** MRSC has no transactions, so a claim, its account, and its first membership could not commit together, and no DynamoDB stream yields the cross-item watermark that `Lookup::Absent` needs (fact 3). MREC would add last-writer-wins on authorization rows and partial transactions (fact 4). It lost on the base design's two strongest guarantees.
- **Keep one North American `global`** (the status quo). It keeps the three costs in "Problem".
- **Owner homes only, with no registry**, where a geography's authority claims logins by asking the other two. That is cross-geography two-phase commit across three DSQL clusters. It lost to one MRSC conditional write.
- **The registry stream as the projection source for names.** It is ordered per item only, so a front could never prove a login absent. Names publish through the home log instead.
- **An S3 freshness board read across every Region** (today). With three continents, each member would read every Region's bucket across oceans every second. MREC replaces that with two local reads. Its weaknesses, last-writer-wins and partial transactions, cannot arise with one writer per item and no transactions.
- **Token hashes only in home-geography fronts.** A traveler's every request would hop to a home front, so a request would depend on a remote front. Hashes of 177-bit secrets replicate instead.
- **A registry of two replicas and a witness.** It is cheaper, but one continent would have no local endpoint, and a continent loss that took a replica would leave a replica and a witness with no third Region for lease reads.
- **Human sealing of a lost geography instead of writer leases.** It needs no registry dependency, but a human cannot tell a cut from a loss, and an unfenced minority side could keep committing.

## Open questions and risks

- What are MRSC write and strongly consistent read latencies on us-east-2, eu-central-1, and ap-northeast-1? AWS publishes none (HowItWorks), so Milestone 0 measures them.
- Does MRSC support PITR and on-demand backup, and how long does a `RegistryGeneration` copy of about 600 million items [guess] take?
- Do DSQL snapshot times in three separate clusters stay within `CLOCK_MARGIN` of EC2 clocks? [unverified] Leases also rest on clock bounds.
- Does a DSQL multi-Region endpoint serve about 130 remote followers across oceans at 10 polls per second within its connection and rate quotas (`research/uptime.md#27`)?
- What does the MREC board cost with replicas in about 12 Regions? About 130 rows written per second plus every edge's two reads [guess, tens of thousands of dollars per month]. Should edges read through their publisher instead?
- Is the escrow of `Open` European secrets in North America acceptable under GDPR transfer rules? [inference: probably with standard contractual clauses, unverified]
- Does GitHub restrict managed users to public reads outside their enterprise, as the `Resident` model assumes? [unverified]
- What is the replication service-linked role's exact name, and can a BYOC bootstrap pre-create it under the workload boundary? [unverified]
