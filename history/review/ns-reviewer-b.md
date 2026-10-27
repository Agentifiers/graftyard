I found 10 problems in the namespace layer: four critical and six warnings. The worst is that a replica that has stopped updating keeps granting access with no time limit, which breaks the revocation guarantee the design claims to make. Labels: **measured** means read in the cited lines, **inferred** means my reasoning from them, **guess** means a number I estimated.

### 1. [critical] Stale replicas keep granting access with no time limit, and the drill requires it
**Location**: DESIGN.md:263, DESIGN.md:428-431, DESIGN.md:441, DESIGN.md:860, DESIGN.md:892, DESIGN.md:997, DESIGN.md:1044
**Finding**: The design serves "yes" answers at once, and the only bound on their staleness is apply lag (263, measured). Nothing puts a ceiling on that lag. If an applier partition parks, "existing facts keep serving" (860, measured). `revoke` gives up after 5 s and returns `covered: false` (428-431, measured). The design never says what happens to the user's request then; [inferred] it succeeds. "Covered" also counts the front publisher's cursor, not the edges: the followers are "one applier per data cell and one publisher per front cell" (445), while edges tail S3 separately (273). So "no front … honors the removed fact" (441) is false. The weekly drill and Milestone 4 require zero failed pushes or merges during a 30-minute cut from `global` (892, 997), which means honoring stale grants for 30 minutes is a pass condition.
**Evidence**:
- Interleaving: data cell `c7` is partitioned from `global`, which stays up. An owner removes `mallory` from `acme`. `revoke` waits 5 s, gets `covered=false`, and the owner gets success [inferred].
- Mallory's PAT is unchanged, so fronts still accept it. `c7`'s `g_membership` still lists her, so she can push and merge for as long as the cut lasts.
- Edge variant: an edge that refused a segment (finding 2) keeps accepting revoked tokens indefinitely. Its health check is a synthetic routed request (272), which [inferred] still passes.
- This is the very defect the synthesis used to reject candidate 2: "unbounded lag while its cell is cut off" (1067, `arena/round2/SYNTHESIS.md:35`).
- Candidate 3, rejected for being "15 minutes stale" (1068), actually had a compiled cap on write staleness. It also marked a node that cannot reach `graph` while its peers can as unfit for writes (`arena/round2/candidate-3/DESIGN.md:194`, `:344`). On this criterion the chosen layer is weaker than the shapes it rejected.
- Tradeoff 1044 ("up to 5 s") understates the exposure.
**Suggestion**: Add a maximum staleness per decision class. Writes and private reads become `Behind`/503 when the deciding partitions' watermark is older than N seconds. An edge removes itself from service when its applied watermark is stale. `revoke` should count edges, not publishers. Change the drill to expect write 503s after the bound.

### 2. [critical] The 0.1% removal guard freezes projections on routine deletes, worst in small installs
**Location**: DESIGN.md:222-224, DESIGN.md:868, DESIGN.md:923, DESIGN.md:937, DESIGN.md:438
**Finding**: `apply_segment` refuses any segment that removes more than 0.1% of entries, and it has no override (222-224, measured). It also refuses any segment whose generation "does not follow", so after one refusal every later segment is refused too. The projection now holds credential hashes as well as names (219-220), and revoked tokens are removed after 30 days (438). BYOC and self-hosted installs run the same edge and publisher code (923, 937).
**Evidence**:
- A self-hosted install with about 1,000 entries (users, repos, tokens) has a 0.1% budget of one entry. The 30-day purge of two revoked tokens, or a repository deletion plus its alias, landing in one 250 ms segment trips the guard on every edge.
- Every later segment is then rejected. New PATs, OAuth logins, and repositories become `Behind` and get 503 indefinitely. Revocations never reach the edge.
- Managed mode at launch scale has the same small-N problem.
- At any scale, a legitimate bulk removal (an org-deletion saga) hits every front cell at once. That is a fleet-wide correlated freeze; the `bad-config` row (868) presents the guard as protection against exactly that.
- The guard came from candidate 2, where it watched a route book with no credentials and had "not newer" semantics plus a signed override (`arena/round2/candidate-2/DESIGN.md:142-143`).
- Whether the hourly snapshot reload bypasses the guard is unspecified (218).
**Suggestion**: Use a threshold that is both absolute and relative. Give removals a signed override like slot moves have, and exempt credential-tombstone expiry. Define recovery as a verified snapshot reload with an alarm.

### 3. [critical] A `global` outage turns every "no" in every cell and front into 503
**Location**: DESIGN.md:11, DESIGN.md:210-213, DESIGN.md:615-616, DESIGN.md:425-427, DESIGN.md:853, DESIGN.md:1043, DESIGN.md:201-202, DESIGN.md:860
**Finding**: `Absent`, `Forbidden`, and `Hidden` all require the watermark to be at or past `asked_at + 50 ms` (210, 425, measured). Idle partitions' watermarks advance only through heartbeat entries, which are writes to `global` (615-616, measured). If `global` cannot take writes, every watermark freezes. The stated blast radius, "identity writes" only (853, 1043, 11), is therefore wrong.
**Evidence**:
- `global` takes a bad deploy at T0. From T0 + 2 s, each of these waits 2 s and then gets 503 in every cell:
  - anonymous `GET` of a private or nonexistent repository;
  - `git clone` of a mistyped URL;
  - `GET /orgs/o/members/u` and `GET /repos/o/r/collaborators/u`, where 404 means "no";
  - unknown or typo'd tokens;
  - [inferred] a push declined by branch protection, because that decision rests on the absence of a bypass grant.
- Per-cell read SLOs count 5xx (803). A cell whose anonymous traffic is more than 5% 404s burns minutes in every cell.
- Login and credential rows sit in the account's partition (201-202, measured). The absence of an unknown login or token hash therefore depends on all 64 partitions, not on 1-2 as `Behind { parts: SmallVec<[_; 2]> }` (211) and the "in that partition" wording of 860 imply. One lagging partition 503s every unknown login and token.
- The cap of 16 waiters per IP (869) then 503s users behind NAT on ordinary misses.
**Suggestion**: Derive watermarks from the follower's snapshot read time instead of written heartbeats, so a write outage does not freeze them ([inferred] this requires DSQL to expose that time). Add a bounded stale-negative mode while `global` is unreachable, and state the real blast radius in 853 and 1043.

### 4. [critical] Owner rename and login reclaim bump every repository's epoch, which cannot fit one DSQL transaction
**Location**: DESIGN.md:192-196, DESIGN.md:262, DESIGN.md:265, DESIGN.md:137
**Finding**: `NameEpoch` is bumped on "owner rename … or a reclaim of the owner's old login" (192-193, measured). A reclaim "bumps the epoch of every repository it used to reach" (265). Yet each of these is promised as "one `global` transaction of 3 to 8 rows" (262), and line 195 says an owner rename "rewrites no repository row". These statements contradict each other.
**Evidence**:
- Org `bigco` with 5,000 repositories renames, or someone later claims `bigco`. That is at least 5,000 `repo_dir` updates plus their log after-images, past DSQL's 3,000-row / 10 MiB limit (137). The rename cannot commit.
- If it is chunked into a saga, there are windows where `name(bigco) = Live{B}` while unbumped repositories still carry epoch e. A front holding `Former{A}` (a positive answer, served at once, 263) sends e. `check_epoch` says `Current`, and the cell serves A's repository at a URL that B now owns.
- That is exactly what `NameEpoch` was grafted to prevent. The synthesis rejected candidate 3's 1-hour quarantine on the grounds that `Behind` plus `NameEpoch` "make reclaims safe" (`arena/round2/SYNTHESIS.md:37`), which holds only if the bump is atomic.
**Suggestion**: Put the routed owner account id in the `Principal` assertion. The cell then checks `g_name(login).account == g_repo_dir.owner` in the snapshot it already reads, so owner renames and reclaims cost O(1) rows. Keep per-repository epochs for repository-level renames and transfers.

### 5. [warning] Slot moves race the `global` applier: lost webhooks, orphaned names, or a permanently parked partition
**Location**: DESIGN.md:89, DESIGN.md:214-217, DESIGN.md:247-256, DESIGN.md:271, DESIGN.md:860
**Finding**: Two things are not ordered. "Every `repo_dir` row placed in the cell gets its local rows" and "every `global` change that concerns the cell's repositories … inserts its webhook outbox event" (271, measured) are done by the applier. The applier's repository-scoped `run_tx` is fenced by `slot_state` (214-215). The Flip commits `slot_map` in a separate `global` transaction (252-253), whose partition has no order relative to the repository's own partition (202).
**Evidence**:
- Fork interleaving (89): `global` commits `repo_name(bob, widgets)`, then Freeze lands, then `c7`'s core transaction is refused. Repair falls to the applier, but:
  - The source applier sees the slot as Frozen and then Moved, never Serving again. Its transaction is refused, retried 10 times, and the partition parks (860). Replay keeps failing.
  - Alternatively it skips, and the target applier, which applied the entry while its own `slot_state` was Frozen or before it saw the Flip, also skips.
- Result: `bob/widgets` is claimed in `global` with no rows in any cell. That gives a permanent 404/500, and bob's re-fork gets a numbered suffix.
- A rename during Freeze through Flip loses its `repository.renamed` event the same way, breaking the "exactly one producer" claim (271).
**Suggestion**: Make slot ownership for applier purposes an explicit step of `move_slot`. Have the target re-scan `repo_dir`/`global` entries for the slot from the Freeze timestamp after Flip. The applier should skip fenced slots, never retry-park on them.

### 6. [warning] The index cell is a hidden single point for sign-in and App tokens across all cells
**Location**: DESIGN.md:112, DESIGN.md:124-125, DESIGN.md:396-398, DESIGN.md:437, DESIGN.md:240-242, DESIGN.md:1067
**Finding**: Identity writes, which include sign-in, OAuth device and web flows, and PAT creation (231, 437, 443), route "to the caller's index cell" (112, measured). Installation tokens can only be minted by the installation account's index cell (124, 396-398). Repositories are placed in other cells whenever the index cell lacks headroom (240-242).
**Evidence**:
- `c4` takes a bad deploy, which the design describes as contained to one cell (825). Alice, indexed at `c4`, cannot sign in, `gh auth login`, or create a PAT, even though `global` is healthy and her work is in `c7`.
- `deploybot` cannot mint tokens for `beta` while `c12` is down, including for `beta` repositories placed in other cells.
- That is GitHub's May 2023 App-token failure (`research/uptime.md#44`) crossing cell boundaries, and the same "home down, cannot authenticate" failure used to reject candidate 2 (1067).
- No failure-mode row lists it.
**Suggestion**: Let any healthy cell run sign-in, OAuth, and PAT handlers, since they only write `global`. In `kid_allowed`, allow a deterministic standby minting cell (for example the index cell's pair) to keep compromise scoping.

### 7. [warning] Every follower writing heartbeats to `global` causes a contention storm on the log heads
**Location**: DESIGN.md:614-616, DESIGN.md:445, DESIGN.md:429, DESIGN.md:989
**Finding**: "Any follower that sees the head older than 200 ms" appends a heartbeat (615-616, measured). There are about 130 followers polling every 100 ms (445), each with one connection (143, 445). `revoke` also reads cursors from `global.follower` (429), which implies cursor writes there too, while 614 says cursors live in the follower's own cluster.
**Evidence** (counts are guesses):
- For each idle partition, about 130 followers see the stale head within the same 100 ms and all try to bump the same head row. One wins and the rest hit 40001.
- Real appends (signups, revocations) contend with that storm and can exhaust their 3 retries.
- Each follower may owe up to 64 heartbeat commits per cycle on one connection. At 10-70 ms per cross-Region commit (1041), that is 0.6-4.5 s per 100 ms cycle, so follower lag grows and breaks the 1 s / 2 s freshness SLO (810).
- The Milestone 0 gate runs one follower (989), so it cannot detect this.
- Minimum latency for any negative answer is also set by heartbeat cadence plus segment cadence.
**Suggestion**: Use one leased heartbeat writer per partition, or snapshot-time watermarks (see finding 3), and run the gate with the real follower count.

### 8. [warning] Every slot move fails in-flight Actions jobs and their `GITHUB_TOKEN`s
**Location**: DESIGN.md:397-398, DESIGN.md:249-250, DESIGN.md:253, DESIGN.md:961, DESIGN.md:997
**Finding**: Index-cell moves get a grace period of one token lifetime (397). Slot moves do not: "a job token's kid.cell must hold the job repository's slot" (398, measured). Freeze refuses connected runners (249-250), `RunServiceUrl` names the old cell's route (961), and the source only "proxies until fronts route by Flip" (253).
**Evidence**: A 2-hour release job is running in slot s when `tributary-fleet` moves s, which is routine rebalancing (60, 269). After Freeze, `renewjob` and log writes are refused. After Flip, the job's `ghs_` token fails `kid_allowed`, so `gh release upload` gets 401. The lease then expires to `Completed{Failure}`. Every job in every network in the slot fails, and this deviation is not listed (781-790).
**Suggestion**: Accept the slot's previous cell in `kid_allowed` for one job-token lifetime, using the slot generation history in the projection. Have the source proxy run-service calls to the target for the maximum job duration instead of refusing them.

### 9. [warning] Transfers cannot honor residency, and refusing them is an unlisted deviation
**Location**: DESIGN.md:267, DESIGN.md:269, DESIGN.md:176-183, DESIGN.md:951, DESIGN.md:781-790
**Finding**: "The network and its slot do not move" on transfer (267, measured). Ids carry their slot for life (176-183), so a single network can never change cells. Yet a resident org "places all its networks … in its residency domain" (951).
**Evidence**: `bob/tool` in a US cell is transferred to `beta` (EU residency). The network stays in the US, breaking the residency promise. Moving it would mean moving the whole slot along with other owners' networks, possibly of other residencies. Refusing the transfer breaks `gh repo transfer` and the transfer API, and the known-deviations table (781-790) does not list it. An org that turns on residency later hits the same problem.
**Suggestion**: Handle a cross-residency transfer like a detached fork: copy into a resident slot and leave an `Alias`. Otherwise return 422 and list it as a deviation. `move_slot` should also check residency for every network in the slot.

### 10. [warning] Rate limits counted per front node multiply the limits and confuse clients
**Location**: DESIGN.md:943, DESIGN.md:945, DESIGN.md:855
**Finding**: "Counters are per front node and approximate" (943, measured), and signup throttling uses the same front limits (945).
**Evidence**:
- With N edge nodes across a Region's front cells, reachable through multivalue DNS and direct NLB IPs, one IP gets about 60×N anonymous requests per hour and N times the signup cap. Signups are the main threat to `global`'s write capacity (855). N in the tens is a guess.
- Clients pinned to different connections see `x-ratelimit-remaining` go up and down and disagree with `GET /rate_limit`. [inferred] This misleads Octokit throttling and Renovate's budget logic.
- Not listed as a deviation.
**Suggestion**: Shard counters by key to one owning node per front cell (consistent hash), with a Region-level budget split. Keep the signup and identity-write caps in the identity handler, where a `global` transaction already runs.