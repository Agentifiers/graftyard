# Tributary candidate 2: the S3 write-ahead log is the repository

## Problem

Tributary must look like a GHES 3.22 appliance to unmodified clients (research/clients.md#1-5, research/wire-census.md#32-34). It must rank availability of git, pull requests, and the APIs first and durability second (FRAME.md). Three facts make the storage shape non-obvious. A Region and its data can vanish for good (research/uptime.md#37-38), so "never lose an acknowledged push" needs a cross-Region answer. Every large host left shared filesystems, and the hosts that kept local disks pay for it with quorums, placement databases, checksums, and repair sweepers (research/git-storage.md#1-6, #13, #23). S3 now offers a durable compare-and-swap (research/git-storage.md#55), and Continuity acknowledges a push once it sits in an S3 log ordered by one CAS (research/git-storage.md#29-31). This candidate bets on that last fact. Each repository is a log in S3, every local disk is a disposable cache, and git never asks the metadata database for durability or ordering. git asks it for authorization facts only when a branch rule names a fact the database owns, such as required status checks. The other binding constraints are these. No Rust library serves git, so upstream git stays the pack engine (research/git-storage.md#44-49). The Actions toolkit classifies Tributary as GHES (research/actions-protocol.md#43-49). Recovery may not launch capacity or call control-plane APIs (research/uptime.md#1-3, #32-35). The vendor's BYOC tooling cannot roll stateful fleets or observe storage (research/byoc-grounding.md#19, #27, #42-43). Because no node holds truth, the stateful-fleet gap mostly disappears, which is why this direction fits three deployment modes with one artifact.

## Usage (caller's view)

**Operator quickstart, self-hosted.** The customer's own CI role is the deploy principal, exactly as in the vendor platform today (research/byoc-grounding.md#23). The customer brings one hostname and its certificate.

```sh
# once per AWS account: the reused bootstrap, parameterized for the tributary product
tofu -chdir=bootstrap/customer apply \
  -var product=tributary -var deploy_principal_arn=arn:aws:iam::111122223333:role/ci-deploy
# target document: the reused v1 schema plus the tributary operations object
cat > target.json <<'EOF'
{ "version": 1, "name": "acme-git", "profile": "tributary-install",
  "aws": { "accountId": "111122223333", "region": "us-east-2" },
  "terraform": { "stateBucket": "platform-terraform-state-111122223333" },
  "operations": { "hostname": "github.acme.internal", "tlsCertArn": "arn:aws:acm:...",
                  "durability": "zonal", "peerRegion": "us-west-2",
                  "gitNodesPerAz": 1, "apiTasksPerAz": 2, "hostedRunners": false } }
EOF
tributary-deploy release fetch --registry prod --release 3.22.0+t1.0.0   # verifies the KMS signature
tributary-deploy deploy --target target.json --bundle ./bundle --events deploy.jsonl
tributary-deploy admin bootstrap --email admin@acme.example                # first site admin
```

At 2am the self-hoster repairs nothing on git nodes. A dead node is replaced by the Auto Scaling group when EC2 is healthy, and until then the other nodes serve its repositories from S3. The only things that can need a human are S3, DSQL, or the whole Region.

**Operator quickstart, BYOC.** The customer runs the same bootstrap with the vendor's deploy principal, an `sts:ExternalId`, and the observer role (research/byoc-grounding.md#3-7). They grant two things: the deploy role and the read-only observer route. They provide the hostname, the certificate, and optionally a second and third Region for the Region-safe class. The vendor's <vendor-fleet-app> schedules the install into a release wave. Nothing in the data path calls back to the vendor.

**Operator quickstart, managed.** The customer clicks "Create enterprise", picks a hostname (`github.acme.com` as a CNAME to the tenant endpoint, or a vendor subdomain), picks a Region pair from a list (Region-safe is the default), connects a SAML or OIDC identity provider, and invites users. The vendor places the tenant on a cell whose durability class and Regions match.

**Journey 1: `git push origin main` to a protected branch.** Datastores touched before the acknowledgment are the edge's in-memory identity snapshot, the repository register, the S3 log bucket or buckets, and the gitd node's NVMe. DSQL is touched only if the branch requires status checks.

1. git resolves `github.acme.com` to the tenant's edge-shard NLB. The `edge` terminates TLS, stamps `X-GitHub-Request-Id` and `X-GitHub-Enterprise-Version: 3.22.0`, and maps the Host header to a cell with the in-memory router map.
2. The edge parses the Basic password as a token, checks its CRC32 offline, and looks up the token hash in the tenant's identity snapshot, which is loaded from S3 into memory. A token missing from the snapshot (created in the last 30 seconds) is looked up through `api`, which reads DSQL. The edge resolves `acme/widgets` to a `RepoId` in the snapshot's repository directory and builds a `VerifiedGrant`.
3. The edge picks a gitd node by rendezvous hashing inside the tenant's shuffle shard, preferring the repository's primary, and forwards the request with the grant over mutual TLS.
4. For `info/refs`, gitd sends a conditional read to the register (an S3 GET with `If-None-Match` in Zonal cells, a strongly consistent DynamoDB read in Region-safe cells). If the register moved, gitd applies the missing log records to its local bare repository. Upstream `git receive-pack --advertise-refs` answers.
5. For `POST git-receive-pack`, gitd first repeats the conditional register read and applies any new records, so git's own old-value check and its thin-pack base lookup run against the newest committed state. Upstream git then writes the pack into quarantine and runs the pre-receive hook, a small Tributary binary that calls gitd over a Unix socket. gitd runs `trib_policy::evaluate` with the policy at the register's `policy_seq`, the grant, and object facts such as ancestry. A "require pull request" rule denies a non-bypass actor with GitHub's `GH006: Protected branch update failed` text. If the rule set requires status checks and the actor may push directly, gitd asks `api` for a `ChecksAttestation` for the new tip. This is the only DSQL read on any push.
6. git takes its local ref locks and fires the reference-transaction hook in the `prepared` state (research/git-storage.md#38). The hook calls the gitd `Committer`. It uploads the quarantine pack and index to S3 under content-addressed keys with `If-None-Match: *`, in parallel to both active Regions in a Region-safe cell. It writes the immutable log record the same way and then compare-and-swaps the register.
7. If the CAS loses to a push accepted on another node, the committer reads the new register, revalidates each ref's old value against it, and retries on top or rejects that ref with "fetch first" (research/git-storage.md#31). If the CAS wins, the hook exits 0, git finalizes local refs, and `report-status` reaches the client. That moment is the acknowledgment.
8. After the acknowledgment, gitd sends a UDP hint to shard peers, writes a best-effort `repo_cursor` hint row in DSQL, and in Zonal cells queues the record for the cross-Region shipper. The projector reads the record and, in one DSQL transaction, updates pull request projections, inserts outbox events, and advances `projected_seq`. The relay then delivers webhooks and triggers workflows.

**Journey 2: `gh pr merge 42 --squash`.** Datastores touched are DSQL (pull request, reviews, checks, projection, outbox), the register, the S3 log, and gitd NVMe.

1. gh sends the `mergePullRequest` GraphQL mutation to `https://github.acme.com/api/graphql`. The edge authenticates from the snapshot and routes to `api` in the cell.
2. The pull request module reads the row with `ReadConsistency::CaughtUp`. It reads the register head; if the row's `applied_seq` is behind, it applies the missing records inline before it continues.
3. It loads the repository's `RefPolicy` from gitd (cached by `policy_seq`), loads reviews and check runs from DSQL, and evaluates the rule set. On success it builds a `MergeAuthorization` that names the head, the expected base tip, `policy_seq`, and digests of the reviews and checks it relied on.
4. It calls gitd `create_merge`. gitd runs `git merge-tree --write-tree` and `git commit-tree` into a quarantine and checks that the squash commit's tree diff against the base equals the pull request's diff (research/uptime.md#49). It returns the commit and the staged pack.
5. It calls gitd `commit` with a `Merge` transaction. The committer re-evaluates policy and refuses if `policy_seq` moved. It CASes one record that moves `refs/heads/main` and carries the merge annotation. The merged state of pull request 42 now lives in the log, atomically with the ref move.
6. The module applies that record to the projection in one DSQL transaction (state `Merged`, `applied_seq`, outbox rows for `pull_request` closed and `push`) and returns `{merged: true, sha}`. If DSQL fails after the CAS, the response still reports the merge, because the log is the truth. The projector applies the record later, and catch-up reads show it immediately.

**Journey 3: a runner job that uses `actions/checkout` and `actions/cache`.** Datastores touched are DSQL (runs, jobs, leases, cache index, check runs, outbox), the Actions S3 bucket (cache archives, logs), the register and S3 log through gitd, and the action mirror repositories.

1. The projector's push event reaches the workflow engine. It reads `.github/workflows/*.yml` at the pushed commit from gitd, parses and expands the matrix, and inserts the run and its jobs in DSQL as `Queued` with their labels.
2. The runner, registered with `use_v2_flow: true`, long-polls `GET https://github.acme.com/_services/broker/message?sessionId=...`. The broker claims one queued job with a conditional update and returns a `RunnerJobRequest` with the run service URL (research/actions-protocol.md#12, #16).
3. The runner calls `acquirejob` and receives the job message. Its `SystemVssConnection` data sets `CacheServerUrl`, `ResultsServiceUrl`, `GenerateIdTokenUrl`, the runtime token, and a stateless `ghs_` `GITHUB_TOKEN`. The `github` context sets `server_url` to the host, `api_url` to `/api/v3`, and `graphql_url` to `/api/graphql` (research/actions-protocol.md#20).
4. The runner calls the launch service to resolve `actions/checkout@v5` and `actions/cache@v4`. The resolver maps each tag to a commit in the tenant's mirror of the action and returns a tarball URL on the same host, served from gitd by `git archive` and cached in S3.
5. `actions/checkout` runs `git fetch` against the host with `GITHUB_TOKEN`. gitd checks the register and serves upload-pack from upstream git.
6. `actions/cache` sees a GHES host and uses cache v1 (research/actions-protocol.md#43). `GET _apis/artifactcache/cache?keys=...&version=...` looks up the DSQL cache index scoped by ref and returns an `archiveLocation` on the host, which the edge streams from S3. On save, `POST caches` reserves an entry, each `PATCH` chunk becomes one S3 multipart part, and the final `POST` completes the upload and marks the row committed.
7. Logs go to results-service signed URLs that accept `comp=appendblock` (research/actions-protocol.md#23). `renewjob` extends the job lease every 60 seconds. `completejob` sets the job `Completed`, updates the check run, and inserts `workflow_job` and `check_run` outbox events in the same transaction.

**What an unmodified `gh` user types.**

```sh
gh auth login --hostname github.acme.com    # device flow against the seeded client id 178c6fc778ccc68e1d6a
gh repo clone acme/widgets
cd widgets && gh pr create --fill && gh pr merge --squash
# or, non-interactively
export GH_HOST=github.acme.com GH_ENTERPRISE_TOKEN=ghp_...
```

## Shape

The load-bearing decisions are these.

- The repository's truth is a chain of immutable, content-addressed objects in S3 plus one small register per repository. A CAS on the register is the only commit point for refs, pull request merges, and branch protection rules.
- Durability class is a property of a cell, fixed at creation. A Zonal cell uses an S3 object as the register and ships the log to a peer Region asynchronously. A Region-safe cell uses a DynamoDB multi-Region strong consistency (MRSC) item as the register and writes every immutable object to buckets in both active Regions before the CAS. The register is a trait with two implementations, so the commit protocol is identical in both classes.
- The tenant is an enterprise with its own hostname, which matches the GHES instance model. Users, tokens, and repository names are scoped to the tenant, so no request ever needs a cross-cell lookup.
- Aurora DSQL in each cell holds metadata. git never reads it for durability or ordering, and reads it for authorization only through the required-checks attestation. The edge authenticates from an identity snapshot pushed to S3, so git keeps working through a DSQL outage.
- Pull request fields that come from refs (head, base, mergeability, merged) are projections of log records. They can lag but never contradict the refs.

Each binding fact in TASK.md is answered here:

| Fact | Answer | Where |
| --- | --- | --- |
| 1. Region loss | Region-safe cells: push and metadata cross-Region RPO 0. Zonal cells: push RPO is shipper lag (p99 target 10 s), metadata RPO is export lag (target 5 min). Managed defaults to Region-safe. | Repository storage, Availability, Deployment modes |
| 2. Object storage as truth | Adopted as the core. Ack after the register CAS, with objects already in S3 Standard. | Repository storage |
| 3. No Rust git server | Upstream git does receive-pack, upload-pack, index-pack, merge-tree, and repack. Rust owns framing, auth, the log, and policy. | Repository storage, Service and crate map |
| 4. Toolkit GHES gate | Cache v1 and v2 served. Artifact v4 via a signed patch for tag refs only. SHA pins run exact bytes and fail as on GHES. | Actions |
| 5. Runner protocol | `use_v2_flow: true`, broker plus run service, no legacy distributedtask. | Actions |
| 6. GHES host shape | One hostname on 443, GHES paths and headers, version 3.22.0, first-party OAuth ids seeded, GraphQL at T0. | Wire compatibility |
| 7. Tokens and ids | Stateless `ghs_` JWT installation tokens, MessagePack node ids with repo id, i64 ids below 2^53. | Identity and tokens |
| 8. BYOC gaps, upgrade downtime | Git nodes are caches, so the new rollout stage is drain and replace. Expand and contract migrations. Zero-downtime upgrades in every mode. | Change safety, Deployment modes |
| 9. Control plane outages | Static capacity at 66 percent per AZ, pushed credentials and maps, no launch or control-plane call in any recovery path inside a Region. | Availability |

### Tenancy and cell placement

A cell is a complete copy of the stack in one Region (or one Region pair for Region-safe), across three AZs: an edge shard set, gitd nodes, api and web tasks, relay and actions tasks, one DSQL cluster, one set of S3 buckets, and in Region-safe cells one MRSC table. Cells share nothing (research/uptime.md#8). The partition key is the tenant, which is selected by the Host header. Large tenants get dedicated cells. A self-hosted or BYOC install is exactly one cell with one tenant.

```rust
/// Below 2^53 by construction; every numeric id in the system is a DbId.
pub struct DbId(u64);
pub struct TenantId(DbId);
pub struct CellId(u32);
pub struct Hostname(String); // validated: lowercase, no port, never *.github.com or *.ghe.com

pub enum DurabilityClass {
    /// Register is an S3 object. Log shipped asynchronously to `peer`.
    Zonal { home: Region, peer: Region },
    /// Register is an MRSC item. Objects written to both `active` buckets before the CAS.
    RegionSafe { active: [Region; 2], witness: Region, on_peer_loss: PeerLossPolicy },
}
pub enum PeerLossPolicy { AckInSurvivor, RefuseWrites }

/// Signed, immutable once published. A cell never changes durability class.
pub struct CellSpec {
    pub id: CellId,
    pub durability: DurabilityClass,
    pub azs: [AzId; 3],
    pub capacity: CellCapacity,      // load-tested limits; admission control reads these
    pub buckets: CellBuckets,        // log, lfs, actions, control; one per active Region
    pub dsql: DsqlEndpoints,         // one or two Regional endpoints
}

pub struct TenantPlacement {
    pub tenant: TenantId,
    pub hostnames: Vec<Hostname>,
    pub cell: CellId,
    pub edge_shard: EdgeShardId,
    pub git_shard: ShuffleShard,     // e.g. 4 of N gitd nodes per AZ
    pub state: PlacementState,
    pub epoch: PlacementEpoch,       // bumps on every change; the router rejects older epochs
}
pub enum PlacementState { Active, Migrating { to: CellId, phase: MigrationPhase }, Forwarded { to: CellId } }
pub enum MigrationPhase { CopyObjects, SealRegisters, FlipRouter, Forget }

/// What the edge holds in memory. Loaded from S3; last-known-good kept on any failure.
pub struct RouterMap { pub version: MapVersion, pub routes: HashMap<Hostname, Route>, pub overrides: Vec<Override> }
pub struct Route { pub tenant: TenantId, pub cell: CellId, pub epoch: PlacementEpoch }

pub trait RouterMapSource {
    /// Conditional GET on the published map. Never returns a map that fails validation.
    async fn refresh(&self, current: &RouterMap) -> Result<Option<RouterMap>, MapError>;
}
pub fn route(map: &RouterMap, host: &Hostname) -> Option<&Route> { todo!() }

/// Rendezvous hashing over shard members, AZ-diverse: element 0 is the preferred
/// primary, element 1 the warm secondary in a different AZ.
pub fn placement_for(repo: RepoId, shard: &ShardMembers, healthy: &HealthView) -> [NodeId; 2] { todo!() }
```

Invariants. A hostname maps to exactly one active route, and the map publisher enforces it with an `If-Match` publish over a monotonic `MapVersion` (research/uptime.md implication 11). Each tenant hostname's DNS record points at its cell's edge NLBs and is written once at tenant creation, so no shared router sits in front of cells and no failover edits DNS. Durability class lives only in `CellSpec`, so a repository cannot be half Region-safe. Cell migration is clone, seal, flip, forget (research/uptime.md#10): objects are copied create-only, each register is sealed with a `Seal { forwarded_to }` record, the tenant's DNS record and router entry flip, and the old cell's edge forwards requests until DNS caches expire. A planned migration is the only operation that edits DNS. Because git truth is a set of immutable objects plus one register, migration has no catch-up phase beyond copying the log tail.

Access patterns. Every request does one `route()` lookup in memory. Every git request does one `placement_for()` computation in memory. Neither touches a network store.

### Repository storage and ref transactions

The layout follows walgit (research/git-storage.md#31) with one change: the register holds a small pointer, not the full ref table, so its size does not grow with ref count. Objects live under `repos/<hash-prefix>/<repo_id>/`, which spreads hot repositories across S3 partitions (research/git-storage.md#54).

```rust
pub struct RepoId(DbId);
pub struct Seq(u64);                       // register sequence, +1 per CAS
pub struct Digest([u8; 32]);               // SHA-256 of the stored object bytes
pub enum ObjectFormat { Sha1, Sha256 }
pub struct Oid { format: ObjectFormat, bytes: [u8; 32], len: u8 }
pub struct RefName(String);                // validated with git check-ref-format rules

/// `None` means absent. A create has old = None; a delete has new = None.
pub struct RefUpdate { pub name: RefName, pub old: Option<Oid>, pub new: Option<Oid> }

pub struct PackRef { pub pack: Digest, pub idx: Digest, pub bytes: u64 }
pub struct RecordId(Digest);

/// Immutable. Stored at `rec/<digest>` with If-None-Match. Chained by `prev`.
pub struct LogRecord { pub repo: RepoId, pub seq: Seq, pub prev: RecordId, pub entries: Vec<Entry>, pub node: NodeId, pub at: UnixMillis }

pub enum Entry {
    Push { updates: Vec<RefUpdate>, packs: Vec<PackRef>, actor: ActorRef, push_options: Vec<String> },
    Merge { pr: PrNumber, update: RefUpdate, packs: Vec<PackRef>, auth: MergeAuthorization },
    /// refs/pull/N/head and refs/pull/N/merge, coalesced, low-priority lane.
    BotRefs { updates: Vec<RefUpdate>, packs: Vec<PackRef>, notes: Vec<PrRefNote> },
    Policy { policy: RefPolicy },
    Checkpoint { id: CheckpointId },
    Seal { forwarded_to: CellId },
}

/// Written by the primary's compactor: packs from geometric repack with MIDX and bitmaps,
/// plus the full ref table as one reftable blob and the policy in force.
pub struct Checkpoint { pub id: CheckpointId, pub seq: Seq, pub packs: Vec<PackRef>, pub midx: Digest, pub bitmap: Digest, pub reftable: Digest, pub policy: RefPolicy }

/// The commit point. Small enough for one S3 object or one DynamoDB item.
pub struct RegisterValue {
    pub format: u16,
    pub seq: Seq,
    pub head: RecordId,
    pub recent: ArrayVec<RecordId, 64>,    // lets a reader within 64 records fetch in parallel
    pub checkpoint: (CheckpointId, Seq),
    pub policy_seq: Seq,
    pub degraded_since: Option<Seq>,       // Region-safe only: acks taken in one Region
    pub sealed: Option<CellId>,
}
pub struct RegisterTag(String);            // S3 ETag or DynamoDB version attribute

pub trait RefRegister: Send + Sync {
    async fn read(&self, repo: RepoId, known: Option<&RegisterTag>) -> Result<RegisterRead, StoreError>;
    /// The only write. No unconditional variant exists.
    async fn compare_and_swap(&self, repo: RepoId, expected: &RegisterTag, next: &RegisterValue) -> Result<CasOutcome, StoreError>;
    async fn create(&self, repo: RepoId, first: &RegisterValue) -> Result<CasOutcome, StoreError>;
}
pub enum RegisterRead { Unchanged, Changed(RegisterValue, RegisterTag) }
pub enum CasOutcome { Committed(RegisterTag), Lost { current: RegisterValue, tag: RegisterTag } }
pub struct S3Register { /* bucket, key layout; bucket policy requires s3:if-match */ }
pub struct MrscRegister { /* table name, Regional endpoint */ }

/// Writes to every bucket the cell's durability class requires and returns only when all are durable.
pub trait ImmutableStore: Send + Sync {
    async fn put(&self, key: &ObjectKey, body: Bytes) -> Result<(), StoreError>; // If-None-Match: *
    async fn get(&self, key: &ObjectKey) -> Result<Bytes, StoreError>;           // local Region only
}

pub struct RefTransaction { pub repo: RepoId, pub kind: TxnKind, pub updates: Vec<RefUpdate>, pub staged: Vec<StagedPack>, pub grant: VerifiedGrant, pub atomic: bool }
pub enum TxnKind { Push, Merge(MergeAuthorization), BotRefs(Vec<PrRefNote>), Policy(RefPolicy) }

pub struct Committed { pub seq: Seq, pub record: RecordId }
pub enum CommitError {
    Stale { name: RefName, current: Option<Oid> },   // maps to "fetch first"
    Denied(Denial),                                   // maps to GH006 text
    PolicyMoved { evaluated: Seq, current: Seq },
    Sealed(CellId),
    Unavailable(StoreError),
}

/// One per repository per node. Group-commits concurrent transactions into one CAS.
pub struct Committer { /* register, store, policy cache, local repo handle */ }
impl Committer {
    pub async fn commit(&self, txn: RefTransaction) -> Result<Committed, CommitError> {
        // 1. upload staged packs and the record body in parallel (ImmutableStore::put)
        // 2. evaluate policy at the current policy_seq; reject denied updates
        // 3. CAS; on Lost: revalidate each update's old value against `current`, rebase, retry (bounded)
        // 4. user Push entries go before BotRefs in a batch; BotRefs coalesce last-writer-wins per ref
        todo!()
    }
}

// Policy: owned by the log, evaluated as a pure function on gitd.
pub struct RefPolicy { pub seq: Seq, pub rule_sets: Vec<RuleSet> }
pub struct RuleSet { pub id: DbId, pub origin: RuleOrigin, pub target: RefPattern, pub rules: Vec<Rule>, pub bypass: Vec<Principal> }
pub enum RuleOrigin { BranchProtection, RepoRuleset, OrgRuleset { org: DbId, version: u64 } }
pub enum Rule {
    NoForcePush, NoDeletion, Lock, LinearHistory, SignedCommits,
    RequirePullRequest { approvals: u8, code_owners: bool, dismiss_stale: bool, resolve_threads: bool },
    RequiredChecks { contexts: Vec<CheckContext>, strict: bool },
    RestrictPushers(Vec<Principal>),
    PreReceiveHook { script_repo: RepoId, path: String, env: Digest },
}
pub fn evaluate(policy: &RefPolicy, grant: &VerifiedGrant, update: &RefUpdate, facts: &ObjectFacts,
                evidence: Evidence<'_>) -> Decision { todo!() }
pub enum Evidence<'a> { None, Checks(&'a ChecksAttestation), Merge(&'a MergeAuthorization) }
```

The acknowledgment point is the `Committed` return of `compare_and_swap`. At that point the packs, the record, and the register live in S3 Standard, which stores data in at least three AZs (research/git-storage.md#54), or in DynamoDB plus S3 in two Regions for Region-safe cells (research/uptime.md#29). The hook then lets git finalize local refs. If the node dies between the CAS and the local finalize, nothing is lost: the next materialization replays the record.

Invariants the types encode. `RefRegister` has no blind write, and the bucket policy enforces `s3:if-match` and `s3:if-none-match` on register and record keys, so even a buggy release cannot overwrite (research/uptime.md#31). A record is referenced by a register only after `ImmutableStore::put` returned for every object it names, because `commit` awaits the puts before the CAS. Ref updates are linearizable per repository through the register CAS. A policy change and a push are totally ordered because both are entries in the same chain, and `PolicyMoved` stops a merge evaluated against an older rule set. Garbage collection deletes a pack only if no checkpoint at or after the current one and no record after it names the pack, and only after a seven-day grace window, so the active generation is never deleted (research/uptime.md implication 11).

Branch protection without the metadata store. The rules live in the log as `Policy` entries, so the API writes them by committing a transaction through gitd, and gitd reads them from its materialized repository. The actor's identity, teams, and signing keys come from the `VerifiedGrant`, which the edge builds from the identity snapshot. Only two rules need metadata facts. `RequiredChecks` on a direct push asks `api` for a `ChecksAttestation`, and fails closed if DSQL is down, which blocks only pushes to branches whose own rule demands DSQL facts. `RequirePullRequest` is satisfied only by a `Merge` entry carrying a `MergeAuthorization` from the pull request module, which read reviews from DSQL. GHES pre-receive hook scripts are themselves files in a Tributary repository and run in a sandbox on gitd with a time limit matching GHES [unverified: GHES default timeout], reading only the pushed objects. Organization rulesets are compiled into per-repository `Policy` entries by a fan-out job through the outbox. Each repository sees the change atomically relative to its pushes, and propagation across an organization takes seconds, with the API reporting progress.

Access patterns. A fetch costs one conditional register read (Continuity reports 304s under 10 ms, research/git-storage.md#29) and zero S3 object reads when the cache is current. A push costs two parallel object writes and one CAS. A cache that is k records behind fetches up to k records in parallel using `recent`; beyond 64 it loads the newest checkpoint plus the tail. Only the primary compacts, under a 30-second TTL lease held as a CAS object, and replicas download the compacted packs (research/git-storage.md#29). Idle repositories are evicted from NVMe and rematerialized on demand. The two nodes from `placement_for` keep every active repository warm in two AZs.

Cross-Region RPO zero. In a Region-safe cell, `ImmutableStore::put` writes to the buckets in both active Regions and returns when both are durable. `MrscRegister::compare_and_swap` is a conditional `PutItem` that replicates synchronously to at least one other Region (research/uptime.md#29). Any gitd node in either Region can accept a push, and the MRSC condition serializes them, so the cell is active-active for git with no promotion step. When one active Region is impaired, an ARC routing control flips the cell's peer-health flag (research/uptime.md#22). With the default `AckInSurvivor` policy, the survivor acknowledges pushes with objects in its own three AZs and marks `degraded_since`. A repairer copies those objects back when the peer returns. Tenants who choose `RefuseWrites` keep RPO 0 strictly and lose write availability during the impairment.

The cost of RPO 0, all [inference] pending the Milestone 1 benchmark: each push waits for one extra inter-Region object write and one MRSC commit, which adds roughly one to two inter-Region round trips (about 10 to 70 ms for same-continent pairs); S3 storage for git objects doubles; every pushed byte pays inter-Region transfer; DSQL becomes a multi-Region cluster; and each Region must carry the whole cell's load, which roughly doubles compute. My estimate is that a Region-safe cell costs 1.8 to 2.2 times a Zonal cell [guess].

### Identity and tokens

Identity is per tenant. The owner of every identity fact is DSQL. The edge reads a derived identity snapshot that an exporter in `api` writes to S3 every 30 seconds (full image hourly, deltas between). This is the "push data proactively" pattern (research/uptime.md#6). A DSQL outage freezes identity changes but does not fail authentication for any token seen in the last snapshot.

```rust
pub enum TokenKind { ClassicPat, FineGrainedPat, OAuth, UserToServer, Refresh, Installation }

/// Parsed at the edge with no I/O. CRC32 and prefix checked offline (research/wire-census.md#29).
pub enum PresentedToken {
    Random { kind: TokenKind, entropy: [u8; 30], crc: u32 },       // ghp_, gho_, ghu_, ghr_, github_pat_
    InstallationJwt { app: DbId, jws: CompactJws },                 // ghs_<app>_<jwt>, about 520 chars
}
pub fn parse_token(raw: &str) -> Result<PresentedToken, TokenError> { todo!() }

pub enum Principal { User(DbId), Team(DbId), App { app: DbId, installation: DbId }, DeployKey(DbId), ActionsJob { run: DbId, job: DbId } }

/// Built by the edge, carried over mutual TLS to gitd, api, and actions. Nothing downstream re-authenticates.
pub struct VerifiedGrant {
    pub tenant: TenantId,
    pub actor: Principal,
    pub scopes: Scopes,                         // echoed in X-OAuth-Scopes
    pub repo_permission: Option<(RepoId, Permission)>,
    pub teams: SmallVec<[DbId; 16]>,
    pub signing_keys: SmallVec<[KeyFingerprint; 4]>,
    pub snapshot_version: SnapshotVersion,
    pub expires_at: UnixMillis,
}

pub struct IdentitySnapshot {
    pub tenant: TenantId,
    pub version: SnapshotVersion,
    pub tokens: HashMap<TokenHash, TokenGrant>,     // SHA-256 of the token, never the token
    pub ssh_keys: HashMap<KeyFingerprint, Principal>,
    pub users: HashMap<DbId, UserEntry>,
    pub teams: HashMap<DbId, TeamEntry>,
    pub repos: RepoDirectory,                       // names, aliases, visibility, grants
    pub revoked_jti: HashSet<Jti>,
}
pub struct RepoDirectory { pub by_name: HashMap<(OwnerLogin, RepoName), RepoEntry>, pub aliases: HashMap<(OwnerLogin, RepoName), RepoId> }

pub trait Authenticator {
    fn authenticate(&self, host: &Hostname, cred: Credential) -> Result<VerifiedGrant, AuthError>;
}

pub enum FirstPartyApp { GhCli, Gcm, VsCode, DesktopProd, DesktopDev }
pub struct OAuthClient { pub client_id: ClientId, pub kind: OAuthClientKind, pub callbacks: Vec<Url>, pub device_flow: bool, pub secret: SecretPolicy }
pub enum OAuthClientKind { FirstParty(FirstPartyApp), Registered { owner: Principal } }
pub enum SecretPolicy { Required(SecretHash), PublicClient /* embedded or absent secret ignored */ }

/// Installation and GITHUB_TOKEN signing: RS256 key decrypted once at boot with KMS, held in memory.
pub struct InstallationTokenClaims { pub iss: DbId, pub installation: DbId, pub repos: Vec<RepoId>, pub permissions: AppPermissions, pub exp: UnixSeconds, pub jti: Jti }

pub struct NodeId(String);
pub enum NodePayload { Top { kind: NodeKind, id: DbId }, RepoScoped { kind: NodeKind, repo: RepoId, id: DbId } }
pub fn encode_node_id(p: &NodePayload) -> NodeId { todo!() }          // PREFIX_ + base64url(msgpack([0, ...]))
pub fn decode_node_id(s: &str) -> Result<NodePayload, NodeIdError> { todo!() } // also accepts legacy base64 "NN:TypeID"

/// Ids come from blocks leased ahead by the install's allocator; a cell holds 30 days of blocks.
pub struct IdBlock { pub start: u64, pub len: u32 }
impl DbId { pub const MAX: u64 = (1 << 53) - 1; }
```

Invariants. A `DbId` cannot hold a value at or above 2^53. The id allocator alarms at 2^52 (research/uptime.md implication 15). Tokens are stored only as hashes, and the prefix and CRC32 are checked before any lookup. Installation tokens are stateless `ghs_` JWTs signed with a cell key, so their validation needs no store at all (research/wire-census.md#30). Revocation travels as a `jti` deny list in the snapshot, and installation tokens expire in one hour. First-party OAuth clients are seeded at install time with the hardcoded ids `178c6fc778ccc68e1d6a` (gh), `0120e057bd645470c1ed` (GCM), `01ab8ac9400c4e429b23` (VS Code), and `de0e3c7e9973e1c4dd77` and `3a723b10ac5575cc5bb9` (Desktop), with GHES-era callbacks and device flow on (research/clients.md#46, implication 5). They use `SecretPolicy::PublicClient` because their secrets ship in public binaries.

Access patterns. Authentication is a hash lookup in memory. Token creation, OAuth exchanges, and membership changes write DSQL and appear in the next snapshot delta. A snapshot miss goes to `api`, which reads DSQL and answers, so a new token works at once while DSQL is up. Every new token takes that path once, so the miss path runs all day and is not an emergency mode. During a DSQL outage only tokens created after the last snapshot fail.

### Issues and pull requests

DSQL owns issues, comments, reviews, labels, check runs, and the non-ref fields of pull requests. The log owns everything that comes from refs. The pull request head is always the value of `refs/pull/N/head` in the base repository's log, for same-repository and fork pull requests alike. The PR module writes that ref through a `BotRefs` entry when the source branch moves.

```rust
pub struct PrNumber(u32);
pub struct PullRequest {
    pub id: DbId, pub repo: RepoId, pub number: PrNumber,
    pub title: String, pub body: String, pub author: DbId, pub draft: bool,
    pub base_ref: RefName,
    pub source: PrSource,
    pub open: OpenState,                 // owned by DSQL: Open or Closed by a user
    pub projection: RefProjection,       // owned by the log, written only by the projector
}
pub enum PrSource { SameRepo { branch: RefName }, Fork { repo: RepoId, branch: RefName } }
pub enum OpenState { Open, Closed { by: DbId, at: UnixMillis } }

/// Derived from records with seq <= applied_seq. Never written from any other input.
pub struct RefProjection {
    pub applied_seq: Seq,
    pub base_oid: Oid,
    pub head_oid: Oid,
    pub mergeability: Mergeability,
    pub merged: Option<MergedFact>,
}
pub struct MergedFact { pub commit: Oid, pub by: DbId, pub at: UnixMillis, pub seq: Seq, pub method: MergeMethod }
pub enum Mergeability { Unknown, Clean { test_merge: Oid, base: Oid, head: Oid }, Conflicted { base: Oid, head: Oid } }
pub enum MergeMethod { Merge, Squash, Rebase }

/// Evidence the PR module hands to gitd. Stored in the log for audit.
pub struct MergeAuthorization {
    pub pr: PrNumber, pub head: Oid, pub base_expected: Oid,
    pub policy_seq: Seq, pub reviews_digest: Digest, pub checks_digest: Digest,
    pub issued_by: ServiceIdentity, pub issued_at: UnixMillis,
}
pub struct PrRefNote { pub pr: PrNumber, pub mergeability: Mergeability }

pub enum ReadConsistency { Projected, CaughtUp }
pub trait PullRequests {
    async fn get(&self, repo: RepoId, n: PrNumber, c: ReadConsistency) -> Result<PullRequest, PrError>;
    async fn merge(&self, grant: &VerifiedGrant, req: MergeRequest) -> Result<MergedFact, MergeError>;
}
pub struct MergeRequest { pub repo: RepoId, pub number: PrNumber, pub method: MergeMethod, pub expected_head: Option<Oid>, pub title: Option<String>, pub message: Option<String> }

/// Pure. Same input, same output; applying a record twice is a no-op because of applied_seq.
pub fn apply_record(prs: &mut PrSetForRepo, record: &LogRecord) -> Vec<OutboxDraft> { todo!() }
```

Can pull request state diverge from refs? It can lag and cannot contradict. The projector is the only writer of `RefProjection`, it applies records in `seq` order under a per-repository lease, and every write sets `applied_seq` in the same transaction. A merge is a log entry, so "merged" and "the base ref moved" are one atomic fact. Reads that matter (the merge button, `GET /pulls/{n}`, GraphQL `pullRequest.mergeable`) use `CaughtUp`, which compares `applied_seq` with the register and applies the gap inline. Repair is replay: after a bug or after a Zonal Region loss, the projector rewinds any repository whose `applied_seq` exceeds the surviving register and rebuilds projections from the newest checkpoint. Comments and reviews keep their own DSQL history.

Mergeability is computed by a worker on the repository's primary when the projector reports a base or head change. It writes `refs/pull/N/merge` and a `PrRefNote` in one `BotRefs` entry, so mergeability is also a log fact. Bot entries are coalesced and yield to user pushes in group commit, which keeps the one-chain-per-repository ceiling for people (research/git-storage.md implication 4).

Access patterns. Listing pull requests is a DSQL index scan on `(repo, open, updated_at)`. The PR page loads the row (DSQL), the diff (gitd, from `base_oid` and `head_oid`), and the review threads (DSQL). A merge is two gitd calls and two DSQL transactions.

### Actions runs and jobs

Workflow runs and jobs live in DSQL. Job state transitions are conditional updates on a version column, so a runner that renews a lease it no longer holds gets a 409 and stops.

```rust
pub struct WorkflowRun {
    pub id: DbId, pub repo: RepoId, pub workflow_path: String, pub head_sha: Oid,
    pub event: TriggerEvent, pub trigger: TriggerSource, pub attempt: u16, pub state: RunState,
}
pub enum TriggerSource { RepoLog { seq: Seq }, Api { by: DbId }, Schedule { at: UnixMillis } }
pub enum RunState { Queued, InProgress, Completed(Conclusion) }
pub enum Conclusion { Success, Failure, Cancelled, Skipped, TimedOut, ActionRequired, Neutral }

pub struct Job { pub id: DbId, pub run: DbId, pub name: String, pub labels: LabelSet, pub needs: Vec<DbId>, pub state: JobState, pub version: u64 }
pub enum JobState {
    Waiting { unmet_needs: u16 },
    Queued { since: UnixMillis },
    Assigned { runner: DbId, request: RunnerRequestId, lease: Lease },
    Running { runner: DbId, lease: Lease, started: UnixMillis },
    Cancelling { runner: DbId, lease: Lease },
    Completed { conclusion: Conclusion, finished: UnixMillis },
}
pub struct Lease { pub expires_at: UnixMillis }   // renewjob every 60 s; expiry requeues or fails per policy

pub enum JobEvent { NeedsMet, Claimed { runner: DbId, request: RunnerRequestId }, Acquired, Renewed, CancelRequested, Completed(Conclusion), LeaseExpired }
/// Pure transition table; the shell persists with `UPDATE ... WHERE id = $1 AND version = $2`.
pub fn transition(state: &JobState, ev: JobEvent, now: UnixMillis) -> Result<JobState, IllegalTransition> { todo!() }

pub struct RunnerRegistration { pub id: DbId, pub scope: RunnerScope, pub group: DbId, pub labels: LabelSet, pub ephemeral: bool, pub public_key: RsaPublicKey, pub version: RunnerVersion }

pub trait Broker {
    async fn create_session(&self, runner: &RunnerRegistration) -> Result<SessionId, BrokerError>;
    async fn next_message(&self, s: &SessionId, wait: Duration) -> Result<Option<BrokerMessage>, BrokerError>;
    async fn acknowledge(&self, s: &SessionId, m: MessageId) -> Result<(), BrokerError>;
}
pub trait RunService {
    async fn acquire_job(&self, req: RunnerRequestId) -> Result<JobMessage, AcquireError>; // 404/409/422 mean skip
    async fn renew_job(&self, plan: PlanId, job: DbId) -> Result<Lease, RenewError>;
    async fn complete_job(&self, plan: PlanId, job: DbId, result: JobResult) -> Result<(), CompleteError>;
}

pub enum ActionRef { Repo { owner: String, repo: String, path: Option<String>, git_ref: ActionGitRef }, Docker(String), Local(String) }
pub enum ActionGitRef { Sha(Oid), TagOrBranch(String) }
pub enum ServedBytes {
    Upstream { commit: Oid, tarball_sha256: Digest },
    CompatPatched { upstream_commit: Oid, upstream_sha256: Digest, patch: PatchId, served_sha256: Digest },
}
/// A SHA-pinned ref always resolves to Upstream. Patching applies only to TagOrBranch refs
/// that the signed catalog lists, and only when the org policy allows it.
pub fn resolve_action(r: &ActionRef, policy: &CompatPolicy, catalog: &PatchCatalog, mirror: &ActionMirror) -> Result<ServedBytes, ResolveError> { todo!() }
```

Invariants. Every job has one owner at a time, enforced by the version column. A job message is built only from DSQL rows and git content at `head_sha`, so a rerun with the same attempt produces the same message. `ServedBytes` records both digests whenever bytes differ from upstream, and the job log prints them.

Access patterns. The broker's claim is `UPDATE jobs SET state = 'assigned', ... WHERE shard = $1 AND state = 'queued' AND labels <@ $2 ORDER BY queued_at LIMIT 1` with OCC retry. The queue is sharded 64 ways per cell to keep DSQL write conflicts off one hot row (research/uptime.md#27). Lease renewals are single-row updates. Cache and artifact indexes are rows keyed by `(repo, scope_ref, key, version)`, and the bytes are S3 objects.

### Event outbox

There is one outbox per cell, in DSQL. Metadata changes write their outbox rows in the same transaction. Git changes enter the outbox through the projector, which turns each log record into rows with deterministic ids. Every consumer reads the outbox. Nothing reads the log for events except the projector.

```rust
/// Deterministic for git events, so re-projection inserts nothing new (ON CONFLICT DO NOTHING).
pub enum EventKey { Repo { repo: RepoId, seq: Seq, index: u16 }, Meta { id: DbId } }

pub struct OutboxEvent {
    pub key: EventKey,
    pub tenant: TenantId,
    pub shard: u16,                       // hash(tenant, repo) % 64
    pub kind: EventKind,                  // push, pull_request.closed, check_run.completed, ...
    pub payload: PayloadRef,              // inline up to 64 KiB, else an S3 object
    pub created_at: UnixMillis,
    pub state: OutboxState,
}
pub enum OutboxState { Pending, Dispatched { at: UnixMillis } }

/// Tracks git progress. The commit path writes `hint_seq` best-effort after the ack;
/// the register sweep repairs missed hints by comparing the register with projected_seq.
pub struct RepoCursor { pub repo: RepoId, pub hint_seq: Seq, pub projected_seq: Seq, pub lease: Option<WorkerLease> }

pub enum Consumer { Webhooks, ActionsTriggers, Notifications, SearchIndex, AuditLog, OrgRulesetFanout }
pub struct WorkItem { pub event: EventKey, pub consumer: Consumer, pub target: Option<DbId>, pub attempt: u16, pub not_before: UnixMillis }

pub struct Delivery {
    pub guid: Uuid,                       // X-GitHub-Delivery, stable across redeliveries
    pub hook: DbId, pub event: EventKey, pub attempts: Vec<Attempt>,
    pub retain_until: UnixMillis,         // 30 days; GitHub's redelivery window is 3 (research/wire-census.md#37)
}

pub trait Projector { async fn project(&self, repo: RepoId, upto: Seq) -> Result<Seq, ProjectError>; }
pub trait Dispatcher {
    /// Claims pending events in one shard, writes one WorkItem per matching consumer target,
    /// and marks the event dispatched, chunked to stay under 3,000 rows per transaction.
    async fn dispatch(&self, shard: u16) -> Result<usize, DispatchError>;
}
```

Invariants. An event exists if and only if the transaction or log record that caused it committed. Re-running the projector or dispatcher is idempotent because keys are deterministic and work items are keyed by `(event, consumer, target)`. Delivery is at least once with a stable GUID. Fan-out larger than 3,000 rows is chunked into idempotent steps (research/uptime.md implication 14).

Lost hints. The commit path writes `hint_seq` after the acknowledgment, so a node that dies in between leaves a committed record with no hint. A register sweep walks every repository in the cell with conditional reads on a fixed budget (1,000 reads per second, so one million repositories take about 17 minutes) and repairs any cursor whose register is ahead. The next push to the repository also repairs it. This is the only way a webhook can be late by more than seconds, and the sweep runs constantly, so it is not a mode that switches on in an emergency.

### Service and crate map

Deployable services, all per cell unless noted:

- `edge` (Rust): TLS and SSH termination, router map, GHES headers on every response, token parsing, identity snapshot, rate limits, and request forwarding. Runs on EC2 behind an NLB per edge shard.
- `gitd` (Rust plus upstream git 2.54 or later): smart HTTP and SSH git handlers, the pre-receive and reference-transaction hook server, the `Committer`, materialization, compaction, the merge worker, the pre-receive sandbox, `git archive` for codeload, the cross-Region shipper for Zonal cells, and the register sweep. Runs on I8g instances with instance-store NVMe in a fixed-size Auto Scaling group per AZ.
- `api` (Rust): REST `/api/v3`, GraphQL `/api/graphql`, OAuth web and device flow, uploads, LFS batch and locks, issues, the pull request module, checks, the identity exporter, and the internal attestation endpoint. Runs on ECS on EC2 capacity.
- `web` (TypeScript, Node 22, React 19 SSR): the browser application. Not Rust, because React SSR needs a JavaScript runtime.
- `relay` (Rust): projector, dispatcher, webhook delivery, notifications, search indexing, and org ruleset fan-out.
- `actions` (Rust): runner registration, broker, run service, launch resolver, results service, cache v1 and v2, artifacts legacy and v4, OIDC issuer, scale-set APIs, and the workflow engine.
- `runnerd` (Rust): host agent for the hosted-runner fleet. It boots Firecracker microVMs on pre-provisioned metal hosts in a separate account.
- `cellctl` (Rust): in-install operator. Publishes the router map, runs cell migrations, leases id blocks, flips durability health flags, and implements the `roll:git-fleet` deploy stage.
- `tributary-deploy` (TypeScript, reused): the vendor's deploy tool with a Tributary product profile. Not rewritten.

Rust crates:

- `trib-ids`: `DbId`, id blocks, node id codec.
- `trib-wire`: GHES conventions: headers, error bodies, pagination links, media types, API versions, ETags, URL builder.
- `trib-wal`: records, checkpoints, `RefRegister` with `S3Register` and `MrscRegister`, `ImmutableStore`, `Committer`, compaction lease, GC.
- `trib-policy`: `RefPolicy`, `evaluate`, GH006 messages. Pure, no I/O.
- `trib-git`: upstream git process supervision, hook protocol, pkt-line parsing, merge worker, materializer.
- `trib-auth`: token parsing, snapshot, `Authenticator`, OAuth clients, installation token signing.
- `trib-meta`: DSQL access, OCC retry on SQLSTATE 40001, row-limit chunking, expand and contract migration runner.
- `trib-pr`: pull request domain, `apply_record`, merge orchestration.
- `trib-rest`: route table generated from the pinned `ghes-3.22.json`.
- `trib-graphql`: schema generated from the pinned GHES 3.22 SDL, resolvers.
- `trib-outbox`: events, cursors, dispatcher, webhook delivery.
- `trib-actions-wf`: Rust port of `GitHub.Actions.WorkflowParser`, expressions, the job message encoder.
- `trib-runner-proto`: broker, run service, launch, and results wire types.
- `trib-blobshim`: Azure block blob dialect over S3 multipart.
- `trib-cell`: `CellSpec`, placement, router map, rendezvous and shuffle sharding.

Each crate exposes its domain types and keeps storage and wire types private, so for example `trib-wal` exports `Committer` and `RefRegister` but not S3 key layouts.

AWS services on the request path: NLB, EC2 instances and their instance store, S3, Aurora DSQL, DynamoDB (MRSC, Region-safe cells only), and Route 53 DNS resolution. KMS decrypt runs at process start and key rotation, not per request. Regional STS issues instance-role credentials, refreshed hours ahead. Nothing on the request path calls IAM, Route 53 record APIs, EC2 launch, CloudFront, Lambda, or API Gateway (research/uptime.md implication 1).

## Wire compatibility

The compatibility target is GHES 3.22. REST shape is validated against the pinned `ghes-3.22.json` and GraphQL against the GHES 3.22 schema (research/wire-census.md#1, #6). "Schemathesis" below means a contract run from that pinned OpenAPI file replayed through `@octokit/rest`, as in research/clients.md.

| Surface | Tier | Oracle | Approach |
| --- | --- | --- | --- |
| `git-http` | T0 | `GCM_TRACE=1 git clone https://<host>/o/r.git` then `git push`, with git 2.39 and 2.54 | edge to gitd; upstream upload-pack; upstream receive-pack with WAL commit hook; with and without `.git` |
| `git-ssh` | T0 | `ssh -T git@<host>` prints the GitHub banner and exits 1; `git clone git@<host>:o/r.git` | russh in edge, key lookup in snapshot, same gitd backend |
| `git-protocol-v2` | T0 | `GIT_TRACE_PACKET=1 git ls-remote` diffed against the captured github.com advertisement | upstream git v2 with `ls-refs=unborn`, `fetch=shallow wait-for-done filter`; `bundle-uri` later |
| `lfs` | T0 | `git lfs track '*.bin' && git push && git lfs fetch --all && git lfs locks` over HTTPS | batch API in api; objects content-addressed in the cell LFS bucket, durability class applied; locks in DSQL; SSH transfer T1 |
| `rest:actions` | T0 | ARC `helm install` scale set from zero; `gh run list`, `gh run view --log`, `gh run rerun` | runner registration, runners, runs, jobs, artifacts zip redirect, secrets with sealed boxes |
| `rest:activity` | T1 | gh acceptance `status` tests; Schemathesis on the activity tag | notifications and events read from DSQL projections |
| `rest:agent-automations` | Out | none | dotcom-only, absent from the GHES 3.22 contract |
| `rest:agent-tasks` | Out | none | dotcom-only |
| `rest:agents` | Out | none | dotcom-only |
| `rest:announcement-banners` | T2 | Schemathesis on the tag; banner shows in web | DSQL row per tenant |
| `rest:apps` | T0 | Renovate with app credentials gets a `ghs_` token; ARC with a GitHub App | app registry in DSQL; stateless `ghs_` JWT installation tokens |
| `rest:billing` | T3 | Schemathesis on the single operation | usage export from metering |
| `rest:branches` | T0 | terraform-provider-github `TestAccGithubBranchProtection` acceptance | protection read and written as `Policy` entries through gitd |
| `rest:campaigns` | Out | none | dotcom-only |
| `rest:checks` | T0 | `gh pr checks` after an Actions run; Jenkins posts and reads checks | check runs and suites in DSQL; attestation for required checks |
| `rest:classroom` | Out | none | dotcom-only |
| `rest:code-quality` | Out | none | dotcom-only |
| `rest:code-scanning` | T2 | `github/codeql-action/upload-sarif` on a pinned runner | SARIF stored in S3, alerts in DSQL |
| `rest:code-security` | T2 | Schemathesis on the tag | configuration rows in DSQL |
| `rest:codes-of-conduct` | T3 | Schemathesis on the tag | static catalog |
| `rest:codespaces` | Out | none | dotcom-only |
| `rest:collaborators` | T0 | terraform `TestAccGithubRepositoryCollaborator`; `gh api repos/o/r/collaborators` | DSQL grants, exported to the snapshot |
| `rest:commits` | T0 | Jenkins Branch Source posts a commit status; go-github integration commit tests via `WithEnterpriseURLs` | git reads from gitd; statuses in DSQL |
| `rest:copilot` | Out | none | dotcom-only |
| `rest:copilot-spaces` | Out | none | dotcom-only |
| `rest:credentials` | Out | none | dotcom-only |
| `rest:dependabot` | T2 | Schemathesis on the tag; `dependabot update -f job.yml` | alerts need an advisory feed; secrets share the Actions sealed-box code |
| `rest:dependency-graph` | T2 | Schemathesis on the tag | SBOM generated from manifests by a worker |
| `rest:deploy-keys` | T0 | terraform `github_repository_deploy_key` then `git clone` over SSH with that key | DSQL rows exported to the snapshot as `Principal::DeployKey` |
| `rest:deployments` | T1 | workflow with `environment:` and protection; `gh api` deployments list | DSQL; environments gate jobs |
| `rest:emojis` | T3 | `gh api emojis` | static catalog |
| `rest:enterprise-admin` | T1 | Schemathesis on the enterprise-admin tag; site admin scripts with go-github | DSQL plus cellctl; Manage API on an admin port at T2 |
| `rest:enterprise-teams` | T2 | Schemathesis on the tag | DSQL |
| `rest:gists` | T2 | `gh gist create`, `gh gist view`, `git clone` of the gist | each gist is a WAL repository |
| `rest:git` | T0 | Renovate `platformCommit=enabled` creates commits; go-github integration git tests | writes become `RefTransaction`s through gitd |
| `rest:gitignore` | T3 | `gh repo create --gitignore Go` | static catalog |
| `rest:interactions` | Out | none | dotcom-only |
| `rest:issues` | T0 | cli/cli acceptance issue tests with `GH_ACCEPTANCE_HOST` | DSQL, outbox events |
| `rest:licenses` | T2 | `gh repo view` license field; Schemathesis | license detection worker on push |
| `rest:markdown` | T1 | VS Code Pull Requests comment preview; `gh api markdown` | Rust renderer matching GFM |
| `rest:meta` | T0 | GCM auto-detect, Desktop `HEAD /api/v3/meta`, `curl /api/v3/meta` shows `installed_version` 3.22.0 | static per install from `trib-wire` |
| `rest:metrics` | T2 | Schemathesis on the tag | counters from the outbox |
| `rest:migrations` | T2 | org export then import into another install | export from log and DSQL into a GHES-style archive |
| `rest:oauth-authorizations` | T3 | Schemathesis on the tag | legacy basic-auth token API; no client in research/clients.md calls it |
| `rest:orgs` | T0 | terraform `TestAccGithubOrganization` and membership tests; ARC org-scope registration | DSQL |
| `rest:packages` | T2 | `gh api /orgs/o/packages` after a container push | package metadata in DSQL |
| `rest:pages` | T2 | Schemathesis on the tag | Pages build as an Actions workflow, static site from S3 |
| `rest:private-registries` | T3 | Schemathesis on the tag | DSQL rows, sealed-box secrets |
| `rest:projects` | T2 | `gh project list` against the GHES schema | DSQL |
| `rest:pulls` | T0 | cli/cli acceptance pr tests; VS Code Pull Requests e2e list, checkout, review, merge | DSQL plus `RefProjection`; merges through gitd |
| `rest:rate-limit` | T0 | VS Code Pull Requests `HEAD /api/v3/rate_limit` detection | per-edge approximate counters |
| `rest:reactions` | T1 | `gh api` reaction create; VS Code Pull Requests reactions | DSQL |
| `rest:releases` | T1 | `gh release create v1 --notes x ./a.tgz` then `gh release download` | DSQL rows, assets in S3, tags through gitd |
| `rest:repos` | T0 | terraform `TestAccGithubRepository`; `gh repo create`, `gh repo view` | DSQL directory plus register create; renames keep aliases |
| `rest:search` | T1 | `gh search issues`, `gh search repos` | issue and PR qualifiers from DSQL at T0 through GraphQL; full text and code search in a per-cell OpenSearch, fail-soft |
| `rest:secret-scanning` | T2 | Schemathesis on the tag | scanner on push records, alerts in DSQL |
| `rest:security-advisories` | T3 | Schemathesis on the tag | DSQL |
| `rest:teams` | T0 | terraform `TestAccGithubTeam` | DSQL, exported to the snapshot |
| `rest:users` | T0 | `gh api user`; GCM PAT validation; Jenkins credential check | DSQL plus snapshot |
| `graphql` | T0 | static validation of every VS Code Pull Requests `.gql`, JetBrains `.graphql`, Renovate, gh, and terraform `githubv4` operation against our introspection, then cli/cli acceptance | schema generated from GHES 3.22 SDL; GitHub `extensions.code` values; cost limits |
| `webhooks` | T0 | Argo CD refreshes on a push webhook; Jenkins multibranch discovers a PR from webhooks | outbox dispatcher, GHES headers, HMAC-SHA256, stable GUID, 30-day delivery log |
| `oauth-web-flow` | T0 | `gh auth login --hostname <host> --web`; Desktop enterprise sign-in in Playwright for Electron | seeded first-party clients, GHES callbacks |
| `oauth-device-flow` | T0 | `gh auth login --hostname <host>`; `vscode.authentication.getSession('github-enterprise', ['repo'])` | `/login/device/code` and polling with `slow_down` |
| `github-apps` | T0 | ARC with app auth; Argo CD `--github-app-enterprise-base-url`; terraform `app_auth` | DSQL app registry; JWT app auth; stateless installation tokens |
| `tokens` | T0 | Renovate token checks for `ghp_`, `github_pat_`, `ghs_`; gh reads `X-OAuth-Scopes` | GitHub prefixes, CRC32, hashed storage, header sizes for 520-char tokens |
| `actions-runner-protocol` | T0 | actions/runner v2.338.1 `config.sh --url https://<host>/o --token T` runs the workflow corpus; ARC scale set | v2 flow: admin registration, broker, run service, launch, results |
| `actions-workflow-syntax` | T0 | differential corpus against `@actions/workflow-parser`; the corpus runs green on the pinned runner | Rust port of the C# WorkflowParser using `workflow-v1.0.json` as data |
| `actions-cache` | T0 | `actions/cache@v4`, `@v5`, `@v6` save then restore across two runs | cache v1 at `_apis/artifactcache`; v2 twirp with block blob shim; S3 storage |
| `actions-artifacts` | T0 | `upload-artifact@v3.2.2` round trip; `upload-artifact@v4` by tag round trip through the compat patch | legacy pipelines API plus twirp ArtifactService with block blob shim |
| `actions-oidc` | T0 | `aws-actions/configure-aws-credentials` with `role-to-assume` in a test account | issuer `https://<host>/_services/token`, RS256, legacy `sub` format |
| `actions-logs-results` | T0 | `gh run view --log`; live log in the web UI | results twirp, `comp=appendblock` to S3, live feed WebSocket |
| `packages-container` | T1 | `docker login <host> && docker push && docker pull`; `oras push` | OCI distribution in api, blobs in S3 |
| `packages-npm` | T2 | `npm publish` and `npm install` with the registry at `/_registry/npm/` | metadata in DSQL, tarballs in S3 |
| `packages-maven` | T2 | `mvn deploy` and dependency resolve | same storage as npm |
| `packages-nuget` | T3 | `dotnet nuget push` | same storage |
| `packages-rubygems` | T3 | `gem push` | same storage |
| `pages` | T2 | `actions/deploy-pages` then `curl` the site | static files from S3 through edge on a separate origin |
| `codeload-archives` | T0 | `curl -L https://<host>/api/v3/repos/o/r/tarball/main`; the runner downloads a `uses:` tarball | gitd `git archive`, cached in S3 by tree id |
| `raw-content` | T1 | `curl https://<host>/o/r/raw/main/README.md` | gitd blob read, ETag by blob id |
| `uploads-release-assets` | T1 | `gh release upload v1 ./b.tgz` | `/api/uploads` to S3 multipart |
| `meta-endpoint` | T0 | GCM, Desktop, VS Code Pull Requests, JetBrains, and Renovate detection runs all classify the host as GHES 3.22 | `trib-wire` serves `/meta`, `/status`, `/api/v3/status`, and `/api/v3/` |

Cross-cutting REST conventions. `trib-wire` implements each convention once and every handler uses it (research/wire-census.md#11-25). It honors `X-GitHub-Api-Version` with default `2022-11-28` and also `2026-03-10`, returns 400 for unknown versions, and echoes `x-github-api-version-selected`. Media types `diff`, `patch`, `sha`, `raw`, `text`, `html`, and `full` are parsed into an enum before handlers run. Pagination emits `link` headers with `/repositories/{id}/...` URLs, defaults to 30, and clamps `per_page` to 100 silently. Every GET carries a weak ETag computed from the row version or the register seq, and a 304 does not count against rate limits. Errors use the `basic-error` and `validation-error` shapes with string `status`. Private resources the caller cannot see return 404, and so do unsupported methods and trailing slashes. Requests without `User-Agent` are rejected. Handlers have a 10-second deadline. Renamed repositories return 301 to `/repositories/{id}` on the API, while git serves the old name with a 200, using the alias table that is also in the snapshot. Hypermedia `*_url` fields come from one URL builder per hostname. Timestamps are UTC ISO 8601 and the `Time-Zone` header is honored on writes. GraphQL connections require `first` or `last` from 1 to 100 and enforce the 500,000-node cap.

Hostname heuristics. The install presents one bare hostname on port 443, because gh rejects ports and the runner drops them (research/clients.md#4, research/actions-protocol.md#4). REST is at `/api/v3`, GraphQL at `/api/graphql`, uploads at `/api/uploads`, OAuth at `/login/oauth/*` and `/login/device/code` (research/wire-census.md#32). Every response, including git smart HTTP, `HEAD` on repository URLs, `/status`, `/api/v3/status` (body starts with `GitHub lives!`), `/api/v3/meta`, and `/api/v3/rate_limit`, carries `X-GitHub-Request-Id` and `X-GitHub-Enterprise-Version: 3.22.0` (research/clients.md implication 2, 4). Unauthenticated 401s carry `X-GitHub-Media-Type` and a `documentation_url` under `https://docs.github.com/enterprise`. We recommend `github.<customer-domain>` so GCM and Desktop detect the host without probing, and the hostname validator rejects anything ending in `.github.com` or `.ghe.com` (research/clients.md#2, #11, #16). Subdomain isolation is optional and needs a wildcard certificate (research/wire-census.md#33). The advertised version is a contract: moving from 3.22.0 to a later version is a release event gated by the full conformance suite against that version's published schemas (research/clients.md#49).

OAuth client ids. The five first-party clients in the Identity section are seeded at install with device flow on and GHES-era callbacks: `http://localhost/` for gh and GCM, and the Desktop protocol handler. The exact Desktop callback is [unverified] (research/clients.md open question 1), so Milestone 2 records it from a real Desktop build.

Id ranges and node ids. Every numeric id is a `DbId` below 2^53 and is served as a JSON number. GraphQL exposes `databaseId: Int` where GHES does and `fullDatabaseId: BigInt` as a string (research/wire-census.md#28). New node ids use GitHub's new format: a type prefix, `_`, and base64url MessagePack of `[0, id]` or `[0, repo_id, id]` (research/wire-census.md#26-27). We copy the prefixes measured on github.com (`U_`, `CR_`, `CS_`, `WFR_`) and choose analogous prefixes for other types [inference]. Carrying `repo_id` lets `node(id:)` find the repository and its gitd shard without a lookup. The tenant comes from the hostname, so node ids carry no cell. Input accepts the legacy `NN:TypeNameID` base64 format too.

## Availability

### SLOs

Targets are monthly for Region-safe cells, the managed default. A minute is bad when the journey's error rate exceeds 5 percent, which is GitHub's own SLA definition (research/uptime.md#51). Zonal cells share the in-Region targets but count a Region outage against the budget. Error budgets assume a 30-day month of 43,200 minutes.

| Journey | Target | Error budget per month | Measurement |
| --- | --- | --- | --- |
| git fetch and clone (HTTPS and SSH) | 99.99% | 4.3 min | Synthetic `git ls-remote` and shallow clone every 10 s per AZ per cell, plus server-side 5xx and timeout ratio from edge logs |
| git push acknowledged | 99.95% | 21.6 min | Synthetic push to a canary repository every 10 s per AZ, verified by a fetch from a different node; plus `report-status` failure ratio excluding policy denials |
| Pull request merge | 99.95% | 21.6 min | Synthetic PR open and merge every minute per cell through GraphQL; merge mutation error ratio |
| REST and GraphQL API | 99.95% | 21.6 min | Edge-recorded 5xx and deadline ratio across all `/api/v3` and `/api/graphql` requests |
| Web pull request page load | 99.9% | 43.2 min | Real-user timing beacon plus synthetic SSR render of a PR page under 3 s |
| Actions job starts | 99.9% | 43.2 min | Share of jobs with first log line within 5 minutes of queueing, GitHub's Actions definition (research/uptime.md#51) |
| Webhook delivered | 99.9% | 43.2 min | Share of events whose first delivery attempt starts within 60 s of commit |

Durability objective: zero acknowledged pushes lost, ever, within a Region. Across Regions, zero in Region-safe cells and at most the shipper lag (p99 target 10 s) in Zonal cells. Region-level RTO is about 2 minutes for Region-safe cells (health-check routing, no promotion) and hours for Zonal cells, which must be rebuilt in the peer Region.

### Failure modes

Stateful components on the request path. Stateless tiers (api, web) run three AZs at no more than 66 percent of load-tested capacity per AZ and are covered by the edge rows.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| S3 log bucket | instance | One request | Per-request deadline and one retry inside a per-node retry budget |
| S3 log bucket | az | None | S3 Standard stores objects in at least three AZs (research/git-storage.md#54) |
| S3 log bucket | region | Zonal cell: all git in the cell. Region-safe: none | Region-safe writes both Regions before the CAS; peer serves. Zonal relies on the shipper copy and a rebuild |
| S3 log bucket | control-plane | None | Buckets, policies, and versioning are created with the cell; no bucket API on any path (research/uptime.md#16) |
| S3 log bucket | bad-deploy | Repositories written by canary nodes in one AZ of one cell | Committer decodes each record with the previous release's decoder before the CAS; canary AZ first; automatic abort on SLO burn |
| S3 log bucket | bad-config | One cell | Bucket policy denies delete on `rec/` and `pack/` except to the GC role; versioning with 30-day noncurrent retention; config staged per cell |
| S3 log bucket | noisy-tenant | One repository | Hashed key prefixes spread partitions; per-repository push admission; group commit (research/git-storage.md#30) |
| Ref register | instance | One request | Retry within budget; CAS is idempotent because a retry with the same expected tag either wins once or sees its own value |
| Ref register | az | None | S3 and DynamoDB replicate across AZs |
| Ref register | region | Zonal cell: writes and reads stop. Region-safe: none | MRSC keeps committing in the survivor with the witness (research/uptime.md#29); `AckInSurvivor` marks degraded records |
| Ref register | control-plane | None | MRSC table created empty with the cell; no replica changes ever needed (research/uptime.md#29) |
| Ref register | bad-deploy | Canary AZ of one cell | No blind-write API exists in `RefRegister`; bucket policy requires conditional writes; revalidation on every lost CAS |
| Ref register | bad-config | One cell | Register backend comes from the signed `CellSpec`; gitd refuses to start if its configured backend differs |
| Ref register | noisy-tenant | One repository | One CAS chain per repository; bot refs coalesced behind user pushes; per-repository admission at about 100 commits per second |
| gitd node cache | instance | In-flight requests on that node; unacked pushes fail and clients retry | Edge marks a node down after three failed requests (research/git-storage.md#3); rendezvous moves the primary; the warm secondary in another AZ already holds the repository |
| gitd node cache | az | One third of nodes | Each active repository is warm in two AZs; 50 percent headroom; edge ejects at most one AZ of nodes and fails open beyond that (research/uptime.md implication 10) |
| gitd node cache | region | Zonal: the cell. Region-safe: none | Region-safe nodes in both Regions are active and warm |
| gitd node cache | control-plane | None while capacity lasts | No launch in recovery; capacity pre-provisioned at 66 percent per AZ; Auto Scaling replaces nodes only when EC2 is healthy |
| gitd node cache | bad-deploy | Canary AZ of one cell | Caches are disposable, so rollback is restart on the old binary plus rematerialization from S3; `roll:git-fleet` stops on SLO burn |
| gitd node cache | bad-config | Canary AZ of one cell | git config and limits ship in the signed config bundle, applied one AZ at a time with a fenced version |
| gitd node cache | noisy-tenant | That tenant's shuffle shard | Tenants map to 4 of N nodes per AZ (research/uptime.md#7); per-tenant concurrency caps on upload-pack; bundle-uri offload for clone storms |
| DSQL cell cluster | instance | Transactions on the failed host retry | OCC retry on 40001 with a deadline shorter than the caller's |
| DSQL cell cluster | az | None | DSQL replicates across AZs [inference from the 99.99% single-Region SLA, research/uptime.md#26] |
| DSQL cell cluster | region | Zonal: metadata in the cell. Region-safe: none | Multi-Region cluster with witness, zero lag on commit (research/uptime.md#26); git keeps working in either case because git never reads DSQL |
| DSQL cell cluster | control-plane | None | Active-active Regional endpoints; no failover API to call |
| DSQL cell cluster | bad-deploy | One cell's metadata | Expand and contract migrations, one DDL per transaction, throttled on saturation; canary cell; git and auth unaffected |
| DSQL cell cluster | bad-config | One cell | Connection pool caps derived from `CellCapacity` and checked in CI against 10,000 connections and 100 new per second (research/uptime.md#27) |
| DSQL cell cluster | noisy-tenant | That tenant, then the cell | Per-tenant admission at the edge; 3,000-row chunking; large tenants on dedicated cells |
| Edge router and identity snapshot | instance | Connections on one host | NLB health checks; clients retry |
| Edge router and identity snapshot | az | One third of edge hosts | ARC zonal shift and weekly autoshift practice on each NLB (research/uptime.md#20-21) |
| Edge router and identity snapshot | region | Zonal: the cell. Region-safe: none | Health-checked DNS answers for both Regions, evaluated by the Route 53 data plane (research/uptime.md#15) |
| Edge router and identity snapshot | control-plane | None | No record changes for failover; maps and snapshots served from memory when S3 or the control plane is down (research/uptime.md#9) |
| Edge router and identity snapshot | bad-deploy | One edge shard | Deploy one edge shard first; tenants spread over shards |
| Edge router and identity snapshot | bad-config | One edge shard | Map and snapshot validated before swap (every host routes to a known cell, change size bounded); last-known-good retained; fenced publish |
| Edge router and identity snapshot | noisy-tenant | That tenant | Per-tenant token buckets and concurrency limits at the edge |
| Outbox relay and projector | instance | Shards leased by that worker pause for one lease period | Leases in DSQL expire in 30 s and move to another worker |
| Outbox relay and projector | az | One third of workers | Workers in three AZs; shard leases rebalance |
| Outbox relay and projector | region | Zonal: events in the cell. Region-safe: none | Workers in both Regions lease disjoint shards from the multi-Region DSQL |
| Outbox relay and projector | control-plane | None | No control-plane calls |
| Outbox relay and projector | bad-deploy | Projections in the canary cell | Projections are derived; replay from checkpoint repairs them; deterministic event keys stop duplicates |
| Outbox relay and projector | bad-config | Webhooks in one cell | Delivery timeouts and concurrency in the staged config bundle |
| Outbox relay and projector | noisy-tenant | That tenant's shard | Per-hook concurrency; slow receivers back off without blocking the shard |
| Actions broker and blob store | instance | Long polls on one task reconnect | Job leases in DSQL; runners retry the broker |
| Actions broker and blob store | az | One third of tasks | Tasks in three AZs; S3 multi-AZ |
| Actions broker and blob store | region | Zonal: Actions in the cell. Region-safe: in-flight log blocks | Job state in multi-Region DSQL; runners reconnect to the other Region |
| Actions broker and blob store | control-plane | Hosted-runner starts beyond warm capacity | microVMs boot on pre-provisioned metal hosts; no EC2 launch per job |
| Actions broker and blob store | bad-deploy | Jobs in the canary cell | Pinned runner version conformance gate; canary cell; job message encoder differential tests |
| Actions broker and blob store | bad-config | Rewritten actions in one cell | Patch catalog signed and staged; SHA-pinned actions never rewritten; org policy can disable patching |
| Actions broker and blob store | noisy-tenant | That tenant's queue shard | Per-tenant concurrent job caps; 64 queue shards; hosted-runner quota per tenant |

Drills. In managed mode we run weekly AZ evacuation of one cell, weekly gitd node kills, and monthly Region-safe Region isolation that cuts one Region's network and verifies zero lost acknowledged pushes with a linearizability checker over the canary push stream (research/uptime.md implication 20). Zonal cells run a quarterly rebuild in the peer Region from shipped data.

### Change safety

Deploys. Every change ships as a signed release through the reused registry (research/byoc-grounding.md#12-15). Inside an install, `cellctl` rolls one AZ at a time. gitd rolls through a new `roll:git-fleet` stage: drain a node (stop accepting, finish in-flight requests, release the compaction lease), replace the binary, warm it from S3, check the canary push stream, move on. There is no replica catch-up or quorum check, because no node holds truth. ECS services roll per AZ behind weighted target groups with an automatic abort on SLO burn. Managed rolls cell by cell in <vendor-fleet-app> waves, never starting wave 1 automatically (research/byoc-grounding.md#31-32).

Config and feature flags. Config is a signed bundle in the cell control bucket, versioned and published with `If-Match`, read into memory, and rolled by the same AZ-by-AZ pipeline as binaries (research/uptime.md implication 16). Feature flags are fields in that bundle, evaluated locally. There is no remote flag service. Certificates and keys alarm 30 days before expiry and a CI test fails before expiry.

Schema migrations. DSQL schema changes are expand and contract. The expand phase adds tables, columns, or indexes with one DDL per transaction (research/uptime.md#27) and is never a rollback barrier. Backfills run as idempotent chunked jobs under 3,000 rows. The contract phase ships at least one release later and is the only barrier. The release record gains a `migrationPhase` attribute so the reused barrier chain marks only contract releases (research/byoc-grounding.md#26, #34). Log format changes follow a two-release rule: release N learns to read format k+1, release N+1 starts writing it. The register carries `format`, so a node that cannot read a register refuses to serve rather than misread it.

Zero-downtime upgrades in every mode. Releases are wire and schema compatible with N-1 in both directions (research/uptime.md implication 24). Managed and BYOC roll as above. Self-hosted's supported HA profile is at least two gitd nodes and two api tasks across two AZs, so one of each can drain at a time. The single-node evaluation profile upgrades by surging a second instance on the same S3 log, which needs an EC2 launch at upgrade time but never during recovery.

## Deployment modes

| Varies | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Operator | Vendor | Vendor through the ExternalId deploy role | Customer |
| AWS account | Vendor cell accounts, one account per cell | Customer's dedicated account | Customer's dedicated account |
| Tenants per install | Many per cell, large tenants on dedicated cells | One | One |
| Cells per install | Many per Region pair | One | One |
| Default durability class | Region-safe | Zonal with peer shipping; Region-safe if the customer grants a second and witness Region | Zonal with peer shipping; Region-safe optional |
| Hostname and TLS | Vendor subdomain or customer CNAME with managed certificate | Customer hostname and ACM certificate | Customer hostname and ACM certificate |
| Deploy principal | Vendor CI | Vendor principal with ExternalId | Customer CI role |
| Rollout driver | <vendor-fleet-app> waves across cells | <vendor-fleet-app> waves across installs | Customer runs `tributary-deploy` |
| Observer role | Vendor internal | Granted by customer | Optional, null by default |
| Hosted runners | Firecracker fleet in a separate runner account | Optional, in a customer runner account | None by default; customer runners and ARC |
| Action source | Vendor-maintained mirror of public actions per Region | Vendor mirror through an outbound sync, or air-gapped sync tool | Outbound sync or air-gapped sync tool |
| Id allocator | Vendor control plane leases blocks to cells | Install-local | Install-local |

Everything else is identical: the same signed bundle, the same services, the same schema, and the same `operations` object. Differences come only from `operations` fields, never from branching on the mode name (research/byoc-grounding.md#11).

Reuse. The release registry, signing, retention, `rollback-check`, the bootstrap trust pattern, the target document, the `operations` posture object, deploy events, the deployer image, and <vendor-fleet-app>'s wave and gate model transfer unchanged (research/byoc-grounding.md implications 1-4). Additions: a `product` field in target and record and Tributary IAM prefixes with PassRole for EC2 and Auto Scaling (implications 5, 12); the `roll:git-fleet` stage, which is drain and replace because nodes are caches (implication 7); observer routes for register read latency, shipper lag, projector lag, compaction backlog, cache hit ratio, and canary push success, plus gate checks that read them (implication 8); expand and contract as a release attribute (implication 9); per-AZ canary with automatic in-install abort (implications 10-11); AMIs inside `release.json` (open question 6); and in managed mode, a <vendor-fleet-app> "install" that stands for one cell (implication 13). Tributary requires its own dedicated account in BYOC and self-hosted, which matches the current model (implication 6).

## Actions

Workflow engine. The server owns triggers and filters, matrix expansion, the job graph, job-level `if`, concurrency groups, reusable workflows, environments, `GITHUB_TOKEN` minting, OIDC, label matching, and the job message (research/actions-protocol.md#62). `trib-actions-wf` is a Rust port of the MIT C# `GitHub.Actions.WorkflowParser` in actions/runner, including `MatrixBuilder` and the template-token and context-data JSON converters, with `workflow-v1.0.json` loaded as data (research/actions-protocol.md#59-60). `@actions/workflow-parser` runs as a differential oracle in CI. Triggers come from outbox events, so a workflow runs if and only if its push committed.

Runner protocol. Tributary returns `use_v2_flow: true` and implements the admin registration flow, the broker, the run service, the launch service, and the results service (research/actions-protocol.md#5-6, #12, #16-17, #22). The legacy distributedtask family is frozen and can be removed by GitHub in any runner release, while the broker family is what the current runner and ARC exercise (research/actions-protocol.md risk ratings B and F). Run-service jobs use results only, so there is one log path instead of two. JIT configs always set `GitHubUrl` so the runner does not infer a hosted server (research/actions-protocol.md#2). Each Tributary release pins a tested runner version and pushes it with `RunnerRefresh` (research/actions-protocol.md#25-26). ARC works through the scale-set APIs (research/actions-protocol.md#30). The risk is that no open server implements the broker today (research/actions-protocol.md#52), so Milestone 5 reads the runner and github-act-runner sources as the specification.

Hosted runner fleet. Managed hosted runners are Firecracker microVMs on pre-provisioned metal instances in a separate runner account per Region. A job start boots a microVM from a warm snapshot, which is our own data plane and does not call EC2 launch, so the October 2025 class of event delays only jobs beyond warm capacity (research/uptime.md#33). The runner account has no network path to any cell except the public hostname, exactly like a customer's self-hosted runner. Images come from the MIT runner-images provisioning scripts with a new `amazon-ebs` Packer source (research/actions-protocol.md#63-65).

Cache. Both cache v1 (`_apis/artifactcache`) and cache v2 (twirp `CacheService`) are served, because a GHES-classified host forces v1 and v2 matters once the gate opens (research/actions-protocol.md#34-36, #43). Uploads map `PATCH` chunks or Azure blocks onto S3 multipart parts through `trib-blobshim`, with `x-ms-request-id` on every response (research/actions-protocol.md#39). Downloads are plain GETs on the host, streamed by the edge from S3. BYOC operators can switch downloads to presigned S3 URLs through a VPC endpoint.

Artifacts and the toolkit GHES gate. `upload-artifact@v4` and later throw before any request on a GHES-classified host (research/actions-protocol.md#44, #49). The launch resolver is the only place a fix can go (research/actions-protocol.md#17). Tributary therefore does three things. It serves the legacy pipelines artifact API for `upload-artifact@v3.2.2`. It serves twirp `ArtifactService` with the block blob shim. It applies a signed, published patch to tag and branch references of the actions that bundle `@actions/artifact` or `@actions/cache`. The patch changes only the hostname test, in the manner of Gitea's runner (research/actions-protocol.md#57). The patch catalog is data, signed with the release key, shipped separately from binaries, and each patched resolution prints the upstream and served digests in the job log. A SHA-pinned reference is never patched. It runs the exact upstream bytes and fails as it does on GHES, and the job annotation names a vendor-maintained fork whose commit SHA the user can pin instead. A SHA pin is a promise about bytes, and silently breaking that promise is worse than a visible failure. Org policy can disable patching. In parallel the vendor upstreams a change for issue #2439, trusting `ACTIONS_RESULTS_URL` (research/actions-protocol.md#51). `@actions/attest` stays unsupported because it requires github.com or `*.ghe.com` (research/actions-protocol.md#47).

Logs. The results service returns `BLOB_STORAGE_TYPE_UNSPECIFIED` with Tributary-signed URLs that accept `comp=appendblock` and `seal=true` (research/actions-protocol.md#23). Each append becomes an S3 object under the step's prefix, and sealing concatenates them into one object. The live feed is a WebSocket served from the actions task that holds the job.

OIDC. The issuer is `https://<host>/_services/token` with the legacy `sub` format by default and the `OWNER@ID/REPO@ID` format as an opt-in (research/actions-protocol.md#41-42, implication 11). Tokens are RS256, signed with a cell key decrypted at boot, and the JWKS is static per key epoch with overlap during rotation.

Coping with GitHub changes. A weekly job runs the newest tags of `actions/checkout`, `cache`, `upload-artifact`, `download-artifact`, `setup-node`, `setup-python`, `setup-go`, and `configure-aws-credentials` against a staging cell with the pinned runner and with the newest runner. A failure opens a patch catalog update or a runner pin change. The catalog ships without a binary release. Runner upgrades are conformance gates, because the runner protocol changes about monthly (research/actions-protocol.md#28).

## Frontend and native app

The web app is React 19 with Primer React, server-rendered by the `web` service in each cell (research/clients.md#62-64). It reaches data through persisted GraphQL queries to `api` in the same cell, plus a small internal REST set for code browsing that `api` answers from gitd. Static bundles are content-hashed and served by the edge from S3, so self-hosted installs need no CDN.

Degraded backends. Code browsing, blame, raw files, and pull request diffs need only gitd and the identity snapshot, because the repository directory is in the snapshot and pull request heads are refs in the log (`refs/pull/N/head`). When DSQL is down, the PR page still renders the diff and commits, and the comment and review panes show an unavailable state behind per-pane error boundaries. Search panes fail soft and never block a page (research/uptime.md#49, implication 18). SSR has a 2-second budget per data call. A call that misses it streams a placeholder and the client fetches the data after hydration. That path is the normal code path on every slow call, not a separate mode.

Native app. A GPUI app does not earn a place at launch. GPUI is pre-1.0 with frequent breaking changes and a heavy build (research/clients.md#57-61), and every task it would do is covered by the web app, gh, VS Code, and JetBrains, which already speak our wire protocol. It becomes worth building after launch only with a narrow scope: an offline-capable review inbox for pull requests and notifications that uses only the public REST and GraphQL API, so it doubles as another conformance client. If built, it vendors `gpui-pre` at a pinned revision.

## Build order

**Milestone 1. The log commits and survives.** `trib-wal` with both `S3Register` and `MrscRegister`, `ImmutableStore`, the `Committer`, and `trib-git` driving upstream receive-pack and upload-pack through hooks, on three gitd nodes in one dev cell with no auth. Gate: stock git 2.39 and 2.54 clients push concurrently to the same repositories through all three nodes for 24 hours while a fault injector kills nodes mid-push and blocks S3 for seconds at a time. A linearizability checker (Elle or Porcupine) over the client-observed acknowledgments must find every acknowledged push present and the ref history linearizable, and `git fsck --full` must pass on a fresh rematerialization. The same run records per-repository CAS throughput and p50 and p99 commit latency on S3 Standard and on MRSC across a real Region trio. This retires the riskiest unknowns first: undocumented CAS rates and MRSC latency (research/git-storage.md open question 3, research/uptime.md open question 3).

**Milestone 2. A GHES host that clients recognize.** `edge`, `trib-wire`, `trib-auth` with snapshots and seeded OAuth clients, and the T0 rows for `rest:meta`, `rest:users`, `rest:repos`, `rest:orgs`, `tokens`, and both OAuth flows. Gate: GCM auto-detects the host and completes device auth then clones and pushes; `gh auth login --hostname` succeeds by device and web flow; JetBrains accepts the version header; Renovate classifies the host; terraform `TestAccGithubRepository` passes.

**Milestone 3. Pull requests, policy, GraphQL.** `trib-pr`, `trib-policy`, the projector, merges, branch protection as log entries, and the GraphQL subset. Gate: cli/cli acceptance pr and issue tests; static validation of all VS Code Pull Requests, JetBrains, Renovate, and gh GraphQL operations; VS Code Pull Requests e2e merge; terraform `TestAccGithubBranchProtection`.

**Milestone 4. Events.** Outbox, dispatcher, webhooks, register sweep. Gate: Argo CD refreshes from a push webhook; Jenkins Branch Source discovers branches and PRs and posts statuses; a projector replay test rebuilds every projection from checkpoints and diffs equal.

**Milestone 5. Actions.** v2 runner protocol, workflow engine, cache v1 and v2, legacy and v4 artifacts with the patch catalog, logs, OIDC. Gate: actions/runner v2.338.1 runs the workflow corpus including `actions/checkout`, `actions/cache@v4`, `upload-artifact@v3.2.2`, and tag-referenced `upload-artifact@v4`; ARC scales a set from zero; `configure-aws-credentials` assumes a role through OIDC.

**Milestone 6. Region-safe cells.** Dual-Region writes, MRSC in production shape, multi-Region DSQL, health-checked routing, `AckInSurvivor`. Gate: the Milestone 1 harness runs across two Regions while one Region's network is cut for 30 minutes; zero acknowledged pushes lost and service from the survivor within 2 minutes.

**Milestone 7. One artifact, three modes.** Product profile in the reused deploy tool, `roll:git-fleet`, new observer routes and gates. Gate: an N-1 to N upgrade under continuous stock-git push and fetch load with zero failed operations, in a managed cell, a BYOC test account, and a self-hosted test account.

**Milestone 8. Ecosystem breadth.** Renovate end to end, Dependabot through labelled runners, container packages, search. Gate: the Renovate and dependabot oracle commands from research/clients.md open real update PRs; `docker push` and `docker pull` succeed.

## Synthesis decision

Filled in by the arena lead.

## Tradeoffs accepted

- We accept one CAS chain per repository, with a ceiling near 120 pushes per second on S3 Standard (research/git-storage.md#30), in exchange for a commit point that needs no quorum, placement database, or repair sweeper. Group commit and the bot-ref lane protect human pushes.
- We accept push latency of two S3 round trips in Zonal cells, and one to two more inter-Region round trips in Region-safe cells, in exchange for an acknowledgment that already lives in three AZs or two Regions.
- We accept that Region-safe cells cost roughly twice a Zonal cell [guess] in exchange for cross-Region RPO 0 and active-active git with no promotion step. Managed defaults to it because FRAME ranks availability and durability above cost.
- We accept `AckInSurvivor` as the default during a peer Region impairment, which lowers durability to one Region for those pushes, in exchange for keeping writes available. `RefuseWrites` exists for tenants who rank durability higher.
- We accept that a DSQL outage freezes identity changes and blocks pushes only to branches whose rules require status checks, in exchange for git that keeps serving through the outage.
- We accept that organization rulesets reach each repository's log a few seconds apart, in exchange for policy that is linearized with pushes inside each repository.
- We accept the hostname-per-tenant model, which rules out a public multi-tenant namespace and cross-tenant forks, in exchange for cells that never need a cross-cell query and an instance model that matches GHES.
- We accept patching tag-referenced artifact actions, a deliberate break from "the bytes GitHub published", in exchange for `upload-artifact@v4` working. SHA pins are exempt.
- We accept a Zonal-cell Region loss RTO of hours, which needs new capacity in another healthy Region, in exchange for a cheap default for self-hosted and BYOC installs.
- We accept a 17-minute worst case for a webhook whose hint was lost in a node crash, in exchange for keeping DSQL off the commit path.

## Alternatives considered

### Spokes-style quorum of full replicas on local NVMe

Three to five full replicas per repository across AZs, with a three-phase commit through the reference-transaction hook, a placement database, checksums, and sweepers (research/git-storage.md#2-6). It lost on interface depth and operability. Durability depends on replica health, so every deploy, AZ event, and node replacement needs replica catch-up and quorum checks, which is exactly the stateful-fleet stage the BYOC tooling lacks (research/byoc-grounding.md implication 7). A self-hoster has to repair replicas at 2am. Cross-Region RPO 0 needs a cross-Region quorum member, which GitHub itself is now building with object storage (research/git-storage.md#6). It exposes placement and repair to operators and hides less behind the commit interface.

### Refs as rows in the cell database, packs in S3

Ref values live in DSQL rows with optimistic concurrency, and packs live in S3, close to the Azure DevOps shape (research/git-storage.md#22). It lost because it makes git ordering depend on the metadata database, and GitHub's incident history shows the database is the largest cause of core outages (research/uptime.md#43-44). A push of 5,000 tags hits DSQL's 3,000-row transaction limit and loses atomicity unless refs are packed into a blob, which rebuilds the register this design already has (research/uptime.md#27). Schema migrations on the ref table would also become git outages.

### One GHES-style appliance per customer, even in managed

Each managed customer gets a dedicated single-account install, so managed equals BYOC run by the vendor. It lost because it multiplies fixed cost per tenant, and because the existing tooling has no cells or placement (research/byoc-grounding.md#37, implication 13). It would still need a storage design, so it does not avoid the core choice. It does survive in this design as the dedicated-cell option for large tenants.

## Open questions and risks

- What are the real sustained CAS rate and p99 latency for one hot key in S3 Standard, and for one MRSC item across a same-continent Region trio? AWS documents neither (research/git-storage.md open question 3, research/uptime.md open question 3). Milestone 1 measures both. If MRSC p99 exceeds about 150 ms, should Region-safe become an opt-in class instead of the managed default?
- Are MRSC strongly consistent reads served in-Region at single-digit milliseconds? Every fetch in a Region-safe cell does one [unverified]. If they are slow, a Region-safe fetch would need a gossip-driven freshness window, which weakens read-your-writes across nodes.
- Does the reference-transaction hook in the `prepared` state behave identically across the git versions we ship, including 2.54's new `preparing` state (research/git-storage.md#38)? A hook-ordering bug would be a durability bug, so should we pin one git version per release and fuzz the hook protocol?
- Is this direction wrong for monorepos with heavy bot traffic? One chain per repository is the ceiling even with group commit. Cursor reached higher rates only on S3 Express One Zone (research/git-storage.md#30), which is single-AZ and cannot hold an acknowledged push alone (research/git-storage.md#56). Should very hot repositories split `refs/pull/*` into a second log, at the cost of cross-log atomicity for merges?
- Is patching tag-referenced actions acceptable to security reviewers (research/actions-protocol.md open question 3)? If not, `upload-artifact@v4` and later stay broken as on GHES and the forks become the only path.
- Can the identity snapshot stay small enough for a 100,000-user tenant with millions of tokens? Sharding by token hash prefix is planned but unmeasured.
- Zonal metadata RPO relies on DSQL change data capture (GA per research/uptime.md#28) feeding a peer-Region archive. The sink options and restore path are [unverified]. Is an hourly backup copy acceptable as the Zonal metadata RPO if CDC export proves unworkable?
- Does vendor-to-customer role chaining cap BYOC deploy sessions at one hour (research/byoc-grounding.md open question 1)? A large `roll:git-fleet` is fast because nodes warm from S3, but a 50-node install might still outlast one hour.

## Next implementation step

Build `trib-wal` with the `RefRegister` trait, both register implementations, the `Committer`, and the Milestone 1 linearizability harness that drives stock git through upstream receive-pack hooks.
