### 1. [critical] The 5 s write re-check ties every write in every cell to the worldwide `global` table, and the per-node budget limits it
**Location**: DESIGN.md:346-347, 397, 273, 891
**Finding**: `WRITE_RECHECK` does a strong `global` read for each token on each node every 5 s. Those reads come out of the same `MISS_BUDGET_PER_NODE = 200` "strong `global` reads per second" as cache misses. So:
- (a) A front node that sees about 1,000 distinct tokens writing within 5 s spends its whole budget on re-checks. Further uncached principals and writes get 503. Line 397 says this load "follows fleet size". It actually follows the number of distinct writers per node.
- (b) Every push, merge and comment in every cell needs a strong read from the three-continent MRSC trio. That is a third service-wide dependency, and line 273 lists only two.
- (c) Journey 5 drops us-east-2, which is also one of `global`'s three Regions. MRSC replicas cannot be added later. After that, `global` runs on eu-central-1 plus ap-northeast-1 indefinitely, and one more loss stops claims and all re-checked writes. The design has no re-protection path for `global`, and `c1` has none either (it would have to evacuate every slot).
**Evidence**: line 346 (re-check) and line 347 (shared strong-read budget), measured. `research/dynamodb.md#18` (no replicas added), measured. Line 1237: a strong read costs a cross-Region round trip, inferred, so each re-check costs a cross-continent round trip in managed.
**Suggestion**: Give re-checks their own budget, sized to the write rate. Better: push token revocations out to edges (a revoked-hash feed taken from the `global` stream) so writes never read `global` synchronously. Specify how `global` and a cell are re-protected after a permanent Region loss.

### 2. [critical] A Single install's `dr` promotion has no write fence, so MREC last-writer-wins can overwrite acknowledged entries
**Location**: DESIGN.md:1003, 323, 1287
**Finding**: In MREC, conditions are evaluated only in the local Region. If the home Region is impaired but alive (journey 7's 3-hour partial impairment), it can still put `e#N+1` while the promoted `dr` Region puts its own `e#N+1`. When replication resumes, last-writer-wins silently replaces one acknowledged entry with the other. The `prev`-digest check only detects this after the loss has happened. The only gate is a runbook decision keyed on AWS Health.
**Evidence**: `research/uptime.md#30` and line 1287 (LWW), measured. `attribute_not_exists` checks only the local replica, inferred from the MREC model.
**Suggestion**: Put a promotion epoch in the key (`e#<epoch>#<seq>`) so writes from the two sides can never collide and divergence shows up as a fork in the chain. Also have home nodes stop writing once they see the epoch in any control bucket.

### 3. [critical] A transfer commits before its name is claimed, and a name collision leaves an orphaned repository
**Location**: DESIGN.md:192, 270, 1243
**Finding**: A transfer's commit point is `Settings { owner }` on the repository chain. The new owner's `Live` `RepoName` comes afterwards, on first touch. If the new owner creates a repository with the same name in between, that `RepoName` `decide` returns 422. The transfer has already committed, so the repository belongs to the new owner but has no name there. Nothing compensates. "Finish on first touch" assumes the follow-up appends cannot fail.
**Evidence**: line 263 (a taken key returns 422) combined with line 192 (commit order), inferred.
**Suggestion**: Make the destination `RepoName` (a `Reserved` or `Live` entry) the commit point, then apply `Settings` and the `Alias`. Give every multi-chain operation a follow-up whose `decide` cannot reject it.

### 4. [critical] Owner chains are a cross-cell, request-path dependency with an oversized core fold, and enterprise policy is not checked
**Location**: DESIGN.md:104, 199, 562, 273, 388-389, 393, 438
**Finding**: Every authorization reads a fresh owner fold (lines 104, 199). A fork lives in its network's cell but its owner chain lives in the owner's home cell. So every clone, push and API call on `bob/widgets` (journey 2's head) does a cross-cell strong read. Line 562 lists only "org-owned fork… on the write path". A home cell outage therefore stops git on all of its accounts' forks across the service, which line 273 leaves out.
The owner fold also has only a core tier (line 438), yet it holds sessions appended at every sign-in (389), `Sso` entries (393), sign-in history and the audit log (388). All of that gates every request.
`authorize` and `deps` take two chains, so enterprise-level policies and enterprise membership removal have no place to be checked.
**Evidence**: the cited lines, inferred.
**Suggestion**: Split the owner chain. Keep an authz tier (members, teams, grants, policies, installations) separate from an identity and audit tier. Add an enterprise dependency. State the cross-cell read path and its blast radius.

### 5. [warning] `RestoredTo` rolls back acknowledged work and does not bring back deleted repositories
**Location**: DESIGN.md:322, 107
**Finding**: `apply` resets both tiers and all refs to `to`. Restoring the 12 force-pushed repositories therefore also undoes legitimate pushes to other branches and every issue, review and merge after `t`. That breaks "never lose an acknowledged push" from the user's point of view. The 3 deleted repositories were removed by owner-chain `RepoName` entries (262-263), and a repository-chain entry does not restore those names.
**Evidence**: line 322 ("resets both tiers"), measured. Deletion is on the owner chain, inferred.
**Suggestion**: Restore chosen refs (`RestoredTo { refs }`), leave metadata alone, and add an owner-chain undelete.

### 6. [warning] The 60 s edge cache defeats Single stale mode, and the 200 ms trigger breaks read-your-writes
**Location**: DESIGN.md:321, 341-343, 107
**Finding**: In a Single install, `global` is a plain table behind the same impaired endpoint. Positive edge entries "live 60 s", so authenticated reads stop about a minute into the outage. That leaves anonymous public reads, not the promised 2 hours. Separately, entering stale mode when a tail read exceeds 200 ms makes a latency spike serve folds that may lack the caller's own push, and turns writes into 503s. Journey 7's 3-hour impairment also exceeds the 2-hour default limit, which pushes operators toward the unfenced promotion in finding 2.
**Evidence**: lines 341-343 and 321, inferred.
**Suggestion**: Keep expired positive entries for stale reads while the stale limit runs. Trigger stale mode on errors only, not latency.

### 7. [warning] The `formats` gate is per cell, but chains are read across cells
**Location**: DESIGN.md:106, 858, 848, 860
**Finding**: Config and operators are per cell. Owner chains are read across cells (finding 4), slot moves copy chains to other cells, and home deliveries cross cells. A canary cell that enables format k+1 makes older-release cells return 503 for every repository whose owner is homed in the canary. That contradicts journey 6's claim of containment to "the chains the canary cell wrote".
**Evidence**: lines 848 and 858 (per-cell operator and config), inferred.
**Suggestion**: Gate format enablement fleet-wide on the `nodes` report from every cell that can read the chain.

### 8. [warning] Graft 1 leaves out commit statuses, and required-check rows grow the core tier without limit
**Location**: DESIGN.md:645, 418, 440
**Finding**: GitHub required status checks can be satisfied by commit statuses, and statuses are stored as "rows" outside the chain (645), so the merge race comes back for Jenkins-style statuses. `RequiredCheck` fires for any SHA whose check name any rule requires (418). In a monorepo that puts every CI conclusion on the chain, and `required_check(sha, …)` rows build up in the "small" core tier (440) with no pruning. Writing the `checks` item and then the chain entry is not atomic, so the UI can show a check passed while the merge still says it is missing.
**Evidence**: the cited lines, inferred.
**Suggestion**: Put statuses on the same path. Keep only rows for open PR heads and protected branch tips, and prune merged SHAs.

### 9. [warning] Two dispatchers writing the same marker items on an MRSC `inbox` cause write conflicts and double the cost
**Location**: DESIGN.md:535, 516
**Finding**: Both Regions' dispatchers raise the same `Dirty` item at the same moment. In MRSC, concurrent cross-Region writes to one item fail with `ReplicatedWriteConflictException`. The loser's condition failure still consumes write units. A hot chain's marker also shares a `(consumer, bucket)` partition with 1/256 of all chains.
**Evidence**: `research/dynamodb.md#18`, measured. Failed conditional writes consume WCU (https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/WorkingWithItems.html#WorkingWithItems.ConditionalUpdate), measured.
**Suggestion**: Key markers by dispatcher Region, or run one dispatcher per shard under a lease with the other on standby.

### 10. [warning] Slot moves cannot meet the 1 s seal-to-serve gate, and id blocks may not move with the slot
**Location**: DESIGN.md:252-258, 226-227, 133
**Finding**: Chains are sealed one at a time, but the `SlotItem` flips only after every chain in the slot is sealed and caught up. The first chain sealed is unwritable until the last one is done. A slot holds about 1/65,536 of all networks and accounts, so that is minutes, not 1 s. Slots also can never be split, so a hot network drags its slot-mates with it. Ids lease from "the `IdBlock` item of the cell", and `move_slot` does not copy it. If the counter is per cell, the target can reissue `(seq << 16) | slot` ids that already exist.
**Evidence**: lines 255 and 226-227. The ordering is inferred; the id reuse is a guess, because the `IdBlock` partition key is not specified.
**Suggestion**: Make every chain in the slot redirect-and-forward to the target from the moment it is sealed. Key `IdBlock` by slot and move it with the slot.