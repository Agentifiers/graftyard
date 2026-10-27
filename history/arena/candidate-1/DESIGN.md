# Spokes on AWS: per-reference quorum on local NVMe, with S3 as the fencing virtual replica

## Problem

Tributary must look exactly like a GHES 3.22 appliance to unmodified clients (`research/clients.md` findings 1 to 47), run as one artifact in managed, BYOC, and self-hosted modes (FRAME.md), and rank git, pull request, REST, and GraphQL availability above everything else, then durability. Five facts make the shape non-obvious. Every large host that put live repositories on shared filesystems moved off them, so repositories must sit on local disk and replicate at the git level (`research/git-storage.md#1`, `#13`, `#23`, `#27`), yet local NVMe vanishes on stop or host failure (`research/git-storage.md#51`). A whole Region and its data can be lost for good (`research/uptime.md#37`, `#38`), so "never lose an acknowledged push" needs a copy outside the zone set and a stated cross-Region answer. No Rust library can serve git, so upstream git stays the pack engine (`research/git-storage.md#44` to `#49`). AWS control planes fail for hours while running instances keep working (`research/uptime.md#32` to `#35`), so recovery must use capacity that already runs. GitHub's own core outages come mostly from shared databases, config pushes, and load (`research/uptime.md#43` to `#47`), so git must not share a failure domain with the metadata database. This candidate bets on the Spokes lineage with GitHub's 2026 changes (`research/git-storage.md#2` to `#7`): full repositories on NVMe in three AZs, a vote per reference driven by git's `reference-transaction` hook, and S3 as a mandatory virtual replica whose create-only write is the final fence for each reference. Disks serve every read. S3 decides every write. An acknowledged push is on two AZ-separated disks and in S3 Standard.

Where the nine binding facts in TASK.md are answered:

1. Region loss and RPO: "Repository storage and ref transactions" (durability tiers) and "Failure modes" (`region` rows). Zero push RPO is the Dual-Region tier.
2. Object storage as truth and Continuity: "Repository storage and ref transactions" (S3 virtual replica and fence) and "Alternatives considered".
3. Upstream git as engine: "Repository storage and ref transactions" and "Service and crate map".
4. Toolkit GHES gate and SHA pins: "Actions" (launch resolver shim).
5. Runner protocol: "Actions" (`use_v2_flow: true`).
6. GHES host shape and GraphQL: "Wire compatibility" and "Identity and tokens".
7. Stateless `ghs_` tokens, node ids, id ranges: "Identity and tokens" and "Wire compatibility".
8. BYOC gaps and upgrade downtime: "Change safety" and "Deployment modes".
9. Control-plane independence: "Tenancy and cell placement", "Failure modes" (`control-plane` rows), and "Change safety".

## Usage (caller's view)

**Self-hosted quickstart.** The customer runs the same deploy tool the vendor uses (`research/byoc-grounding.md#23`), in a dedicated AWS account.

```
# once, with local state, as an account administrator
tofu -chdir=bootstrap/customer apply \
  -var product=tributary -var deploy_principal_arn=arn:aws:iam::111122223333:role/ci-deploy
# the target names the account, Region, three AZ subnets, hostname, and posture
cat > tributary.target.json <<'EOF'
{ "version": 1, "name": "corp-git", "profile": "tributary-customer",
  "aws": { "accountId": "111122223333", "region": "us-east-2" },
  "terraform": { "stateBucket": "platform-terraform-state-111122223333" },
  "operations": { "hostname": "github.corp.example", "durability": { "regional": { "dr": "us-west-2", "drLearner": false } },
                  "storage": { "instanceType": "i8g.2xlarge", "nodesPerAz": 1 } } }
EOF
tributary-deploy release fetch --registry prod --label 2026.11.1
tributary-deploy deploy --target tributary.target.json --bundle ./2026.11.1
tributary-deploy admin bootstrap --email admin@corp.example
# DNS: github.corp.example and *.github.corp.example CNAME to the install's NLB name
```

An upgrade is the same `deploy` command with a newer bundle. It never needs maintenance mode. The minimum install is three storage nodes (one per AZ) plus stateless services on ECS across the same three AZs.

**BYOC quickstart.** The customer applies the same bootstrap with the vendor's deploy principal, an ExternalId, and the optional observer role (`research/byoc-grounding.md#3` to `#7`). They hand the vendor three subnet ids, a hostname, and either a certificate ARN or a delegated subdomain for ACME. The vendor's <vendor-fleet-app> installs and upgrades in gated waves. The vendor has no runtime path into the data plane.

**Managed quickstart.** In the vendor console the customer picks a slug (`acme`), a home Region, a durability tier (Regional or Dual-Region), and connects a SAML or OIDC IdP, then clicks Create. The global control plane assigns a cell, writes the tenant into that cell's config bundle, issues certificates for `acme.tributary.dev` and `*.acme.tributary.dev`, and creates one CNAME to the cell's DNS name. Nothing in that sequence sits on any later request path.

**What an unmodified gh user types.**

```
gh auth login --hostname acme.tributary.dev    # device flow, gh's own client id 178c6fc778ccc68e1d6a
gh repo clone acme/app && cd app
git switch -c fix && git commit -am "fix" && git push -u origin fix
gh pr create --fill
gh pr merge --squash
export GH_HOST=acme.tributary.dev               # optional default host
```

**Journey 1: `git push` to a protected branch.** The repository is `acme/app`, branch `main` is protected with "restrict pushers to team release" and "require linear history".

1. DNS resolves `acme.tributary.dev` to the owning cell's NLB (zonal IPs). `trib-edge` terminates TLS with the tenant certificate, reads `Authorization: Basic` with a `ghp_` password, checks the prefix and CRC32 offline, and resolves the token hash in its in-memory auth cache (DSQL read only on a cold miss). It stamps `X-GitHub-Request-Id`.
2. `GET /acme/app.git/info/refs?service=git-receive-pack` goes to `trib-git`. It finds the network's replica set in its in-memory placement map (loaded from an S3 snapshot), asks two voters for their `NetworkDigest` in parallel, and streams the advertisement from the newer one.
3. `POST /acme/app.git/git-receive-pack` is routed by rendezvous hash on the repository id to one `trib-git` process, the coordinator for that repository. The coordinator tees the request body to `trib-spokesd` on all three voters, starts an S3 multipart upload to `pack/<net>/<sha256>.pack`, and writes an intent key `txidx/<hour>/<shard>/<txn>` (If-None-Match).
4. Each voter runs upstream `git receive-pack --stateless-rpc`. Git quarantines and indexes the pack. The `pre-receive` hook asks the coordinator for the policy verdict, which the coordinator computes once: actor in team `release` (auth cache), and new tip a descendant of the old tip (`git merge-base --is-ancestor` on its local voter).
5. Git migrates the quarantine, takes the per-ref lock on `refs/heads/main`, and runs the `reference-transaction` hook in state `prepared`. The hook binary `trib-vote` sends a `Vote` (digest of the updates, the ref's current `RefSeq`) to the coordinator over the cell network.
6. When two voters in two AZs have matching votes and the S3 multipart upload has completed, the coordinator claims the fence: `PutObject refseq/<net>/<repo>/<refhash>/<seq+1>` with `If-None-Match: *`. A 200 is the commit point.
7. The coordinator tells the voters to commit. Each hook exits 0, git writes the ref, and `trib-spokesd` returns the new network checksum. After two voters have applied, the coordinator lets receive-pack's `report-status-v2` through to the client. The third voter gets 200 ms more before the coordinator stops waiting.
8. The coordinator sends a hint to `trib-events`, which reads the fence record and inserts a `git_events` row plus outbox rows in DSQL. Consumers then update pull request heads, deliver the `push` webhook, and trigger workflows.

Datastores touched before the acknowledgment: NVMe on three voters, S3 (pack, intent, fence). DSQL is touched only by the auth cache on a cold miss and by event ingestion after the acknowledgment.

**Journey 2: merging a pull request.** `gh pr merge 42 --squash` sends the GraphQL `mergePullRequest` mutation with `expectedHeadOid`.

1. `trib-edge` authenticates the `gho_` token. `trib-api` loads pull request 42, its reviews, check runs, and the branch protection rule from DSQL in one read transaction and evaluates the merge policy (a pure function).
2. `trib-api` calls `trib-git` `merge(net, base=refs/heads/main, head=expectedHeadOid, Squash)`. The coordinator asks its local voter to run `git merge-tree --write-tree` and `git commit-tree`, then checks that the tree diff between the old base and the result equals the diff between the merge base and the head (`research/uptime.md#49`), and builds a pack of the new objects with `git pack-objects`.
3. The coordinator commits an internal `RefTxn` with `refs/heads/main: old=<base the merge was computed on>`, through exactly the push path of Journey 1 steps 3 to 7 (tee, votes, S3 fence). If someone pushed to `main` in between, the old value mismatches, the vote fails, and the API returns GitHub's "Base branch was modified" error.
4. `trib-api` then writes, in one DSQL transaction, the pull request state `Merged { commit }`, the timeline event, and the outbox row for `pull_request.closed`. If that write cannot finish within the request deadline, the response still reports the merge (git is the truth), and the `pr-sync` consumer completes the row when it ingests the git event, because it marks merged any open pull request whose head becomes reachable from its base.

Datastores: DSQL (read, then write), NVMe on three voters, S3 (pack, intent, fence).

**Journey 3: `actions/runner` runs a job with `actions/checkout` and `actions/cache`.**

1. Registration: `config.sh --url https://acme.tributary.dev/acme --token <reg>` posts `/api/v3/actions/runner-registration` with `RemoteAuth`. `trib-actions` answers `{url, token, token_schema, use_v2_flow: true}`. The runner lists groups and posts `/api/v3/actions/runners/register` with its public key (`research/actions-protocol.md#5`, `#6`). Rows land in DSQL.
2. `run.sh` opens a broker session (`POST /_services/broker/session`) and long-polls `GET /message`. The session lives in the memory of one `trib-actions` broker process. The durable job queue is in DSQL.
3. The push event from Journey 1 reaches the workflow consumer. It reads `.github/workflows/ci.yml` through `trib-git` from a voter, expands the matrix, and inserts the run and its jobs in DSQL. A queued job matching the runner's labels is claimed with an OCC update (`state='queued'` to `assigned`) and delivered as a `RunnerJobRequest`.
4. The runner posts `acquirejob` to `/_services/run/` and receives the job message. Its `SystemVssConnection` data points `CacheServerUrl` and `ResultsServiceUrl` at `https://acme.tributary.dev/_services/...`, and `GenerateIdTokenUrl` at the OIDC endpoint. `GITHUB_TOKEN` is a stateless `ghs_` token scoped to the repository.
5. For each `uses:`, the runner asks the launch service. It returns codeload tarball URLs from the cell's `actions` mirror organization, with the toolkit shim applied where the rules in "Actions" allow. `actions/checkout` runs `git fetch` with `GITHUB_TOKEN`, which follows the normal `trib-edge` and `trib-git` read path to a voter.
6. `actions/cache` restore classifies the host as GHES and calls cache v1: `GET _apis/artifactcache/cache?keys=...` (`research/actions-protocol.md#34`). `trib-actions` answers with an S3 presigned GET. The post step reserves, uploads `Content-Range` chunks that become S3 multipart parts, and commits. Cache rows live in DSQL.
7. Step logs go through the results twirp service, which hands out Tributary-signed URLs that accept `comp=appendblock` and `seal=true` and buffer into S3 (`research/actions-protocol.md#23`). The runner posts `renewjob` every 60 s and finally `completejob` with conclusion, outputs, and annotations.
8. `completejob` updates the job and check run in DSQL and writes outbox rows for `workflow_job` and `check_run`. Webhook delivery and the pull request checks UI follow from the outbox.

Datastores: DSQL (registration, runs, jobs, cache index, checks), S3 (cache blobs, logs, action tarballs), NVMe (workflow file read and checkout fetch).

## Shape

### Tenancy and cell placement

```rust
/// A cell is one install: the full Tributary stack in one Region across three AZs.
/// Managed runs many cells per Region. BYOC and self-hosted run exactly one.
pub struct Cell {
    pub id: CellId,
    pub home: Region,
    zones: [AzId; 3],                 // private: Cell::new rejects duplicates
    pub ids: IdRange,                 // disjoint across every cell ever created
    pub durability: DurabilityTier,   // immutable after creation
}

pub enum DurabilityTier {
    /// Fence in the home S3 bucket. Bucket replicates asynchronously to `dr`.
    Regional { dr: Region, dr_learner: bool },
    /// Fence in multi-Region DSQL. Packs land in both Regions' buckets before ack.
    DualRegion { peer: Region, witness: Region },
}

/// Numeric ids for every entity type come from this range. Invariant: base + len <= 2^53.
pub struct IdRange { base: u64, len: u64 }
impl IdRange { pub fn new(base: u64, len: u64) -> Result<Self, IdRangeError> { todo!() } }

/// A hostname belongs to one tenant. A tenant lives in exactly one cell.
pub struct Tenant {
    pub id: TenantId,
    pub hostname: Hostname,           // Hostname::parse rejects *.github.com, *.ghe.com, ports
    pub cell: CellId,
    pub shard: ShuffleShard,          // the storage nodes this tenant's networks may use
    pub migration: Option<Migration>,
}

/// Clone, flip authoritative, redirect, forget (research/uptime.md#10).
pub enum Migration {
    Cloning { to: CellId },
    Flipped { to: CellId },
    Redirecting { to: CellId, until: Timestamp },
}

pub struct ShuffleShard { per_az: [SmallVec<[NodeId; 8]>; 3] }

/// Served from memory, loaded from the signed cell config bundle in S3. Never reads DSQL.
pub trait TenantDirectory {
    fn resolve(&self, sni_or_host: &Hostname) -> Option<TenantRoute>;
}
pub enum TenantRoute { Local(TenantId), Forward { cell: CellId, endpoint: SocketAddr } }
```

The routing decision is DNS. A tenant hostname is a CNAME to its cell's name, and the cell's name points at the cell's NLB. There is no shared Tributary router on any request path. This choice follows from three facts. GHES clients need one bare hostname per instance (`research/clients.md#4`, `#13` to `#17`), so the hostname is a natural partition key. SSH on port 22 carries no SNI, so an L4 or L7 router cannot tell tenants apart without terminating SSH, while DNS can. And AWS's cell guidance calls the router the one shared component to minimize (`research/uptime.md#10`). The per-tenant hostname also means each tenant is its own GHES-shaped enterprise with its own user namespace, like GHE.com. There is no global user table, which removes the shared identity database behind GitHub's February 2026 outage (`research/uptime.md#47`).

Invariants the types encode: a cell has three distinct AZs; id ranges never overlap and stay below 2^53, so a tenant can migrate between cells without renumbering; a hostname can never shadow GitHub's own host classes; the durability tier is fixed per cell, so no repository ever has an ambiguous commit rule.

Access patterns. Every request resolves `Hostname` to `TenantId` in `trib-edge` from memory (O(1) hash lookup). Cell migration flips `Tenant.cell` in the global control plane, rewrites both cells' config bundles, then changes the CNAME. During the DNS TTL the old cell's `TenantDirectory` returns `Forward`, and `trib-edge` proxies TCP to the new cell. DNS edits happen only in planned migrations, never in failure recovery (`research/uptime.md#12`, `#15`). Region failover uses Route 53 ARC routing controls on pre-created failover records, with all five cluster endpoints hardcoded (`research/uptime.md#22`).

Large tenants get a dedicated cell. A tenant can never exceed one cell, so cell capacity bounds tenant size. Cells scale storage nodes horizontally and DSQL scales writes horizontally, so that bound is high [inference]. New managed cells avoid us-east-1 as home Region (`research/uptime.md#22` implication).

### Repository storage and ref transactions

```rust
pub struct NetworkId(u64);   // fork network: one shared object store, one ref namespace per repo
pub struct RepoId(u64);
pub struct ReplicaEpoch(u64);
pub struct RefSeq(u64);      // per-ref version, strictly increasing; a gap marks a burned claim
pub struct RefName(Box<str>);           // parsed with git check-ref-format rules
pub enum Oid { Sha1([u8; 20]) }         // Sha256 variant added with Git 3.0 support

pub struct ReplicaSet {
    pub network: NetworkId,
    pub epoch: ReplicaEpoch,
    voters: [Voter; 3],                 // private: three distinct AZs, enforced by ReplicaSet::new
    pub learners: Vec<Learner>,         // non-voting: DR copies and read scale-out, warmed from S3
}
pub struct Voter { pub node: NodeId, pub az: AzId }

pub struct RefUpdate { pub name: RefName, pub old: Option<Oid>, pub new: Option<Oid> }

/// The only way a ref moves. Pushes, merges, web edits, REST git/refs, and PR head refs.
pub struct RefTxn {
    pub id: TxnId,                      // ULID, carries the hour used by the intent index
    pub repo: RepoId,
    pub network: NetworkId,
    pub epoch: ReplicaEpoch,
    updates: SortedRefUpdates,          // non-empty, sorted by name, unique
    pub cause: TxnCause,
    pub lane: Lane,
}
pub enum TxnCause {
    Push { actor: ActorId, push_options: Vec<String> },
    Merge { pr: PullRequestId, method: MergeMethod },
    PullHead { pr: PullRequestId },
    MergePreview { pr: PullRequestId },
    WebEdit { actor: ActorId },
    RestApi { actor: ActorId },
}
pub enum Lane { User, Background }      // Background (merge previews) never queues ahead of User

/// Sent by the reference-transaction hook in state `prepared`, while git holds the ref locks.
pub struct Vote {
    pub voter: NodeId,
    pub txn: TxnId,
    pub epoch: ReplicaEpoch,
    pub digest: VoteDigest,             // sha256(txn, epoch, sorted (name, old, new))
    pub current: Vec<RefSeq>,           // the voter's seq for each ref in the txn
}

/// The S3 virtual replica's record. One per (ref, seq), written create-only.
pub struct FenceRecord {
    pub txn: TxnId,
    pub repo: RepoId,
    pub updates: Vec<(RefName, RefSeq, Option<Oid>)>,
    pub pack: Option<PackKey>,          // content-addressed pack/<net>/<sha256>.pack
    pub cause: TxnCause,
    pub format: u16,
}
pub enum Claim { Won, Lost { holder: TxnId } }

pub trait FenceStore {
    /// Regional: PutObject If-None-Match on refseq/<net>/<repo>/<refhash>/<seq>.
    /// DualRegion: INSERT into ref_claims (net, repo, refhash, seq) in multi-Region DSQL.
    async fn claim(&self, key: &FenceKey, rec: &FenceRecord) -> Result<Claim, FenceError>;
    async fn lookup(&self, key: &FenceKey) -> Result<Option<FenceRecord>, FenceError>;
}

pub struct NetworkDigest {
    pub checksum: [u8; 32],             // XOR over refs of sha256(name || oid), updated per apply
    pub applied_total: u64,             // sum of RefSeq over all refs, strictly increasing
}

pub struct Committed { pub seqs: Vec<RefSeq>, pub applied_on: ArrayVec<NodeId, 3> }
pub enum TxnError { StaleOld(RefName), Denied(Denial), NoQuorum, FenceLost(TxnId), StaleEpoch(ReplicaEpoch), Deadline }
pub enum PackSource { Client(BodyStream), Built(PackKey), Empty }

pub trait RefTransactions {
    async fn commit(&self, txn: RefTxn, objects: PackSource) -> Result<Committed, TxnError>;
}

/// Pure: given digests from at least two voters, return a replica holding every acked write.
pub fn choose_read_replica(set: &ReplicaSet, seen: &[(NodeId, NetworkDigest)]) -> Result<NodeId, ReadError> { todo!() }
```

**On disk.** Each storage node (`trib-spokesd` on EC2 i8g with instance-store NVMe in managed mode, `research/git-storage.md#51`) holds full bare repositories: `networks/<net>/objects` shared by the network, and `networks/<net>/repos/<repo>.git` with alternates pointing at it. Repositories use git's `files` ref backend, because its lock is per ref, which gives per-reference scope for free. Reftable locks one `tables.list` per repository (`research/git-storage.md#18`), so it would bring back the repository-wide lock that GitHub is removing (`research/git-storage.md#6`). Upstream git 2.54 or later, built and pinned in the storage AMI, does every object operation: `receive-pack`, `upload-pack`, `pack-objects` with bitmaps and MIDX bitmaps, geometric repack, cruft packs, `merge-tree`, `fsck` (`research/git-storage.md#32` to `#41`). Rust owns framing, orchestration, the vote, the fence, placement, and scheduling (`research/git-storage.md` implication 8). `core.fsync` is set for objects and references, so a `prepared` vote means objects and the lock are on stable storage [inference].

**Commit rule.** A `RefTxn` commits when, in order: (a) two voters in two AZs return matching `Vote`s, which means each has the objects fsynced, passed git's connectivity check, and holds the ref locks; (b) the pack is durable in S3 (CompleteMultipartUpload returned); (c) the coordinator wins `FenceStore::claim` for `seq+1` of every ref in the transaction, in sorted ref order. Step (c) is the linearization point for each reference. The acknowledgment waits for two voters to report the ref applied. So at acknowledgment the push exists on two NVMe disks in two AZs and in S3 Standard across at least three AZs (`research/git-storage.md#54`). This is GitHub's announced design, with blob storage as a voting member and per-reference scope (`research/git-storage.md#6`), made concrete on AWS with conditional writes (`research/git-storage.md#55`).

**Why S3 is both a voter and the fence.** The voter majority holds git's own per-ref lock files, so two coordinators cannot both hold a majority. That is the Spokes fast path (`research/git-storage.md#2`, `#4`). The create-only S3 key is a second, independent fence. It catches a stale coordinator, a split membership view, or a replica bug, which is the DynamoDB Enactor lesson of fencing at write time rather than checking first (`research/uptime.md#32`, implication 11). Every (ref, seq) has exactly one winner, so recovery from S3 alone is deterministic. Unlike Continuity's single manifest per repository (`research/git-storage.md#29`, `#30`), there is no per-repository hot key: different refs commit in parallel, and S3's 3,500 writes per second per prefix (`research/git-storage.md#54`) applies per network prefix.

**Multi-ref atomic pushes.** Git takes all the locks in one transaction. The coordinator claims fence keys in sorted ref order, and each key carries the full record, so every key names every ref in the transaction. A transaction is committed if and only if it won the claim for every ref in its record. If the coordinator loses one claim (412), it claims nothing more and aborts the votes. The keys it already won become burned seqs: they exist, so no one else can claim them, and they resolve to an uncommitted transaction. The next writer of that ref gets a 412 on the burned key, looks up its holder, sees it uncommitted, and claims the following seq. Voters set a ref's seq from the committed record on apply, so seqs stay strictly increasing with gaps [inference].

**Coordinator crash.** A voter that voted `prepared` and hears nothing for 2 s looks up the fence keys. If they name a committed transaction it applies it. If they are still absent at the transaction deadline (30 s), it aborts locally and releases the locks. The fence stays the truth even if a slow coordinator claims after that: the next transaction on the ref gets a 412 on that key, finds a committed holder, and the voters roll it forward from its pack in S3 before anything else proceeds on that ref. The client of the slow push saw an error but its push landed, which git users already see after timeouts. No step depends on synchronized clocks [inference].

**Reads.** `trib-git` asks two voters for their `NetworkDigest` (one intra-Region round trip, in parallel). With write-applied quorum 2 and read quorum 2 out of 3, every read set overlaps every acknowledged write set. If checksums match, either replica serves. Otherwise the one with the higher `applied_total` serves and `trib-git` posts a repair hint to the sweepers. If neither dominates (two concurrent transactions on different refs), it asks the third voter, then retries once after 50 ms. If only one voter answers, it serves only after confirming from `FenceStore::lookup` that none of its refs has a newer seq, capped at 1,000 refs, otherwise the network returns 503 until repair. Reads never touch S3 in the normal path, so a regional S3 impairment stops writes but not clones and fetches. Comparing digests on every read is also continuous anti-entropy, on top of Spokes's compare-after-every-update (`research/git-storage.md#4`).

**Durability tiers and cross-Region RPO (fact 1).**

| Tier | Push commit | Push cross-Region RPO | Metadata cross-Region RPO | Region RTO | Added cost |
| --- | --- | --- | --- | --- | --- |
| Regional (default) | Fence in home S3 bucket | S3 Cross-Region Replication lag, seconds typical, 15 min bound with Replication Time Control [unverified], alarmed at 60 s | DSQL CDC stream applied to a warm DSQL cluster in the DR Region, seconds [inference] | 60 min with `dr_learner`; hours without, because recovery then launches capacity in the DR Region | CRR transfer, one learner replica per network in DR Region |
| Dual-Region | Fence in multi-Region DSQL, pack PUT to both Regions' buckets before claim | 0 | 0 (DSQL multi-Region, `research/uptime.md#26`) | 5 min | 3 learners per network in peer Region (2x disk), 2x S3, cross-Region transfer, about one cross-Region round trip plus a DSQL multi-Region commit added to every push and metadata write [inference] |

In Dual-Region, the per-ref claim lives in multi-Region DSQL because it has a witness Region. An isolated home Region cannot commit, which fences split-brain without any timestamp logic. S3 has no witness, and two independent create-only registers in two Regions can disagree. If the peer bucket is unreachable, pushes continue with the home bucket only and an alarm fires. That is a double failure (peer bucket down, then home Region lost) before data is at risk. Aurora Global was rejected because its unplanned failover loses seconds and is an RDS control-plane call (`research/uptime.md#24`, `#25`).

**Placement and sweepers.** `trib-placement` is the only writer of the `placement` schema in the cell's DSQL cluster: nodes, networks, replica sets, epochs, states. Every change is a conditional update on `epoch`. It writes a signed snapshot to S3 every 30 s and on change, and every `trib-git` process routes from that snapshot in memory, so routing keeps working with DSQL down (`research/uptime.md#9`). Membership changes move one voter at a time: add a learner, let it catch up from S3 and from tee'd live writes, bump the epoch to swap it in for the old voter. Voters reject any vote or apply with a stale epoch. Sweepers: `repair` (catch up or rehydrate lagging replicas, preferring S3 over peers so serving replicas are not loaded), `balance` (disk and CPU skew), `rules` (AZ diversity, shard membership), `checksum` (full digest compare plus sampled `git fsck`), `checkpoint` (one leased replica per network uploads a geometric repack and MIDX bitmap to S3 daily, so S3 restore replays a bounded tail), and `janitor` (stale locks, expired intents, packs superseded for 14 days). Every sweeper has a velocity cap: at most one node per AZ hydrating at a time per network group, and at most 5% of a cell's networks moving per hour (`research/uptime.md` implication 10). After an AZ loss the sweepers do not re-replicate for 6 hours by default, because copying a third of the cell would load the survivors; S3 remains the third durable copy. Failure detection is Spokes's: three consecutive failed requests from a frontend mark a node offline for reads on that frontend, and only heartbeats clear the mark (`research/git-storage.md#3`).

**Dominant access patterns.** Fetch: memory placement lookup, two digest RPCs, one `upload-pack` on one replica. Push: placement lookup, tee to three voters plus S3 multipart, votes, one S3 claim per ref, apply. Ref listing for the API (branches, compare): `trib-spokesd` `ls-refs` on the replica chosen by digest. Pull request mergeability: background `MergePreview` transactions on `refs/pull/N/merge` in the `Background` lane. Maintenance: per replica, under a cell-wide concurrency budget, never two voters of one network at once.

### Identity and tokens

```rust
/// Prefixes and checksum follow GitHub's 2021 format: 30 base62 chars plus CRC32 (research/wire-census.md#29).
pub enum TokenKind { Classic, FineGrained, OAuth, UserToServer, Refresh, Installation }

pub enum Credential {
    Opaque { kind: TokenKind, secret: SecretBytes, crc_ok: bool },   // ghp_, github_pat_, gho_, ghu_, ghr_
    Installation(InstallationJwt),                                    // ghs_<app_id>_<jwt>, about 520 chars
    Basic { user: String, password: SecretString },                   // git over HTTPS
    SshKey(PublicKeyFingerprint),
    RunnerAssertion(SignedAssertion),                                 // runner OAuth JWT-bearer
}

/// Parsed and verified at the edge with no database read.
pub struct InstallationClaims {
    pub app: AppId,
    pub installation: InstallationId,
    pub repos: RepoScope,               // All or an explicit list of RepoId
    pub permissions: PermissionSet,
    pub job: Option<JobRef>,            // set for GITHUB_TOKEN
    pub key_id: SigningKeyId,           // cell-local key, rotated weekly, two keys live
    pub expires_at: Timestamp,          // at most 60 minutes after issue
}

pub enum Principal {
    User { id: UserId, scopes: ScopeSet, via: TokenKind },
    Installation(InstallationClaims),
    Runner { id: RunnerId },
    Anonymous,
}

pub enum OAuthClientKind {
    /// Seeded at install: gh, GCM, VS Code, Desktop. Public client, no consent screen.
    FirstParty { name: &'static str, legacy_callback: &'static str },
    ThirdParty { secret_hash: [u8; 32] },
}

pub trait Authenticator {
    async fn authenticate(&self, cred: Credential) -> Result<Principal, AuthError>;
}
/// Pure over a cached snapshot of grants; returns the 404-not-403 decision the wire needs.
pub fn authorize(p: &Principal, action: Action, on: &ResourceRef, grants: &Grants) -> Decision { todo!() }
```

Token parsing checks prefix and CRC32 before anything else, so malformed tokens never reach a cache or the database (`research/wire-census.md#29`). Opaque tokens map by SHA-256 hash to a principal in DSQL; `trib-edge` serves every lookup from an in-memory cache refreshed in the background, with revocations pushed through the outbox, so the database sees constant work, not a load spike on cache loss (`research/uptime.md#5`, `#47`).

Installation tokens and `GITHUB_TOKEN` use the stateless `ghs_<app_id>_<jwt>` form that github.com adopted on 2026-10-02 (`research/wire-census.md#30`), even though GHES has not. Verification needs only the cell's public keys, so a database outage cannot break app tokens, which is the failure GitHub had for 12 hours on a shared permissions cluster (`research/uptime.md#44`). Tools that run against github.com already handle the 520-character form, and Renovate keys only on the `ghs_` prefix (`research/clients.md#38`). Revocation is a short expiry plus a revocation list in the config push. Signing keys are generated per cell, stored KMS-encrypted in S3, decrypted once at process start, and held in memory, so KMS is never on the request path (`research/uptime.md` implication 2).

First-party OAuth clients are seeded with their hardcoded ids: gh `178c6fc778ccc68e1d6a`, GCM `0120e057bd645470c1ed`, VS Code `01ab8ac9400c4e429b23`, Desktop production `de0e3c7e9973e1c4dd77` and dev `3a723b10ac5575cc5bb9` (`research/clients.md#46`). They are public clients: the server accepts the embedded secrets gh and GCM send, accepts no secret from VS Code, registers `http://localhost/` as the legacy callback for gh and GCM, and enables device flow (`research/clients.md#8`, `#13`, `#22`). Web sign-in uses the tenant's SAML or OIDC IdP. Each install also has a pre-provisioned break-glass local admin whose credentials work without the IdP (`research/uptime.md` implication 25).

Authorization data (memberships, team grants, repository roles, installation grants) is read through a per-process `Grants` snapshot keyed by tenant, refreshed from DSQL every 30 s and on outbox invalidation. If DSQL is unreachable the snapshot keeps serving for up to 15 minutes, then fails closed. The `authorize` function is pure and returns 404 for unauthorized access to private resources (`research/wire-census.md#21`).

### Issues and pull requests

```rust
pub struct IssueId(u64);           // every id column is i64 in DSQL, allocated from the cell IdRange
pub struct IssueNumber(u64);       // per-repo sequence shared by issues and pull requests
pub struct PullRequestId(u64);
pub struct ReviewThreadId(u64);    // 64-bit from day one (research/uptime.md#44, Vitess overflow)

pub struct PullRequest {
    pub id: PullRequestId,
    pub repo: RepoId,
    pub number: IssueNumber,
    pub base: RefName,
    pub head: HeadRef,
    pub state: PrState,
    pub mergeability: Mergeability,
    pub row_version: u64,                     // ETag source and OCC guard
}
pub struct HeadRef {
    pub repo: RepoId,                         // a fork in the same network, or the base repo
    pub name: RefName,
    pub sha: Oid,                             // snapshot of the last ingested git event
    pub seq: RefSeq,                          // the fence seq that produced `sha`
}
pub enum PrState {
    Draft,
    Open,
    Closed { at: Timestamp, by: ActorId },
    Merged { at: Timestamp, by: ActorId, commit: Oid },
}
pub enum Mergeability { Unknown, Clean { preview: Oid, base_seq: RefSeq }, Conflicting, Blocked(Vec<Requirement>) }
pub enum MergeMethod { Merge, Squash, Rebase }

pub struct MergeRequest {
    pub pr: PullRequestId,
    pub method: MergeMethod,
    pub expected_head: Option<Oid>,           // GraphQL expectedHeadOid, REST `sha`
    pub title: Option<String>,
    pub message: Option<String>,
}

/// Pure policy over a consistent DSQL read: reviews, checks, protection, actor grants.
pub fn merge_allowed(pr: &PullRequest, reviews: &[Review], checks: &[CheckRun], rule: &ProtectionRule, actor: &Principal) -> Result<(), Vec<Requirement>> { todo!() }

pub trait PullRequests {
    async fn merge(&self, req: MergeRequest, actor: &Principal) -> Result<Merged, MergeError>;
    /// Idempotent on (repo, ref, seq). Called by the pr-sync consumer for every git event.
    async fn on_ref_moved(&self, repo: RepoId, name: &RefName, seq: RefSeq, new: Option<Oid>) -> Result<(), MetaError>;
}
```

Issues, comments, reviews, review threads, labels, milestones, timeline events, check suites, check runs, commit statuses, and protection rules are rows in the cell's DSQL cluster, owned by `trib-meta`. Every mutation goes through one `trib-meta` function that writes the change and its outbox rows in the same transaction. DSQL limits shape it: one transaction may change at most 3,000 rows and 10 MiB, isolation is Repeatable Read with OCC, and there are no triggers (`research/uptime.md#27`). So bulk operations (repository transfer, org-wide permission change, closing a milestone with 10,000 issues) run as chunked, idempotent outbox jobs, and every write path retries SQLSTATE 40001 within a budget. Issue numbers come from a per-repository counter row; concurrent creation in one repository conflicts and retries, which is acceptable at human and bot rates [inference].

Can pull request state diverge from refs? Yes, briefly, and the design makes the refs win. Refs are the truth for heads, bases, and merges. `HeadRef.sha` is a cache of the last ingested git event, tagged with its `RefSeq`, so `on_ref_moved` ignores stale or duplicate events by comparing seqs. Merge never trusts the cache: it passes `expected_head` and the base's old value into the `RefTxn`, so the fence rejects a merge computed against a moved branch. A pull request becomes `Merged` either from the merge API's own write or from `pr-sync` noticing that the head is reachable from the base after any push, which is also how GitHub marks manual merges. Mergeability previews are `Background` lane transactions on `refs/pull/N/merge` and are recomputed when either ref's seq changes.

Access patterns: list open pull requests for a repository (index on `(repo, state, updated_at)`), load one pull request with reviews and checks (point reads by `(repo, number)`), the checks UI (index on `(repo, head_sha)`), and timeline pagination by `(issue_id, id)` cursors. All are single-partition reads within one repository, which keeps OCC conflict scope small.

### Actions runs and jobs

```rust
pub struct RunId(u64);
pub struct JobId(u64);

pub enum RunState {
    Queued,
    Waiting { environment: EnvironmentId },   // protection rule approval
    InProgress,
    Completed(Conclusion),
}

pub enum JobState {
    Blocked { needs: Vec<JobId> },
    Queued { labels: LabelSet, since: Timestamp },
    Assigned { runner: RunnerId, message: JobMessageId, ack_by: Timestamp },
    Running { runner: RunnerId, renew_by: Timestamp },   // renewjob every 60 s extends this
    Completed { conclusion: Conclusion, at: Timestamp },
}
pub enum Conclusion { Success, Failure, Cancelled, Skipped, TimedOut }

pub enum JobEvent {
    NeedsSatisfied,
    Claimed { runner: RunnerId },
    Acquired,
    Renewed,
    Finished(Conclusion),
    LeaseExpired,
    CancelRequested,
}

/// Pure transition table. Each persisted transition is a DSQL OCC update on (job, row_version).
pub fn transition(state: &JobState, ev: JobEvent, now: Timestamp) -> Result<JobState, IllegalTransition> { todo!() }

/// Built by the engine, serialized by trib-actions-proto into AgentJobRequestMessage.
pub struct JobPlan {
    pub job: JobId,
    pub steps: Vec<TemplateToken>,            // runner evaluates step-level expressions
    pub context: PipelineContextData,
    pub endpoints: ServiceEndpoints,          // CacheServerUrl, ResultsServiceUrl, GenerateIdTokenUrl
    pub token: InstallationClaims,            // minted as ghs_ GITHUB_TOKEN
}

pub enum ActionSource {
    Upstream { repo: RepoId, commit: Oid },                          // bytes exactly as mirrored
    Shimmed { repo: RepoId, commit: Oid, shim: ShimVersion, digest: Sha256 },
}
/// Launch resolver: decides the bytes for every `uses:` (research/actions-protocol.md#17).
pub fn resolve_action(uses: &UsesRef, policy: &ShimPolicy, mirror: &MirrorIndex) -> Result<ActionSource, ResolveError> { todo!() }
```

The engine owns what the server must own (`research/actions-protocol.md#62`): trigger matching with path and branch filters, matrix expansion, `needs`, job-level `if`, concurrency groups, reusable workflows, environments and approvals, token permissions, OIDC, label matching, and building the job message. The runner owns step-level evaluation (`research/actions-protocol.md#61`). Runs and jobs are DSQL rows. A queued job is claimed by the broker with one OCC update, so two broker processes cannot assign one job twice. A `Running` job whose `renew_by` passes becomes `Completed { TimedOut }` through `LeaseExpired`, and an `Assigned` job whose `ack_by` passes returns to `Queued`. Access patterns: the broker reads queued jobs by `(tenant, label_set_hash, since)`; the UI reads jobs by `run_id`; the checks integration reads by `(repo, head_sha)`.

### Event outbox

```rust
pub struct EventId(u64);
pub struct Partition(u16);                    // hash(repo id) mod 256; per-repo order holds within it

pub enum Origin {
    Meta { txn_row_version: u64 },            // written in the same DSQL transaction as the change
    Git { txn: TxnId },                       // ingested from an S3 fence record, idempotent on txn
}

pub struct OutboxEvent {
    pub id: EventId,
    pub partition: Partition,
    pub tenant: TenantId,
    pub repo: Option<RepoId>,
    pub kind: EventKind,                      // push, pull_request, issues, check_run, workflow_job, ...
    pub payload: EventPayload,                // domain form; the webhook JSON is rendered at delivery
    pub origin: Origin,
    pub created_at: Timestamp,
}

/// One cursor row per (consumer, partition). Consumers never share a cursor.
pub trait Consumer: Send + Sync {
    const NAME: &'static str;
    /// Must be idempotent on event id. Errors retry with backoff; poison events park after 20 tries.
    async fn handle(&self, ev: &OutboxEvent) -> Result<(), ConsumerError>;
}

pub struct WebhookDelivery {
    pub guid: Uuid,                           // X-GitHub-Delivery, stable across redelivery
    pub hook: HookId,
    pub event: EventId,
    pub attempts: Vec<DeliveryAttempt>,       // status, duration, response excerpt
    pub retain_until: Timestamp,              // at least 3 days (research/wire-census.md#37)
}

/// Reads one fence record and writes git_events plus outbox rows. Safe to call any number of times.
pub async fn ingest_git_txn(key: &IntentKey, fence: &dyn FenceStore, meta: &Meta) -> Result<Ingested, IngestError> { todo!() }
```

There are two event origins and one outbox. Metadata changes write outbox rows in their own DSQL transaction (transactional outbox). Git changes cannot, because the push path deliberately does not touch DSQL. Instead every `RefTxn` writes an intent key `txidx/<hour>/<shard>/<txn>` to S3 before it claims the fence. `ingest_git_txn` resolves an intent: if the fence keys name the transaction, it inserts `git_events` (unique on `txn`) and the outbox rows; otherwise it drops the intent. It is triggered by a hint from the coordinator after the acknowledgment, and by a sweep that lists the current and previous hour's intent prefixes every 2 s. That is one ingest function with two triggers, and it closes Praefect's race where hooks fire before replication finishes (`research/git-storage.md#10`): a `push` event exists only for a fenced transaction.

Consumers: `pr-sync`, `workflows`, `webhooks`, `notifications`, `search-indexer`, `authz-invalidate`, `audit`. Each keeps its own cursor per partition (per-actor state, no shared cursor). Webhooks are at-least-once with a stable GUID, a 10 s receiver budget, a 25 MB cap, and delivery records for 3 days, behind the redelivery API (`research/wire-census.md#36`, `#37`). A DSQL outage delays events; it never loses one, because intents stay in S3 until ingested.

### Service and crate map

Deployable services (all Rust except `trib-web`):

- `trib-edge`: TLS and SSH termination with per-tenant certificates and host keys, token parsing, auth cache, rate limits, GHES headers, and routing to the services below.
- `trib-git`: git smart HTTP and SSH frontend, replica selection by digest, push coordinator, and the merge and internal-commit entry point.
- `trib-spokesd`: the storage node daemon that runs upstream git, installs `trib-vote` as the reference-transaction hook, applies commits, serves digests, and runs local maintenance and hydration.
- `trib-placement`: sole owner of the placement schema, publisher of placement snapshots, and host of the sweepers.
- `trib-api`: REST under `/api/v3` and GraphQL under `/api/graphql`, plus LFS, OAuth, `/meta`, `/status`, and the web app's internal endpoints, in one binary.
- `trib-events`: git intent ingestion and every outbox consumer, including webhook delivery.
- `trib-actions`: workflow engine, runner registration, broker, run service, launch resolver, results, cache, artifacts, and OIDC issuer.
- `trib-web`: Node server that renders the React app with SSR and calls `trib-api`.
- `trib-runners`: managed-mode hosted runner fleet controller, deployed in separate runner accounts, outside any cell.
- `trib-control`: managed-mode global control plane for signup, cell assignment, certificates, and DNS, never on a request path.

Crates: `trib-ids` (ids, `IdRange`, node id codec), `trib-wire` (GHES headers, errors, pagination, media types, ETags), `trib-auth` (token formats, `Authenticator`, `authorize`), `trib-repo` (`RefTxn`, `ReplicaSet`, git process wrapper), `trib-quorum` (votes, `FenceStore` impls, `choose_read_replica`), `trib-meta` (DSQL access, OCC retry, outbox writes), `trib-rest` (route table generated from the pinned `ghes-3.22.json`), `trib-graphql` (async-graphql schema generated from the GHES 3.22 SDL), `trib-actions-proto` (job message, template tokens, context data converters), `trib-workflow` (Rust port of the MIT C# `GitHub.Actions.WorkflowParser`, `research/actions-protocol.md#60`), `trib-blobdialect` (Azure Block Blob dialect over S3 multipart), `trib-shim` (toolkit bundle transforms), `trib-config` (signed config bundles with last-known-good). Internals sit behind `pub(crate)`; only the traits and types above are public.

AWS services on the request path: NLB, EC2 and ECS data planes, S3, Aurora DSQL, and Route 53 resolution. Not on the request path: KMS (decrypt at process start only, SSE-S3 for buckets by default), STS (instance credentials from IMDS), IAM, EC2 launch, Route 53 changes, ACM, and every control-plane API (`research/uptime.md` implication 1). Rust does not pay in pack generation (upstream git), in SSR (Node), or in search (OpenSearch), so those are not Rust.

## Wire compatibility

Targets: GHES 3.22 REST and GraphQL schemas, REST versions `2022-11-28` and `2026-03-10` (`research/wire-census.md#11`, `#12`). "Schemathesis on X" means a contract run of the pinned `ghes-3.22.json` operations in category X, with responses replayed through `@octokit/rest` typed calls and `octokit.paginate` (`research/clients.md` conformance table).

| Surface | Tier | Oracle | Approach |
| --- | --- | --- | --- |
| `git-http` | T0 | `GCM_TRACE=1 git clone https://<host>/o/r.git` then `git push`, with git 2.39 and 2.5x | `trib-git` runs upstream `upload-pack` and `receive-pack --stateless-rpc` on replicas; with and without `.git`; dumb HTTP refused (`research/wire-census.md#39`) |
| `git-ssh` | T0 | `git clone git@<host>:o/r.git`; `ssh -T git@<host>` greets and exits 1 | SSH in `trib-edge` with a per-tenant host key that moves with the tenant; same `trib-git` path |
| `git-protocol-v2` | T0 | `GIT_TRACE_PACKET=1 git ls-remote https://<host>/o/r` matches github.com's capability lines | Advertise `ls-refs=unborn`, `fetch=shallow wait-for-done filter`, `server-option`, `object-format=sha1` (`research/wire-census.md#41`); `bundle-uri` and `packfile-uris` later |
| `lfs` | T0 | `git lfs track '*.bin' && git push && git lfs fetch --all && git lfs locks` over HTTPS and SSH | Batch API in `trib-api`, S3 presigned transfers, locks in DSQL, `git-lfs-authenticate` over SSH |
| `rest:actions` | T0 | `gh run list`, `gh run view --log`, `gh secret set`; ARC scale set completes a job | Runs, jobs, logs, secrets (sealed box), runners, registration tokens first; the other operations by T1 |
| `rest:activity` | T1 | Schemathesis on activity; `gh status` | Notifications, stars, watching from DSQL |
| `rest:agent-automations` | Out | None | dotcom-only (`research/wire-census.md#4`); no GHES client calls it |
| `rest:agent-tasks` | Out | None | dotcom-only |
| `rest:agents` | Out | None | dotcom-only |
| `rest:announcement-banners` | T2 | Schemathesis on announcement-banners | GHES-only admin feature |
| `rest:apps` | T0 | Renovate with app auth; Argo CD `--github-app-enterprise-base-url` repo add | App JWT auth, installations, `ghs_` token minting |
| `rest:billing` | T2 | Schemathesis on billing | One GHES operation, backed by metering rows |
| `rest:branches` | T0 | `TestAccGithubBranchProtection` in terraform-provider-github (v3 resource) | Protection rules in DSQL, enforced in the push coordinator and merge policy |
| `rest:campaigns` | Out | None | dotcom-only |
| `rest:checks` | T0 | Jenkins Branch Source posts checks; Actions check runs render in `gh pr checks` | Check suites and runs in DSQL, outbox events |
| `rest:classroom` | Out | None | dotcom-only |
| `rest:code-quality` | Out | None | dotcom-only |
| `rest:code-scanning` | T2 | `github/codeql-action` upload-sarif on a Tributary runner | SARIF storage and alert rows |
| `rest:code-security` | T2 | Schemathesis on code-security | Configuration rows only |
| `rest:codes-of-conduct` | T3 | Schemathesis on codes-of-conduct | Static data |
| `rest:codespaces` | Out | None | dotcom-only |
| `rest:collaborators` | T0 | `TestAccGithubRepositoryCollaborator` in terraform-provider-github | Grants in DSQL, authz invalidation via outbox |
| `rest:commits` | T0 | `gh api repos/o/r/commits`; Renovate commit lookups | gix object reads on a replica; diffs via `git diff-tree` |
| `rest:copilot` | Out | None | dotcom-only |
| `rest:copilot-spaces` | Out | None | dotcom-only |
| `rest:credentials` | Out | None | dotcom-only |
| `rest:dependabot` | T2 | dependabot-core `dependabot update -f job.yml` against the host | Alerts and secrets; updates run as Actions jobs (`research/clients.md#41`) |
| `rest:dependency-graph` | T2 | Schemathesis on dependency-graph | Manifest parsing worker |
| `rest:deploy-keys` | T0 | `argocd repo add git@<host>:o/r.git --ssh-private-key-path` | Keys in DSQL, served by the SSH auth cache |
| `rest:deployments` | T1 | Schemathesis on deployments; Actions `environment:` jobs create deployments | Rows plus outbox |
| `rest:emojis` | T3 | Schemathesis on emojis | Static data |
| `rest:enterprise-admin` | T1 | Schemathesis on enterprise-admin; `gh api /enterprise/settings/license` | GHES admin API over cell admin rows; Manage API on the admin port |
| `rest:enterprise-teams` | T2 | Schemathesis on enterprise-teams | Rows |
| `rest:gists` | T2 | `gh gist create` and `gh gist view` | Gists as repositories in their own networks |
| `rest:git` | T0 | Renovate commits through the git data API; `gh api repos/o/r/git/refs` | Blob, tree, and commit creation via `trib-git` internal commit; ref writes are `RefTxn` |
| `rest:gitignore` | T3 | Schemathesis on gitignore | Static data |
| `rest:interactions` | Out | None | dotcom-only |
| `rest:issues` | T0 | cli/cli acceptance tests (`go test -tags=acceptance ./acceptance`) issue scripts | DSQL rows, outbox |
| `rest:licenses` | T2 | Schemathesis on licenses | Licensee-equivalent detection worker |
| `rest:markdown` | T1 | Schemathesis on markdown | comrak with GitHub extensions |
| `rest:meta` | T0 | GCM auth-mode probe; `gh auth login` reads `installed_version` | `GET /`, `/meta`, `/octocat`, `/zen` per GHES schema |
| `rest:metrics` | T2 | Schemathesis on metrics | Community profile computed from rows |
| `rest:migrations` | T2 | Schemathesis on migrations | Export and import jobs through the outbox |
| `rest:oauth-authorizations` | T3 | Schemathesis on oauth-authorizations | GHES-only legacy API, deprecated upstream |
| `rest:orgs` | T0 | `TestAccGithubOrganization*` and Argo CD SCM provider org listing | DSQL rows |
| `rest:packages` | T2 | Schemathesis on packages | Metadata over the registries below |
| `rest:pages` | T2 | Schemathesis on pages | Builds as Actions workflows |
| `rest:private-registries` | T3 | Schemathesis on private-registries | Rows |
| `rest:projects` | T2 | Schemathesis on projects | Projects v2 rows; gh uses GraphQL for projects |
| `rest:pulls` | T0 | cli/cli acceptance PR scripts; Jenkins multibranch PR discovery | DSQL rows, merge through `RefTxn` |
| `rest:rate-limit` | T0 | `HEAD /api/v3/rate_limit` probe from VS Code Pull Requests | Edge counters; limits off by default like GHES (`research/wire-census.md#19`) |
| `rest:reactions` | T1 | Schemathesis on reactions | Rows |
| `rest:releases` | T0 | `gh release create v1 ./a.tgz` and `gh release download` | Rows plus S3 assets |
| `rest:repos` | T0 | `TestAccGithubRepository` in terraform-provider-github; `gh repo create` | Rows plus network creation through placement |
| `rest:search` | T1 | `gh search issues` and `gh search code` | OpenSearch, fed by the outbox, fails soft |
| `rest:secret-scanning` | T2 | Schemathesis on secret-scanning | Pattern scan on push events |
| `rest:security-advisories` | T3 | Schemathesis on security-advisories | Rows |
| `rest:teams` | T0 | `TestAccGithubTeam` in terraform-provider-github | Rows, authz invalidation |
| `rest:users` | T0 | cli/cli acceptance `gh api user`; GCM user lookup | Rows |
| `graphql` | T0 | Static validation of every VS Code PR `.gql`, JetBrains `.graphql`, Renovate, terraform `githubv4`, and gh query against our introspection, then the cli/cli acceptance suite | async-graphql on the GHES 3.22 SDL, `extensions.code` errors including `undefinedField` |
| `webhooks` | T0 | Argo CD refresh on push; Jenkins multibranch trigger; HMAC verified by `@octokit/webhooks` | Outbox consumer, 78 GHES events by T1, T0 for push, pull_request, issues, issue_comment, check_run, check_suite, status, create, delete, ping, workflow_run, workflow_job, installation |
| `oauth-web-flow` | T0 | GitHub Desktop enterprise sign-in (Playwright for Electron); `gh auth login --web` | `/login/oauth/authorize` and `/access_token`, seeded first-party clients |
| `oauth-device-flow` | T0 | `gh auth login --hostname <host>`; VS Code `getSession('github-enterprise')` with device code | `/login/device/code` and polling per `research/wire-census.md#38` |
| `github-apps` | T0 | Renovate and Argo CD with GitHub App credentials | Manifest flow, JWT auth, installation tokens |
| `tokens` | T0 | Renovate token detection by prefix; gh reads `X-Oauth-Scopes` | All six prefixes, CRC32, stateless `ghs_` |
| `actions-runner-protocol` | T0 | Pinned `actions/runner` registers, runs a job, and completes; ARC `gha-runner-scale-set` scales from zero | `use_v2_flow: true`: broker, run service, launch, results |
| `actions-workflow-syntax` | T0 | Differential test of `trib-workflow` against the C# parser and `@actions/workflow-parser` over a corpus of public workflows | Rust port of `GitHub.Actions.WorkflowParser` |
| `actions-cache` | T0 | `actions/cache@v4`, `@v5`, `@v6` restore and save on a Tributary runner | Cache v1 server always; cache v2 server for shimmed bundles |
| `actions-artifacts` | T0 | `upload-artifact@v3.2.2` and `@v4` to `@v7` plus `download-artifact` round trip | Legacy pipeline API plus twirp v4 with Azure block dialect, v4+ via the shim |
| `actions-oidc` | T0 | `aws-actions/configure-aws-credentials` assumes a role trusting `https://<host>/_services/token` | Per-cell signing key, GHES issuer and legacy `sub` (`research/actions-protocol.md#42`) |
| `actions-logs-results` | T0 | `gh run view --log` after a run; live log in the web UI | Results twirp with Tributary-signed append URLs into S3 |
| `packages-container` | T1 | `docker login <host> && docker push <host>/o/img:tag` | OCI distribution on S3 |
| `packages-npm` | T2 | `npm publish --registry https://<host>/_registry/npm/` | Registry over S3 |
| `packages-maven` | T2 | `mvn deploy` to `https://<host>/_registry/maven/` | Registry over S3 |
| `packages-nuget` | T3 | `dotnet nuget push` to the NuGet v3 endpoint | Registry over S3 |
| `packages-rubygems` | T3 | `gem push --host https://<host>/_registry/rubygems/` | Registry over S3 |
| `pages` | T2 | Publish with `actions/deploy-pages` and fetch the site | Static sites from S3 on an isolated subdomain |
| `codeload-archives` | T0 | `gh release download --archive tar.gz`; the runner downloads action tarballs | `git archive` on a replica, cached in S3 by (repo, commit) |
| `raw-content` | T0 | `curl -L https://<host>/o/r/raw/main/README.md` follows to the raw origin | `raw.<host>` with subdomain isolation, `/raw/` path without |
| `uploads-release-assets` | T1 | `gh release upload v1 ./b.tgz` | `https://<host>/api/uploads` to S3 multipart |
| `meta-endpoint` | T0 | Desktop `HEAD /api/v3/meta`; Dependabot `GET /status`; VS Code PR `GET /api/v3/status` | `installed_version`, `verifiable_password_authentication`, `GitHub lives!` body |

**Cross-cutting conventions** (`research/wire-census.md#11` to `#38`). `trib-wire` is the only place these live, and every handler returns domain types that it renders. Every response, including git smart HTTP, `HEAD` on repository URLs, `/status`, and errors, carries `X-GitHub-Request-Id` (a GitHub-shaped hex id) and `X-GitHub-Enterprise-Version: 3.22.0` (`research/clients.md` implication 2). `X-GitHub-Api-Version` defaults to `2022-11-28`, accepts `2026-03-10`, returns 400 for unknown versions, and echoes `x-github-api-version-selected`; the per-version differences (`research/wire-census.md#13`) are a version enum matched in the renderer. Pagination uses the `link` header with `/repositories/{id}/...` URLs, a default page of 30, and silent clamping at 100. ETags are weak and derived from `row_version` plus a representation hash; a 304 does not count against rate limits. Errors use `basic-error` and `validation-error` bodies with `status` as a string, 404 for unauthorized private resources, unsupported methods, and trailing slashes, and a `documentation_url` under `https://docs.github.com/enterprise` (`research/clients.md` implication 4). Requests without `User-Agent` are rejected. Every handler has a 10 s deadline. Renames write an alias row; the API answers 301 to `/repositories/{id}` and git serves the old name with a 200, and creating a repository at the old name deletes the alias (`research/wire-census.md#23`). `GET /api/v3/` returns the root hypermedia document with `rate_limit_url`, and unauthenticated 401s carry `X-GitHub-Media-Type` (`research/clients.md#43`).

**Host shape and hostname heuristics.** One bare hostname on port 443 serves the web app, `/api/v3`, `/api/graphql`, `/api/uploads`, `/login/oauth/*`, `/login/device/code`, the runner services under `/_services/`, and git. gh rejects ports and the runner drops non-default ports (`research/clients.md#4`, `research/actions-protocol.md#4`). Subdomain isolation (`raw.`, `codeload.`, `uploads.`, `media.`, `pages.`, `containers.`) is on by default in every mode, with a wildcard certificate per tenant (`research/wire-census.md#33`); GCM strips `raw.` and `gist.` before probing (`research/clients.md#12`). Self-hosted and BYOC docs recommend a hostname whose first label is `github.` so GCM and Desktop classify it without a network probe (`research/clients.md` implication 9). Managed hostnames are `<slug>.tributary.dev`, which the probes classify through `X-GitHub-Request-Id`. The toolkit and the runner classify every Tributary host as GHES (`research/actions-protocol.md#1`, `#43` to `#46`); "Actions" says what Tributary does about it. JIT runner configs always set `GitHubUrl` so the runner does not infer github.com (`research/actions-protocol.md#2`).

**Advertised version.** The GHES version Tributary claims is a release attribute derived from the conformance gate: a release may advertise 3.22.0 only if the 3.22 REST and GraphQL schema checks pass. Bumping to 3.23 is a release event, never a config flag (`research/clients.md` implication 3).

**OAuth client ids.** The five first-party ids in "Identity and tokens" are seeded rows that every install creates at bootstrap. They cannot be deleted, only disabled.

**Id ranges.** Every numeric id is i64 in DSQL and comes from the cell's `IdRange`. Processes lease blocks of 10,000 ids per entity type from an `id_blocks` row with an OCC update. Ranges are disjoint across cells and stay below 2^53, with an alarm at 50% of any range (`research/uptime.md` implication 15). GraphQL exposes both `databaseId` and `fullDatabaseId: BigInt` as a string (`research/wire-census.md#28`).

**Node ids.** Tributary emits GitHub's newer format, a type prefix, `_`, and base64url of a MessagePack array: `[0, id]` for top-level objects and `[0, repo_id, id]` for repository-scoped objects, as in `CR_` (`research/wire-census.md#27`). Because a tenant lives in one cell, the node id needs no cell field. The repository id in the payload lets `node(id:)` go straight to the right partition. The decoder also accepts the legacy base64 `NN:TypeNameID` form on input (`research/wire-census.md#26`).

## Availability

### SLOs

The definition copies GitHub's SLA: a minute counts as bad when more than 5% of requests for that journey fail, or when external probes see it down (`research/uptime.md#51`). Budgets are per 30-day month (43,200 minutes).

| Journey | Monthly target | Error budget (min) | Measurement |
| --- | --- | --- | --- |
| git clone and fetch | 99.99% | 4.3 | Probes from a second Region every 10 s per cell (HTTPS and SSH clone of a canary repo), plus `trib-git` status codes |
| git push | 99.95% | 21.6 | Probe pushes to a canary repo every 10 s, plus coordinator outcomes excluding client-caused rejections |
| Pull request review and merge | 99.95% | 21.6 | Probe that opens, reviews, and merges a PR every minute, plus merge API outcomes |
| REST and GraphQL API | 99.95% | 21.6 | Edge logs for `/api/v3` and `/api/graphql` excluding 4xx, plus probe queries |
| Web page load | 99.9% | 43.2 | Real-user timing beacons and synthetic page loads of repo, PR, and file views |
| Actions job start | 99.9% | 43.2 | Fraction of jobs with first log line within 5 min of trigger, GitHub's Actions definition (`research/uptime.md#51`) |
| Webhook delivery | 99.9% | 43.2 | Fraction of deliveries attempted within 60 s of the event |
| Acknowledged push durability | 100% | 0 | Daily replay: every fence record of the past day resolves to objects in S3 and on a voter (`git cat-file -e`) |

### Failure modes

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| Repository replicas | instance | Networks with a voter on the node lose one copy; no requests fail | Frontends mark the node offline after 3 failed requests; W=2 and R=2 continue on the other two; `repair` rehydrates onto headroom of existing nodes from S3 |
| Repository replicas | az | One voter of every network; a second failure elsewhere makes the networks it shares unavailable, about 1/30 of the cell at 30 nodes per AZ | Quorum continues at 2 of 3; NLB zonal shift; no re-replication storm for 6 h; S3 is the third durable copy |
| Repository replicas | region | Every voter in the cell | Regional: DR learner promoted and two more replicas hydrated from the replicated bucket on pre-provisioned spares, RPO equals CRR lag, RTO 60 min. Dual-Region: peer learners promoted, RPO 0, RTO 5 min |
| Repository replicas | control-plane | None; nodes are running instances with local data | No EC2 launch on any recovery path; hot spares pre-launched per AZ; nodes held at 66% disk (`research/uptime.md#2`) |
| Repository replicas | bad-deploy | At most one voter per network at a time | `roll:storage-fleet` updates one node per AZ, checks votes and digests after each node, and halts on mismatch |
| Repository replicas | bad-config | One canary node, then halt | Config bundle validated by a pure check, applied to one node first, last-known-good kept on disk |
| Repository replicas | noisy-tenant | The tenant's shuffle shard of nodes | Per-network and per-tenant `pack-objects` concurrency caps in `trib-git`; tenants placed on shuffle shards; large tenants on dedicated cells |
| S3 cell bucket | instance | One S3 request | SDK retries inside the request deadline with a retry budget (`research/uptime.md` implication 9) |
| S3 cell bucket | az | None | S3 Standard stores data in at least three AZs (`research/git-storage.md#54`) |
| S3 cell bucket | region | Regional impairment: pushes, merges, LFS writes, and Actions blobs fail; clones and fetches continue from disks. Region loss: home copies gone | Reads never touch S3; CRR copy in DR Region; Dual-Region tier writes both buckets before ack |
| S3 cell bucket | control-plane | None at runtime | Buckets, policies, and replication pre-created at install; no bucket-config call at runtime (`research/uptime.md#16`) |
| S3 cell bucket | bad-deploy | One cell, because cells deploy in waves | `FenceRecord.format` versioned; writers emit a new format only after every reader in the cell understands it |
| S3 cell bucket | bad-config | One cell | Bucket policy denies deletes under `refseq/` and `pack/` to every role except `checkpoint`; lifecycle rules never match fence keys |
| S3 cell bucket | noisy-tenant | The tenant's prefixes | Keys spread by network id hash; per-tenant push rate limits at the edge |
| DSQL cell cluster | instance | None | DSQL is active-active across AZs with no primary to fail over (`research/uptime.md#26`) |
| DSQL cell cluster | az | None | Same; capacity sized for two AZs |
| DSQL cell cluster | region | Regional tier: a regional DSQL impairment stops PR, issue, and API writes in the cell while git keeps serving; Region loss loses the cluster | Regional: warm DR cluster fed by CDC, RPO seconds. Dual-Region: peer endpoint keeps serving, RPO 0 |
| DSQL cell cluster | control-plane | None | No database control-plane call on recovery; connections pre-opened and rotated before the 1 h limit; IAM auth tokens signed locally [inference] |
| DSQL cell cluster | bad-deploy | One cell | Expand and contract migrations; every query has a deadline below its caller's; canary AZ first |
| DSQL cell cluster | bad-config | One cell | Pool sizes validated against the 10,000 connection cap (`research/uptime.md#27`); config staged per cell |
| DSQL cell cluster | noisy-tenant | The tenant's hot rows | OCC retries with a per-tenant budget; per-tenant write token buckets; bulk work chunked under 3,000 rows through the outbox |
| Placement map | instance | One frontend restarts | Frontend loads the latest signed snapshot from S3 at start |
| Placement map | az | None | Snapshot in S3; placement rows in DSQL |
| Placement map | region | Same as the cell | Placement rows replicate with DSQL; DR learners already listed as learners |
| Placement map | control-plane | None | No AWS API involved |
| Placement map | bad-deploy | Moves stop | Epoch-fenced writes; sweeper velocity caps of one node per AZ and 5% of networks per hour |
| Placement map | bad-config | Rejected before apply | Placement rules pass a pure validator that refuses any rule breaking AZ diversity |
| Placement map | noisy-tenant | The tenant's shard | Rebalancing of a hot tenant stays inside its shuffle shard |
| Edge auth cache | instance | Requests on one edge node during refill | Refill coalesced per key; static capacity at 66% (`research/uptime.md#14`) |
| Edge auth cache | az | One third of edges | Zonal shift; other AZs hold warm caches |
| Edge auth cache | region | Same as the cell | Rebuilt from DSQL in DR; stateless `ghs_` tokens verify from public keys alone |
| Edge auth cache | control-plane | None | No STS or IAM call; signing keys decrypted once at start |
| Edge auth cache | bad-deploy | One AZ of one cell | Token parsing is a pure crate with golden tests for every prefix; AZ canary |
| Edge auth cache | bad-config | Rejected | TTL bounds compiled in; out-of-range config refused |
| Edge auth cache | noisy-tenant | That tenant | Per-token and per-tenant token buckets; per-tenant cache memory cap |
| Actions broker | instance | Long-poll sessions on that process | Runners recreate sessions on `RunnerSessionInvalid`; assignments are DSQL rows; unacked jobs requeue at `ack_by` |
| Actions broker | az | One third of sessions | Zonal shift; runners reconnect |
| Actions broker | region | In-flight jobs fail | Queued runs survive in the DR or peer DSQL |
| Actions broker | control-plane | New hosted capacity cannot launch; dispatch continues | Warm runner pools pre-launched; launches only add capacity |
| Actions broker | bad-deploy | Actions only, core untouched | `trib-actions` deploys separately after core services, gated by the pinned runner conformance run |
| Actions broker | bad-config | Actions only | Label routing config validated against registered runners |
| Actions broker | noisy-tenant | That tenant's queue | Per-org concurrency limits; weighted fair queuing across orgs |
| Search index | instance | Search latency | OpenSearch multi-AZ domain |
| Search index | az | Search capacity | Replicas across AZs |
| Search index | region | Search unavailable until rebuilt | Derived data, rebuilt from DSQL and git in DR |
| Search index | control-plane | None on queries | Domain pre-created |
| Search index | bad-deploy | Search only | New mappings build a new index; alias flip (`research/byoc-grounding.md#41`) |
| Search index | bad-config | Search only | Staged per cell |
| Search index | noisy-tenant | Search for that tenant | Per-tenant query cost caps; search failure never reaches git, PR, or REST list endpoints (`research/uptime.md#49`) |

**Region drill.** Managed mode flips one internal dogfood cell to its DR or peer Region monthly and one customer-facing canary cell quarterly, using ARC routing controls only. Replica loss and AZ evacuation are drilled weekly through zonal autoshift practice runs, and a failed practice run blocks deploys (`research/uptime.md#21`, implication 20).

### Change safety

**Deploys.** Every change ships as a signed, content-addressed release in the vendor's registry (`research/byoc-grounding.md#12` to `#15`). Managed order: dogfood cell, then one AZ of one canary cell, then that cell's other AZs, then waves of cells through <vendor-fleet-app> gates with a 60-minute soak (`research/byoc-grounding.md#31`, `#32`), then BYOC waves. Inside a cell, each stateless service runs as three ECS services, one per AZ, and rolls one AZ at a time, so no two AZs run a new build on the same step (`research/uptime.md#4`). An SLO burn rate above 10x during a step aborts that cell automatically and rolls the AZ back, which is the in-install abort the vendor machinery lacks (`research/byoc-grounding.md` reuse items 10, 11).

**Storage nodes.** A new stage kind, `roll:storage-fleet`, handles them (`research/byoc-grounding.md` reuse item 7). Per node: stop routing new reads to it, quiesce long clones as Spokes does (`research/git-storage.md#3`), restart `trib-spokesd` and git in place on the same instance (instance store survives a reboot, `research/git-storage.md#51`), verify digests against the other voters, then move on. One node per AZ at a time, never two voters of one network. AMI or kernel changes replace instances from the hot-spare pool and rehydrate from S3 at the sweeper's capped rate. The stage reads new observer routes (replica lag, quorum health per network group, oldest unapplied fence record, S3 intent backlog, disk headroom) as gate checks next to `components-healthy` (`research/byoc-grounding.md` reuse item 8).

**Config and feature flags.** Config is a signed bundle in S3, versioned and written with If-Match, and rolled out on the same cell and AZ path as code. Services validate a bundle with a pure check, apply it, and keep last-known-good on disk; a bundle that fails validation is never applied (`research/uptime.md` implications 11, 16). Feature flags live in the same bundle, so there is one path for config. Certificate and key expiry alarms fire at 30 days.

**Schema migrations.** Expand and contract only. Every release reads and writes the schema of N-1 and N+1. The expand phase is not a rollback barrier. Only the contract phase is, which extends the vendor's barrier model instead of making every change one-way (`research/byoc-grounding.md` reuse item 9). DSQL allows one DDL per transaction and separates DDL from DML (`research/uptime.md#27`), so the migration runner applies one statement per step and records a marker row. Backfills run as chunked outbox jobs under 3,000 rows.

**Zero-downtime upgrades in every mode.** All three modes run the same topology shape: three AZs, per-AZ stateless services, three or more storage nodes, one DSQL cluster. So all three upgrade the same way, through the same stages, with no maintenance mode, unlike GHES and GitLab (`research/uptime.md#53` to `#56`). Version skips are bounded: a release declares the oldest release it upgrades from, and CI tests every pair in that window.

## Deployment modes

| Varies | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Operator | Vendor | Vendor through the deploy role only | Customer |
| AWS account | Vendor cell accounts | Customer's dedicated account | Customer's dedicated account |
| Cells per Region | Many, tenants assigned by `trib-control` | One | One |
| Hostname | `<slug>.tributary.dev` or a custom domain | Customer domain | Customer domain |
| Certificates | ACME issued by `trib-control` | Customer ARN or delegated ACME | Customer ARN or delegated ACME |
| Default durability tier | Regional with DR learner; Dual-Region offered | Customer choice | Regional without DR learner |
| Storage instances | i8g instance store | i8g instance store | i8g, or m-class with gp3 |
| Hosted runners | Vendor runner accounts | Optional runner fleet in a second customer account | Customer runners, ARC or VMs |
| Upgrade trigger | Vendor waves | Vendor waves with a customer window gate | Customer runs `tributary-deploy` |
| Id range | Assigned by `trib-control` | Assigned by the vendor registry | Random range, checked on any later import |
| Telemetry | Full | Observer role routes only | Optional |

Everything else is identical: the release bundle, binaries, schema, topology shape, storage protocol, and wire behavior.

Reuse from `research/byoc-grounding.md`: the release registry with create-only S3 and KMS P-256 signing (findings 12 to 15), the bootstrap trust pattern with ExternalId, permissions boundary, and observer role (findings 3 to 7), the target document and `operations` posture object (findings 8, 11), the deployer image (finding 16), deploy events (finding 30), and <vendor-fleet-app> waves, gates, and rollback-as-a-deploy (findings 31 to 35). A managed cell is an install to <vendor-fleet-app>, which answers its missing placement concept (reuse item 13).

What Tributary adds: a `product` field in the target, the record, and the contract export so <vendor-fleet-app> can read two contracts (reuse item 12); a Tributary bootstrap with `trib-` role prefixes and PassRole for EC2 Auto Scaling (reuse item 5); the `roll:storage-fleet` stage (reuse item 7); storage observer routes and gate checks (reuse item 8); expand and contract as a release attribute with barriers only on contract (reuse item 9); per-AZ ECS services with automatic in-install abort (reuse items 10, 11); and the storage AMI inside `release.json` instead of shared out of band (finding 46). Storage node rollout sits outside OpenTofu, which owns only the fleet's shape (reuse item 15). Tributary requires its own dedicated account and does not share one with another vendor product (reuse item 6).

## Actions

**Workflow engine.** `trib-workflow` is a Rust port of the MIT C# `GitHub.Actions.WorkflowParser`, including `MatrixBuilder`, reusable workflow loading, and permissions (`research/actions-protocol.md#60`), using the shared `workflow-v1.0.json` as data. `trib-actions-proto` ports the template-token and context-data JSON converters (`research/actions-protocol.md#19`). Two oracles check it on every upstream change: the C# parser itself, run in CI, and `@actions/workflow-parser` (`research/actions-protocol.md#59`). The engine is an outbox consumer, so a workflow trigger can never fire for an unfenced push.

**Runner protocol (fact 5).** The registration response sets `use_v2_flow: true`: runner-admin registration, broker, run service, launch service, and results (`research/actions-protocol.md#6`, `#12`, `#16`, `#17`, `#22`). Reasons: it needs no VSS location service and no GUID-addressed resources; it is the family GitHub actively develops, while the legacy family is frozen and can be removed in any runner release (`research/actions-protocol.md` risk ratings for B); and its job, log, and annotation flow is smaller. Tributary pins a tested runner version per release, pushes it with `RunnerRefresh`, and refuses older runners with `RunnerVersionTooOld` (`research/actions-protocol.md#25`). A runner upgrade is a conformance gate. ARC compatibility (`_apis/runtime/runnerscalesets`, sessions, `acquirejobs`, `generatejitconfig`) supports a pinned ARC range (`research/actions-protocol.md#30`). Whether GHES 3.22 itself uses the broker is unknown (`research/actions-protocol.md` open question 1); it does not matter, because the server picks the family.

**The toolkit GHES gate (fact 4).** Every Tributary host is GHES to the toolkit, so `upload-artifact@v4` and later throw before any network call and `actions/cache` uses cache v1 (`research/actions-protocol.md#43` to `#49`). Tributary does three things.

1. It implements the GHES subset fully, so unshimmed bytes work as on GHES: cache v1 (`_apis/artifactcache`), the legacy artifact pipeline API for `upload-artifact@v3.2.2`, and checkout and setup actions.
2. It implements the newer services too: cache v2 and artifact v4 twirp at `ACTIONS_RESULTS_URL`, with `trib-blobdialect` accepting Put Blob, Put Block, Put Block List, and `comp=appendblock`, returning `x-ms-request-id`, and mapping in-order 8 MiB and 64 MiB blocks onto S3 multipart parts (`research/actions-protocol.md#35`, `#37`, `#39`, implication 6). Downloads are presigned S3 GETs, because non-Azure hosts get a plain GET (`research/actions-protocol.md#36`).
3. The launch resolver serves a shimmed bundle for an allow-list of first-party actions (`actions/upload-artifact`, `actions/download-artifact`, `actions/cache`, and actions that bundle `@actions/cache` or `@actions/artifact`). The shim is one published, deterministic transform: it replaces the body of the bundled `isGhes()` with `return false` and nothing else, the technique Gitea's runner uses across 118 bundles (`research/actions-protocol.md#57`). The vendor computes shimmed digests at release time and signs them in the release record. Every run records, per step, the upstream commit, the shim version, and the served digest, and the job log prints them.

SHA-pinned actions: a `uses: actions/upload-artifact@<sha>` gets the shimmed bundle only if that commit is reachable from an upstream release tag in the mirror and the pair (commit, shim version) is in the signed digest list. The integrity promise of a SHA pin, that the code is what the upstream project published at that commit, holds up to one reviewed, signed transform. An enterprise or org policy `actions_toolkit_shim: off` serves exact upstream bytes for everything, with the GHES behavior that follows. Third-party actions are never modified. If a new upstream bundle does not match the shim's pattern, the resolver serves it unmodified and annotates the run with a warning. Upstream toolkit PR #2123 and issue #2439 would remove the need for the shim if they landed with a vendor-neutral switch (`research/actions-protocol.md#51`).

**Hosted runner fleet (managed).** Runners live in separate per-Region runner accounts with no network path into any cell VPC. They reach the cell only through its public hostname with job-scoped `ghs_` tokens and runner credentials, exactly like a customer's own runner. Each job runs in a Firecracker microVM on pre-launched metal hosts, so job start is a data-plane operation on existing capacity and an EC2 launch outage reduces new capacity, not dispatch (`research/uptime.md#33`). Images come from the MIT runner-images provisioning scripts with a new `amazon-ebs` Packer source, starting with Ubuntu 24.04 on x64 and arm64 (`research/actions-protocol.md#63` to `#65`). Actions is not in the core availability set: a broker or runner outage never touches git, pull requests, or the APIs.

**Cache, artifacts, logs.** Cache entries are S3 objects indexed in DSQL by repository, key, version, and scope, with a 10 GB per-repository LRU. Artifacts are S3 objects with retention rows. Logs use the results service: Tributary-signed URLs that accept append blocks, buffered into S3 multipart objects, plus a live WebSocket feed (`research/actions-protocol.md#22`, `#23`).

**OIDC.** The issuer is `https://<host>/_services/token` with JWKS under it, a per-cell signing key, the GHES claim set, and the legacy `sub` format by default. The immutable `OWNER@ID/REPO@ID` format is opt-in per org (`research/actions-protocol.md#41`, `#42`, implication 11). `@actions/attest` refuses non-GitHub issuers (`research/actions-protocol.md#47`), so provenance attestation stays unsupported.

**Coping with toolkit churn.** A nightly conformance job on a staging cell runs a corpus of the top public actions at every major version against the latest upstream releases of actions, toolkit, and runner. A failure opens a release blocker. The toolkit's churn is high (`research/actions-protocol.md` risk rating for G), so this job is permanent, not a launch task.

## Frontend and native app

The web app is React 19 with Primer React, rendered on the server by `trib-web` (Node), the stack GitHub itself uses (`research/clients.md#62` to `#64`). It reads data through the public GraphQL schema, so the web app and third-party clients exercise the same resolvers, plus a few internal `/_web/` endpoints in `trib-api` for things GraphQL does not model (rendered blobs, diff hunks, blame). Diffs are computed on a replica with `git diff-tree` and cached in S3 by (base, head). Static assets are content-hashed files inside the release bundle, served by `trib-edge`, so no CDN is required in BYOC or self-hosted. Managed mode puts CloudFront in front for assets only.

What keeps the UI up when backends degrade: every page section declares its own data dependency and deadline, and SSR renders each section independently, so a slow or failing dependency affects only its section. Code browsing depends on replicas and the authz snapshot, not on DSQL writes. Search panels depend only on OpenSearch and show an inline error when it is down. There is no alternate data path, only partial rendering of the one path (`research/uptime.md#5`).

GPUI does not earn a place at launch. The web app, VS Code Pull Requests, JetBrains, Desktop, and gh already cover native needs through the wire surfaces Tributary must serve anyway. GPUI is pre-1.0 with frequent breaking changes and heavy builds (`research/clients.md#57` to `#61`). After Milestone 7 a small GPUI pull request review client may follow: inbox, diff, review threads, and merge, using only public GraphQL, vendored by git rev. Its value would be proof that the public API is complete enough for a first-class client.

## Build order

1. **Milestone 1: the storage core in one cell.** Three `trib-spokesd` nodes in three AZs, `trib-git` with HTTPS basic auth against a static token, the reference-transaction vote, the S3 fence, digest-based reads, and the `repair` sweeper. Gate: unmodified git 2.39 and 2.5x clone, fetch, and push (single ref, `--atomic` multi-ref, force-push) pass; a fault-injection run of 100,000 pushes across 1,000 refs, with coordinator kills, replica partitions, and a full AZ blackhole, records ref histories that the Elle linearizability checker accepts per ref, with zero acknowledged pushes missing from both S3 and a voter. This retires the riskiest unknown first: the cost and correctness of quorum plus fence, and its p50 and p99 push latency.
2. **Milestone 2: GHES host shape and identity.** `trib-edge` headers, `/meta`, `/status`, `/api/v3/` root, token formats, seeded OAuth clients, device and web flow, SSH. Gate: `gh auth login --hostname`, GCM auto-detection with `GCM_TRACE=1`, Desktop enterprise sign-in, and JetBrains version parsing all pass against a test cell.
3. **Milestone 3: core REST and GraphQL.** Repositories, issues, pulls, users, orgs, teams, git data, contents on DSQL. Gate: the cli/cli acceptance suite passes, every harvested VS Code PR, JetBrains, Renovate, gh, and terraform GraphQL operation validates against our introspection, and Schemathesis on the T0 REST categories passes for both API versions.
4. **Milestone 4: outbox, webhooks, merge, protection.** Gate: Renovate opens an onboarding PR and an automerged update PR; terraform `TestAccGithubBranchProtection` passes; Argo CD refreshes on a push webhook; a nightly job confirms every fence record has a `git_events` row.
5. **Milestone 5: Actions.** Gate: the pinned `actions/runner` runs a workflow using `actions/checkout@v4`, `actions/cache@v4` and `@v6`, and `upload-artifact@v3.2.2` and `@v4` (shimmed), and ARC scales a runner from zero.
6. **Milestone 6: one artifact in three modes.** Gate: the same release installs as self-hosted and BYOC from the vendor registry, and an upgrade from N-1 to N under a load of 50 pushes per second and 500 fetches per second completes with zero failed git operations and no SLO burn.
7. **Milestone 7: Dual-Region tier and Region drills.** Gate: an ARC routing-control flip of a loaded Dual-Region test cell completes in under 5 minutes with every acknowledged push and metadata write present in the peer Region.

## Synthesis decision

Filled in by the arena lead.

## Tradeoffs accepted

- We accept three full NVMe copies of every repository, plus S3, in exchange for reads that never wait on S3 and an AZ loss that costs nothing visible. Idle repositories cost the same as busy ones.
- We accept push availability no better than S3's in exchange for every acknowledged push being durable in S3 and fenced per reference. There is no disk-only commit mode when S3 is impaired, because a rarely used mode is the fallback AWS warns against (`research/uptime.md#5`).
- We accept that git writes do not depend on DSQL, so `push` events are ingested from S3 intents rather than written transactionally, in exchange for git surviving a metadata database outage.
- We accept two `FenceStore` implementations, S3 for Regional and multi-Region DSQL for Dual-Region, in exchange for a zero-RPO tier with a real witness. The tier is fixed per cell, so no repository has two rules.
- We accept running upstream git three times per push (index-pack on every voter) in exchange for using git's own receive path and hooks unchanged, as Spokes does.
- We accept the `files` ref backend, and its slower listing on repositories with huge ref counts, in exchange for per-ref locks.
- We accept DNS as the cell router, with planned TTL waits and proxying during migration, in exchange for no shared Tributary component on any request path and correct SSH routing.
- We accept one hostname per managed tenant and no shared user namespace across tenants, in exchange for no global identity database.
- We accept serving a signed, transformed bundle for allow-listed first-party actions, including SHA pins, in exchange for `upload-artifact@v4` and later working on a GHES-classified host.
- We accept DSQL's OCC retries and 3,000-row transactions in exchange for no primary, no failover API, and a zero-RPO multi-Region option.

## Alternatives considered

- **S3 write-ahead log with disks as cache (Continuity shape).** Each repository has a WAL in S3 Standard, one If-Match CAS on a manifest linearizes pushes, and local repositories are evictable caches (`research/git-storage.md#29` to `#31`). Cursor sells this shape as Origin. It hides more behind a smaller interface and needs no placement database. It lost here for three reasons. Every read must check the manifest ETag in S3 to be fresh, so an S3 impairment stops reads as well as writes, or the system serves unchecked caches, which is a fallback mode. One CAS chain per repository caps writes near 120 pushes per second on S3 Standard (`research/git-storage.md#30`) and puts merge previews and user pushes on the same key. And evicted repositories rematerialize from S3 on first access, which moves clone latency onto S3 first-byte times. Tributary's answer to Origin keeps its strongest property, S3 durability at acknowledgment, and adds disk-served reads that survive an S3 impairment.
- **Refs as rows in the metadata database, packs in S3 (Azure DevOps and Mononoke shape).** A ref move is a conditional row update in DSQL and objects live in S3 (`research/git-storage.md#19`, `#22`, `#28`). The interface is the smallest of the three. It lost because pack generation must either be reimplemented, which no Rust library can do today (`research/git-storage.md#44` to `#48`), or run on materialized local copies, which brings back a disk tier without a quorum. It also puts git writes in the same failure domain as issues and pull requests, the coupling behind GitHub's worst core outages (`research/uptime.md#43`, `#44`).
- **One shared hostname for all managed tenants (github.com shape).** All tenants share `tributary.dev` with a global user namespace, and a path-based router sends `/{org}/{repo}` to cells. It lost because users, OAuth grants, `/user`, and GraphQL `node(id:)` span cells, which forces a global identity and routing store shared by every tenant, the component AWS says to keep thinnest and GitHub's 2026 outages trace to (`research/uptime.md#10`, `#47`). SSH also cannot be routed without terminating it in a shared tier.

## Open questions and risks

- Is the quorum's engineering cost acceptable? Placement, sweepers, membership changes, digest reads, and the vote protocol are the "high" cost row in `research/git-storage.md`'s options table. Milestone 1 measures it before anything else depends on it.
- What are the real p99 latencies of a create-only PutObject on S3 Standard in our Regions? Research found no number (`research/git-storage.md#58`). If p99 exceeds 150 ms, should the Regional tier move the fence to single-Region DSQL and accept the coupling?
- Is serving shimmed bundles for SHA-pinned first-party actions acceptable to security reviewers, or should the default be `off`? This is a trust decision, not a protocol one (`research/actions-protocol.md` open question 3).
- Do any GHES-era tools break on a 520-character `ghs_` token, given GHES itself still issues short ones (`research/wire-census.md#30`)? If so, would a release-level switch back to opaque installation tokens be worth the database dependency?
- Is three NVMe copies of cold repositories too expensive for managed margins? A cold tier with one voter plus S3 would change the commit rule for those networks, which this design avoids.
- Does the multi-ref claim protocol, with burned seqs and partial records, hold under the Milestone 1 checker? It is the least proven part of the fence [inference].
- Will GitHub's end-state design, due after 2026-10-21, show a better per-reference ordering than S3 create-only keys (`research/git-storage.md#7`)?
- Is DSQL available in every Region and Region trio a customer will want, and does its CDC stream keep up for the Regional tier's warm DR cluster [unverified]?
- This direction is weakest for idle repositories and for small self-hosted installs, where three storage nodes are more than the workload needs. Is a three-node minimum acceptable for self-hosters with a few hundred repositories?

## Next implementation step

Write `trib-quorum` (`RefTxn`, `Vote`, `FenceStore` on S3, and `choose_read_replica`) with `trib-vote` as git's reference-transaction hook, and run it against three local git repositories and a real S3 Standard bucket under the Elle checker before any network code exists.
