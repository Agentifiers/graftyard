### 1. [critical] When `global` is down, every denial and every not-found becomes a 503, across the whole service
**Location**: DESIGN.md:209-213, 263, 423-425, 615-616 (contradicts 11, 853, 1043)
**Finding**: The design treats a `global` outage as losing only identity writes. Under its own `Lookup::Behind` rule, it also turns every negative answer into a 503: 404s, 401s, and 403s.
**Evidence**:
- Measured from the text:
  - `Absent` exists only when the watermark is at or after `asked_at + 50 ms` (209-212).
  - Watermarks advance only through log entries or follower heartbeats, and heartbeats are appends to `global` (615-616).
  - `Forbidden` and `Hidden` turn into `Behind`, which retries for 2 s and then returns 503 (213, 423-425).
- Inferred failure path:
  1. `global` becomes unreachable at T0, so every watermark stops at T0.
  2. Any request after T0 gets 503 where it should get 404, 401, or 403. Examples:
     - an anonymous `git clone` or `GET /repos` of a private repository;
     - a nonexistent `owner/repo`;
     - an unknown token, or one revoked more than 30 days ago (438);
     - a member without write access.
  3. Clients misbehave. Git and GCM only clear and re-prompt for credentials on a 401. A 503 makes Renovate abort with a host error. Terraform retries the 5xx and then fails.
  4. Inside each cell, every `Hidden` retries "on a new snapshot" for 2 s (424). Anonymous scanners hitting private repositories therefore multiply `core` reads in every cell at once.
- Blast radius: the same happens to one data cell cut off from `global`, and to one front whose projection stalls.
- Why nothing catches it: the summary (11), the `global` bad-deploy row (853), and the tradeoff (1043) list only identity writes. The Milestone 4 gate (997) counts only failed fetches, pushes, and merges, so it passes while this happens.
- The rejected shape avoids this concretely. `arena/round2/candidate-2/DESIGN.md:153` answers an anonymous unknown login with a 404 straight from its book. Its 195-196 authorize from canonical rows in the owning cell, so an outage of its registry does not turn denials into 503s.
**Suggestion**: State this blast radius in the rows and SLOs, or define a degraded mode. For example, after a global-unreachable signal, answer "no" from the last watermark and accept a bounded risk of wrongly denying something granted in the last second before the outage.

### 2. [critical] A frozen slot stops `global` replication for the whole cell, and a stuck move stops it for good
**Location**: DESIGN.md:271, 575-578, 614-618, 214-217, 248-250
**Finding**: The applier applies each `global` entry and emits that change's outbox event in the same transaction. That transaction goes through the slot-fenced `CellStore::run_tx`, so a frozen slot blocks it.
**Evidence**:
- Measured from the text:
  - The applier inserts the webhook outbox event "in the same transaction that applies it" (271), through `follow(.., dst: &dyn CellStore, ..)` (617-618).
  - `run_tx` reads the slot fence of whatever it writes and refuses unless the slot is `Serving` (575-576).
  - Freeze sets `Frozen` in the source cell (249-250). The target cell is also `Frozen` for the whole Clone phase (248). `Frozen` never expires (215).
  - The follower's cursor advances inside the apply transaction, so entries apply strictly in order (614-615).
- Inferred failure path:
  1. During a move, the first `global` entry that touches the slot (a grant, rename, visibility change, or fork claim) cannot commit.
  2. Its whole partition stops in that cell for up to 60 s (1054). Every lookup in that partition, cell-wide, turns into a `Behind` 503.
  3. Revocations in that partition do not apply in that cell.
  4. If the move tool crashes after Freeze, the stop lasts until someone intervenes. Before that, the partition parks after 10 failures (860), so every routine move pages.
- The alternative is no better. If the applier skipped the event for frozen slots, the webhook (`repository.renamed`, `member.added`) would be lost. The target's applier faces the same choice during Clone.
- Account-fenced index moves have the same coupling (216, 271).
**Suggestion**: Separate replica apply, which is never fenced, from event emission. Write a per-slot pending-emit row that the slot's owner drains after Flip.

### 3. [critical] Rows whose log partition depends on a changeable owner can permanently revert to an old owner
**Location**: DESIGN.md:201-204, 265, 267, 210
**Finding**: Each row's partition is "its root": the org for org-scoped rows, otherwise the account (201). That root changes when a login is reclaimed or a repository is transferred. Partitions apply independently, and an applied after-image simply overwrites the row with no row-version check. (The inter-cell log has last-writer-wins by `version` at 635. The `global` log has nothing equivalent.)
**Evidence**:
- Inferred failure path for a transfer:
  1. Org X's partition pX lags, which the design itself produces through a poison-record park (860), the 2,000 rows/s apply cap during a saga (862), or a single-partition saga (855).
  2. X changes repository R (visibility or a grant), which appends to pX. X then transfers R to Y, which appends R's new after-image to pY.
  3. The cell applies pY first: owner Y. It later applies pX's backlog, which writes owner X back.
  4. `g_repo_dir(R).owner` stays X until R next changes. X's admins keep admin on R in that cell, and `full_name` renders `X/R`.
- Inferred failure path for a reclaim: `Former{A}` lives in pA and `Live{B}` in pB. The same reordering leaves `alice → A` in a projection indefinitely. That is wrong name ownership.
- The `Absent` dependency set is also wrong:
  1. A front that still holds `Former{A}` treats `alice/bar` as depending only on pA (210).
  2. pA's watermark passes `asked_at`, so the front answers 404 for B's just-created repository, whose row is in pB.
  3. This breaks the "created a second ago works everywhere" claim (263).
**Suggestion**: Partition by a stable key (a hash of the name key, the repository id), or carry a per-row version in every after-image and apply last-writer-wins.

### 4. ["Revocations apply ahead of bulk records" is incompatible with an in-order cursor
**Location**: DESIGN.md:862 vs 614-615, 428-431, 441
**Finding**: There is one sequence cursor per partition, and it commits with each apply (614-615). Applying revocations first means applying out of sequence order.
**Evidence** (inferred interleaving):
1. In pX, seq 1000 is a bulk saga step that inserts `g_repo_grant(R, T, write)`. Seq 1001 is an admin removing T from R, in the revocation class.
2. The applier applies 1001 first (a delete of a row that is not there yet), then 1000. The grant is resurrected permanently, which is a security hole.
3. If the cursor jumps to 1001 instead, seq 1000 is never applied while the watermark claims coverage. A committed grant is then denied, which breaks "a grant committed before the request is always honored" (441).
4. Either way, `revoke` waits on the cursor (428-429). It cannot cover 1001 until 1000 applies, so during sagas it reports `covered = false` anyway, and the Revocation coverage SLO (811) burns.
**Suggestion**: Keep apply strictly in order and contain bulk load by write admission (855), or keep per-row versions and a separate watermark per class.

### 5. [critical] A compromised data cell can replay any user's principal to any other cell
**Location**: DESIGN.md:404-406, 274, 121, 276, 253
**Finding**: `Principal` carries the actor, scopes, `asked_at`, and the signing front. It has no audience cell and no binding to the request (404-406).
**Evidence**:
- Measured from the text: data cells accept front-signed principal assertions from peer cells (274). GraphQL forwards "alice's principal" from the entry cell to other cells (121, 276), and a moved slot's old cell proxies requests (253).
- Inferred attack:
  1. A compromised cell `c4` receives the bearer assertion of every user indexed there (every GraphQL query enters at the caller's index cell, 276), and of every user touching its repositories.
  2. Within the assertion's validity, which must exceed `BEHIND_WAIT` plus the delegated work, `c4` replays alice's (acme admin) assertion to `c7` with a different request: add a deploy key, change `branch_rule`, delete a repository.
- This undoes the property the design paid for with `kid_allowed`: "a compromised cell mints for its own accounts and slots, never for every installation" (439).
**Suggestion**: Bind each assertion to its target cell and a request digest, and have the front sign one assertion per cell for each delegated batch, or have the delegating cell re-sign under a constrained, auditable "on behalf of" scope.

### 6. [critical] `revoke` reports `covered` before edge nodes have the change, and a stalled edge keeps honoring revoked sessions and suspended accounts
**Location**: DESIGN.md:428-431, 440-441, 273, 221-224, 868, 437, 945, 810
**Finding**: `revoke` waits for "every follower cursor in `global.follower`" (428-429). For fronts, that follower is the front cell's publisher (445). Edge nodes apply later, from S3 segments (273).
**Evidence**:
- Measured from the text:
  - An edge that rejects a segment (generation gap, more than 0.1% of entries removed, more than 5 slots moved) "keep[s] the last good store", with no age limit (222-224, 868).
  - Several checks happen only at the front: `sessions_not_before` (437), installation `not_before` (396-398, 440), and credential rejection for suspended accounts ("fronts reject the account's credentials; cells hide its content at render", 945).
  - Freshness is measured from follower watermarks (810), meaning the publisher, not the edges. The health check is one synthetic routed request per front cell (272, 867).
- Inferred failure path:
  1. A user hit by account takeover signs out everywhere; `revoke` returns `covered`.
  2. A stalled edge, or a whole front cell whose edges are stuck, keeps accepting the attacker's session cookie indefinitely. The same goes for a suspended spammer's PAT.
  3. That front stays in DNS because its health check only exercises a positive route.
**Suggestion**: Gate `covered` on edge-level applied watermarks reported back. Have an edge fail health and refuse authenticated requests once its projection is older than a hard bound. Re-check session and suspension state in the cell from `g_*`.

### 7. [critical] An account's index cell is a single point of failure for its App tokens and sign-in in every cell
**Location**: DESIGN.md:396-398, 124, 439, 112, 231-233, 276 (contradicts 270)
**Finding**: An installation token is valid only if its `kid.cell` is the installation account's index cell (396-398), and minting routes there (124). Identity writes, including sign-in and OAuth token exchange, route to the caller's index cell (112, 231-233). Every viewer GraphQL query also enters there (276).
**Evidence** (inferred failure path):
1. `acme`'s index cell `c7` is down. Causes include a Regional cell during a Region loss until ARC promotion (823), a bad deploy in `c7` (825), or a `core` blackhole.
2. Every App on acme (CI, Renovate, bots) can no longer mint tokens for acme repositories in the healthy cells `c9` and `c12`. Within one token lifetime, every App integration on acme stops.
3. Users indexed in `c7` cannot sign in, create a PAT, or finish `gh auth login`, even though `global` is healthy.
4. This brings back the flaw that rejected candidate 2, "home down → cannot authenticate anywhere" (`arena/round2/SYNTHESIS.md:35`), for new credentials. It breaks "a bad deploy … stops at the cell" (270).
**Suggestion**: Identity writes touch only `global`, so route them to any healthy cell. As a fallback for App tokens, let the cell holding a repository mint tokens scoped to repositories in its own slots, with `kid_allowed` checking that every repository in scope sits in `kid.cell`. This also narrows what a compromised cell can mint.

### 8. [warning] Old-username redirects end earlier than on GitHub, and large-owner renames exceed DSQL's transaction size
**Location**: DESIGN.md:265, 192-194, 262, 266, 137
**Finding**: On reclaim, "A new claim of the old login overwrites `Former`, ends the redirect" (265). GitHub and GHES keep each redirect until the new owner creates a repository with the same name ("If the new owner of your old username creates a repository with the same name as your repository, that will override the redirect entry", https://docs.github.com/en/account-and-profile/concepts/username-changes, measured). This difference is missing from "Known deviations" (781-790).
**Evidence**:
- Client-visible break (inferred): the moment someone squats `alice`, every `go.mod` import, CI clone, and Terraform module source under `alice/*` that is below the retirement thresholds breaks. On GitHub, each one keeps working until the squatter creates that specific repository.
- Transaction size (measured):
  - An owner rename and a reclaim each bump the epoch of every repository the owner has (192-194, 265).
  - A rename also writes `retired` rows in the same transaction (266).
  - The design calls these "3 to 8 rows" (262), but DSQL caps a transaction at 3,000 rows (137).
  - So renaming, deleting, or reclaiming an org with more than about 3,000 repositories cannot commit as specified.
**Suggestion**: Keep a list of former owners per login key and fall through to it on a `repo_name` miss. That matches GitHub and removes the mass epoch bump.

### 9. [warning] Every negative answer waits in steady state, and heartbeat writes contend on the log's head rows
**Location**: DESIGN.md:209-213, 615-616, 201, 438, 789, 810, 869, 1045
**Finding**: The watermark is the `at` of the last applied entry (221, 612). Idle partitions get a heartbeat only once their head is older than 200 ms (615-616). So an ordinary miss must wait until a log entry written at least 50 ms after the request has crossed the whole pipeline: publisher poll every 100 ms (445), 250 ms segments (273), then the S3 tail.
**Evidence**:
- Inferred wait times: a typical 404 or 401 waits about 0.4 to 1 s. For an unknown login or token hash the owning account is unknown, so the answer depends on all 64 partitions (201) and waits for the slowest of them.
- Measured from the text:
  - The front lag SLO is p99 under 2 s (810), which equals `BEHIND_WAIT` (213). Any minute that misses the SLO turns every 404 into a 503.
  - A NAT'd CI fleet probing for repositories it expects to be missing (go-get, Renovate presets) exceeds the 16 waiters per IP (869) and gets 503.
  - The deviation row (789) covers only "created a moment ago", and the tradeoff (1045) frames this as an exception.
- Heartbeats:
  - "Any follower" writes them (615), which is about 130 followers (445). Each one races to write to the same idle head rows that real claims must bump, producing 40001 retries.
  - That contradicts "the only writer of `global`" (203-204).
  - The Milestone 0 gate runs one follower and never exercises heartbeats (989).
**Suggestion**: Make the watermark the follower's read-snapshot time whenever it has consumed everything through the head seq in that snapshot. That removes both the heartbeats and most of the wait.

### 10. [warning] Residency can be bypassed by a transfer, and resident orgs' secrets are not resident
**Location**: DESIGN.md:267, 949, 243-245, 436, 381
**Finding**: "Transfers … The network and its slot do not move" (267). Yet residency "places all its networks … in its residency domain" (949), and only forks get the detached path (243-245, 949).
**Evidence**:
- Transfer bypass (inferred): `bob` transfers `bob/tool`, whose network lives in a US cell, to the EU-resident `beta`. Beta's refs, packs, issues, and Actions state then stay in US `core` and `aux` with no error. Turning on residency for an existing org likewise has no migration path.
- Secrets: `global` holds org Actions secrets as ciphertext and org webhooks (436), in North America, and `g_*` replicates the authorization graph into every cell (381). The residency disclosure lists only logins, ids, memberships, grants, and credential hashes (949). Whether secrets ride in `g_*` is an inference. If they do, a compromised cell also holds every org's secret ciphertext.
**Suggestion**: Make a transfer into a resident owner use the detached-network path or reject it with a 422, and either list secrets and webhooks in the residency disclosure or keep them in the org's resident index cell.