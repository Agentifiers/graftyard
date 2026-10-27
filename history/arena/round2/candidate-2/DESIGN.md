# Owner-home cells with a thin global claim registry

## Problem

Managed Tributary is now one public host with one namespace. Logins of users and orgs are unique service-wide, repository names are unique per owner, public repositories allow anonymous reads, and forks, pull requests, stars, follows, notifications, and search cross owners (`FRAME.md`, "Operator decision, 2026-10-24"). `DESIGN.md` routed by tenant hostname and kept every fact inside one cell. That rule bought its best properties: a merge commits the head ref, the base ref, the pull request, and the outbox in one DSQL transaction, permission checks read the same snapshot as the write they guard, and a bad cell hurts only its own tenants. This design keeps those properties and puts the smallest possible layer above the cells.

Five facts shape the layer.

1. Uniqueness needs one linearizable writer per name, and that writer must survive a Region loss, because a lost claim would hand one login to two people. Aurora DSQL multi-Region clusters exist only inside one AWS Region set, which is one continent (https://docs.aws.amazon.com/aurora-dsql/latest/userguide/what-is-aurora-dsql.html). No DSQL cluster can be the service-wide authority. DynamoDB multi-Region strong consistency (MRSC) can span any three of 15 Regions across continents, with RPO zero and conditional writes evaluated against the latest version of an item, but without transactions (`research/uptime.md#29`, https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html).
2. A shared identity database is where GitHub has lost the most core availability: a permissions cluster broke App tokens for 12 hours, and an auth and user database overload broke sign-in in February 2026 (`research/uptime.md#44`, `#47`). A global identity tier would rebuild that outage source.
3. On GitHub a new repository at an old name breaks the rename redirect, git answers 200 at the old name, and the API answers 301 to `/repositories/{id}` (`research/wire-census.md#23`). An owner rename frees the old login for anyone, and popular `OLD-OWNER/REPO` pairs are retired forever (https://docs.github.com/en/account-and-profile/reference/username-reference).
4. Forks and their upstream share one object store and one pull request space on GitHub. `DESIGN.md`'s merge invariant needs the head branch's ref row and the base ref in one cluster.
5. New-format node ids carry the repository id first, so a router can resolve `node(id:)` from the id alone if ids carry placement (`research/wire-census.md#27`, implication 4).

The direction: every owner has a home, the home holds its identity, credentials, memberships, and names, and the only global state is a thin claim registry kept off the request path. The design question is what "home" means once forks cross owners.

## Usage (caller's view)

The managed host is `tributary.dev`. gh, GCM, Desktop, VS Code, and the runner classify it as GHES because it is not `github.com`, `*.ghe.com`, or `*.localhost` (`research/clients.md#1`, `research/actions-protocol.md#1`). Untrusted content uses isolation subdomains (`raw.`, `codeload.`, `uploads.`, `avatars.`, `pages.`), as GHES subdomain isolation does (`research/wire-census.md#33`), and GCM strips `raw.` and `gist.` before probing (`research/clients.md#12`).

```sh
gh auth login --hostname tributary.dev      # device flow, gh's own client id, seeded once for the service
gh repo fork acme/widgets --clone && cd widgets
git switch -c fix && git commit -am fix && git push -u origin fix
gh pr create --repo acme/widgets --fill     # a maintainer later runs: gh pr merge 42 --squash
```

Terms used in the journeys. A **slot** is one of 65,536 placement units, and a **cell** holds many slots. Every numeric id carries its slot in its high bits. An owner's **home** is its slot and the cell that holds it. A repository **network** (a root and its forks) lives in one slot. The **front door** is the Regional fleet behind `tributary.dev` that routes each request to a cell. The **RouteBook** is a per-Region, read-only copy of the registry that front doors and cells keep on local disk. A **passport** is a short-lived signed principal that the caller's home mints and any cell verifies.

**Journey 1: anonymous `git clone https://tributary.dev/torvalds/linux.git`.**

1. Route 53 latency records resolve `tributary.dev` to the nearest Region's front-door NLB, health-checked (data plane, `research/uptime.md#15`).
2. The front door terminates TLS with the service certificate, classifies `GET /torvalds/linux.git/info/refs?service=git-upload-pack` as an owner target with no credential, and charges the anonymous budget for the client's address block (/24 for IPv4, /48 for IPv6): at most 8 concurrent upload-packs per block per front-door node [guess], shuffle-sharded so one block cannot drain a node.
3. The front door looks up `torvalds` in its in-memory RouteBook: owner id, then slot by bit shift, then cell `c2` from the slot map in the same book. Its repo-route cache maps `(torvalds, linux)` to `(slot, repo id)`. Store: local memory and disk. No registry read.
4. It forwards over mTLS to the nearer of `c2`'s two Regions. The cell's `edge` role applies per-repository admission (64 concurrent upload-packs per node, from `DESIGN.md`) and a per-address CPU budget of 90 CPU-seconds per minute, GitHub's secondary limit (`research/wire-census.md#18`).
5. `c2` resolves `repo_name(torvalds, linux)` to the repository, reads its visibility (public), authorizes `Actor::Anonymous` for read, and serves `info/refs` and `POST git-upload-pack` exactly as `DESIGN.md` Journey 1 describes (one `ref_shard` snapshot, cache node, upstream `git upload-pack`). Stores: `c2` `core` (`repo_name`, `repo`, `ref_shard`, `ref`, `pack`), S3 pack buckets, NVMe.

Every step after the front door runs inside `c2`. The only state outside it is the front door's RouteBook copy, which is local. A cold front door with no repo-route entry sends the request to the owner's home, which here is the same cell. Smart HTTP is not cacheable by a CDN, so popular clones are carried by the cell's widened rendezvous sets (3 to 9 nodes per repository in `DESIGN.md`).

**Journey 2: `bob` (home `c3`) forks `acme/widgets` (home `c7`), pushes, and opens a pull request that a maintainer merges.**

1. `gh repo fork` sends `POST /api/v3/repos/acme/widgets/forks`. The front door reads bob's `gho_` token: prefix and CRC offline, then the home slot from the token's first three characters, then `c3`. Its passport cache misses, so it calls `c3`'s `Home::passport` once. `c3` checks the token hash in its `credential` table and returns a signed passport valid 15 minutes. The front door routes the path owner `acme` to `c7` and forwards with the passport. Synchronous dependency outside `c7`: `c3`, once per token per front-door node per 15 minutes.
2. `c7` holds acme's network. `place_fork` returns `WithNetwork(slot of acme/widgets)`, so the fork joins the network's object store and no pack is copied. Fork creation is a two-cell saga with the name as its commit point. `c7` allocates the repo id in the network's slot and inserts `repo(state = Creating)`. It calls `c3` to insert `repo_name(bob, widgets) -> repo id` (a unique key, so a second fork or an existing `bob/widgets` fails here with 422). Then `c7` flips the repo to `Live` with the `fork` outbox event, and the API answers 202. A sweeper resolves any `Creating` row older than 10 minutes by asking `c3` whether the name row exists. Synchronous dependency outside `c7`: `c3`, because the fork writes into bob's namespace.
3. `git push origin fix`: the front door routes `bob` to `c3`, `c3` finds that `repo_name(bob, widgets)` points at a repo in slot 7, forwards to `c7`, and returns a route hint. The front door caches `(bob, widgets) -> (slot 7, repo id)`, so later pushes go straight to `c7`. `c7` authorizes from the passport (bob owns the repo row) and runs `commit_push` unchanged. Stores: `c7` `core`, S3, NVMe.
4. `gh pr create` sends a GraphQL `createPullRequest`. GraphQL goes to the caller's home, `c3`, whose executor sees that the mutation's `repositoryId` node id carries slot 7 and forwards the whole document to `c7`. The pull request row, its head spec (bob's fork, branch `fix`), and the base repository are all in `c7` `core`.
5. The maintainer's `gh pr merge 42 --squash` sends `mergePullRequest` to the maintainer's home, which forwards it to `c7` by the pull request's node id. `merge()` from `DESIGN.md` runs unchanged: one `core` transaction compare-and-sets bob's head ref, moves `main`, marks the pull request merged, and writes the outbox events. The invariant holds in full, because the fork lives with its network.
6. `c7`'s `Notifications` consumer computes recipients from watch and thread-subscription rows in `c7` and sends bob's notification to `c3` through the `CrossCell` consumer. Bob's inbox lives in `c3` `aux`.

What weakens: if the fork had been owned by an org `forkco` homed in another cell, `c7` would authorize forkco's members from a mirror of forkco's memberships and team grants that `c7` holds, which lags forkco's home by seconds ("Identity, orgs, and authorization"). The merge transaction itself never weakens.

**Journey 3: two users race to create `newco`, then `newco/a` is renamed to `newco/b`.**

1. Alice (home `c3`, EU) and carol (home `c9`, US) submit the new-organization form at the same moment. Each request goes to the requester's home, which picks a slot for the org with `place_owner` and inserts `owner(id, login = newco, state = Pending)` in that slot.
2. Each home calls `Registry::claim(Login(newco), Owner(id))`, a DynamoDB conditional `PutItem` on partition key `login#newco` that succeeds if the item is absent or is an `Alias`. MRSC evaluates the condition against the latest version in every Region, so exactly one succeeds. The other gets `ConditionalCheckFailedException`, or `ReplicatedWriteConflictException` and then the failed condition on retry.
3. The winner's cell commits `owner.state = Active` and the creator's admin membership. The loser's cell deletes its `Pending` row and answers 422 with `errors: [{resource: "Organization", field: "login", code: "already_exists"}]` (`research/wire-census.md#20`). If a cell dies between claim and activation, its sweeper reads the claim and activates the row when the claim names its owner id, or deletes the row otherwise. The claim is the commit point.
4. The registry's stream feeds the RouteBook publisher, which writes a one-second delta to every Region's RouteBook bucket. Front doors pick it up within about 3 seconds [guess].
5. `PATCH /repos/newco/a {"name":"b"}` goes to newco's home, which owns the names of newco's repositories. One `core` transaction turns `repo_name(newco, a)` into `Alias -> repo id`, inserts `repo_name(newco, b)`, updates the repo row, and emits `repository.renamed`. The registry is not involved. If the repository lived in another cell (an org fork), a `RepoName` envelope updates that cell's display copy seconds later.
6. `GET /api/v3/repos/newco/a` reaches newco's home, finds the alias, and answers `301` with `Location: https://tributary.dev/api/v3/repositories/<id>` and the `Moved Permanently` body. The client follows, and the front door routes the id by its slot bits. `git clone https://tributary.dev/newco/a.git` reaches the same alias and gets `200` with the advertisement served directly (`research/wire-census.md#23`). The repo-route cache never stores aliases, so every old-name request consults the home. Creating a new `newco/a` deletes the alias row in the same transaction.

Synchronous dependencies outside the serving cell: the registry, in step 2 only. Steps 5 and 6 touch only newco's home and, for an id route, the cell that holds the repository.

**Journey 4: alice (home `c3`) belongs to `acme` (`c7`), `globex` (`c9`), and `initech` (`c12`).**

1. `gh repo list` asks GraphQL for `repositoryOwner(login: "alice") { repositories(ownerAffiliations: OWNER) }` [inference from gh's query builder, `research/clients.md#48`]. It reaches `c3`, which reads alice's `repo_name` rows, including names whose repositories live in other cells, and the display fields it keeps for them. One cell, no fan-out.
2. GraphQL `viewer { repositories }` defaults to `ownerAffiliations: [OWNER, COLLABORATOR]` [unverified]. `c3` serves it from `repo_name` rows and `collaborator_ref` mirror rows. One cell.
3. `GET /user/repos` defaults to owner, collaborator, and organization member affiliations. `c3` reads alice's `membership_ref` mirror (3 orgs), then calls `fan_out` on `c7`, `c9`, and `c12` with `repos_visible_to(org, passport)` sub-queries under a 300 ms per-source deadline, and merges by `full_name`. If any org cell fails, the request returns 503 with `Retry-After` and never a silently partial page. Synchronous dependencies outside `c3`: the three org cells.
4. `PUT /user/starred/acme/widgets` routes by the owner in the path to `c7`, which checks read access in its own snapshot, writes the canonical `star` row and the stargazer count, and emits a `Star` envelope that inserts `star_ref` in `c3`. `GET /user/starred` is served by `c3` from those mirror rows.
5. A push to `acme/widgets` that alice watches produces a notification row in `c3` `aux` through `CrossCell`. `GET /notifications` is served by `c3`, and email goes out from `c3`.

**Journey 5: a GitHub App owned by `toolco` (home `c5`) is installed on `acme` (`c7`) and `globex` (`c9`).**

1. An acme admin installs the app through a web flow served by `c7`. `c7` reads the app's permission manifest, public keys, and webhook config from `c5` once (the webhook secret sealed to `c7`'s directory key), then commits the canonical `installation` row, keyed by an `InstallationId` allocated in acme's slot, and sends an `Installation` envelope to `c5` so `GET /app/installations` lists it. Later key or webhook changes reach `c7` as `AppKeys` envelopes. `c7` therefore verifies app JWTs and signs webhook deliveries without calling `c5`. Synchronous dependency outside `c7`: `c5`, at install time only.
2. The app signs a JWT and calls `POST /api/v3/app/installations/<id>/access_tokens` for each installation. The front door routes each call by the installation id's slot, to `c7` and to `c9`. Each cell verifies the JWT against its mirrored keys and mints a stateless `ghs_` token whose claims name the issuing cell and key epoch, signed with that cell's key.
3. `GET /api/v3/repos/acme/x` with acme's token goes to `c7`, which verifies its own signature, checks the deny list, intersects the claims with the installation rows in its snapshot, and serves. The globex token goes to `c9` in the same way. A token used on a public repository in a third cell verifies there too, because every cell's public signing keys are in the signed `CellDirectory`. No step calls `c5`.
4. Uninstall or suspension bumps the installation's `not_before` in `c7`. `c7`'s revocation feed reaches every Region's front doors within 15 seconds [guess].

## Shape

### Names and the directory

```rust
/// One of 65,536 placement units, fixed for the life of the service. Slots move between cells. Ids never change.
pub struct Slot(u16);
pub struct CellId(u16);

/// Every numeric id: bits 37..53 hold the slot the entity lives in, bits 0..37 a per-slot sequence from 1.
/// Invariant: 0 < id < 2^53 (research/wire-census.md#28). Allocation fails closed at 2^37 per slot and kind.
pub struct DbId(i64);
impl DbId { pub const fn slot(self) -> Slot { Slot((self.0 >> 37) as u16) } }
pub struct OwnerId(DbId);         // slot = the owner's home slot
pub struct RepoId(DbId);          // slot = the slot of the repository's network; a transfer keeps it
pub struct AppId(DbId); pub struct InstallationId(DbId); pub struct EnterpriseId(DbId);

/// 1 to 39 ASCII letters, digits, or single hyphens, not leading or trailing, ASCII case-folded for uniqueness.
pub struct Login { shown: ArrayString<39>, folded: ArrayString<39> }
/// Managed users are "<base>_<shortcode>", the GitHub EMU scheme. They are never claimed one by one:
/// the enterprise's shortcode claim reserves the whole suffix, and the enterprise's home resolves the base.
pub struct Shortcode(ArrayString<8>);
pub enum OwnerName { Public(Login), Managed { base: ArrayString<30>, shortcode: Shortcode } }

/// The only facts that need one answer service-wide.
pub enum ClaimKey { Login(Login), Shortcode(Shortcode), AppSlug(Login), SshKey(Fingerprint), Email(EmailDigest) }
pub enum Holder { Owner(OwnerId), Enterprise(EnterpriseId), App(AppId), UserKey(OwnerId), DeployKey(RepoId) }
pub enum ClaimState {
    Active { holder: Holder, version: u64 },
    /// A renamed login. Routes the old owner's repository paths until someone claims the name.
    Alias { owner: OwnerId, since: Timestamp, version: u64 },
    /// Route words (api, login, settings, orgs, users, explore, ...) and names held for abuse review.
    Reserved { reason: ReserveReason },
}
/// Stored under the same partition key as the login's claim item, so one Query returns both.
pub struct Retired { pub login: Login, pub repo: RepoName }

/// DynamoDB MRSC in managed mode; the `name_claim` table in `core` in single-cell installs.
/// Only the `api` role writes. No request path reads it, except `get` on a bounded authenticated cold miss.
pub trait Registry {
    /// Conditional put: absent, or Alias. A repeat by the same holder returns the existing claim.
    async fn claim(&self, key: ClaimKey, holder: Holder) -> Result<Claimed, ClaimError>;
    /// Rename, step 2 of 2: Active{owner} becomes Alias{owner}, plus Retired items. CAS on `seen`.
    async fn alias(&self, old: &Login, owner: OwnerId, seen: u64, retire: &[RepoName]) -> Result<(), ClaimError>;
    async fn release(&self, key: &ClaimKey, holder: Holder, seen: u64) -> Result<(), ClaimError>;
    /// Strongly consistent read of the claim item and its Retired items.
    async fn get(&self, key: &ClaimKey) -> Result<Option<(ClaimState, Vec<Retired>)>, RegistryError>;
    /// Slot map entries are items too. CAS on generation. Only `migrate_slot` calls this.
    async fn set_slot(&self, slot: Slot, entry: SlotEntry, seen_generation: u64) -> Result<(), ClaimError>;
}
pub enum ClaimError { Taken, Retired, Stale, Invalid(LoginRule), Unavailable }

/// Per-Region read-only projection of the registry, built by `route-publisher` from the registry's stream.
/// s3://trib-routebook-<region>/g<gen>/shard-0000..4095 plus one-second deltas under d/<seq>.
pub struct RouteBook {
    pub generation: u64, pub watermark: Timestamp,
    logins: Fst,       // folded login -> (OwnerId, OwnerKind, Active | Alias | Reserved)
    ids: Fst,          // OwnerId -> shown login, so any cell renders any actor without a call
    keys: Fst,         // SSH key fingerprint -> Holder
    shortcodes: Fst,   // shortcode -> EnterpriseId
    // Email digests are separate S3 shards in the same bucket. Cells fetch them on miss; front doors never load them.
    pub slots: SlotMap,
}
pub struct SlotMap { pub generation: u64, entries: Box<[SlotEntry; 65_536]> }
pub struct SlotEntry { pub cell: CellId, pub route: SlotRoute }   // Serving | Frozen | Moved { to }
/// Refuses a delta whose generation is not newer, whose removals exceed 0.1% of entries, or that moves more than
/// 64 slots without a signed override. The node then keeps its last known good book.
pub fn apply_delta(book: &RouteBook, d: &Delta) -> Result<RouteBook, DeltaRejected> { todo!() }
```

Invariants. Each key has one item, so a claim needs no transaction, which MRSC does not offer. The claim item is the commit point of owner creation, renames, SSH key registration, and email verification. Every saga writes its cell rows in a pending state first, claims second, and activates third, and a sweeper finishes or undoes any saga by reading the claim. A rename is two conditional writes (claim the new login, then turn the old one into `Alias`). Between them both names route to the same owner, which is harmless. Retired pairs are computed by the home at rename time from its clone and Actions-use counters (more than 100 in the prior week, the GitHub rule) and written under the old login's partition. When someone later claims that login, its home copies the `Retired` items into `retired_repo_name` rows before the owner becomes `Active`, so repository creation stays cell-local.

Why each item is global. A login, a shortcode, and an app slug each name one thing in URLs. An SSH key must name one principal before the client sends its command, because SSH carries no hostname. A verified email must map to one user for commit attribution. The slot map must be one fenced value per slot. Nothing else qualifies. Repository names, aliases, tokens, sessions, memberships, permissions, stars, installations, and id sequences are each owned by one cell.

Sizing [guess]: 200 million owners, 100 million keys, and 300 million emails give about 600 million items of under 200 bytes each, around 120 GB, and a write rate equal to signups plus key and email changes. The front door preloads logins, ids, keys, shortcodes, and slots, roughly 6 GB as finite-state transducers on local NVMe, mapped into memory. Email digests stay in S3 shards that cells fetch on demand.

Dominant access patterns. A request with an owner in its path costs one in-memory lookup of the login and one slot map index. A request carrying an id costs a shift and an index. A cold front door costs a local disk read. An unknown login is a 404 from the book, with no registry read, when the request is anonymous. An authenticated miss reads the registry only if the book's watermark is older than the claim could be, under a per-node budget of 20 reads per second and a 5-second negative cache, so scanners never reach the registry. An owner creation reads and writes the registry once.

### Identity, orgs, and authorization

```rust
/// GitHub's stored-token shape: prefix, 30 base62 characters, 6-character CRC32 (research/wire-census.md#29).
/// The first 3 characters encode the issuing home slot (62^3 > 65,536), leaving 160 random bits.
/// Session cookies, OAuth codes, device codes, and refresh tokens carry their slot the same way.
pub struct OpaqueToken { kind: TokenKind, home: Slot, secret: [u8; 27], crc: [u8; 6] }

/// Minted by the caller's home, carried on every internal hop, verified anywhere with directory keys.
pub struct Passport {
    pub actor: Actor, pub login: Login, pub credential: CredentialRef, pub scopes: ScopeSet,
    pub flags: AccountFlags,           // suspended, site_admin, managed_by: Option<EnterpriseId>
    pub issued_by: CellId, pub key_epoch: u32, pub iat: Timestamp, pub exp: Timestamp,  // exp = iat + 15 min
}
pub struct SignedPassport(Jws<Passport>);

/// Signed fleet config, shipped in config waves: every cell's Regions and public keys. Private keys never leave a cell.
pub struct CellDirectory { pub generation: u64, cells: HashMap<CellId, CellEntry> }
pub struct CellEntry { pub regions: SmallVec<[Region; 2]>, pub sign: [Ed25519Public; 2], pub seal: X25519Public }

/// The internal API each cell serves for the slots it holds.
pub trait Home {
    async fn passport(&self, presented: &PresentedCredential) -> Result<SignedPassport, AuthError>;
}

/// Each cell's revocations, installation not_before epochs, and suspensions, published by its CacheHint consumer
/// to s3://trib-control-<region>/rev/<cell>/ and tailed by every front door every 5 s.
pub struct RevocationView { cursors: HashMap<CellId, FeedCursor>, denied: DenySet }

pub enum AuthStep { Ready(Principal), NeedPassport { home: Slot }, Denied(AuthError) }
pub fn authenticate(h: &RequestAuth, dir: &CellDirectory, cache: &PassportCache,
                    rev: &RevocationView, book: &RouteBook) -> AuthStep { todo!() }

/// DESIGN.md's claims with `tenant` replaced by the issuing cell, which is the installation's cell.
pub struct InstallationClaims { pub issued_by: CellId, pub app: AppId, pub installation: InstallationId,
    pub repos: RepoScope, pub permissions: PermissionSet, pub jti: Ulid, pub iat: Timestamp, pub exp: Timestamp,
    pub key_epoch: u32 }

pub struct Principal { pub actor: Actor, pub scopes: ScopeSet, pub credential: Credential, pub passport: SignedPassport }

/// Unchanged contract. Pure over permission rows that live in the cell holding the resource.
pub fn authorize(p: &Principal, action: Action, on: &ResourceRef, perms: &PermissionSnapshot) -> Decision { todo!() }
```

Authentication at a cell that is not the caller's home. The front door, not the cell, authenticates. It checks the prefix and CRC offline, reads the home slot from the token, and looks for a cached passport keyed by the token's SHA-256. On a miss it calls the home once, and the home does the primary-key read of `credential` that `DESIGN.md` did. Every later request in the 15-minute life of the passport checks only the local `RevocationView`. The serving cell receives the passport over mTLS, verifies its signature against the directory, and authorizes against its own rows. SSH works the same way after the key step. The front door verifies the client's signature over the session with the offered public key, looks the fingerprint up in the RouteBook to find the principal and home, and fetches a passport. A deploy key's holder is a repository, so the passport comes from the repository's cell. One service host key serves every SSH connection, as on github.com, held wrapped on front-door disks and unwrapped at boot through KMS multi-Region keys.

If the home is unreachable when a passport expires, the front door keeps honoring it for up to 2 hours, the same signed limit as `DESIGN.md`'s stale-fetch mode. While the home is down it cannot commit a revocation, a password change, or a suspension, so a cached passport can only miss revocations that committed in the seconds before the outage and had not reached the feed. A user whose home is down and who has no cached passport cannot authenticate anywhere. That is the coupling this direction accepts. Paired cells make it a two-Region event. A weekly drill blackholes a canary cell's `Home` API and checks that its users keep working on cached passports.

Where each relation lives. The canonical row sits in the cell that authorizes against it, which is the cell of the object being granted. The other party's home keeps a derived mirror that serves "my" listings. Two mirrors feed access decisions: `OrgAuthority`, which authorizes and is described after the table, and `AppKeys`, which verifies app JWTs.

| Relation | Canonical row | Derived copy |
| --- | --- | --- |
| Org and team membership | Org's home (`org_member`, `team_member`) | `membership_ref` in the member's home; `OrgAuthority` mirror in each cell that hosts a fork the org owns |
| Repository collaborator | Repository's cell | `collaborator_ref` in the collaborator's home |
| Star, watch | Repository's cell | `star_ref`, `watch_ref` in the user's home |
| Follow | Follower's home | `follower_ref` in the followee's home |
| App installation | Target owner's home | `installation_ref` in the app's home |
| Repository name and aliases | Owner's home (`repo_name`) | display copy on the repo row in the hosting cell |
| SSO session and SSO token grant | Org's home | none |
| Notification | Recipient's home `aux` | none |

The one mirror that authorizes is `OrgAuthority`. A fork owned by an org lives with its network, so the hosting cell must decide what the org's members may do to it. The org's home sends its member and team rows to every cell that hosts one of its forks (at most the number of cells), and an hourly anti-entropy pass compares row hashes and resends differences. A removed member keeps access to those forks until the envelope applies: p99 under 10 seconds [guess], unbounded while the hosting cell is unreachable from the home. A user-owned fork needs no mirror, because ownership is a column on the repo row.

Orgs, SSO, and enterprises. SAML and OIDC attach to an org or an enterprise account, as on GitHub Enterprise Cloud. The SSO session row and the per-token SSO grant live in the org's home and are read in the same snapshot as the guarded operation. Enterprise Managed Users get `base_shortcode` logins, as on GitHub (https://docs.github.com/en/enterprise-cloud@latest/admin/managing-iam/iam-configuration-reference/username-considerations-for-external-authentication). Their homes are slots in the enterprise's residency, and only the shortcode reaches the registry. Underscores are not legal in public logins, so the suffix cannot collide.

OAuth and first-party clients. The five first-party OAuth apps (`FirstPartyClient`) are seeded once for the service in a system slot, and their client ids are the same in every cell. The web flow runs on the browser session's home. Its authorization code carries the slot, so `POST /login/oauth/access_token` routes there. `POST /login/device/code` lands on a cell in the front door's Region. Its `device_code` carries that cell's slot, and 2 of the 8 characters of `user_code` encode the cell. The user approves on their home, which asks the flow cell to bind the code. The flow cell has the home mint the token and hands it to the polling client.

Ids. Each slot allocates from `id_block(slot, kind)` in leased blocks of 1,000 per node, with 2^37 ids per slot and kind and an alarm at 2^36 [inference: GitHub's whole check-run id space is about 1.2e11, `research/wire-census.md#28`, so 1.4e11 per slot is ample]. Node ids keep GitHub's new format with the repository id first (`research/wire-census.md#27`), so `node(id:)` routes by arithmetic.

### Placement and routing

```rust
pub enum Target<'r> {
    Owner(&'r OwnerName, Option<&'r RepoName>),   // /{o}/{r}..., /api/v3/repos/{o}/{r}..., /orgs/{o}, /users/{u}
    Id(DbId),                                     // /repositories/{id}, /user/{id}, /teams/{id}, node and installation ids
    Caller,                                       // /api/v3/user..., /notifications, /api/graphql: the caller's home
    App,                                          // /api/v3/app...: the slot of the JWT's `iss`
    Flow(FlowCode),                               // OAuth and device codes carry a slot
    Search,                                       // /search/*, the derived search tier
    Local,                                        // /meta, /status, /api/v3/status, /api/v3/, /rate_limit
}
pub fn classify(req: &RequestHead) -> Result<Target<'_>, NotFound> { todo!() }

pub enum Hop { Cell { cell: CellId, slot: Slot }, NotFound }
pub fn route(book: &RouteBook, cache: &RepoRouteCache, t: &Target<'_>, caller: Option<&SignedPassport>) -> Hop { todo!() }

/// (owner, repo) -> (slot, repo id), learned only from a home's forward of a Live name, never of an alias.
/// The receiving cell checks that the repo id still has that owner and name, and answers Misrouted otherwise.
pub struct RepoRouteCache(LruMap<(OwnerId, FoldedRepoName), (Slot, RepoId)>);

/// One value per DSQL Region set, because a Paired cell's clusters cannot leave their set.
pub enum Residency { Global, Us, Eu, Apac }
pub struct Cell { pub id: CellId, pub durability: Durability, pub residency: Residency,
                  pub core: DsqlEndpoints, pub aux: DsqlEndpoints, pub packs: PackBuckets, pub limits: CellLimits }

/// New owners go to a slot on the cell with the most headroom in their residency, nearest the creator's Region.
/// Enterprises above 5% of a cell's limits get dedicated slots on a dedicated cell.
pub fn place_owner(kind: OwnerKind, residency: Residency, near: Region, fleet: &Fleet) -> Slot { todo!() }
/// A root network goes to the owner's home slot, or round-robin over the owner's extra slots for very large orgs.
pub fn place_network(owner: &OwnerFacts) -> Slot { todo!() }
pub enum ForkPlacement { WithNetwork(Slot), Detached(Slot) }
/// WithNetwork unless the network's cell is outside the fork owner's residency.
pub fn place_fork(network: Slot, owner: &OwnerFacts, fleet: &Fleet) -> ForkPlacement { todo!() }

/// ACT-3's fence, keyed by slot: one slot_state row per slot in each of the cell's two clusters.
pub enum SlotState { Serving, Frozen { generation: u64 }, Moved { to: CellId, generation: u64 } }
/// Every transaction is bound to one slot. Every key type exposes slot(), and Tx refuses a key from another slot.
pub struct TxCtx { pub slot: Slot, pub deadline: Duration, pub request: RequestId }
/// Clone, Freeze, Drain, Flip (Registry::set_slot, then a RouteBook delta), Redirect, Forget, as in DESIGN.md.
pub async fn migrate_slot(slot: Slot, from: CellId, to: CellId) -> Result<(), MigrationError> { todo!() }
```

What "home" means. An owner's home slot holds its identity (profile, emails, passwords, 2FA, WebAuthn, sessions, credentials, SSH and GPG keys), its name authority (`repo_name` and aliases for every repository it owns, wherever the repository lives), its mirrors (memberships, collaborations, stars, watches, installations), its notification inbox, and every network it roots. A fork it owns lives in the fork network's slot, because the merge invariant needs the head ref and the base ref in one cluster, and on GitHub a network shares one object store anyway. Forks are the only repositories that live away from their owner's home. A transfer changes the owner column and moves the name row between homes, but the network stays in its slot, so `RepoId` routing never needs a forward.

Invariants. A transaction touches one slot, so any cross-slot change is a saga with a named commit point. A slot lives on exactly one cell, which the slot map and the `slot_state` fence define. Every id routes by its slot bits. Only owner names need the book.

Request routing, traced. The front door classifies the request, picks the hop, attaches the passport, and forwards over mTLS to the nearer Region of the target cell. Routes by owner and repo use the repo-route cache when it has an entry and the owner's home otherwise. A home that holds the name but not the repository forwards inside the cell tier and returns a route hint. Its `Misrouted` answer costs one extra hop and evicts the entry. GraphQL goes to the caller's home, because GraphQL requires a token, as on github.com [unverified]. Its executor resolves each root field to a cell by login or node id and forwards the whole document when every root field resolves to one cell, which covers every mutation. Otherwise it runs each root field's subtree on its cell and stitches the results. Nested fields that cross owners resolve through the same `fan_out`. Actor logins and avatars come from the RouteBook's id index and the id-derived avatar URL, so most nested owner fields need no remote call.

### Cross-cell features

```rust
/// Produced in the source cell's outbox (Consumer::CrossCell), applied by the destination slot's inbox.
pub struct Envelope { pub source: CellId, pub event: Ulid, pub to: Slot, pub entity: EntityRef,
                      pub entity_version: u64, pub op: MirrorOp }
pub enum MirrorOp {
    Membership { org: OwnerId, user: OwnerId, role: Option<OrgRole> },
    OrgAuthority { org: OwnerId, members: Chunk<MemberRow>, teams: Chunk<TeamGrantRow> },
    Collaborator { repo: RepoId, user: OwnerId, perm: Option<Permission> },
    Star { repo: RepoId, user: OwnerId, on: bool }, Watch { repo: RepoId, user: OwnerId, level: Option<WatchLevel> },
    Follow { follower: OwnerId, followee: OwnerId, on: bool },
    RepoName { repo: RepoId, owner: OwnerId, name: RepoName, visibility: Visibility },
    Installation { installation: InstallationId, app: AppId, target: OwnerId, live: bool },
    AppKeys { app: AppId, keys: Vec<PublicKeyPem>, hook: SealedBox<HookConfig> },  // sealed to the destination cell
    OwnerFlags { owner: OwnerId, flags: AccountFlags },
    Notify(Box<NotificationRow>),
}
pub trait Inbox {
    /// One transaction per batch and destination slot: insert inbox_seen(source, event), then upsert each mirror
    /// row only where entity_version is newer. A repeat or an older version changes nothing.
    async fn apply(&self, slot: Slot, batch: Vec<Envelope>) -> Result<(), InboxError>;
}
/// Lists that span owners: per-source deadline 300 ms, merged by the endpoint's sort key, any source error -> 503.
pub async fn fan_out<T: SortKey>(sources: &[(CellId, SubQuery)], p: &SignedPassport, page: MergeCursor)
    -> Result<Page<T>, FanOutError> { todo!() }
```

Delivery. `CrossCell` is one more consumer in `DESIGN.md`'s per-consumer outbox, so an envelope exists if and only if its source change committed. The handler batches pending rows by destination cell, resolves each slot through the slot map on every attempt (so a migrated slot follows its cell), and POSTs to the destination's inbox with the source cell's mTLS identity. A destination that is down holds back only its own envelopes. The handler skips them with backoff and does not park them, so deliveries to other cells continue. Mirror writes go to `core` for the mirrors that authorize (`OrgAuthority`, `AppKeys`) and to `aux` for listings and notifications.

Stars and follows. Counts live with the canonical rows: stargazer count in the repository's cell, follower rows in the follower's home, follower counts in the followee's home from mirrors. Notifications. The event's cell computes recipients from its own watch and subscription rows, groups them by home cell, and sends `Notify` batches. A repository with 100,000 watchers makes 100,000 rows in many homes, so the source cell's notification fan-out runs under a per-repository budget of 5,000 recipients per second [guess] in `aux`. Search. A derived search tier per Residency, one set of OpenSearch shards keyed by repo id, is fed by every cell's `SearchIndex` consumer. Each document carries visibility and reader tags (owner, org, team, user). A query fans out to every shard with the caller's tags, which the home computes from the passport and `membership_ref`. Search outages touch only `/search` and the search box (`research/uptime.md#49`). The public `GET /events` timeline comes from the same tier. Commit attribution. Cells map author emails to users through the per-Region email digest shards of the RouteBook, fetched and cached on miss, then render the login from the id index.

## Public service concerns

Anonymous access and rate limits. Anonymous REST gets GitHub's 60 requests per hour per address, counted per front-door node and therefore approximate, which GitHub's own docs allow (`research/wire-census.md#18`). Anonymous GraphQL returns 401, as on github.com [unverified]. Anonymous git has per-address concurrency at the front door and the CPU budget at the cell (Journey 1). Authenticated limits (5,000 per hour per user, 1,000 per repository for `GITHUB_TOKEN`) are counted at the front door per passport. An anonymous lookup of an unknown owner never reaches the registry.

Abuse and spam. Claims are the scarce write, so the front door caps signups and org creations per address and per account (5 per hour per address [guess]), and the registry's on-demand maximum write throughput caps the table so a claim storm cannot become a cost event. A new account carries a reputation flag in its passport that limits repository creation, issue and comment rate (GitHub's 80 content-creating requests per minute, `research/wire-census.md#18`), and forks per hour. Squatting is handled by `Reserved` claims that only an admin flow sets. Spam suspensions set `OwnerFlags`, which the home sends to every cell hosting the owner's repositories and publishes in its revocation feed.

Takedowns. A legal takedown sets `Disabled { notice }` on a repository row, or on every repository in a network in one transaction per 1,000 repositories, which is possible because a network lives in one slot. Disabled repositories answer 451 on the API [unverified that GitHub uses 451] and refuse git with the notice text. Account takedown is a suspension plus disabling owned repositories, including forks in other cells through `OwnerFlags`.

CDN for public content. CloudFront fronts the isolation subdomains only: `raw.` and `codeload.` by commit SHA, which never changes, release assets, and avatars by id. The origins are the front doors, which apply visibility, so a repository that turns private stops serving within the 5-minute `max-age` of uncached content, and SHA-addressed objects are purged through an outbox consumer. Git smart HTTP and the APIs never go through a CDN. CloudFront's control plane is in us-east-1 (`research/uptime.md`, Answer), so distributions are created at install, and a CloudFront failure moves the isolation subdomains to the front doors' NLBs through Route 53 health checks.

Enterprise residency and SSO. Residency is offered to enterprises that use managed users, which matches GitHub offering residency on GHE.com [unverified]. Their orgs, users, and every network they root live in slots on cells inside one DSQL Region set. The registry holds only the shortcode and the org logins, which are public names. A fork of an outside public repository into a resident org is `Detached`: a new network inside the residency with the objects copied, `fork: true`, and a 422 with an explanatory message for a pull request back to upstream. Managed users cannot push to or open pull requests on repositories outside their enterprise on GitHub either (https://docs.github.com/enterprise-cloud@latest/admin/managing-iam/understanding-iam-for-enterprises/abilities-and-restrictions-of-managed-user-accounts). SSO is per org or enterprise, and its sessions live in the org's home.

## Failure modes

New stateful components on the request path. Cells, DSQL, S3, cache nodes, Actions, and search keep their rows from `DESIGN.md`, with "tenant" read as "slot". The per-tenant edge rows are replaced by the front door rows.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| Name registry (DynamoDB MRSC: replicas us-east-2 and eu-west-1, witness us-west-2) | instance | Single claim calls fail | SDK retries under the 10% retry budget. Claims are idempotent per holder. |
| Name registry | az | None; DynamoDB spans AZs | Nothing to do. |
| Name registry | region | One replica Region: claims from cells nearer it go to the other replica | MRSC keeps reads and writes with one replica plus the witness at RPO zero (`research/uptime.md#29`) [inference for writes during a replica loss]. Nothing on the request path changes, because routing reads only the RouteBook. A second lost Region stops claims (signups, renames, key and email changes) until a new table is seeded from the survivor, because an MRSC table cannot add a replica in place (AWS global tables page cited in "Problem"). |
| Name registry | control-plane | None at runtime | The table, its stream, and its alarms are created at install. Nothing in recovery calls a DynamoDB management API. |
| Name registry | bad-deploy | Wrong claims or aliases service-wide | Every write is a conditional create or a version CAS from `trib-registry`, the only writer. Claim code ships in the last wave after a week on the canary cells. A daily audit compares every `Active` claim with its home's owner row and alarms on any orphan. Point-in-time recovery is enabled on both replicas [unverified for MRSC]. |
| Name registry | bad-config | Claims fail where the config is wrong | The readiness probe claims and releases a canary key. Signed config, last known good. |
| Name registry | noisy-tenant | One actor's signups or renames | Per-address and per-account claim limits at the front door and the table's maximum write throughput. |
| RouteBook (publisher, per-Region buckets, local copies) | instance | Publisher stops: new claims stop propagating | The publisher holds a lease in both registry Regions and a standby takes it within 30 s. Front doors keep their book. Authenticated misses fall through to the bounded registry read. |
| RouteBook | az | None | Publisher in two AZs. S3 spans AZs. |
| RouteBook | region | That Region's bucket: its front doors stop receiving deltas | Front doors keep the last known good book on disk, and only owners created during the outage are not routable there. Front doors in that Region are already unhealthy if the Region is down. |
| RouteBook | control-plane | None | Buckets pre-created, presigned reads computed locally. |
| RouteBook | bad-deploy | A wrong or empty book in every Region | `apply_delta` refuses non-monotonic generations, removals above 0.1%, and more than 64 slot moves without a signed override, the stale-plan lesson of the October 2025 DynamoDB DNS race (`research/uptime.md#32`). A full rebuild is compared with the incremental book before publication. |
| RouteBook | bad-config | Same as bad-deploy | Same guards. Signed config. |
| RouteBook | noisy-tenant | Name churn by one actor | Claim limits bound delta size. |
| Front door (Regional fleet with passport cache) | instance | Connections on that node | NLB health checks with the 33% removal cap. Clients retry. |
| Front door | az | One third of a Region's front doors | Static 66% sizing, zonal shift, weekly autoshift practice. |
| Front door | region | Clients routed to that Region | Route 53 latency records with deep health checks move clients to the next Region. Cells keep serving through other Regions' front doors. |
| Front door | control-plane | Dead nodes not replaced while EC2 launch is impaired | N+1 static capacity. Certificates, book, directory, and wrapped keys are on local disk, so a node boots from its own Region alone. |
| Front door | bad-deploy | One Region's clients | Waves go one Region at a time, AZ by AZ, by hot swap with automatic swap-back on 5xx, TLS, or routing-error rate. DNS moves clients away from a sick Region. |
| Front door | bad-config | One Region | Signed config waves by Region, last known good kept. |
| Front door | noisy-tenant | One owner's or address's traffic | Per-address, per-passport, and per-owner admission, and forward pools to cells shuffle-sharded by owner, so a hot owner cannot hold every connection to its cell (`research/uptime.md#7`). |
| Cross-cell delivery (CrossCell consumer and inboxes) | instance | Envelopes in flight | Lease moves. Apply is idempotent on (source, event). |
| Cross-cell delivery | az | None | Runs in the cell's `worker` role across AZs. |
| Cross-cell delivery | region | Paired cells: none. Regional destination down: its envelopes wait | Per-destination backoff without parking. Other destinations continue. |
| Cross-cell delivery | control-plane | None | No management calls. |
| Cross-cell delivery | bad-deploy | Wrong mirrors in many cells | Mirrors other than `OrgAuthority` and `AppKeys` only serve listings. `rebuild_mirrors(owner)` replays from canonical rows. The hourly anti-entropy pass catches authority drift. |
| Cross-cell delivery | bad-config | Envelopes sent to the wrong cell | The inbox refuses envelopes for slots it does not hold, and the sender re-resolves through the slot map. |
| Cross-cell delivery | noisy-tenant | A hot repository's notification fan-out | Per-repository fan-out budget, per-destination batching, and the work stays in `aux`. |

## Deployment modes

BYOC and self-hosted run the same binary with one cell, every slot mapped to cell 1, and the install's own hostname, so every route resolves to the local cell and no hop crosses cells.

| Aspect | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Namespace | Service-wide, registry in DynamoDB MRSC | The install's own | The install's own |
| `Registry` backend | `MrscRegistry` | `CoreRegistry`: table `name_claim` in `core`, same items and conditions | `CoreRegistry` |
| Hostname | `tributary.dev` plus isolation subdomains | Customer domain | Customer domain |
| Cells and slots | Many cells, 65,536 slots | One cell holding every slot | One cell holding every slot |
| Front door | Regional fleets in front of cells | The `front` role on the cell's nodes | The `front` role on the cell's nodes |
| RouteBook source | Registry stream | `name_claim` changes through the outbox, same S3 format | Same as BYOC |
| Passports, `CrossCell`, inboxes | Across cells | Same code, every hop local | Same code, every hop local |
| Search | Residency search tier | The cell's OpenSearch domain | Optional |
| Rate limits | On | Off by default | Off by default |
| Imported ids | New ids | Source ids kept, since every slot is local | Source ids kept |

`Registry` has two implementations because the backing store is the deployment-mode variable. A contract test runs the same claim, alias, and race suite against both. Self-hosted listings now read mirrors that lag by seconds, where the old design read the canonical rows. That is the cost of one code path.

## Changes to DESIGN.md

Deletions first.

- **Summary.** Delete the bullet "Each tenant is one GHES-shaped hostname" and "managed tenant" wording in the BYOC sentence.
- **Usage.** Delete the managed quickstart (host label, `<label>.tributary.cloud`, residency pair per tenant, tenant map write) and the `acme.tributary.cloud` host in the `gh` example. Journey 1 step 1 (tenant NLB) and step 2 (SNI to tenant) are deleted.
- **Tenancy and cell placement.** Delete `TenantId`, `TenantHost`, `TenantState`, `RouteState`, `TenantRoute`, `TenantMap`, `TenantMapSource`, `MAX_ROUTE_CHANGES` as a host-keyed cap, `route(map, sni)`, and `migrate_tenant`. Delete the bullets "The tenant is the host", "Fork networks live inside a tenant", "SSH resolves the tenant from the port", and "Ids are tenant-local", and the tenant-placement bullet.
- **Repository storage and ref transactions.** Delete `tenant` from `RepoKey` and "all keys lead with `tenant_id`" from the table list. Delete the per-tenant stale-fetch limit wording.
- **Identity and tokens.** Delete per-tenant signing keys that move with the tenant, `Principal.tenant`, `id_block(tenant, kind)`, "seeded into every tenant", "SAML and OIDC per tenant", and the per-tenant credential and permission snapshot.
- **Service and crate map.** Delete per-tenant SSH target ports and host keys from `edge` and `trib-edge`. Delete "places tenants, provisions hosts and certificates" from `tributary-fleet`.
- **Failure modes.** Delete the seven "Edge and tenant map" rows.
- **Deployment modes.** Delete the "Tenants per cell" and "Host and certificate" rows.
- **Tradeoffs accepted.** Delete "one host per tenant, with no global user graph and no cross-tenant forks" and "one NLB per tenant".
- **Alternatives considered.** Delete "A github.com-style shared namespace across tenants in managed mode", now the requirement.
- **Open questions and risks.** Delete the per-tenant NLB quota question.
- **Review resolutions.** Mark ACT-6 superseded: keys are unique service-wide, so no shared listener can be captured.

Replacements.

- **Tenancy and cell placement** becomes **Slots, homes, and placement**: this document's "Placement and routing" types, `SlotState` as the ACT-3 fence, `migrate_slot`, and `TxCtx.slot`. ACT-3 is re-keyed by slot, with the same 30 s transaction horizon and 60 s freeze bound.
- **Repository storage.** `RepoKey { network: NetworkId, repo: RepoId }`, where both ids carry the slot. Every table is keyed by an entity id that carries its slot. `tenant_state` becomes `slot_state`. The stale-fetch limit is per cell. Stale fetches authenticate from cached passports and the `RevocationView` (ACT-14 re-keyed).
- **Identity and tokens.** `OpaqueToken` carries the home slot, `Passport` and `CellDirectory` replace the credential snapshot, per-cell signing keys in the directory replace per-tenant keys (ACT-9 re-keyed), `InstallationClaims.issued_by` replaces `tenant`, `id_block(slot, kind)`, and first-party apps are seeded once in a system slot.
- **Issues and pull requests.** `HeadSpec` keeps "same network as the base repo", now guaranteed by `place_fork`. The merge sketch is unchanged.
- **Event outbox.** `OutboxEvent.tenant` becomes the slot, partitions are `hash(repo or owner) % 256` per cell, and `Consumer` gains `CrossCell`.
- **Actions.** The OIDC issuer is `https://tributary.dev/_services/token` for the service, with every cell's key in its JWKS. The `actions` mirror org is one per install. `concurrency_slot` is keyed by repository or org.
- **Wire compatibility.** The `git-ssh` oracle becomes: `ssh -T git@tributary.dev` greets the key's user, and registering one key on a second account returns 422 `key is already in use`. `rest:search` and `rest:activity` name the search tier and `CrossCell`. The hostname paragraph names `tributary.dev` and its isolation subdomains.
- **Deployment modes.** Rows from this document's table replace the deleted rows.

Additions.

- **Shape** gains **Names and the directory** and **Cross-cell features** from this document.
- **Service and crate map** gains the `front` role, `route-publisher` in `worker`, and crates `trib-registry` (the only code that writes claims) and `trib-route` (`classify`, `route`, `RouteBook`, `apply_delta`).
- **SLOs** gain name claims at 99.9% (43.2 minutes per month), measured by a synthetic claim and release every minute per registry Region, and cross-cell mirror freshness at p99 under 10 s, measured by canary envelopes.
- **Failure modes** gain the 28 rows above. **Drills** gain the weekly `Home` blackhole and a monthly registry Region isolation through AWS FIS, which supports MRSC (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html).
- **Known deviations from GitHub** gain three rows: org fork access lags member removal by seconds; repository display names in a fork's hosting cell lag a rename by seconds, so a webhook in that window shows the old `full_name`; a new login can 404 for about 3 s at another Region's front door for anonymous callers.
- **Build order.** Milestone 2 gains the front door, the RouteBook, passports, and `CoreRegistry`, with the gate: a self-hosted install routes every `research/clients.md` oracle through the `front` role. A new Milestone 4b brings `MrscRegistry`, two cells, `CrossCell`, and the five journeys of this document as its gate, with a registry Region isolated mid-run and zero double claims. Milestone 4's migration gate moves a slot instead of a tenant.
- **Open questions** gain this document's list.

## Tradeoffs accepted

- We accept that a user whose home cell is unreachable and who holds no cached passport cannot authenticate anywhere, in exchange for no global credential store and no global auth database to overload.
- We accept that cached passports keep working for up to 2 hours while their home is unreachable, and can miss revocations that committed in the seconds before the outage, in exchange for users who keep working through a home outage.
- We accept forks that live away from their owner's home, a home on the first-request path to its owner's forks, and an `OrgAuthority` mirror that lags member removal by seconds, in exchange for keeping the one-transaction merge for every pull request.
- We accept a front-door fleet in every serving Region and a RouteBook of about 6 GB per node [guess], in exchange for routing that never reads a database.
- We accept a cross-continent registry with claim latency of one intercontinental round trip or more [inference], in exchange for one uniqueness authority that survives a Region loss, which DSQL cannot be because its multi-Region clusters stay in one Region set.
- We accept that public logins and org names are stored in the registry's Regions whatever the owner's residency, in exchange for one namespace. Residency covers everything else, and managed users never reach the registry.
- We accept 503 for `GET /user/repos` when any of the caller's org cells is down, in exchange for no per-member copy of every org's repository list.
- We accept that a migration moves a whole slot, and so moves its neighbors along with a hot owner, in exchange for ids that route by arithmetic forever.
- We accept 16 bits of every id for the slot, which caps a slot at 2^37 ids per kind, in exchange for routing ids without a lookup.

## Alternatives considered

- **A global identity tier.** Users, orgs, credentials, memberships, and permissions in one service-wide store, with cells holding only repositories. Every authenticated request, and every authorization that touches an org, would read a global store, which is the shape of GitHub's 2023 permissions-cluster outage and its 2026 auth database overload (`research/uptime.md#44`, `#47`). DSQL cannot be that store across continents, and DynamoDB MRSC has no transactions, so membership changes and the checks that read them could not commit together. It lost on blast radius and on the transaction boundary.
- **Forks in the fork owner's home, with cross-cell pull requests.** Every repository lives with its owner, so "home" stays literal. A pull request from bob's fork in `c3` into acme's repository in `c7` cannot compare-and-set the head ref in the merge transaction. `c7` would merge against a mirrored head that can be stale, which is GitHub's "Head branch was modified" race with no detection, and objects would be copied between cells for every cross-cell fork. It lost because the merge invariant is the core of `DESIGN.md`.
- **A routing table of every repository name in the global tier.** It would remove the home hop for forks and make every repository a global item. Repository names change far more often than logins, the table would hold billions of items, and every repository create and rename would become a cross-continent write. It lost on size and on putting the global tier on repository writes.
- **Name lookups on the request path through the registry with a cache, no RouteBook.** It is simpler and smaller at each node, but a registry problem or a scan of unknown names would reach the uniqueness authority on the read path, the fallback-under-load pattern AWS warns against (`research/uptime.md#5`). It lost on availability.

## Open questions and risks

- What is MRSC claim latency for us-east-2, eu-west-1, and us-west-2, and from Asia-Pacific cells? AWS publishes none (`research/uptime.md` open question 3).
- Is about 6 GB of RouteBook per front-door node acceptable at 200 million owners, or should logins shard across front-door stripes? [guess on size]
- Hot networks: `torvalds/linux` and its forks live in one slot on one cell. Is a dedicated cell enough, or do public clones of a few networks need read-only replicas in other Regions?
- Does any client break on a 160-bit-random `gho_` token whose first characters encode a slot? The format and length are unchanged [inference].
- Does GitHub's GraphQL `viewer.repositories` default to owner and collaborator affiliations? [unverified] If it includes org membership, Journey 4 step 2 fans out too.
- Does encoding the cell in 2 of 8 `user_code` characters weaken device-flow brute-force resistance enough to need a shorter code lifetime?
- Can an MRSC table that lost a Region for good be replaced without a claim outage? The seeding procedure needs a drill before launch.
- Is the `OrgAuthority` lag acceptable for security teams, or must member removal also push an immediate deny to the passport revocation feed for that org's forks?
