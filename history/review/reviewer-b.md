I found 12 problems: four critical and eight warnings. I was read-only on the design. The one thing I ran was a throwaway git experiment in `/tmp` (since deleted) to test finding 1.

### 1. [critical] Content-addressed pack ids collide on ordinary pushes, and reused ids can lose data
**Location**: DESIGN.md:216, 219, 256, 268, 274, 276, 89, 548
**Finding**: `PackId` is the pack's trailing checksum (DESIGN.md:216). The `pack` and `pack_inline` rows are keyed `(tenant, network, pack_id)` (DESIGN.md:268), and every push inserts a pack row (DESIGN.md:256, 90). A pack can only go from `Live` to `Superseded`, never back (DESIGN.md:219). Nothing in the design handles a second commit whose pack has the same id as an existing row.
**Evidence**:
- *Measured* (git 2.50.1, scratch repo in `/tmp`, since deleted): I pushed `HEAD:refs/heads/b3` and then `HEAD:refs/heads/b4` at a commit the remote already had. Both sent the same 32-byte empty pack (`PACK`, v2, 0 objects), with the same trailer `029d0882…`.
- *Inferred*: so the second "new branch at an existing commit" push anywhere in a network hits a duplicate key in `pack` or `pack_inline`. That error is not 40001, so `run_tx` does not retry it. The push fails, and every later push of that shape in the network fails the same way. The same applies to lightweight tags.
- *Inferred*: suppose the implementation instead ignores the conflict (`ON CONFLICT DO NOTHING`, or treating S3's 412 on `If-None-Match` as success). Then the new ref depends on a row that may already be `Superseded`:
  - Compaction has already deleted the inline bytes (DESIGN.md:276).
  - GC's "no live row has the same id" check (DESIGN.md:274) looks at that same superseded row and deletes the S3 object after 7 days.
  - `rest:git` blob creation (DESIGN.md:548) writes single-object packs, which are deterministic. If the first copy of a blob was unreachable when compaction ran, compaction dropped it. Re-creating the blob and committing a tree that uses it then leaves a ref pointing at a missing object. That is lost data on an acknowledged write, and it breaks the reachability invariant.
- *Inferred*: packs are PUT before `BEGIN` (DESIGN.md:89). When the commit then fails (Stale, protected ref, or a crash), the object has no row. GC only walks superseded rows, so these orphans are never deleted and leak without bound.

**Suggestion**: Write no pack row for zero-object packs. Give each placement its own id (for example `push_id`) and keep the content hash only as an attribute. Alternatively, let an insert flip a `Superseded` row back to `Live` inside the commit transaction. Add an orphan sweeper keyed on object age with no row, with a grace period longer than the longest push.

### 2. [critical] Tenant migration has no fence in the data layer, so acknowledged pushes can be lost
**Location**: DESIGN.md:161, 176-181, 664, 666
**Finding**: Freeze and Flip are enforced only through the tenant map, which each edge reads on its own. Edges are designed to keep a stale "last known good" map (DESIGN.md:664, 666). The old cell's DSQL never rejects writes for the moved tenant.
**Evidence**: *Inferred* failure path:
1. Edge E fetches the generation where the tenant is `Frozen { until }` (DESIGN.md:161), then fails to fetch the Flip generation.
2. When `until` passes, E has no newer state and serves the tenant locally again.
3. Pushes are acknowledged by the old cell's DSQL after the CDC delta was declared drained.
4. Forget deletes those rows after 7 days (DESIGN.md:180).

Several other write sources bypass the edge freeze entirely:
- Receive-packs admitted before Freeze that commit after it.
- `worker` outbox consumers (pr-sync, Actions triggering, run owners).
- Runners connected to the old cell's broker.

This contradicts "zero acknowledged pushes lost" (DESIGN.md:632).
**Suggestion**: Make Freeze a DSQL row in the source cell (`tenant_state = Frozen`, fenced by generation). Every `run_tx` for the tenant checks it, including worker and background transactions. Stop the CDC drain only once that row has committed and the in-flight transaction horizon (the 2 s deadline plus the 5-minute DSQL maximum) has passed. An expired `Frozen` must never fall back to Serving.

### 3. [critical] The merge's write-skew guard is defeated by its own retry and by checking protection outside the transaction
**Location**: DESIGN.md:125, 381-383, 460, 98, 361-362
**Finding**:
- Protection is evaluated on an early snapshot (DESIGN.md:381: "snapshot -> check_protection -> git.plan_merge -> pack_store.place -> run_tx {…}").
- The transaction only does a blind "bump review_version" (DESIGN.md:382).
- `run_tx` retries the closure on 40001 from a fresh snapshot (DESIGN.md:460), and the closure does not re-run `check_protection`.

**Evidence**: *Inferred*:
- DSQL's OCC only detects conflicts between transactions that overlap in time. A "Request changes" or a dismissal that commits during `merge-tree` or pack placement lands before `run_tx` begins. There is no conflict, and the merge commits on stale reviews.
- When the transactions do overlap, the 40001 that DESIGN.md:125 relies on is simply retried. The base CAS still passes and the merge commits. That is a branch-protection bypass.
- The transaction also never checks that the PR is still `Open`, so a PR closed in that window becomes `Merged`.
- Journey 2 reads `refs/pull/17/head` as the head (DESIGN.md:98). That ref is derived and lags by up to 10 s (DESIGN.md:388, 609), which contradicts DESIGN.md:361-362. A user who pushes a fix and runs `gh pr merge` right away gets the previous head merged, checked against that head's `check_run` rows. The PR then closes as merged.

**Suggestion**: Run `check_protection` inside the closure on the transaction's snapshot. CAS `review_version = :seen` and `state = Open`. Read the head branch's own row, and force it into the conflict set (CAS its target against `plan.head`). A changed input should return a non-retryable error, not another retry.

### 4. [critical] Outbox dispatch blocks behind its oldest rows, so a search outage stops webhooks, Actions and pr-sync
**Location**: DESIGN.md:453, 465-467, 477
**Finding**: Dispatch selects "pending rows in partition ORDER BY id LIMIT 500" and calls every consumer whose bit is unset (DESIGN.md:465-466). Rows are deleted only when all bits are set (DESIGN.md:467). DESIGN.md:477 says a slow consumer "leaves its bit clear while the other consumers move on".
**Evidence**: *Inferred*:
- With OpenSearch down, the 500 oldest rows in a partition eventually all wait only on `SearchIndex`. Every loop re-selects those same 500, and newer rows are never read.
- So `Webhooks`, `ActionsTrigger`, `RunOwner` (run status), and the `credential.revoked` cache hint all stop for that partition.
- Before that point, the down handler is called inline per row and waits on each timeout, which collapses throughput.
- pr-sync, which moves `refs/pull/N/head`, emits `synchronize`, and closes PRs when their head branch is deleted (DESIGN.md:388), is not in the `Consumer` enum at all (DESIGN.md:453). Its progress model is unspecified.

The search index row says "Git, PR, and API writes never touch it", but PR-triggered CI and webhooks would stop.
**Suggestion**: Give each consumer its own cursor or pending index, for example a per-consumer pending table or a partial index per bit. Run handlers under per-consumer concurrency with circuit breakers. Add `PrSync` as a consumer.

### 5. [warning] In Regional cells, compaction deletes inline bytes before their replacement reaches the DR Region
**Location**: DESIGN.md:145, 274, 276, 277, 642
**Finding**: Compaction moves inline packs to S3 "within minutes and deletes the inline rows" (DESIGN.md:276). The DR copy of DSQL follows CDC (5-minute target), but bucket packs follow S3 Replication Time Control (15 minutes) (DESIGN.md:277).
**Evidence**: *Inferred*:
- If the Region is lost after CDC has shipped the supersede and the inline delete, but before replication has shipped the compacted pack, the DR cluster has live pack rows whose bytes exist nowhere in DR.
- A push acknowledged 10 minutes before the loss is then lost, even though it is older than the 5-minute metadata RPO the design gives for inline packs (DESIGN.md:277).
- Its refs dangle, which breaks the reachability invariant (DESIGN.md:274) and corrupts repositories instead of just rewinding them.
- No recovery step reconciles refs against the bytes that are actually present, and failback (the original cluster's commits that never reached DR) is not described.

**Suggestion**: In Regional cells, delay supersede and inline deletion until the replacement object is confirmed in the DR bucket (HEAD or replication status). Define a recovery-time reconciliation that rolls refs back to the newest `ref_log` entry whose packs are present.

### 6. [warning] Region failover depends on health checks the design never specifies, and the edge is built to look healthy
**Location**: DESIGN.md:83, 194, 598, 642, 663, 759
**Finding**: Paired failover is "Route 53 health checks move traffic in 1 to 3 minutes" (DESIGN.md:642), but the design never says what the health check probes.
**Evidence**: *Inferred*:
- The edge deliberately stays up and returns well-formed 503s when DSQL is unreachable (DESIGN.md:194), and `GET /status` returns 200 (DESIGN.md:598).
- If a Region's DSQL endpoint loses quorum while its edge and NLB are fine, a shallow check keeps routing that Region's share of traffic into errors indefinitely.
- The milestone 4 gate ("traffic recovered by health checks alone", DESIGN.md:759) tests a network cut, which also kills the edge. So it never exercises this case.

**Suggestion**: Use a deep regional health endpoint that commits a canary DSQL write and reads a canary S3 object through the local endpoints. Rely on Route 53's fail-open when every target is unhealthy, and add a velocity cap.

### 7. [warning] A Paired cell's tenant map, certificates and keys can depend on one Region
**Location**: DESIGN.md:164, 337, 664, 777
**Finding**:
- The tenant map is one object, `s3://trib-<cell>-control/tenant-map.json` (DESIGN.md:164).
- Map and certificates are "cached in memory" (DESIGN.md:664).
- Signing keys are KMS-decrypted at node start (DESIGN.md:337).

The design names no per-Region copy of the control bucket, no local-disk last known good, and no multi-Region KMS key.
**Evidence**: *Inferred*: after losing the Region that holds the control bucket or the key, any edge process that starts in the surviving Region has no map, certificates or keys, so it serves nothing. That includes Auto Scaling replacements, crash restarts, and the hot-swap process itself. Map writes (freeze, placement) are also impossible. This contradicts "Region-loss recovery with no human action" (DESIGN.md:777).
**Suggestion**: Keep one control bucket per cell Region, both written with `If-Match`. Persist the last known good map, certificates and wrapped keys to local disk. Use multi-Region KMS keys.

### 8. [warning] Stateless installation tokens outlive uninstall, suspension and repository removal
**Location**: DESIGN.md:297, 300, 337-338
**Finding**: `ghs_` tokens embed `repos` and `permissions` and are "verified with no database read", with revocation through a jti deny list (DESIGN.md:297, 300). DESIGN.md:338 claims "there is no cache of grants to go stale".
**Evidence**: *Inferred*:
- Uninstalling or suspending an app, removing a repository from an installation, or downgrading permissions leaves every outstanding token working until it expires.
- The server never stored the jtis, so it cannot list them to deny.
- `authorize` is never said to intersect the claims with installation rows.
- How the deny list is delivered, and how a node reloads it after a restart, is unspecified. If it rides the outbox, finding 4 delays it.

**Suggestion**: Add a per-installation "not before" epoch to the pushed deny list. For requests that already read a DSQL snapshot, have `authorize` intersect the claims with installation rows. Record jti with exp per job so `GITHUB_TOKEN` can be revoked at job end.

### 9. [warning] Webhooks are retried automatically for 3 days, which GitHub never does
**Location**: DESIGN.md:477, 602-612
**Finding**: Delivery workers "retry for 3 days" (DESIGN.md:477). *Measured* from the research: "GitHub never redelivers automatically" (`research/wire-census.md#37`, line 93). This deviation is missing from "Known deviations" (DESIGN.md:602-612).
**Evidence**: *Inferred*:
- A Jenkins or deploy receiver that does its work and then exceeds the 10 s deadline, or returns 5xx, gets the same event re-POSTed. The GUID is the same, but most receivers do not deduplicate, so builds and deploys run twice.
- A receiver that comes back from an outage gets three days of stale `push` events replayed.

**Suggestion**: Make one attempt, as GitHub and GHES do, and keep manual redelivery through the API. Offer retries only as an opt-in that is documented as a deviation.

### 10. [warning] Broker label matching by exact hash cannot do subset matching
**Location**: DESIGN.md:436
**Finding**: The hot query uses the index `(tenant, label_set_hash, state, since)` (DESIGN.md:436).
**Evidence**: *Inferred* from GitHub's semantics (the job's labels must be a subset of the runner's labels; the research says the server owns label matching, `research/actions-protocol.md#62`):
- A runner registered with its default labels `self-hosted, Linux, X64` hashes differently from a job with `runs-on: self-hosted`. The job never starts.
- The alternative, querying every subset of the runner's labels, costs 2^n queries per poll.

**Suggestion**: Keep a small per-tenant table of distinct queued label sets and test subsets in broker memory. Then query by the matching hashes. Label case also needs normalizing.

### 11. [warning] "Skip up to four releases" contradicts N±1 schema compatibility
**Location**: DESIGN.md:693, 695
**Finding**: "Release N runs against schema N-1 and N+1" (DESIGN.md:693), yet self-hosted and BYOC "may skip up to four releases" with zero downtime (DESIGN.md:695).
**Evidence**: *Inferred*:
- During the per-AZ hot swap, binary N keeps serving while the schema advances toward N+5.
- If any release between them is a contract phase (the `rollbackBarrier`), it removes something binary N still reads. Requests fail mid-upgrade unless every N node is drained first, which is downtime.

**Suggestion**: Define skip upgrades as stepping through each contract boundary: deploy the intermediate binary for each barrier, automated. Or require that skips never cross a contract phase, and have CI enforce it.

### 12. [warning] The 1% route-change guard blocks dedicated cells and migrations out of small cells
**Location**: DESIGN.md:666, 192, 179
**Finding**: Map writes "are rejected if more than 1% of routes change" (DESIGN.md:666).
**Evidence**: *Inferred*:
- A dedicated cell (DESIGN.md:192) holds one tenant, so adding that tenant changes 100% of its routes.
- Any cell with fewer than 100 tenants rejects every single placement, freeze, or flip (DESIGN.md:179).

So the guard blocks exactly the dedicated-cell and migration paths the design depends on.
**Suggestion**: Use an absolute cap (for example at most N routes per generation), plus an explicit, signed override for planned migrations.