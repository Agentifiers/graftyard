# Refs are rows: one Aurora DSQL database per cell holds refs, pack manifests, pull requests, and the outbox

## Problem

Tributary must look exactly like a GitHub Enterprise Server (GHES) host to unmodified clients, keep git, pull requests, and the APIs up before anything else, and never lose an acknowledged push, in three modes that ship as one artifact. Five research facts make the shape non-obvious. A Region and its data can vanish for good (`research/uptime.md#37`, `#38`), so "never lose a push" needs a copy outside the Region at acknowledgment time, not later. Every large host that keeps refs on local disks pays for a replica quorum, placement database, checksums, and sweepers (`research/git-storage.md#2`-`#6`), and the vendor's BYOC machinery has no deploy stage for stateful fleets and cannot observe storage health (`research/byoc-grounding.md` reuse items 7 and 8). No Rust library can serve git, so upstream git must stay the pack engine (`research/git-storage.md#44`-`#49`). Pull request state, refs, and the webhook stream are three facts about one event, and hosts that keep them in separate stores show divergence races, for example Praefect's PostReceiveHook firing before replication (`research/git-storage.md#10`). Finally, recovery must never call a control-plane API or launch capacity (`research/uptime.md#32`-`#35`), which rules out any database whose cross-Region failover is an API call (`research/uptime.md#24`, `#25`). This candidate bets that the Azure DevOps lineage (`research/git-storage.md#22`, options table last row) is now the strongest shape on AWS because Aurora DSQL commits synchronously across Regions with no failover step (`research/uptime.md#26`): refs, pack manifests, small packs, issues, pull requests, permissions, and the outbox live in one DSQL cluster per cell; pack bytes live in S3; every disk is a cache.

Where this design answers each required fact from `arena/TASK.md`:

1. Region loss and RPO: "Repository storage and ref transactions" (the commit rule), "Availability / Failure modes" (`region` rows), and "Deployment modes" (durability class). Paired cells give push and metadata RPO zero; Regional cells state their RPO.
2. Object storage as truth: the whole design. S3 plus DSQL is the only durable state; NVMe repositories are disposable caches ("Repository storage and ref transactions").
3. Upstream git stays the engine: "Service and crate map" (`trib-git`) and "Build order".
4. Toolkit hostname heuristics and SHA pins: "Actions", toolkit section, and the `actions-artifacts` row of "Wire compatibility".
5. `use_v2_flow`: "Actions", runner protocol. Tributary returns `use_v2_flow: true`.
6. GHES look: "Wire compatibility", text after the table.
7. Token and id formats: "Identity and tokens" and "Wire compatibility" text.
8. BYOC machinery gaps and upgrade downtime: "Change safety" and "Deployment modes".
9. No launch or control-plane dependency in recovery: "Availability", every `control-plane` row, and "Change safety".

## Usage (caller's view)

**Operator quickstart, self-hosted.**

A self-hoster runs three EC2 instances of one binary, one Aurora DSQL cluster, one S3 bucket, and one NLB. There is no database server, no Redis, and no replica set to repair. The deploy tool, bootstrap, and signed registry are the vendor's existing BYOC machinery with a Tributary product profile (`research/byoc-grounding.md#22`, `#23`).

```sh
# 1. One time, in a dedicated AWS account. Same bootstrap module, product prefix "trib-".
tofu -chdir=bootstrap/customer apply \
  -var product=tributary -var deploy_principal_arn=arn:aws:iam::111122223333:role/ci-deploy
# 2. Fetch and verify the signed release (ECDSA P-256, create-only registry).
platform-deploy release fetch --registry prod --release tributary-2027.03.1
# 3. Describe the install. The "tributary" block is the only product-specific part of the target.
cat > target.json <<'EOF'
{ "version": 1, "name": "git-prod", "profile": "tributary-customer",
  "aws": { "accountId": "111122223333", "region": "eu-west-1" },
  "terraform": { "stateBucket": "platform-terraform-state-111122223333" },
  "tributary": {
    "hostname": "github.example.com",
    "durability": { "class": "regional", "drRegion": "eu-central-1" },
    "nodes": { "layout": "combined", "perAz": 1, "instanceType": "i8g.2xlarge" } } }
EOF
platform-deploy deploy --target target.json --bundle tributary-2027.03.1
# 4. First admin and first OAuth device login.
tributary admin bootstrap --host github.example.com --first-admin alice
```

At 2am the self-hoster has nothing to repair by hand. A dead node is replaced by its Auto Scaling group whenever EC2 can launch, and the two surviving nodes already serve every repository from S3 and DSQL. Upgrades are `platform-deploy deploy` with a newer bundle and never need a maintenance window.

**Operator quickstart, BYOC.**

The customer applies the same bootstrap with `deploy_principal_arn` set to the vendor's deploy principal and an ExternalId, creates the read-only observer role, and delegates one DNS subzone (for example `git.example.com`) or uploads a certificate. The customer chooses the durability class. Paired is the BYOC default and needs a second Region and a witness Region enabled in the same account. The vendor then runs the same `deploy` command from its pipeline. The customer grants nothing else, and the install keeps serving with the vendor unreachable.

**Operator quickstart, managed.**

The customer clicks "Create enterprise", picks a host label (`acme` becomes `acme.tributary.cloud`, or a custom domain by CNAME plus DNS validation), picks a residency pair (for example EU: eu-west-1 and eu-central-1 with an eu-west-3 witness), and connects SAML or OIDC. The fleet control plane places the tenant on a Paired cell with headroom, writes the tenant map, and creates the DNS record and certificate. Those are control-plane calls, and they happen only at provisioning time.

**What a `gh` user types.**

```sh
gh auth login --hostname acme.tributary.cloud   # device flow with gh's own client id 178c6fc778ccc68e1d6a
export GH_HOST=acme.tributary.cloud             # or let gh infer the host from the git remote
gh repo clone acme/payments && cd payments
gh pr create --fill && gh pr checks --watch && gh pr merge --squash --delete-branch
```

gh classifies the host as GHES because it is not `github.com`, `*.ghe.com`, or `github.localhost` (`research/clients.md#1`), derives `/api/v3/` and `/api/graphql` (`#3`), and reads `installed_version` from `/api/v3/meta` (`#9`). Nothing else is configured.

**Journey 1: `git push` to a protected branch.**

`git push origin main` from a user without bypass rights, over HTTPS.

1. DNS resolves `acme.tributary.cloud` through Route 53 health-checked latency records to the cell's NLB in Region A (data plane only, `research/uptime.md#15`).
2. NLB (TCP 443, TLS passthrough) hands the connection to an `edge` process. The edge terminates TLS with the tenant certificate from memory, maps the SNI host to tenant 42 in cell `c7` from its in-memory tenant map, and stamps `X-GitHub-Request-Id`.
3. `GET /acme/payments.git/info/refs?service=git-receive-pack`. The edge validates the Basic password as a `gho_` token: prefix and CRC32 offline, then a primary-key read of the token hash in DSQL (cached 30 s, revoked by outbox hint). Datastore: DSQL `credential`.
4. The edge forwards to the `git` role on the repository's rendezvous-preferred cache node in the caller's AZ. That node reads `ref_shard` versions and the live pack list in one DSQL snapshot, compares them with its local cache, pulls missing packs from S3 or inline rows, applies changed refs to the local reftable, and advertises `report-status report-status-v2 delete-refs side-band-64k ofs-delta atomic object-format=sha1 quiet push-options` (`research/wire-census.md#44`). Datastores: DSQL `ref_shard`, `ref`, `pack`; S3 pack bucket; local NVMe.
5. `POST /git-receive-pack`. Rust parses the commands and streams the pack into upstream `git index-pack --stdin --fix-thin --strict` in a quarantine directory, then runs `git rev-list --objects --stdin --not --all` for connectivity against the cache.
6. Rule evaluation in Rust over a DSQL snapshot of `branch_rule` and `permission`: `main` requires a pull request and the user has no bypass, so `refs/heads/main` gets `ng refs/heads/main protected branch hook declined` with the `GH006: Protected branch update failed` text on side-band 2. Other refs in a non-atomic push continue. With `--atomic`, the whole push is rejected.
7. For refs that pass, the pack is placed. At most 1 MiB goes inline into the commit transaction. Larger packs are PUT to S3 with `If-None-Match: *` under `n/<network>/<pack-sha>.pack|.idx`, to two of the three Regional buckets in a Paired cell, before the transaction opens.
8. One DSQL transaction: compare-and-set every accepted ref row, update `refs/pull/N/head` for open PRs whose head is a pushed branch, bump the touched `ref_shard` rows, insert the `pack` row, `push` row, and `ref_log` rows, and insert `push` and `pull_request.synchronize` outbox rows. Commit. Datastores: DSQL `ref`, `ref_shard`, `pack`, `pack_inline`, `push`, `ref_log`, `outbox`.
9. Only after the commit returns does `report-status` say `ok` for each accepted ref. The local cache then moves the quarantine objects into its repository. If the node dies between commit and report, the push is durable and the client sees a broken connection, the same contract git gives for any server crash after the ref update.

**Journey 2: merging a pull request.**

`gh pr merge 17 --squash` sends the GraphQL `mergePullRequest` mutation.

1. Edge, as in journey 1, routes `/api/graphql` to the `api` role.
2. The `api` role reads one DSQL snapshot: the PR row, `refs/pull/17/head`, the base ref, reviews, `branch_rule`, and `check_run` rows for the head SHA. It evaluates protection in Rust. Datastores: DSQL `pull_request`, `ref`, `review`, `branch_rule`, `check_run`.
3. It asks the `git` role on the network's cache node for a merge plan. The node runs `git merge-tree --write-tree base head`, `git commit-tree`, verifies that the tree diff of the result equals the PR's diff (the April 2026 merge-queue lesson, `research/uptime.md#49`), and packs the new commit and trees with `git pack-objects`. The plan returns the new commit id and the pack bytes.
4. The `api` role places the pack (inline in practice) and runs one DSQL transaction: CAS `refs/heads/main` from the snapshot's base SHA to the merge commit, set the PR to `Merged`, bump `review_version` guard, insert `merged` and `closed` timeline events, delete the head branch ref if requested, bump ref shards, insert the pack row, and emit `pull_request` (`closed`, merged) and `push` outbox rows. Datastores: DSQL `ref`, `ref_shard`, `pull_request`, `issue_event`, `pack`, `pack_inline`, `outbox`.
5. If the base CAS fails because `main` moved, the role recomputes the plan once, then returns GitHub's `Base branch was modified. Review and try the merge again.` error. A merged PR whose base ref did not move is impossible, and so is the reverse.

**Journey 3: `actions/runner` runs a job with `actions/checkout` and `actions/cache`.**

1. The `push` outbox row from journey 2 is claimed by the `worker` role that holds partition `hash(repo) % 256`. Its Actions trigger consumer reads `.github/workflows/*.yml` at the pushed SHA from the cache node, parses them with the Rust port of `GitHub.Actions.WorkflowParser` (`research/actions-protocol.md#60`), and in one transaction inserts `workflow_run`, `job` rows in `Queued`, `check_suite`, and `check_run` rows plus `workflow_run` and `workflow_job` outbox rows. Datastores: DSQL `workflow_run`, `job`, `check_suite`, `check_run`, `outbox`.
2. The runner was registered earlier: `POST /api/v3/actions/runner-registration` with `RemoteAuth` returned `use_v2_flow: true`, then `POST /api/v3/actions/runners/register` created a `runner` row and the runner exchanged its RSA-signed assertion for a JWT at `authorizationUrlV2` (`research/actions-protocol.md#5`, `#6`, `#8`).
3. The runner long-polls `GET /_services/broker/message?sessionId=...` on the `actions` role. The broker matches its labels against queued jobs and wins the job with a DSQL compare-and-set `Queued -> Assigned{runner, lease}`. It returns a `RunnerJobRequest` with `RunServiceUrl`. Datastore: DSQL `job`, `runner_session`.
4. `POST {run}/acquirejob` returns the `AgentJobRequestMessage`. Its `SystemVssConnection` data carries `CacheServerUrl`, `ResultsServiceUrl`, `GenerateIdTokenUrl`, and the runtime token, and `system.github.launch_endpoint` points at the launch service (`research/actions-protocol.md#16`, `#20`). The job becomes `InProgress`.
5. The runner posts `runnerresolve/actions` for `actions/checkout@v5` and `actions/cache@v4`. The launch service resolves both in the install's `actions` mirror org (vendor-synced copies of upstream, byte-identical) and returns `https://acme.tributary.cloud/_codeload/actions/checkout/legacy.tar.gz/<sha>`. The codeload path is served by `git archive` on a cache node and cached in S3 by `(repo, sha, format)`.
6. `actions/checkout` runs `git fetch` with the job's `ghs_` token. That is the upload-pack half of journey 1: DSQL snapshot read, local cache, upstream `git upload-pack`.
7. `actions/cache` classifies the host as GHES (`research/actions-protocol.md#43`) and uses cache v1: `GET {ACTIONS_CACHE_URL}_apis/artifactcache/cache?keys=...&version=...`. The `actions` role reads `cache_entry` by `(repo, scope ref, key, version)` in DSQL and returns a presigned S3 URL, and the runner does a plain GET because the host is not `*.blob.core.windows.net` (`#36`). On a miss, the post step reserves (`POST caches`), uploads `Content-Range` chunks (`PATCH caches/{id}`) that the role streams into an S3 multipart upload, and finalizes (`POST caches/{id}`), which commits the `cache_entry` row. Datastores: DSQL `cache_entry`; S3 cache bucket.
8. Logs go through the results service: `GetStepLogsSignedBlobURL` returns a Tributary-signed URL with `BLOB_STORAGE_TYPE_UNSPECIFIED`, and the role accepts `comp=appendblock` and `seal=true` into S3 chunk objects (`research/actions-protocol.md#23`). `renewjob` extends the DSQL lease. `completejob` commits `Completed{conclusion}`, the `check_run` conclusion, and `workflow_job.completed` in one transaction. The run's own status is derived by the run's single owner from job events, so parallel matrix jobs never contend on one row.

## Shape

The cell is one Aurora DSQL cluster (single-Region or multi-Region), a set of S3 buckets, one fleet of identical `tributary` nodes running role subsets, and an OpenSearch domain that is a derived index. Everything canonical is a DSQL row or an immutable S3 object addressed from a DSQL row. Nodes keep only caches, in-flight requests, and long-poll sessions, all rebuildable.

**Why Aurora DSQL, defended against its limits.** DSQL multi-Region commits synchronously to two Regions with a witness, with "zero replication lag on commit" and no failover operation (`research/uptime.md#26`). That answers fact 1 for every row in the database at once, including refs, and needs no control-plane call during Region loss. Aurora PostgreSQL Global Database loses seconds on unplanned failover, its failover is an RDS control-plane call, and it does not guarantee to fence the old primary (`#24`, `#25`). DynamoDB MRSC has no transactions (`#29`), so a merge could not commit the ref, the PR, and the outbox together. DSQL's limits (`#27`) are real, and each has a specific answer here:

| DSQL limit (`research/uptime.md#27`) | Design answer |
| --- | --- |
| 3,000 rows and 10 MiB per transaction | An atomic push carries at most 1,000 ref updates (about 2,100 rows worst case with ref logs, PR head refs, and shards). Non-atomic pushes above 1,000 refs commit in chunks of 1,000 with per-ref status, which git's non-atomic semantics allow. Inline packs are capped at 1 MiB. A branch may head at most 100 open PRs. Bulk operations (org permission changes, transfers, deletes) are outbox sagas of idempotent steps of at most 1,000 rows. |
| 5-minute transactions | No transaction spans network I/O to clients or S3. Pack upload and git work finish before `BEGIN`. Request transactions have a 2 s deadline. |
| Optimistic concurrency, SQLSTATE 40001 | A ref compare-and-set is already optimistic, so a 40001 retry re-reads the snapshot and reports `fetch first` if the expected value is gone. Retries are capped at 3 with jitter under a per-node retry budget of 10% of calls. No push path writes a per-repository counter. The only per-repository hot rows are the 40 ref-shard rows (32 for user refs, 8 for `refs/pull/*`) and the issue-number counter. |
| Repeatable Read, so write skew is possible | Every input to a merge decision that is PR-scoped (reviews, dismissals, draft state, protection overrides) bumps `pull_request.review_version`, which the merge transaction also writes, so the two conflict instead of skewing. |
| No triggers, PL/pgSQL, or TRUNCATE | All rules live in Rust. The outbox row is written by the same `Tx` handle as the data, never by a trigger. Deletes are chunked. |
| One DDL per transaction, DDL separate from DML | The migration runner applies one statement per transaction under a lease, expand and contract only ("Change safety"). |
| 10,000 connections, 100 new per second, 1 hour lifetime | Each role has a fixed per-cell connection budget (cell total 4,000). Connections recycle at a jittered 40 to 55 minutes. After an AZ loss, reconnects queue behind a client-side 60-per-second limiter. |
| 1 KiB keys, C collation | Ref names are capped at 1,000 bytes. Case-insensitive names (logins, repository names) store a lowercased shadow column with a unique index. |
| 1,000 tables, 24 indexes | The schema budget is 400 tables. The CI schema linter fails the build above it. |

### Tenancy and cell placement

```rust
/// Allocated by the fleet control plane in managed mode; always 1 in BYOC and self-hosted.
pub struct TenantId(u32);
pub struct CellId(u16);
pub struct AzId(String);

/// A validated host: not github.com or *.github.com, not *.ghe.com, not *.localhost.
/// Parsing rejects those names, so a tenant can never be misclassified by gh, GCM, or the runner.
pub struct TenantHost(String);

pub enum Durability {
    /// One single-Region DSQL cluster. Packs replicate to `dr` asynchronously.
    Regional { home: Region, dr: Region },
    /// DSQL multi-Region: two read-write Regions plus a witness Region.
    /// Pack bytes land in at least two of the three Regional buckets before commit.
    Paired { a: Region, b: Region, witness: Region },
}

pub struct Cell {
    pub id: CellId,
    pub durability: Durability,
    pub azs: [AzId; 3],
    pub dsql: DsqlEndpoints,       // one endpoint per read-write Region
    pub packs: PackBuckets,        // one bucket per Region named by `durability`
    pub limits: CellLimits,        // load-tested ceilings, enforced by admission control
}

pub enum RouteState { Serving, Frozen { until: Timestamp }, Moved { to: CellId } }
pub struct TenantRoute { pub tenant: TenantId, pub cell: CellId, pub state: RouteState }

/// s3://trib-<cell>-control/tenant-map.json, written only with If-Match on the previous ETag.
pub struct TenantMap { pub generation: u64, pub routes: HashMap<TenantHost, TenantRoute> }

pub trait TenantMapSource {
    /// Newest map that passed validation, else the last known good map and its age.
    fn current(&self) -> (Arc<TenantMap>, Staleness);
}

/// Pure. Called once per connection on the SNI name.
pub fn route(map: &TenantMap, sni: &str) -> Result<TenantRoute, RouteError> { todo!() }

pub enum MigrationStep { Clone, Freeze, Flip, Redirect, Forget }
pub async fn migrate_tenant(t: TenantId, from: CellId, to: CellId) -> Result<(), MigrationError> {
    // Clone: copy the tenant's rows in 1,000-row chunks and its S3 objects; tail DSQL CDC for the delta.
    // Freeze: tenant writes get 503 + Retry-After for at most 30 s while the delta drains.
    // Flip: write the new map generation in both cells. Redirect: the old edge proxies until DNS TTLs pass.
    // Forget: delete the tenant's rows in the old cell after 7 days.
    todo!()
}
```

Decisions and invariants:

- **The tenant is the host.** Every tenant, in every mode, is one GHES-shaped hostname with its own user namespace, as GHE.com does per enterprise. Managed is many such hosts per cell. BYOC and self-hosted are one host per install. Clients therefore see the same thing in all three modes, and there is no global user graph, no cross-tenant fork, and no global router on the request path. DNS points each tenant host directly at its cell's NLBs.
- **The cell is the blast radius.** One cell is one DSQL cluster, its buckets, its node fleet, and its search domain, sharing nothing with other cells (`research/uptime.md#8`). A cell never spans tenants' failure domains beyond itself: a bad deploy, a hot tenant, or a DSQL problem stops at the cell.
- **Fork networks live inside a tenant**, so the network, its packs, and its PRs are always in one cell and one database. This is what lets a push to a fork update the upstream's `refs/pull/N/head` in the same transaction.
- **Ids are tenant-local.** Every key starts with `tenant_id`. A tenant migration keeps every id, node id, and issue number, and a GHES import can keep the source ids.
- **Placement** (managed only) puts a new tenant on the Paired cell in its residency pair with the most headroom under `CellLimits`. Tenants above 20% of a cell's limits get a dedicated cell. Cell migration ships in milestone 4, per `research/uptime.md#10`.

Dominant access pattern: every request maps SNI to `TenantRoute` with one in-memory hash lookup. The edge never reads DSQL to route, so a DSQL problem cannot stop the edge from returning a well-formed `503` with `Retry-After`.

### Repository storage and ref transactions

```rust
pub struct DbId(i64);                 // invariant: 0 < id < 2^53, checked at allocation
pub struct RepoId(DbId);
pub struct NetworkId(DbId);           // a root repository and all its forks share one object store
pub struct RepoKey { pub tenant: TenantId, pub network: NetworkId, pub repo: RepoId }

pub struct ObjectId([u8; 20]);        // SHA-1 at launch; SHA-256 is a new variant, not a flag
pub struct RefName(Box<[u8]>);        // git check-ref-format valid and at most 1,000 bytes

/// Derived from the name, never stored independently. 0..32 for user refs,
/// 32..40 for refs/pull/*, so bot-written PR refs never conflict with user pushes.
pub struct RefShard(u8);
impl From<&RefName> for RefShard { fn from(n: &RefName) -> Self { todo!() } }

pub struct RefUpdate { pub name: RefName, pub expect: Option<ObjectId>, pub new: Option<ObjectId> }
pub enum Atomicity { AllOrNothing, PerRef }
pub struct RefBatch(BoundedVec<RefUpdate, 1000>);   // keeps a push under 3,000 rows

pub struct PackId([u8; 20]);          // the pack's trailing checksum, so S3 keys are content addressed
pub enum PackLocation { Inline, Buckets(BucketSet) }   // Inline bytes live in `pack_inline`, max 1 MiB
pub enum PackKind { Push, Compacted { generation: u32 }, Cruft }
pub enum PackState { Live, Superseded { at: Timestamp, by: PackId } }
pub struct PackRow { pub network: NetworkId, pub id: PackId, pub kind: PackKind,
                     pub location: PackLocation, pub state: PackState, pub bytes: u64 }

/// Only `PackStore::place` constructs this, after the bytes meet the cell's durability rule.
pub struct DurablePack { id: PackId, location: PackLocation, bytes: u64, _sealed: Sealed }

pub struct PushIntent {
    pub repo: RepoKey, pub actor: Actor, pub batch: RefBatch, pub atomicity: Atomicity,
    pub pack: Option<DurablePack>, pub options: PushOptions, pub request: RequestId,
}

/// Proof that DSQL committed. `report_status` takes `&Acked`, so `ok` cannot precede the commit.
pub struct Acked { pub push: PushId, pub per_ref: Vec<(RefName, RefOutcome)> }
pub enum RefOutcome { Ok, Rejected(RejectReason) }
pub enum RejectReason { Stale, NonFastForward, Protected(RuleViolation), TooManyRefs, Hidden }

pub struct ShardVersions([u64; 40]);
pub struct SnapshotDelta { pub shards: ShardVersions, pub changed_refs: Vec<(RefName, Option<ObjectId>)>,
                           pub live_packs: Option<Vec<PackRow>> }

pub trait PackStore {
    /// Inline if <= 1 MiB. Otherwise PUT If-None-Match:* to the class's buckets:
    /// Regional: home bucket. Paired: return after 2 of 3 Regional buckets confirm.
    async fn place(&self, net: NetworkId, staged: StagedPack) -> Result<DurablePack, PackError>;
    /// Download missing packs, local first, then any bucket in the row's location set.
    async fn materialize(&self, packs: &[PackRow], into: &CacheRepo) -> Result<(), PackError>;
}

pub async fn commit_push(cell: &CellStore, intent: PushIntent, rules: &RuleSet) -> Result<Acked, PushError> {
    // run_tx (retries 40001 up to 3 times from a fresh snapshot):
    //   for u in batch: UPDATE ref SET target = u.new, version = version + 1
    //                   WHERE (tenant, repo, name) = .. AND target IS NOT DISTINCT FROM u.expect
    //     rowcount 0 -> Stale; AllOrNothing aborts everything, PerRef marks this ref only
    //   rules.check(snapshot, accepted) -> Protected rejections (pure function)
    //   for each open PR whose head is (repo, u.name): UPDATE refs/pull/N/head in the base repo;
    //     emit pull_request.synchronize
    //   UPDATE ref_shard SET version = version + 1 for every touched (repo, shard)
    //   INSERT pack (+ pack_inline), push, ref_log rows; emit Push
    todo!()
}

/// One snapshot read per git request. Returns only what changed since `have`.
pub async fn refresh(cell: &CellStore, repo: RepoKey, have: &ShardVersions) -> Result<SnapshotDelta, StoreError> { todo!() }

/// Runs under a fenced `maintenance_lease` row. Builds a geometric pack with bitmap on a cache copy
/// with upstream git, places it, then supersedes exactly the pack ids it consumed.
pub async fn compact(cell: &CellStore, lease: MaintenanceLease, net: NetworkId) -> Result<(), MaintError> { todo!() }
```

Tables (all keys lead with `tenant_id`): `ref(tenant, repo, name) -> target, shard, version`, secondary index `(tenant, repo, shard)`; `ref_shard(tenant, repo, shard) -> version`; `pack(tenant, network, pack_id) -> kind, state, location, bytes`; `pack_inline(tenant, network, pack_id) -> bytes`; `push(tenant, repo, push_id)`; `ref_log(tenant, repo, name, push_id) -> old, new, actor`; `maintenance_lease(tenant, network) -> owner, fence, expires`.

Invariants the structure enforces:

- **Acknowledgment point.** A push is acknowledged when, and only when, the DSQL transaction that contains its ref updates and its pack row has committed. At that instant the pack bytes are either inside that transaction (inline) or already in S3 Standard in at least three AZs, and in a Paired cell in two Regions (`research/git-storage.md#54`). The ref rows themselves are in DSQL across AZs, and in a Paired cell across two Regions. `Acked` and `DurablePack` make the order a type error to get wrong.
- **Linearizable per ref, atomic per push.** Each ref is one row, and the update is a conditional write on its expected value inside an optimistic transaction, so concurrent updates to one ref serialize and the loser sees `Stale`. A multi-ref push is one transaction, so readers see all of it or none of it. Pushes to different refs commit concurrently unless they share a shard row, which is GitHub's 2026 per-reference ordering (`research/git-storage.md#6`) obtained from row granularity instead of a new quorum protocol.
- **Reachability.** Every ref target is reachable from the network's live pack set, because the pack row commits with the ref. Compaction supersedes only the pack ids it read, so a push that lands during compaction stays live. GC deletes a superseded pack from S3 only 7 days after `Superseded.at`, after a second check that no live row has the same id, at a capped delete rate, with bucket versioning keeping deleted objects 30 more days. This is the fenced-write lesson of the DynamoDB enactor race (`research/uptime.md#32`, implication 11).
- **Caches are disposable.** A cache node holds a bare repository per network on NVMe with `--ref-format=reftable`, packs copied from S3 or inline rows, and refs written from DSQL with `git update-ref --stdin`. Upstream `git upload-pack` serves fetches from it. Losing it costs a re-download.
- **Small pushes do not touch S3.** Most pushes are small [guess: well over 90% under 1 MiB]. They commit with one DSQL transaction, which in a Paired cell is the whole cross-Region durability story. Compaction moves inline packs into S3 within minutes and deletes the inline rows.
- **Cross-Region RPO, stated per class.** Paired cells: RPO zero for an acknowledged push and zero for metadata. Inline packs and every row commit synchronously in two Regions (`research/uptime.md#26`), larger packs are in two of three Regional buckets before the commit, and LFS objects reach the second Region before `verify` returns. A `worker` consumer copies each bucket-placed pack to its missing Regional bucket within seconds and updates the row's location set. Regional cells: push RPO up to 15 minutes, bounded by S3 Replication Time Control for bucket packs [unverified SLA], and metadata RPO (which includes inline packs) targeted at 5 minutes through a DSQL CDC export to the recovery Region [unverified lag]. The price of zero is the Paired column of "Tradeoffs accepted": a second Region sized for full load, a witness Region bucket, and one cross-Region round trip per commit.

Dominant access patterns traced:

- *Fetch or clone* (the hottest path): one DSQL snapshot read of 40 `ref_shard` rows by primary-key range. If nothing changed, upload-pack runs on the local cache with no further reads. If shards changed, one indexed read per changed shard plus the live pack list. A CI fan-out of 1,000 clones of one SHA costs 1,000 small reads and spreads across the repository's rendezvous set, which widens from 3 to 9 nodes when per-repository concurrency exceeds 64.
- *Push*: N primary-key conditional updates, a few shard updates, a few inserts, one commit. No S3 call for small packs.
- *API reads of refs* (`GET /repos/{o}/{r}/branches/{b}`, GraphQL `ref`): primary-key reads, no git process.
- *Maintenance*: one lease per network, scheduled by pack count and bytes, geometric factor 2, full repack with bitmaps weekly for networks above 1 GiB. Commit-graph and multi-pack-index files are rebuilt locally by each cache, because both are cheap and derivable.

### Identity and tokens

```rust
pub enum TokenKind { Pat, FineGrained, OAuth, UserToServer, Installation, Refresh }
// prefixes:          ghp_  github_pat_  gho_   ghu_          ghs_          ghr_

/// Stored tokens: 30 base62 random chars plus a CRC32 in 6 base62 chars (research/wire-census.md#29).
pub struct OpaqueToken { kind: TokenKind, secret: [u8; 30], crc: [u8; 6] }

/// Installation tokens (incl. Actions GITHUB_TOKEN): "ghs_" + compact JWS signed by the cell key.
/// Verified with no database read. Revocation is a pushed deny list keyed by `jti`.
pub struct InstallationClaims {
    pub tenant: TenantId, pub app: AppId, pub installation: InstallationId,
    pub repos: RepoScope, pub permissions: PermissionSet, pub jti: Ulid,
    pub exp: Timestamp, pub key_epoch: u32,
}

pub enum Credential { Stored(StoredCredentialId), Stateless(InstallationClaims), Ssh(KeyFingerprint),
                      Session(SessionId), RunnerJwt(RunnerId), RuntimeToken(JobId) }

pub enum Actor { User(UserId), App { app: AppId, installation: InstallationId },
                 Job { run: RunId, job: JobId }, Runner(RunnerId), Anonymous }

pub struct Principal { pub tenant: TenantId, pub actor: Actor, pub scopes: ScopeSet,
                       pub credential: Credential }

/// First-party GitHub clients seeded into every tenant with their real client ids.
pub enum FirstPartyClient { GhCli, Gcm, VsCode, DesktopProd, DesktopDev }
impl FirstPartyClient {
    pub const fn client_id(self) -> &'static str { match self {
        Self::GhCli => "178c6fc778ccc68e1d6a", Self::Gcm => "0120e057bd645470c1ed",
        Self::VsCode => "01ab8ac9400c4e429b23", Self::DesktopProd => "de0e3c7e9973e1c4dd77",
        Self::DesktopDev => "3a723b10ac5575cc5bb9" } }
}

/// Node ids in GitHub's new format: type prefix + "_" + base64url(msgpack([0, ids..])).
/// Repository-scoped types carry the repo id first, as GitHub's CR_ and WFR_ ids do.
pub struct NodeId { pub ty: NodeType, pub ids: SmallVec<[DbId; 3]> }
pub fn parse_node_id(s: &str) -> Result<NodeId, NodeIdError> { todo!() }  // accepts legacy base64 "NN:TypeID" too

pub fn authenticate(h: &RequestAuth, keys: &KeyRing, store: &CredentialCache) -> Result<Principal, AuthError> { todo!() }
/// Pure over a snapshot of permission rows. Denial on a private resource returns Hidden, which
/// the wire layer renders as 404 (research/wire-census.md#21).
pub fn authorize(p: &Principal, action: Action, on: &ResourceRef, perms: &PermissionSnapshot) -> Decision { todo!() }
pub enum Decision { Allow, Forbidden, Hidden }
```

Decisions:

- **Stored tokens** (`ghp_`, `github_pat_`, `gho_`, `ghu_`, `ghr_`) are SHA-256 hashed into `credential(tenant, hash)`. The edge checks prefix and CRC offline, then does one primary-key read, cached 30 s per node and invalidated by a `credential.revoked` outbox hint. Revocation therefore takes effect within 30 s in the worst case.
- **Installation tokens are stateless**, matching github.com since 2026-10-02 (`research/wire-census.md#30`). The cell signing key (Ed25519, per `key_epoch`) is decrypted with KMS once at node start and held in memory, so token minting and verification need neither KMS nor DSQL at request time. Tokens are about 400 characters and match `ghs_[A-Za-z0-9\.\-_]{36,}`, which Renovate accepts (`research/clients.md#38`). GHES itself still issues 40-character tokens, so a client that assumes 40 would break. I found none, and the gh, Renovate, and ARC oracles exercise this.
- **Users, orgs, teams, and permissions are rows in the same database as refs.** Permission evaluation reads `permission` and `team_member` rows in the same snapshot as the operation it guards, so there is no cache of grants to go stale and no separate auth cluster to overload (the February 2026 GitHub incident, `research/uptime.md#47`).
- **OAuth**: authorization code (web flow) and device flow on the bare host at `/login/oauth/*` and `/login/device/code`. First-party apps are seeded with GHES-era callbacks (`http://localhost/` for gh and GCM, `http://127.0.0.1/` also accepted, `x-github-desktop-auth://` for Desktop [unverified exact path]) and are treated as public clients, so the embedded secrets are accepted and ignored (`research/clients.md#44`-`#47`).
- **Ids**: each tenant allocates `DbId`s from `id_block(tenant, kind)` rows in leased blocks of 1,000 per node, so the allocator row is written once per 1,000 creations. Allocation fails closed at 2^53 and alarms at 2^52. Issue and PR numbers come from `repo_counter(tenant, repo)`, one hot row per repository on creation only.
- **SSO**: SAML and OIDC per tenant, with SCIM provisioning at T1. Break-glass admin credentials are pre-provisioned local accounts with hardware keys and no IdP dependency (`research/uptime.md` implication 25).

### Issues and pull requests

```rust
pub struct IssueNumber(u32);
pub enum IssueKind { Issue, PullRequest }

pub struct Issue {
    pub repo: RepoKey, pub id: DbId, pub number: IssueNumber, pub kind: IssueKind,
    pub state: IssueState, pub title: String, pub body: Markdown, pub author: UserId,
    pub version: u64,                // optimistic concurrency and the ETag source
}

pub struct HeadSpec { pub repo: RepoId, pub branch: RefName }   // same network as the base repo

pub struct PullRequest {
    pub issue: DbId, pub base: RefName, pub head: HeadSpec, pub state: PrState,
    pub review_version: u64,         // bumped by every PR-scoped input to a merge decision
}
// There is no head_sha field. While open, the head is the row refs/pull/N/head in the base repo,
// which commit_push moves in the same transaction as the head branch.
pub enum PrState {
    Open { draft: bool },
    Closed { at: Timestamp, final_head: ObjectId },
    Merged { at: Timestamp, by: UserId, merge_commit: ObjectId, final_head: ObjectId },
}

/// Keyed by the SHA pair, so a stale result is visibly stale instead of needing invalidation.
pub struct Mergeability { pub base: ObjectId, pub head: ObjectId, pub result: MergeabilityResult }

pub enum MergeMethod { Merge, Squash, Rebase }
/// Produced by the git role. The tree-diff check has already passed when this exists.
pub struct MergePlan { pub base_old: ObjectId, pub head: ObjectId, pub commit: ObjectId,
                       pub pack: StagedPack, pub method: MergeMethod }

pub fn check_protection(rule: &BranchRule, pr: &PrView, checks: &[CheckRunView], reviews: &[ReviewView]) -> Result<(), RuleViolation> { todo!() }

pub async fn merge(cell: &CellStore, git: &GitRole, pr: PrRef, method: MergeMethod,
                   expected_head: Option<ObjectId>, actor: Principal) -> Result<Merged, MergeError> {
    // snapshot -> check_protection -> git.plan_merge -> pack_store.place ->
    // run_tx { CAS base ref; set Merged; bump review_version; timeline rows; shard bumps;
    //          pack row; emit PullRequest(Closed{merged}) and Push }
    todo!()
}
```

Can pull request state diverge from refs? No, by construction. The PR head is a ref row, the head branch and `refs/pull/N/head` move in one transaction, and a merge moves the base ref and sets `Merged` in one transaction. There is no repair job because there is nothing to repair. The cost is that one push writes up to 100 extra rows when one branch heads many PRs, and the 100-PR cap enforces the bound.

Other decisions: issues and PRs share one number space and one table, as on GitHub. Comments, reviews, review threads, and timeline events are append-mostly rows keyed `(tenant, repo, issue, id)`, so a busy PR does not contend on its own row except for `review_version` bumps. Review comments anchor to `(commit, path, line)` and are re-anchored lazily on read by the git role's diff, not rewritten on push. Required status checks read `check_run` rows by head SHA in the merge snapshot. A new failing check that commits after that snapshot does not block the merge, which matches GitHub's behavior [inference]. Merge queue (T1) is one owner per base branch under a lease row, so queued merges serialize in the owner instead of thrashing on 40001.

Access patterns: PR list pages read `(tenant, repo, state, updated_at)` indexes. A PR page reads the PR row, its head and base ref rows, and the first page of timeline rows in one snapshot, and fetches the diff from the git role. The files-changed diff is computed by the git role from cache and cached in S3 by `(base, head)`.

### Actions runs and jobs

```rust
pub struct RunId(DbId); pub struct JobId(DbId); pub struct RunnerId(DbId);

pub enum JobState {
    Pending { needs: SmallVec<[JobId; 4]> },
    Queued { labels: LabelSet, since: Timestamp },
    Assigned { runner: RunnerId, lease_until: Timestamp },
    InProgress { runner: RunnerId, lease_until: Timestamp },
    Completed { conclusion: Conclusion, at: Timestamp },
}
pub enum Conclusion { Success, Failure, Cancelled, Skipped, TimedOut, StartupFailure }

pub enum JobEvent { NeedsMet, NeedsFailed, Matched(RunnerId), Acquired, Renewed, Finished(Conclusion),
                    LeaseExpired, Cancel }

/// Pure transition table; the store applies it as a CAS on (job_id, version).
pub fn transition(s: &JobState, e: &JobEvent, now: Timestamp) -> Result<JobState, IllegalTransition> { todo!() }

/// Run status is derived from job states by the run's single owner (outbox partition lease holder),
/// never written by job completions directly.
pub enum RunStatus { Queued, InProgress, Completed(Conclusion) }
pub fn derive_run_status(jobs: &[JobState]) -> RunStatus { todo!() }

pub struct WorkflowRun { pub id: RunId, pub repo: RepoKey, pub workflow: WorkflowRef, pub head_sha: ObjectId,
                         pub event: TriggerEvent, pub attempt: u16, pub concurrency: Option<ConcurrencyKey> }

/// Wire type, private to trib-actions. Built from domain rows, never stored.
struct AgentJobRequestMessage { /* plan, timeline, steps as template tokens, context data, variables,
                                   SystemVssConnection data: CacheServerUrl, ResultsServiceUrl, ... */ }

pub trait Broker {
    async fn poll(&self, session: RunnerSessionId, wait: Duration) -> Option<RunnerMessage>;
    async fn acquire(&self, runner: RunnerId, job: JobId) -> Result<AgentJobRequestMessage, AcquireError>;
    async fn renew(&self, job: JobId) -> Result<Timestamp, RenewError>;
    async fn complete(&self, job: JobId, result: JobResult) -> Result<(), CompleteError>;
}
```

Invariants: a job has one runner at a time, because `Queued -> Assigned` is a compare-and-set on the job row's version. A lease that expires (no `renewjob` for 10 minutes) moves the job to `Completed{Failure}` with GitHub's infrastructure-failure category, or back to `Queued` if it was never acquired. Job rows write only on state changes: `renewjob` arrives every 60 s, but the lease row is written only when less than 5 minutes remain, so a 2-hour job writes its lease about 24 times, not 120. Run creation inserts at most about 900 jobs per transaction; larger runs are created in chunks under a `Creating` run marker and flipped to `Queued` in a final idempotent step. Concurrency groups are one `concurrency_slot(tenant, key)` row each, won by compare-and-set.

Access patterns: the broker's hot query is "oldest queued jobs for this tenant matching this label set" on index `(tenant, label_set_hash, state, since)`. Each broker node keeps sessions in memory and wakes long polls from a per-cell in-memory notice that the `worker` sends when it queues jobs. The notice is only a hint. Brokers also poll DSQL every 2 s, so a lost notice costs latency, not a job.

### Event outbox

```rust
pub struct OutboxPartition(u16);          // hash(repo or org) % 256 per cell

pub struct OutboxRow {
    pub partition: OutboxPartition, pub id: Ulid, pub tenant: TenantId,
    pub entity: EntityRef, pub entity_version: u64, pub event: DomainEvent,
    pub done: ConsumerSet,                // bitset, written only by the partition owner
}

/// Generated from the GHES 3.22 webhook schema: one variant per event, one sub-enum per action.
pub enum DomainEvent { Push(PushEvent), PullRequest(PrAction, PrPayloadRef), Issues(IssueAction, IssueRef),
                       WorkflowJob(JobAction, JobId), CheckRun(CheckAction, DbId), CredentialRevoked(Ulid), /* .. */ }

pub enum Consumer { Webhooks, ActionsTrigger, RunOwner, SearchIndex, Notifications, CacheHint, Audit }

/// The only way to write an outbox row. Every domain command returns Change<T>;
/// CellStore::run_tx commits the rows and the events together, so "forgot to emit" cannot compile.
pub struct Change<T> { pub value: T, pub events: SmallVec<[DomainEvent; 4]> }
pub trait CellStore {
    async fn run_tx<T, F>(&self, ctx: TxCtx, f: F) -> Result<Committed<T>, StoreError>
    where F: Fn(&mut Tx<'_>) -> Result<Change<T>, DomainError>;   // Fn: may run up to 3 times on 40001
}

pub struct PartitionLease { pub partition: OutboxPartition, pub owner: NodeId, pub fence: u64, pub expires: Timestamp }
pub async fn dispatch(cell: &CellStore, lease: PartitionLease, consumers: &ConsumerTable) -> Result<(), DispatchError> {
    // loop: SELECT pending rows in partition ORDER BY id LIMIT 500 (no cursor: late commits are still pending)
    //   for each consumer not in row.done: handler(row) (idempotent, keyed by row id)
    //   UPDATE done bits fenced by lease.fence; DELETE rows with all bits set
    todo!()
}

pub struct HookDelivery { pub guid: Uuid, pub hook: DbId, pub outbox_id: Ulid, pub attempts: SmallVec<[Attempt; 4]>,
                          pub payload: S3Key, pub expires: Timestamp }   // kept 3 days, research/wire-census.md#37
```

Invariants: an event exists if and only if its data change committed, because both are in one transaction. Producers only insert, so they never conflict with the dispatcher. Delivery is at least once. The `X-GitHub-Delivery` GUID is derived from `(outbox id, hook id)`, so a redelivery after a crash keeps the same GUID, as GitHub's does (`research/wire-census.md#36`). Order is per partition by ULID, except that a transaction that commits late can deliver after a later one. Consumers that care (search, run owner) compare `entity_version` and drop stale events. Webhook ordering is not guaranteed by GitHub either [inference].

The webhook consumer only writes `hook_delivery` rows and payloads to S3, then returns. Separate delivery workers POST with a 10 s deadline, per-tenant shuffle-sharded concurrency, and retries for 3 days visible in the deliveries API. A slow consumer, for example search with OpenSearch down, leaves its bit clear while the other consumers move on, and the table grows only by that backlog. Oldest-pending-row age per consumer is an SLO signal and a deploy gate.

### Service and crate map

Deployables:

- **`tributary`**, one Rust binary with roles selected by config. `edge` terminates TLS (rustls, SNI) and SSH (russh), routes by tenant host, verifies tokens, enforces rate limits, and serves the web app's static assets. `api` serves REST under `/api/v3`, GraphQL at `/api/graphql`, OAuth, uploads, and LFS batch. `git` owns the NVMe cache, receive-pack, upload-pack spawning, merge plans, archives, and diffs. `actions` serves registration, broker, run service, launch, results, cache, artifacts, and OIDC. `worker` runs outbox dispatch, webhook delivery, workflow triggering and run ownership, compaction, GC, search indexing, and migrations. Small installs run all roles on every node. Managed cells run `edge+api`, `git`, `actions`, and `worker` as separate Auto Scaling groups.
- **Upstream git**, pinned (2.56 at design time), shipped inside the node image and the release bundle. It does index-pack, upload-pack, pack-objects, repack, merge-tree, archive, and fsck (`research/git-storage.md` implication 8).
- **`tributary-web`**, the React app, built into the release and embedded in the `edge` role.
- **`tributary-runner-host`**, managed and optional BYOC only. It runs unmodified `actions/runner` in Firecracker microVMs in separate runner accounts.
- **`tributary-fleet`**, managed only. It places tenants, provisions hosts and certificates, runs cell migrations, and feeds <vendor-fleet-app>.

Crates. Each crate's internals are private, and only the listed items are public.

- `trib-domain`: ids, tenancy, refs, PRs, Actions state machines, events, rules. Pure, no IO.
- `trib-store`: the only crate that contains SQL. `CellStore`, `run_tx`, retry budgets, the migration runner, the schema linter.
- `trib-pack`: `PackStore`, bucket quorum, inline packs, GC, cache materialization.
- `trib-git`: receive-pack protocol, git subprocess supervision, cache repositories, merge plans, archives.
- `trib-auth`: tokens, OAuth, sessions, SSH keys, `authenticate`, `authorize`, node ids.
- `trib-wire`: GitHub conventions shared by REST and GraphQL: headers, error bodies, the 404 rule, pagination links, ETags, media types, API versions, rate-limit headers.
- `trib-rest`: routes and request and response types generated from the pinned `ghes-3.22` OpenAPI file, plus handlers.
- `trib-graphql`: schema generated from the GHES 3.22 SDL, resolvers, cost accounting.
- `trib-actions`: the Rust port of `GitHub.Actions.WorkflowParser`, scheduler, broker, run service, launch, results, cache v1 and v2, artifacts, OIDC, Azure Blob dialect shim. Wire types stay private.
- `trib-hooks`: outbox dispatch, webhook payload rendering, delivery.
- `trib-edge`: TLS, SSH, tenant map, admission control.
- `trib-ops`: observer routes, deploy stages, drills.

Rust does not pay for itself in pack generation, repacking, or merge computation, which stay in upstream git, or in the web UI, which is React.

AWS services on the request path: NLB (data plane), Route 53 DNS resolution and health checks (data plane), Aurora DSQL (data plane, IAM auth tokens signed locally with instance credentials), and S3 (data plane, presigned URLs computed locally). KMS is off the request path: node start decrypts keys once, and S3 uses bucket keys for SSE-KMS [unverified that bucket keys remove per-request KMS calls in every case]. OpenSearch is on the search path only. SES (email) is off every request path.

## Wire compatibility

An "Octokit contract run" means Schemathesis cases generated from the pinned `ghes-3.22` OpenAPI file for that category, executed through unmodified `@octokit/rest` with `octokit.paginate`, under both API versions `2022-11-28` and `2026-03-10`, with responses validated against the schema (`research/clients.md` conformance table, octokit row). It is always paired with a named client run where one exists.

| Surface | Tier | Oracle | Approach |
| --- | --- | --- | --- |
| `git-http` | T0 | `git clone`, `git push --atomic`, `git fetch --filter=blob:none` with git 2.39 and 2.51, plus `GCM_TRACE=1 git clone` with no provider override | Smart HTTP only, dumb HTTP refused. Rust receive-pack, upstream upload-pack on the cache, refs from DSQL. |
| `git-ssh` | T0 | `ssh -T git@HOST` prints the GitHub-style greeting and exits 1, then `git clone git@HOST:o/r.git` and push | russh in `edge`, key fingerprint lookup in DSQL, same `git` role. |
| `git-protocol-v2` | T0 | `GIT_TRACE_PACKET=1 git ls-remote https://HOST/o/r` shows `ls-refs=unborn` and `fetch=shallow wait-for-done filter` | Advertise exactly github.com's capability set (`research/wire-census.md#41`). `bundle-uri` and `packfile-uris` stay off until T2. |
| `lfs` | T0 | `git lfs track '*.bin' && git push && git lfs fetch --all && git lfs locks` over HTTPS and SSH | Batch API with presigned S3 URLs, `verify` action required, locks as DSQL rows. Paired cells copy to the second Region before `verify` returns. |
| `rest:actions` | T0 | `gh run list`, `gh run view --log`, `gh workflow run`, `gh secret set`, plus Octokit contract run | Registration, runners, runs, jobs, secrets (sealed box), variables at T0. Remaining operations T1. |
| `rest:activity` | T1 | `gh status`, `gh api notifications`, Octokit contract run | Notifications and events from outbox consumers. Stars and watches are rows. |
| `rest:agent-automations` | Out | none | dotcom-only Copilot agent surface, absent from GHES 3.22. |
| `rest:agent-tasks` | Out | none | dotcom-only, absent from GHES 3.22. |
| `rest:agents` | Out | none | dotcom-only, absent from GHES 3.22. |
| `rest:announcement-banners` | T2 | Octokit contract run | Rows per enterprise and org. |
| `rest:apps` | T0 | Renovate in GitHub App mode, ARC with app credentials, `@octokit/app` installation token flow | App JWT auth, installations, stateless `ghs_` tokens. |
| `rest:billing` | T2 | Octokit contract run | Metering rows. |
| `rest:branches` | T0 | `TF_ACC=1 go test ./github -run TestAccGithubBranchProtectionV3`, Octokit contract run | `branch_rule` rows, enforced in receive-pack and merge. |
| `rest:campaigns` | Out | none | dotcom-only. |
| `rest:checks` | T0 | `gh pr checks`, Jenkins GitHub Branch Source posting checks | `check_suite` and `check_run` rows keyed by head SHA. |
| `rest:classroom` | Out | none | dotcom-only. |
| `rest:code-quality` | Out | none | dotcom-only. |
| `rest:code-scanning` | T2 | `github/codeql-action/upload-sarif` on a pinned runner | SARIF in S3, alerts as rows. |
| `rest:code-security` | T2 | Octokit contract run | Configuration rows. |
| `rest:codes-of-conduct` | T3 | Octokit contract run | Static data. |
| `rest:codespaces` | Out | none | dotcom-only, and Codespaces is a separate product. |
| `rest:collaborators` | T0 | `go test ./github -run TestAccGithubRepositoryCollaborator`, Octokit contract run | `permission` rows. |
| `rest:commits` | T0 | Jenkins multibranch commit status, `gh api repos/o/r/commits`, Octokit contract run | Commit data from the `git` role, statuses as rows. |
| `rest:copilot` | Out | none | dotcom-only. |
| `rest:copilot-spaces` | Out | none | dotcom-only. |
| `rest:credentials` | Out | none | dotcom-only. |
| `rest:dependabot` | T2 | `dependabot update -f job.yml` with `source.hostname=HOST` | Dependabot runs as Actions jobs on labelled runners (`research/clients.md#41`). |
| `rest:dependency-graph` | T2 | `actions/dependency-review-action` on a pinned runner | Manifests parsed by a worker consumer. |
| `rest:deploy-keys` | T0 | `gh repo deploy-key add`, then `git clone` with that key | `ssh_key` rows scoped to one repository. |
| `rest:deployments` | T1 | A workflow with `environment:` protection and `gh api` deployment listing | Environments and deployments rows, approvals gate job queueing. |
| `rest:emojis` | T3 | Octokit contract run | Static data. |
| `rest:enterprise-admin` | T1 | Octokit contract run over the GHES-only operations, `gh api` admin scripts | User and org administration first. Appliance console operations map to the install's own admin API. |
| `rest:enterprise-teams` | T2 | Octokit contract run | Team rows at enterprise scope. |
| `rest:gists` | T2 | `gh gist create`, `gh gist view` | Gists are repositories in the same storage. |
| `rest:git` | T0 | Renovate with `platformCommit=enabled`, Octokit contract run | Objects written by the `git` role as a small pack, refs through `commit_push`. |
| `rest:gitignore` | T3 | Octokit contract run | Static templates. |
| `rest:interactions` | Out | none | dotcom-only. |
| `rest:issues` | T0 | `gh issue create/list/view/comment/close` in cli/cli acceptance tests | Issue rows, numbers from `repo_counter`. |
| `rest:licenses` | T2 | Octokit contract run | License detection by a worker consumer. |
| `rest:markdown` | T1 | `gh api markdown -f text=...`, VS Code Pull Requests preview | Rust renderer matched against GitHub's output corpus. |
| `rest:meta` | T0 | GCM provider probe, `gh auth status`, Desktop sign-in | `installed_version: "3.22.0"`, `verifiable_password_authentication: false`. |
| `rest:metrics` | T2 | Octokit contract run | Derived from outbox counters. |
| `rest:migrations` | T1 | Octokit contract run with an export and import round trip | Archives in S3, imports as chunked sagas that keep source ids. |
| `rest:oauth-authorizations` | T3 | Octokit contract run | Deprecated GHES-only API, read paths only. |
| `rest:orgs` | T0 | `go test ./github -run TestAccGithubOrganization`, `gh org list`, Octokit contract run | Org, membership, and role rows. |
| `rest:packages` | T2 | `gh api` package listing after a container push | Metadata over the registry rows. |
| `rest:pages` | T2 | `gh api repos/o/r/pages` after a Pages deploy | Site metadata rows. |
| `rest:private-registries` | T2 | Octokit contract run | Sealed-box secrets as rows. |
| `rest:projects` | T2 | `gh project list`, Octokit contract run | Project rows. |
| `rest:pulls` | T0 | cli/cli acceptance `pr` tests, Renovate onboarding and update PRs | PR rows, head ref rows, merge through `merge`. |
| `rest:rate-limit` | T0 | VS Code Pull Requests `HEAD /api/v3/rate_limit`, Jenkins hub4j endpoint check | Per-node approximate counters, documented as approximate. |
| `rest:reactions` | T1 | Octokit contract run | Reaction rows. |
| `rest:releases` | T0 | `gh release create --generate-notes`, `gh release upload`, `gh release download` | Release rows, assets in S3. |
| `rest:repos` | T0 | `go test ./github -run TestAccGithubRepository`, `gh repo create/view/clone`, Octokit contract run | Repository rows, contents through the `git` role, renames as alias rows. |
| `rest:search` | T1 | `gh search issues`, `gh search prs`, `gh search repos` | Per-cell OpenSearch fed by the outbox. Code search T2. |
| `rest:secret-scanning` | T2 | Octokit contract run with a seeded secret push | Scanner as a push consumer. |
| `rest:security-advisories` | T2 | Octokit contract run | Advisory rows. |
| `rest:teams` | T0 | `go test ./github -run TestAccGithubTeam` | Team rows, CODEOWNERS uses them. |
| `rest:users` | T0 | `gh api user`, cli/cli acceptance tests | User rows, SSH and GPG keys. |
| `graphql` | T0 | Static validation of VS Code PR's 4 `.gql` files, JetBrains' 86 `.graphql` files, Renovate's queries, and gh's raw strings against introspection, then the cli/cli acceptance run and a `@vscode/test-electron` list-review-merge run | Schema generated from GHES 3.22 SDL, so `__type` probes match. Resolvers for the harvested operation set at T0, the rest T1. Unimplemented fields return `undefinedField` errors. |
| `webhooks` | T0 | Argo CD refresh on push, Jenkins multibranch discovery, go-playground/webhooks parsing recorded payloads for all 272 event-action pairs | Outbox consumer. Core events (push, pull_request, issues, workflow_job, check_run) T0, the rest T1. |
| `oauth-web-flow` | T0 | `gh auth login --web`, GitHub Desktop Playwright sign-in | Authorization code flow with seeded first-party apps. |
| `oauth-device-flow` | T0 | `gh auth login`, VS Code `getSession('github-enterprise', ['repo'])`, `GCM_GITHUB_AUTHMODES=device` | Device flow with `slow_down` and form or JSON responses. |
| `github-apps` | T0 | Renovate app mode, ARC app auth, Argo CD `--github-app-enterprise-base-url` | Manifests, private keys, JWT auth, installation tokens. |
| `tokens` | T0 | Renovate token checks (`ghp_`, `github_pat_`, `ghs_`), gh `X-Oauth-Scopes` handling | GitHub prefixes and CRC32, stateless `ghs_`. |
| `actions-runner-protocol` | T0 | Pinned unmodified `actions/runner` `config.sh` then a job, and ARC `gha-runner-scale-set` scaling from zero (ARC at T1) | `use_v2_flow: true`: broker, run service, launch, results. |
| `actions-workflow-syntax` | T0 | A corpus of public workflows parsed differentially against `@actions/workflow-parser`, then executed by the pinned runner | Rust port of the runner's `WorkflowParser` and `MatrixBuilder`. |
| `actions-cache` | T0 | `actions/cache@v4` and `@v5` save and restore on a GHES-classified host | Cache v1 at T0, twirp v2 with the Azure Blob dialect shim at T1. |
| `actions-artifacts` | T0 | `upload-artifact@v3.2.2` and `download-artifact@v3`, then `upload-artifact@v4` through the pin map (T1) | Legacy pipelines artifacts at T0. Twirp v4 plus block shim at T1, reachable through vendor forks ("Actions"). |
| `actions-oidc` | T0 | `aws-actions/configure-aws-credentials` assuming a role through an IAM OIDC provider | Issuer `https://HOST/_services/token`, legacy `sub` format. |
| `actions-logs-results` | T0 | `gh run view --log` after a pinned runner job, live log streaming in the web UI | Results twirp, `comp=appendblock` into S3. |
| `packages-container` | T1 | `docker login HOST && docker push HOST/o/img:tag && docker pull` | OCI distribution over S3 blobs and DSQL manifests. |
| `packages-npm` | T2 | `npm publish --registry https://HOST/_registry/npm/` | Same blob store. |
| `packages-maven` | T2 | `mvn deploy` to the GHES Maven path | Same blob store. |
| `packages-nuget` | T2 | `dotnet nuget push` | Same blob store. |
| `packages-rubygems` | T2 | `gem push --host` | Same blob store. |
| `pages` | T2 | `actions/deploy-pages` through the pin map, then `curl` the site | Static files in S3 behind the edge. |
| `codeload-archives` | T0 | `gh release download --archive=tar.gz`, runner action tarball download | `git archive` on the cache, cached in S3. |
| `raw-content` | T0 | `curl -L https://HOST/o/r/raw/main/README.md` | Served by the `git` role with a sandbox CSP, `nosniff`, and `text/plain`. |
| `uploads-release-assets` | T0 | `gh release upload v1 ./dist/app.tgz` | `/api/uploads` streams to S3. |
| `meta-endpoint` | T0 | GCM HEAD probe for `X-GitHub-Request-Id`, gh feature detection, Desktop header read | GHES `/meta` shape, version from the build. |

Cross-cutting conventions (`research/wire-census.md` section B) live in `trib-wire`, so no handler can skip them. Every response, git smart HTTP included, carries `X-GitHub-Request-Id` (cell, node, and trace id in GitHub's colon-separated hex shape) and `X-GitHub-Enterprise-Version: 3.22.0`. REST responses carry `x-github-media-type: github.v3; format=json` and `x-github-api-version-selected`. `X-GitHub-Api-Version` selects `2022-11-28` (default) or `2026-03-10`, an unknown value returns 400, and the `2026-03-10` breaking changes (`#13`) are encoded as per-version response shapes generated from the two versioned OpenAPI files. ETags are weak and derive from row `version` columns, so a `304` costs one primary-key read and does not count against the rate limit (`#17`). Pagination emits `link` headers with `/repositories/{id}/...` URLs, clamps `per_page` to 100 silently, and uses keyset scans behind `page`, `since`, `before`, and `after`. Error bodies use `basic-error` and `validation-error` with string `status`. The 404 rule (`#21`) is the `Decision::Hidden` variant: a handler cannot render 403 for a private resource because `authorize` never returns `Forbidden` there. Requests without `User-Agent` are rejected. Renamed repositories resolve through an alias table that returns 301 to `/repositories/{id}` in the API and serves git transparently (`#23`). GHES rate limits are off by default (`#19`) in BYOC and self-hosted and on in managed.

Hostname heuristics (`research/clients.md` and `research/actions-protocol.md` section H): `TenantHost` refuses names that end in `github.com`, `ghe.com`, or `localhost`, so every client classifies Tributary as GHES. That is the target: `/api/v3`, `/api/graphql`, `/api/uploads`, `/login/oauth/*`, and `/login/device/code` on one bare host on port 443, because gh rejects ports and the runner drops them (`research/clients.md#4`, `research/actions-protocol.md#4`). `GET /status` returns 200 with `X-GitHub-Request-Id` for Dependabot, `GET /api/v3/status` returns a body starting `GitHub lives!` for VS Code Pull Requests, and `GET /api/v3/` returns the root document with `rate_limit_url` for Jenkins (`research/clients.md` implication 4). Self-hosters are told that a `github.` label (for example `github.example.com`) lets GCM and Desktop skip their probe. Subdomain isolation (`raw.`, `codeload.`, `uploads.`) is on in managed, where the tenant certificate covers `HOST` and `*.HOST`, and optional elsewhere.

OAuth client ids: the five first-party ids in `FirstPartyClient` are seeded in every tenant at creation and cannot be deleted. Ids: every numeric id is `i64` in storage and JSON, allocation stops below 2^53, and GraphQL exposes `databaseId: Int` only where the value fits and `fullDatabaseId: BigInt` as a string everywhere (`research/wire-census.md#28`). Node ids use GitHub's new MessagePack format with the repository id first for repository-scoped types, accept the legacy base64 form as input, and honor `X-Github-Next-Global-ID: 1` (`#26`, `#27`).

## Availability

### SLOs

Targets are per cell and per month (43,200 minutes), for Paired cells. A minute is bad when more than 5% of the journey's requests in that cell fail, GitHub's own SLA definition (`research/uptime.md#51`), or when the external probe fails. Regional cells lower the four 99.99% targets to 99.95%, because a single-Region DSQL cluster carries a 99.99% SLA (`#26`) and Tributary serves no stale reads.

| Journey | Target | Error budget (min/month) | Measured by |
| --- | --- | --- | --- |
| git fetch and clone | 99.99% | 4.3 | Edge logs per cell, plus synthetic `git clone` of a 50 MB repo every minute from three external vantage points |
| git push acknowledged | 99.95% | 21.6 | Receive-pack outcomes excluding client-caused rejections, plus a synthetic push every minute per AZ |
| Pull request merge | 99.95% | 21.6 | `mergePullRequest` and `PUT .../merge` outcomes excluding rule failures, plus a synthetic merge every 5 minutes |
| REST and GraphQL reads | 99.99% | 4.3 | Edge logs by route class, 5xx and deadline overruns |
| REST and GraphQL writes | 99.95% | 21.6 | Edge logs by route class |
| Web page load (repository, PR, issue) | 99.9% | 43.2 | Browser RUM beacons plus a synthetic Playwright run every minute |
| Actions job start (first log line within 5 minutes of trigger) | 99.9% | 43.2 | Run-owner timestamps, GitHub's Actions SLA definition (`research/uptime.md#51`) |
| Webhook first attempt within 60 s | 99.9% | 43.2 | `hook_delivery` first-attempt latency |

Durability is not a percentage target: zero acknowledged pushes lost is an invariant, checked by the linearizability harness from milestone 1 and by a weekly drill that replays every synthetic push's `push_id` against DSQL and S3.

### Failure modes

Stateful components on the request path. The git cache node and the edge hold only rebuildable state, and they are listed because they hold it.

| Component | Failure | Blast radius | Containment |
| --- | --- | --- | --- |
| DSQL cell cluster | instance | Transactions in flight on the failed service host see connection errors or 40001 | Bounded retries under a 10% retry budget. Every write carries the request id, so a retried commit is idempotent. |
| DSQL cell cluster | az | None for data; DSQL spans AZs. One third of node connections drop | Nodes in healthy AZs already hold their connection budgets. Reconnects queue at 60 per second per node class. |
| DSQL cell cluster | region | Paired: the lost Region's endpoint. Regional: the whole cell | Paired: the surviving Region keeps reads and writes through the witness with RPO 0, and Route 53 health checks move traffic in 1 to 3 minutes. Regional: DR restore in the recovery Region from backups plus the CDC archive, RPO target 5 minutes, RTO hours, stated as DR. |
| DSQL cell cluster | control-plane | None at runtime | Clusters are created at install. Nothing at runtime or in recovery calls a DSQL or RDS management API. Auth tokens are signed locally from cached instance credentials. |
| DSQL cell cluster | bad-deploy | One cell, the canary cell first | Statement timeouts of 2 s, a query plan linter in CI, per-AZ then per-cell waves, automatic in-cell abort on OCC-retry rate or commit p99 burn. |
| DSQL cell cluster | bad-config | One cell | Config ships as a signed bundle through the same waves. Nodes validate it and keep the last known good config. |
| DSQL cell cluster | noisy-tenant | That tenant's hot rows; other tenants share capacity | Per-tenant transaction token buckets and concurrency caps on every node, the retry budget, and migration of a tenant above 20% of cell limits to a dedicated cell. |
| S3 pack buckets | instance | Single requests see 5xx or SlowDown | Retries with jitter. Keys spread by network id prefix. |
| S3 pack buckets | az | None; S3 Standard stores data in at least three AZs (`research/git-storage.md#54`) | Nothing to do. |
| S3 pack buckets | region | Paired: one of three buckets. Regional: large pushes and cold cache fills in the cell | Paired: 2-of-3 quorum still acknowledges and reads use the row's location set. Regional: pushes up to 1 MiB still commit inline through DSQL and warm caches keep serving fetches. |
| S3 pack buckets | control-plane | None at runtime | Buckets are pre-created. No bucket configuration call at runtime. Presigned URLs are computed locally. |
| S3 pack buckets | bad-deploy | Durable bad packs or deleted live packs, in one cell | Every pack passes `index-pack --strict` and connectivity before upload. GC deletes only 7-day superseded packs, double-checked, at a capped rate. Versioning keeps deletes for 30 days. GC runs in the last wave. |
| S3 pack buckets | bad-config | One cell; writes fail before acknowledgment | Readiness probe writes and reads a canary object in every configured bucket and refuses traffic on failure. |
| S3 pack buckets | noisy-tenant | That network's prefix | Per-tenant push bytes per second and concurrent push caps in the `git` role. |
| Git cache node | instance | In-flight requests on that node. Unacknowledged pushes fail and clients retry | Rendezvous sets keep each repository warm on one node per AZ. NLB health checks remove the node, capped at 33% removal. |
| Git cache node | az | One third of nodes | Static sizing at 66% per AZ (`research/uptime.md#2`), zonal shift and weekly autoshift practice. |
| Git cache node | region | Paired: that Region's fleet | The other Region's fleet is sized for full load and kept warm by `CacheHint` prefetch. Route 53 moves traffic. |
| Git cache node | control-plane | Dead nodes are not replaced while EC2 launch is impaired | N+1 static capacity. No recovery step launches anything (`research/uptime.md#33`). |
| Git cache node | bad-deploy | One AZ of one cell | Binary hot swap keeps the old binary for instant swap-back. New git versions pass a differential pack test before the canary cell. |
| Git cache node | bad-config | One AZ of one cell | Per-AZ config rollout with automatic abort on cache-miss rate or upload-pack latency. |
| Git cache node | noisy-tenant | That repository's rendezvous set | Per-repository upload-pack concurrency of 64, widened sets for hot repositories, per-tenant CPU shares in cgroups. |
| Edge and tenant map | instance | Connections on that node | NLB health checks with the 33% removal cap. Clients retry. |
| Edge and tenant map | az | One third of edge nodes | Static sizing and zonal shift. |
| Edge and tenant map | region | Paired: that Region's endpoint | Route 53 health-checked records, no record edits. |
| Edge and tenant map | control-plane | None | Tenant map and certificates are cached in memory, last known good kept. DNS records are pre-created. |
| Edge and tenant map | bad-deploy | One AZ of one cell | Hot swap per AZ with automatic swap-back on 5xx or TLS error rate. |
| Edge and tenant map | bad-config | One cell | Map writes use If-Match on the previous generation, are rejected if more than 1% of routes change, and nodes keep the last known good map. |
| Edge and tenant map | noisy-tenant | That tenant | Per-tenant and per-token rate limits and connection caps, shuffle-sharded edge subsets per tenant (`research/uptime.md#7`). |
| Search index | instance | Search queries in that cell slow down or fail | Multi-AZ OpenSearch domain. Only search endpoints depend on it. |
| Search index | az | Reduced search capacity | Multi-AZ domain. Git, PR, and API writes never touch it. |
| Search index | region | Paired: that Region's domain | Each Region has its own domain fed by the outbox. |
| Search index | control-plane | None at runtime | Domains are pre-created. A rebuild needs no new domain. |
| Search index | bad-deploy | Wrong results in one cell | Mapping changes build a new index and flip an alias, never mutate in place (`research/byoc-grounding.md#41`). |
| Search index | bad-config | One cell | Same alias flip. Old index kept until the new one verifies. |
| Search index | noisy-tenant | That tenant's queries | GitHub's search rate limits per user and per tenant query cost caps. |
| Actions broker and results | instance | Long polls on that node | Runners reconnect to another node. Job leases and assignments are DSQL rows. |
| Actions broker and results | az | One third of broker nodes | Static sizing and zonal shift. |
| Actions broker and results | region | Paired: runners connected to that Region | Runners reconnect through DNS to the other Region. Leases survive in DSQL. |
| Actions broker and results | control-plane | Hosted runner capacity cannot grow | Warm Firecracker pools on running hosts keep starting jobs. Self-hosted runners are unaffected. |
| Actions broker and results | bad-deploy | One cell's job dispatch | Pinned-runner conformance gate before release, canary cell first, automatic abort on job-start SLO burn. |
| Actions broker and results | bad-config | One cell | Same signed config waves. |
| Actions broker and results | noisy-tenant | That tenant's queue | Per-tenant concurrent job limits and fair queuing by tenant in the broker. |

### Change safety

**Deploys.** A release rolls one cell at a time in <vendor-fleet-app> waves, the internal canary cell first, and inside a cell one AZ at a time (`research/uptime.md#4`). Inside an AZ the new binary starts beside the old one with `SO_REUSEPORT`, takes new connections, and the old process drains: API requests finish, long clones get 10 minutes, broker long polls end with a reconnect. The NVMe cache stays warm because the instance never stops. A gate after each AZ checks the SLO burn rate, OCC retry rate, outbox age, and pack quorum backlog. A failure swaps back to the kept binary automatically. This is the in-install abort that the BYOC machinery lacks (`research/byoc-grounding.md` item 11). Node image changes (OS and git) replace instances per AZ through the Auto Scaling group with the same gates. That launches capacity during a deploy, never during recovery.

**Config and feature flags.** Config is code (`research/uptime.md` implication 16). Flags and settings ship as a signed config bundle per cell, rolled through the same waves, validated on load, with last known good kept. There is no runtime flag service. Certificates and keys alarm 30 days before expiry, and a CI test fails before expiry.

**Schema migrations.** DSQL needs one DDL per transaction, so the migration runner applies statements one at a time under a lease row. Every change is expand, then backfill in 1,000-row idempotent chunks, then contract. A release declares its phase in the release record, and only a contract phase sets `compatibility.rollbackBarrier` (`research/byoc-grounding.md` item 9). Release N runs against schema N-1 and N+1. Index builds use DSQL's asynchronous index creation [unverified]. Migrations throttle on commit latency and OCC retry rate, the lesson of GitHub's May 2026 migration at peak (`research/uptime.md#44`).

**Zero-downtime upgrades in every mode.** The hot swap and expand-contract rules are the same everywhere. A single-node evaluation install upgrades the same way, by process handoff on one host. BYOC and self-hosted may skip up to four releases, tested in CI. DSQL engine upgrades are AWS-managed [unverified that they cause no client-visible downtime]. The upgrade never needs maintenance mode, unlike GHES feature releases (`research/uptime.md#55`).

**What the BYOC machinery gets.** The deploy pipeline gains two stage kinds. `roll:nodes` does the per-AZ hot swap and gates. `migrate:expand` and `migrate:contract` wrap the runner. It needs no stateful-fleet stage with fencing, leader handoff, or replica catch-up (`research/byoc-grounding.md` item 7), because no node holds canonical state. The observer API gains `GET /v1/system/observer/storage` with OCC retry rate, commit p99, connection errors, oldest pending outbox row per consumer, unrepaired pack locations, and cache hit rate, and <vendor-fleet-app> gains `storage-healthy` and `outbox-fresh` gate checks (item 8).

## Deployment modes

One signed release bundle: the `tributary` binary for arm64 and x86_64, the node image, upstream git, the web assets, the OpenTofu module, the schema migrations, and the deploy stages. Everything not in this table is identical.

| Aspect | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| Operator | Vendor SRE | Vendor SRE through the ExternalId deploy role | Customer |
| AWS accounts | Vendor cell accounts, one per cell per Region | Customer's dedicated account | Customer's dedicated account |
| Tenants per cell | Many, large tenants on dedicated cells | 1 | 1 |
| Default durability | Paired | Paired | Regional, Paired optional |
| Host and certificate | `<label>.tributary.cloud` or a custom domain, vendor ACME automation | Customer domain, delegated subzone or uploaded certificate | Customer domain and certificate |
| Deploy principal | Vendor CI | Vendor principal plus ExternalId | Customer CI |
| Release timing | Weekly waves after the canary cell | Waves after managed soak, customer may pin up to N-2 | Customer pulls, may skip up to 4 releases |
| Observer role | Vendor | Vendor read-only observer | Optional |
| Hosted runners | Vendor runner accounts | Optional runner account in the customer's org | None; customer runners or ARC |
| Fleet control plane (placement, migration) | `tributary-fleet` | Not deployed | Not deployed |
| Encryption keys | Vendor CMK per cell | Customer CMK | Customer CMK |
| Node layout | Role-split Auto Scaling groups | Combined roles, 3 to 9 nodes | Combined roles, 3 nodes (1 for evaluation) |
| Rate limits | On | Off by default | Off by default |

Reuse of the existing BYOC machinery (`research/byoc-grounding.md`): the release registry, signing, retention, and `rollback-check` transfer unchanged with a Tributary bucket and key (item 1). The bootstrap trust pattern transfers with a `trib-` name prefix and PassRole for Auto Scaling (items 2 and 5). The target document gains a `tributary` block, the release record gains a `product` field, and <vendor-fleet-app> reads one contract per product (item 12). The fleet model, waves, gates, and the rule that automatic advance never starts wave 1 transfer, with a <vendor-fleet-app> "install" standing for a cell (items 4 and 13). Tributary requires its own dedicated account per install (item 6). What Tributary adds: the `roll:nodes` and migration stages, the storage observer route and gates, the automatic in-cell abort, and, for managed only, tenant placement and cell migration.

## Actions

**Workflow engine.** The server owns triggers and filters, matrix expansion, the `needs` graph, job-level `if`, concurrency groups, reusable workflows and `secrets: inherit`, environments and approvals, `GITHUB_TOKEN` minting and `permissions`, OIDC, label matching, job outputs, and retention (`research/actions-protocol.md#62`). The parser is a Rust port of the runner's MIT `GitHub.Actions.WorkflowParser` including `MatrixBuilder`, with `workflow-v1.0.json` as data and `@actions/workflow-parser` as a differential oracle (`#59`, `#60`). The template-token and context-data JSON converters are ported byte-exact (`#19`). Triggering is an outbox consumer, so a run exists only if its push or PR event committed, and run creation is idempotent on `(event id, workflow path)`.

**Runner protocol.** Tributary returns `use_v2_flow: true` and implements the runner admin registration, broker, run service, launch service, and results service (`research/actions-protocol.md#5`, `#6`, `#12`, `#16`, `#17`, `#22`). The legacy distributedtask family is frozen and needs the VSS location service. The broker family is where GitHub invests, and the launch service is the one place Tributary controls which bytes run for each `uses:` (`#17`). Each Tributary release pins one tested runner version, sends `RunnerRefresh` to older runners, and refuses versions below the floor with `RunnerVersionTooOld` (`#25`). JIT configs always set `GitHubUrl` and `IsHostedServer=false` (`#2`). ARC scale-set endpoints (`_apis/runtime/runnerscalesets`, sessions, message queue, `acquirejobs`, `generatejitconfig`) ship at T1 against a pinned ARC range (`#30`).

**Toolkit hostname heuristics and SHA pins.** Tributary is GHES to the toolkit, and nothing on the server can change that, because `isGhes()` reads only `GITHUB_SERVER_URL` (`research/actions-protocol.md#43`, `#44`). The T0 contract is the GHES contract: cache v1 for `actions/cache@v4+`, legacy artifacts for `upload-artifact@v3.2.2` and `download-artifact@v3`, and the runner protocol. Tributary never alters the bytes of an action. The launch service resolves every `uses:` to the exact upstream bytes, from the install's `actions` mirror org, which the vendor syncs from upstream tags and commits byte-for-byte. For `upload-artifact@v4+`, `download-artifact@v4+`, `upload-pages-artifact`, and `deploy-pages`, the vendor publishes forks under `tributary-actions/*` that differ from upstream only in the `isGhes()` test (they also trust `TRIBUTARY_ACTIONS=true`, which every job sets). Each fork release is built reproducibly by CI from the upstream tag, and CI regenerates a signed **pin map** from upstream SHA to fork SHA for every upstream release, so the map is derived, not hand-kept. Substitution is an org policy with two settings. With tag substitution on (the managed default for orgs migrating from github.com), `actions/upload-artifact@v4` resolves to `tributary-actions/upload-artifact@v4`. SHA pins are substituted only when an org admin has approved the vendor's signed pin map, and then only for SHAs that appear in it. Every substitution prints one log line naming both references and both SHAs, and the job's OIDC token and run metadata record the bytes that ran. An unapproved SHA pin to `upload-artifact@v4+` fails exactly as it does on GHES, with an annotation that links the pin-map approval page. I chose this over Gitea's JavaScript rewriting (`#57`) because rewriting changes the bytes behind a SHA without anyone approving it and depends on string patterns in 118 bundles.

**Hosted runner fleet.** Managed hosted runners run unmodified `actions/runner` inside Firecracker microVMs on long-lived metal hosts in separate runner AWS accounts per Region. One VM runs one job and is destroyed. The VM gets a JIT config and reaches the cell only through the public edge, exactly like a customer's runner, so a compromised VM holds only that job's runtime token, `ghs_` token, and OIDC capability. Runner accounts have no IAM trust into cell accounts and no network route to them other than the edge. The instance metadata service is blocked inside VMs, and egress goes through per-pool NAT with published ranges in `/meta`. Warm VM pools on running hosts absorb EC2 launch outages. Pool growth uses EC2 launches, so the Actions job-start SLO, not the core SLOs, carries that risk. Images are built by porting `runner-images` Packer templates to an `amazon-ebs` source (`research/actions-protocol.md#64`, `#65`), starting with a smaller curated Ubuntu image.

**Cache.** Cache v1 (`_apis/artifactcache`) at T0 and twirp `CacheService` v2 at T1, with the Azure dialect shim that accepts Put Blob, Put Block, and Put Block List and returns `x-ms-request-id` (`#35`, `#39`). Downloads are presigned S3 URLs (`#36`). Entries are scoped by repository and ref, with GitHub's restore order (current ref, then default branch) and LRU eviction per repository at 10 GB by a worker consumer.

**Artifacts.** Legacy pipelines artifacts (`#40`) at T0. Twirp `ArtifactService` v4 at T1, uploads through the block shim that maps 8 MiB blocks onto S3 multipart parts in order, and downloads by 302 to presigned S3 URLs (`#37`, `#38`).

**Logs.** Results service twirp with Tributary-signed URLs that accept `comp=appendblock` and `seal=true` (`#22`, `#23`). Blocks land as S3 chunk objects and a worker compacts them per step. Live logs stream over the `FeedStreamUrl` WebSocket and `WorkflowStepsUpdate`.

**OIDC.** Issuer `https://HOST/_services/token`, JWKS under it, the claim set and legacy `sub` format of GHES, and the `OWNER@ID/REPO@ID` format as an opt-in (`#41`, `#42`). Signing keys are per cell, rotated per `key_epoch`, held in memory.

**Coping with GitHub changing the toolkit.** The runner is pinned per release. A nightly conformance job runs the newest runner, the newest major of each first-party action, and the newest `@actions/cache` and `@actions/artifact` against the canary cell, and opens a ticket on the first failure. Fork rebuilds and pin-map regeneration run in the same job. If GitHub merges a vendor-neutral escape such as trusting `ACTIONS_RESULTS_URL` (toolkit issue #2439, `#51`), the forks become unnecessary and the substitution policy is retired.

## Frontend and native app

**Web.** React 19 with Primer React, the stack GitHub itself ships (`research/clients.md#62`-`#64`). The app reads data through the same public GraphQL and REST endpoints that clients use, with persisted queries, plus a small private `/_t/ui/*` API for diffs, file trees, and blame computed by the `git` role. Dogfooding the public API means a UI regression is often a wire regression caught by the same oracles. There is no Node server-side rendering tier, because that would add a runtime and a stateful-looking fleet to every mode. Instead the edge serves the HTML shell and, for the first route, runs that route's persisted GraphQL query and inlines the JSON, so first paint needs no second round trip. Static assets are embedded in the edge binary and served with immutable cache headers, and managed mode puts CloudFront in front of the asset path only.

**Staying up when backends degrade.** The shell and assets come from edge memory, so the UI loads whenever the edge does. Each page region (file tree, diff, checks, timeline, search box) is its own error boundary with its own query, so a search outage empties the search box and nothing else (`research/uptime.md#49`). The client retries only idempotent reads, with a retry budget and jitter, and it never polls faster than `x-poll-interval`. Long diffs virtualize lines, following GitHub's 2026 diff work (`research/clients.md#62`).

**GPUI native app.** It does not earn a place at launch. Unmodified gh, GitHub Desktop, VS Code, and JetBrains already cover native workflows against a GHES-shaped host, and that compatibility is the product. GPUI is pre-1.0 with frequent breaking changes and is tied to Zed's repository (`research/clients.md#57`-`#59`). One narrow scope could justify it later: an offline-capable review inbox for very large pull requests, if the web diff misses its latency target on PRs above 10,000 changed lines. That decision waits for measurements from the web app, and the app would be a pure API client with no private endpoints.

## Build order

**Milestone 1: refs-as-rows git core under unmodified git.** One cell in one Region with a single-Region DSQL cluster and, in the same milestone, a three-Region multi-Region cluster, one S3 bucket per Region, three cache nodes, `commit_push`, `refresh`, `PackStore` with inline and bucket placement, Rust receive-pack, and upstream upload-pack. No REST, no UI. This retires the riskiest unknown, DSQL hot-row and commit-latency behavior under real git traffic, before anything is built on it. **Gate:** a harness drives 64 unmodified `git` 2.51 clients doing `git push --atomic`, non-atomic pushes, and `git clone`/`git fetch` against one repository, with half the clients contending on `main` and half on distinct branches, while it kills cache nodes and drops one AZ's network. Knossos checks the recorded ref histories for linearizability per ref, and Elle checks atomicity per push. The gate passes with zero lost acknowledged pushes, push p99 under 1 s Regional and under 1.5 s Paired, at least 50 pushes per second sustained to one repository, and the measured OCC retry rate recorded.

**Milestone 2: GHES identity surface.** Edge with tenant map, `trib-wire` headers, `/meta`, `/status`, `/api/v3/`, OAuth web and device flows with seeded first-party apps, stored and stateless tokens. **Gate:** `gh auth login --hostname HOST` (device and `--web`), `GCM_TRACE=1 git clone` selecting the GitHub provider with device auth, and JetBrains and Renovate version-header checks.

**Milestone 3: issues, pull requests, merge, GraphQL core.** **Gate:** the cli/cli acceptance suite for `issue` and `pr`, the static validation of VS Code PR, JetBrains, and Renovate operations, and a `@vscode/test-electron` run that lists, reviews, and merges a PR.

**Milestone 4: outbox, webhooks, cell migration, Paired cells.** **Gate:** Argo CD refreshes on push and Jenkins discovers branches and PRs, then a staging Region is cut off at the network layer during continuous pushes and merges, and the checker finds zero lost acknowledged pushes or merges with traffic recovered by health checks alone. A tenant migrates between cells under load with at most 30 s of write freeze.

**Milestone 5: Actions on the v2 flow.** **Gate:** a pinned unmodified `actions/runner` runs a workflow with `actions/checkout@v5`, `actions/cache@v4` (miss then hit), `upload-artifact@v3.2.2`, and `aws-actions/configure-aws-credentials` via OIDC, and `gh run view --log` shows the log.

**Milestone 6: one artifact in three modes.** **Gate:** a self-hosted install from the signed bundle into a fresh account using the existing deploy tool, then an upgrade to the next release under a continuous push, fetch, and merge load with zero failed requests attributable to the upgrade, then the same bundle as BYOC through the vendor principal.

**Milestone 7: breadth.** Terraform, Desktop, ARC, Dependabot, the pin map, cache v2 and artifacts v4, container packages. **Gate:** each remaining row's oracle in "Wire compatibility" at its tier.

## Synthesis decision

Filled in by the arena lead.

## Tradeoffs accepted

- We accept Aurora DSQL's 3,000-row, 10 MiB, and 5-minute transaction limits, which cap atomic pushes at 1,000 refs, cap a branch at 100 open PRs, and turn bulk operations into sagas, in exchange for synchronous cross-Region commits with no failover call and one database that holds every canonical fact.
- We accept a DSQL snapshot read on every git request in exchange for having no cache-coherence protocol, no gossip, and no replica quorum. A cache is either proven fresh by that read or refreshed by it.
- We accept that Tributary serves no stale reads when its cell's DSQL endpoint is unreachable, in exchange for never showing a state that was not committed. That caps Regional cells at DSQL's 99.99% SLA and is why Paired is the managed and BYOC default.
- We accept storing small packs as DSQL rows for minutes, which costs more per byte than S3 [unverified pricing ratio], in exchange for small pushes that are cross-Region durable at RPO zero with one commit and keep working when S3 is impaired.
- We accept about 2.2 times the compute, two to three copies of pack bytes, inter-Region transfer for every pushed byte, and one cross-Region round trip on every write commit (10 to 70 ms depending on the Region trio [guess]) for Paired cells, in exchange for RPO zero on pushes and metadata and Region-loss recovery with no human action.
- We accept a cold-cache penalty the first time a node serves a network in exchange for nodes with no canonical state, which removes the stateful-fleet problem from every deploy and every mode.
- We accept one host per tenant, with no global user graph and no cross-tenant forks, in exchange for identical client behavior in all three modes, tenant-local ids that survive migration, and no shared router on the request path.
- We accept that Tributary runs only in Regions with Aurora DSQL and is tied to an AWS-only engine, in exchange for a self-hosted install with no database to operate.
- We accept up to 30 s of write freeze per tenant migration, in exchange for a migration that needs no dual-write mode.

## Alternatives considered

- **S3 write-ahead log with a CAS manifest per repository (the Continuity and walgit shape).** Refs live in an S3 manifest updated by `If-Match`, and metadata lives in a separate database (`research/git-storage.md#29`-`#31`). It lost because refs and pull requests then live in two stores with no shared transaction, so a merge needs an ordered two-step write with a repair path, and the outbox needs a third coordination point. Its cross-Region story is also weaker: S3 offers no synchronous cross-Region CAS, so push RPO zero needs a second mechanism on top of the log. Its interface is deeper for git alone and shallower for the whole product, because every consumer of ref state must also learn the manifest protocol.
- **Spokes-style replica quorum on local NVMe** (`research/git-storage.md#2`-`#6`, `#23`). Three replicas across three AZs vote on each ref transaction through the reference-transaction hook, with a placement database and repair sweepers. It lost on operability and on BYOC: every node holds canonical state, so every deploy needs fencing, replica catch-up, and quorum checks that the vendor's machinery lacks (`research/byoc-grounding.md` item 7), and self-hosters would repair replicas at 2am. It still needs a separate database for PRs, so ref and PR state can diverge.
- **The same refs-as-rows shape on Aurora PostgreSQL with a Global Database.** It would remove the 3,000-row limit and keep triggers and full PostgreSQL. It lost on facts 1 and 9: unplanned cross-Region failover loses seconds of commits, the failover is an RDS control-plane call, and Aurora does not guarantee to fence the old primary (`research/uptime.md#24`, `#25`). It is a database variant of this design, not a different shape, and it is the fallback if milestone 1 shows DSQL cannot carry hot repositories.
- **Refs in DynamoDB MRSC with metadata elsewhere.** RPO zero across three Regions (`research/uptime.md#29`), but no transactions, so a merge cannot commit the ref, the PR, and the event together.
- **A github.com-style shared namespace across tenants in managed mode.** It would allow cross-tenant forks and a global user graph. It lost because it needs a global identity store and a global router on every request, and because managed would then behave differently from BYOC and self-hosted.

## Open questions and risks

- Can DSQL carry the hottest repositories? Merge queues and bot fleets on one `main`, plus the 40 shard rows, are where optimistic concurrency could thrash. Milestone 1 measures this first. If it fails, should hot repositories get a per-repository write serializer in the `git` role, or should the design move to Aurora PostgreSQL and give up RPO zero?
- What is DSQL multi-Region commit latency for the Region trios customers need, and which trios does DSQL support? Research found no published numbers (`research/uptime.md` open question 3).
- Is Aurora DSQL available in every Region where BYOC and self-hosted customers need it? A missing Region blocks an install outright, because this design has no second database engine.
- Is the per-byte cost of inline packs and of CDC export acceptable at managed scale, and is 1 MiB the right inline threshold?
- Is the signed pin map an acceptable answer for security teams that pin SHAs, or will they see any substitution as rewriting?
- Does DSQL CDC deliver to a consumer we can run in the recovery Region with a lag that supports the stated 5-minute Regional RPO? [unverified]
- Does returning `GitHub lives!` and GitHub documentation URLs, which VS Code Pull Requests and other clients check, raise trademark issues for the vendor?
- Where this direction is weakest: Actions leases, step updates, and cache metadata are high-churn, low-value state, and a key-value store would be cheaper than DSQL for them. This design keeps them in DSQL anyway, writing leases only near expiry, so the comparison stays honest. Would a separate store for Actions churn be worth a second stateful system?
- Does any GHES-targeted client assume 40-character installation tokens?
- Does vendor-to-customer role chaining cap BYOC deploy sessions at one hour (`research/byoc-grounding.md` open question 1)? A cell rollout with 10-minute clone drains in three AZs fits, but only barely.

## Next implementation step

Build `trib-store::commit_push` and `refresh` against a multi-Region DSQL cluster, and the milestone 1 harness that drives unmodified `git push --atomic` clients and checks ref histories with Knossos and Elle.
