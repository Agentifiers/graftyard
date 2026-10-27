### 1. [critical] The `Behind` rule turns git's anonymous 401 challenge into 503 while `global` is unreachable, so private-repo git over HTTPS fails during an outage the design says it contains
**Location**: DESIGN.md:421-427, DESIGN.md:263, DESIGN.md:615-616 (also 11, 892, 997)
**Finding**: The design promises that "Fetches, pushes, merges … keep working in every cell while `global` is down" (DESIGN.md:11). The 30-minute path-cut drill must also show "zero failed fetches, pushes, or merges" (892, 997). Both promises fail for HTTPS git on private repositories.
**Evidence**:
- *Measured from the text.*
  - `authorize` turns any `Forbidden` or `Hidden` into `Behind` when its deciding `g_*` rows are in partitions that are behind `asked_at`. After `BEHIND_WAIT` the answer is 503 (421-425, 213).
  - Watermarks only move through log entries. Idle partitions only move through heartbeats, and a heartbeat is an append to `global` (615-616).
  - So when `global` is down, or the cell's path to it is cut, every partition watermark in the cell freezes. Every denial made after that point becomes 503 after a 2 s wait.
- *Inferred from git's HTTP auth flow.* A client with a credential helper sends the first `info/refs` request without credentials. It calls the helper only after a 401 with `WWW-Authenticate` (`research/wire-census.md#40` records that 401 challenge).
- The failure path, for anonymous `GET /acme/private.git/info/refs`:
  1. The cell decides `Hidden`, based on `g_repo_dir.visibility` in acme's partition.
  2. That partition is behind, so the answer becomes `Behind`, then 503.
  3. git prints `The requested URL returned error: 503` and never asks for credentials.
  4. Every developer clone, fetch, or push of a private repository fails.
- *Inferred.* `actions/checkout` sends a pre-emptive `http.extraheader`, so a synthetic drill that uses token headers passes while real users fail.
- *Inferred, normal operation.* With the cell apply lag of roughly 100 to 300 ms (100 ms pull, 200 ms heartbeat), every private-repo git operation pays that wait on its first round trip.
**Suggestion**: Treat the anonymous 401 challenge as non-final. It only asks for credentials, so the front or the cell can answer it without a freshness check. Make the drill run real `git` with a credential helper and no pre-emptive auth.

### 2. [critical] The projection delta guard freezes every edge's copy of credentials and revocations, fleet-wide and with no exit
**Location**: DESIGN.md:218-224, DESIGN.md:868 (also 438, 440, 273)
**Finding**: The guard was grafted from round-2 candidate 2, where it protected a routing book of names only (`arena/round2/candidate-2/DESIGN.md:142-144`). Here the projection also holds credential hashes, the `revoked` flags, installation `not_before`, and suspensions (218-221). A segment that removes more than 0.1% of entries is refused, the edge "keeps its last good store", and the generation chain then rejects every later segment (222-224).
**Evidence**:
- *Measured.* Revoked tokens are removed from the projection after 30 days (438).
- *Inferred failure path.*
  1. An OAuth app deletion or a leaked-token sweep revokes N tokens.
  2. 30 days later the purge removes them, and enough land in one 250 ms segment (273) to exceed 0.1%.
  3. At launch scale 0.1% is tiny. With 1M entries it is 1,000, which is one 1,000-row saga step.
  4. Every front cell's publisher builds the same deterministic segment, so every edge in every Region rejects it at once.
- From then on:
  - No front learns any revocation, so the promise "no front accepts an older token after 5 s" (440) is false indefinitely, and `revoke` only reports `covered: false` (430-431).
  - Suspensions stop taking effect (945).
  - Every new token and login is a miss that ends in 503 (263).
- The hourly snapshot (273) either bypasses the guard, which makes the guard pointless, or is also refused. The design says neither, and gives no recovery procedure.
**Suggestion**: Keep the guard on slot-map moves only. Exempt credential and revocation records, or let a signed purge record override the removal count. Define a maximum projection age past which an edge stops honoring stored credentials, so a stale store fails closed, and alarm on it.

### 3. [critical] Index cells make one cell's outage reach every cell, through GraphQL, installation tokens, and org runners
**Location**: DESIGN.md:228, DESIGN.md:276, DESIGN.md:124, DESIGN.md:961, DESIGN.md:642 (contradicts 92, 97, 270)
**Finding**: "The cell is the blast radius" (270), and Journey 2 says the merge touches "nothing outside `c7` except the front" (97). Neither holds, because several request paths go through the caller's or the owner's index cell, which is unrelated to where the repositories live.
**Evidence**:
- *Measured.* `/api/graphql` is `RouteClass::Viewer` (228), and "GraphQL enters at the caller's index cell" (276). That contradicts line 92, which has the front route `mergePullRequest` by slot straight to c7.
- *Measured.* Installation tokens can only be minted by the installation account's index cell (124, 396-398).
- *Measured.* Org runners register in the org's index cell (961).
- *Measured.* An index cell is "the handling cell" at account creation (642), and new networks go to other cells once it lacks headroom (240-242).
- *Inferred failure path, cell c4 down:*
  - Every user indexed in c4 loses `gh pr merge`, `gh pr create`, VS Code PR, and Renovate GraphQL against healthy cells.
  - Every org indexed in c4 loses all GitHub App tokens (Renovate, Argo CD, merge bots) and its org runners, for repositories in every cell.
- No failure-mode row lists these effects (DSQL `core` region row, 823).
**Suggestion**: Route single-target GraphQL at the front by node id or repository, as line 92 assumes, and make that part of `RouteClass`. Let any cell that holds a slot of the installation's repositories mint tokens scoped to its own slots. Add index-cell rows to the failure table.

### 4. [critical] Owner renames and login reclaims must bump the epoch of every repository in one transaction, which DSQL caps at 3,000 rows
**Location**: DESIGN.md:192-196, DESIGN.md:262, DESIGN.md:265 (also 137, 203-204, 266)
**Finding**: The design calls each claim "one `global` transaction of 3 to 8 rows" (262). But a reclaim "bumps the epoch of every repository it used to reach" (265), and `NameEpoch` is also bumped on an owner rename (192-193).
**Evidence**:
- *Measured.* Each `repo_dir` write also appends its after-image to the change log (203-204), so a reclaim writes about 2 rows per repository.
- *Measured.* A reclaim also overwrites the `Former` row, and an owner rename adds `retired` rows in the same transaction (266).
- *Inferred.* Any former owner with about 1,500 or more repositories cannot be renamed, and its old login cannot be reclaimed, without breaking the 3,000-row limit (137).
- *Inferred.* If an implementer makes it a saga, as line 137 does for transfers and deletes, there is a window where `name(old) = Live{B}` while some of A's repositories still carry epoch e:
  1. A stale front resolves `old/x` through `Former{A}` to A's repository with epoch e.
  2. `check_epoch` returns `Current`.
  3. The cell serves A's content under B's login.
- That is exactly the misroute `NameEpoch` exists to prevent (264). A front stuck as in finding 2 holds this state indefinitely.
- *Measured, smaller issue.* Journey 4 claims a name with `INSERT` and treats a unique violation as `already_exists` (112), so reclaiming over an existing `Former` row needs a different statement that the design never specifies.
**Suggestion**: Add an owner-level epoch on the account row. The front sends both the owner epoch and the repository epoch, `check_epoch` compares both, and a rename or reclaim bumps one row.

### 5. [warning] Unknown-login and unknown-token lookups depend on all 64 partitions, so one stalled partition causes 503s everywhere, not "in that partition"
**Location**: DESIGN.md:201-202, DESIGN.md:209-211, DESIGN.md:860
**Finding**: Partitions are keyed by account (201-202), and an opaque token carries no account (378). So a miss on a login or a token hash cannot name its partition, and `Absent` needs every one of the 64 watermarks to pass `asked_at`. `Lookup::Behind { parts: SmallVec<[GlobalPart; 2]> }` (211) assumes one or two partitions.
**Evidence**:
- *Measured.* Line 860 says a poison record in one partition affects only "new names, grants, and tokens in that partition".
- *Inferred.* The publisher builds segments deterministically, so a poison record stalls the same partition at every front. Then:
  - every mistyped owner returns 503 instead of 404, service-wide;
  - every unknown or long-purged token returns 503 instead of 401, so credential helpers never clear or re-prompt;
  - all of it burns the REST read SLO (803).
- *Inferred, normal operation.* Every 404 or 401 at a front waits for the slowest of 64 partition watermarks. Front lag is allowed to reach 2 s at p99 (810), which equals `BEHIND_WAIT`, so the slow tail of legitimate 404s turns into 503s.
**Suggestion**: Partition name rows and credential rows by a hash of their own key, not by account, so a miss depends on one partition. Correct line 860.

### 6. [warning] The applier ignores the slot fence: an entry for a frozen slot either wedges its partition or leaves an orphaned repository
**Location**: DESIGN.md:271, DESIGN.md:214-216, DESIGN.md:247-255, DESIGN.md:89, DESIGN.md:614-616
**Finding**: Each cell's applier materializes `repo_dir` rows "in one of its slots" and writes repository outbox events in the same transaction that applies the change (271). Its cursor advances only in that transaction (614-615). Every repository-scoped `run_tx` refuses unless the slot is `Serving` (214-215). None of the move steps reconcile `global` entries for the slot (247-255).
**Evidence**: *Inferred failure paths.*
- **Wedge.** During a freeze of up to 60 s (1054), a rename, visibility change, or grant on a repository in that slot cannot apply in the source cell. Line 860 says a partition that fails 10 times parks, so 1/64 of all facts in the cell turn `Behind` until an operator replays it. If the applier skips the entry instead, neither cell emits the `repository` or `member` webhook, which breaks the one-producer rule (271).
- **Orphan.**
  1. `place_fork` commits `repo_name(bob, widgets)` and `repo_dir(R2)` to `global` (89).
  2. The slot is frozen before the `core` create, which is refused.
  3. The target's applier skipped R2 before the Flip because the slot was not yet its own.
  4. After the Flip, no cell creates R2's rows. The name exists with no repository behind it, and bob's retry of `gh repo fork` gets the numbered suffix `widgets-1`.
**Suggestion**: At Flip, the target scans `g_repo_dir` for the slot and materializes anything missing, emitting any pending repository events. The applier defers frozen-slot entries to a per-slot backlog instead of stalling the partition.

### 7. [warning] A slot move invalidates the `GITHUB_TOKEN` of every running job in that slot
**Location**: DESIGN.md:396-398, DESIGN.md:247-253
**Finding**: App tokens get a grace period after an index-cell move ("previous index cell until one token lifetime after the move"). Job tokens get none: "A job token's `kid.cell` must hold the job repository's slot."
**Evidence**: *Inferred.* Once fronts see the Flip, every job token signed by the source cell fails `kid_allowed` and gets 401. Jobs run for hours, and these tokens are used for `git push`, release uploads, and API calls. Runners are already refused during the freeze (997), so every in-flight job in a moved slot fails. The control plane moves the hottest slots routinely (60, 269), and those are the slots with the most Actions traffic.
**Suggestion**: Accept the source cell's `kid` for the slot until the longest job-token lifetime after the move, the same rule App tokens already get. Alternatively, drain or re-mint job tokens as part of `move_slot`.

### 8. [warning] Heartbeats from every follower pile onto the same 64 head rows when traffic is quiet
**Location**: DESIGN.md:615-616, DESIGN.md:445, DESIGN.md:989
**Finding**: "Any follower that sees the head older than 200 ms" appends a heartbeat. There are about 130 followers (100 appliers and 30 publishers, 445), and each polls every 100 ms.
**Evidence**: *Guess, from the design's own counts.* When `global` is quiet, each idle partition gets up to about 130 conflicting multi-Region append attempts every 200 ms. That is about 130 × 64 × 5 ≈ 41,600 attempted transactions per second, against the stated read load of about 1,300 queries per second (445). All but one attempt per partition fails with 40001. Real identity writes on the same head row compete with the heartbeats and get retried. The Milestone 0 gate tests 200 writers and one follower (989), so it never exercises this.
**Suggestion**: Make one leased heartbeat writer per partition, or let the `global` writer emit heartbeats. Add many concurrent followers to the gate.

### 9. [warning] Transfers bypass residency
**Location**: DESIGN.md:267, DESIGN.md:951
**Finding**: Transfers keep the network in place ("The network and its slot do not move"), yet residency promises "all its networks" stay in the org's residency domain.
**Evidence**: *Inferred.* `POST /repos/bob/data/transfer` to an EU-resident `beta` leaves `beta/data` in a US cell. A fork cannot move out of its network at all (269). An org that turns on residency later is also left with its networks where they are. The known-deviations table (790) only covers pull requests from detached forks.
**Suggestion**: Refuse cross-residency transfers with a 422 and list that under known deviations, or perform a detached copy as fork creation already does.

### 10. [warning] Nothing specifies how card subscriptions are set up when access changes, so listings can be wrong
**Location**: DESIGN.md:629-631, DESIGN.md:646, DESIGN.md:120, DESIGN.md:788
**Finding**: Subscriptions are "computed from the replica" (629-631). Nothing says who sends the initial cards when a membership, team, base-permission, or transfer change adds a viewer's index cell.
**Evidence**: *Inferred.*
1. alice joins an org with 50,000 repositories under base read.
2. Her `/user/repos` in `c4` finds the `g_repo_dir` rows but no cards.
3. The response must either omit those repositories, so listings and Terraform or Argo CD SCM discovery are wrong, or emit null `default_branch` and `pushed_at`, which breaks the Octokit schema contract.

The "at most 60 s" bound (120, 788) covers changes to existing cards, not new subscriptions. One membership change can mean 50,000 cards per new viewer cell, which no velocity cap mentions.
**Suggestion**: Answer listings from `g_repo_dir` plus defaults that satisfy the schema when a card is missing, and mark that as a deviation. Run subscription backfill as a capped saga with its own lag SLO.