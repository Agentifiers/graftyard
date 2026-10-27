I found 10 problems in `DESIGN.md`: 4 critical and 6 warnings, most severe first. I did not edit anything. Each claim is labelled measured (taken from AWS documentation or the repository files), inferred, or guess.

### 1. [critical] The 5-second token re-check makes every write in a continent depend on a cross-continent read
**Location**: DESIGN.md:346 (also :344-345, :802, :1239)
**Finding**: In managed mode, a write whose cached token entry is more than 5 s old does a strong read of `global` and returns 503 if that read fails. `global` is one MRSC table across us-east-2, eu-central-1 and ap-northeast-1 (:334, :891). If one continent is cut off from the other two, or two `global` Regions are impaired, every push, merge and API write in every cell on that continent fails closed, even though those cells' own tables are healthy. That breaks "the cell is the blast radius" (:273). It is also the flaw that cost candidate 1 the foundation (SYNTHESIS.md:18). When nothing is wrong, the first write in each 5 s window still pays a cross-continent round trip.
**Evidence**: Measured: the AWS docs say MRSC "write and strongly consistent read latencies ... require cross-Region communication" (V2globaltables_HowItWorks). Measured: the judge credited candidate 3 because tokens resolve "from the local replica with eventually consistent reads" (review/round4-judge.md:37; candidate-3/DESIGN.md:152), and graft 3 removed that. Inferred: the Milestone 3 gate, `gh api user` "within 100 ms through another continent's edge" (:1114), cannot pass by the design's own 95–225 ms cross-continent estimate (:1240).
**Suggestion**: Do the re-check as an eventually consistent read of the local replica, which picks up revocations in about a second and never needs another continent. If the local replica is down, keep serving from the cache within the 60 s bound instead of returning 503.

### 2. [critical] A transfer commits before the destination name is reserved, and nothing can find an unfinished signup
**Location**: DESIGN.md:192, :270, :266
**Finding**: A transfer commits on the repository chain (`Settings { owner }`). The new owner's `Live` `RepoName` is appended later, on first touch, and that owner chain's `decide` "rejects a taken key with 422" (:266). If the new owner creates or receives a repository with the same name in that window, which can last up to a minute, the finishing step can never succeed and the committed transfer cannot be undone. The repository then belongs to the new owner but resolves under neither name, or under the old owner's stale `Live` entry. Forks avoid this because they commit on the chain that holds the contested name. Separately, a signup commits in `global`, not on a chain. `Audit` is a chain consumer driven by `log` streams (:535), and `global` sits in another account (:891), so an untouched signup is never finished.
**Evidence**: Inferred from :192, :266, :270, :535.
**Suggestion**: Make the destination owner's `RepoName` entry, carrying the repository id and the authorization, the transfer's commit point. Then finish `Settings` and the `Alias` from it. Give signups a pending claim state, or write `Created` on the owner chain before the login goes `Live`.

### 3. [critical] Slot moves fence only `log` chains, and the slot map flips once for the whole slot
**Location**: DESIGN.md:252-258, :226-227, :1130
**Finding**:
- **Only chains are copied.** The copy covers chains, checkpoints, snapshots and packs. The canonical items in `checks`, `actions` and `inbox` are not copied or fenced: check runs that no rule requires, running jobs and their leases, concurrency groups, stars, notifications, cards, markers and cursors. After the flip, `completejob` and `renewjob`, which route by the job's slot (:1035), reach a cell with no job item.
- **Ids can repeat.** Ids are `(seq << 16) | slot`, with seq leased "from the `IdBlock` item of the cell that holds the slot". `LogSk::IdBlock(IdKind)` (:133) has no slot in its key and `move_slot` does not transfer it, so the target cell can mint ids the source already issued.
- **The seal-to-serve gap is per slot.** `SlotItem` flips once, after every chain in the slot has caught up. Every chain sealed early is unwritable until then, so the Milestone 5 gate of "each chain's seal-to-serve gap under 1 s" cannot pass for slots with many chains.
**Evidence**: Inferred from :253-255 and :226. The gate only checks that a running job keeps its token (:1130).
**Suggestion**: Make id sequences per (slot, kind), and move them under the seal. Seal and copy the work-table items for the slot too, or route them by chain. Flip routing per chain (the `Sealed` entry already names the target), not per slot.

### 4. [critical] S3 keys for spilled bodies, snapshots and archives come from chain position, so racing writers and restores collide
**Location**: DESIGN.md:303-304, :313, :319
**Finding**: `Spill { chain, seq }` is written before the conditional put, while two deciders on different nodes or Regions can both be targeting seq N+1. Snapshots and archives are keyed only by chain and seq: they carry no schema, release, or `restore_epoch`. Two failure paths follow, and one of them applies whichever bucket rule holds:
- **If overwrites are allowed**, the losing writer can replace the winning entry's spilled body. `apply` then refuses the chain on the digest mismatch, and the acknowledged body survives only as a noncurrent version for 35 days.
- **If `If-None-Match` is enforced** (it is for packs, :319), the first spilled entry after a table restore lands on a key the abandoned timeline already used, and gets 412 at that seq indefinitely.

Packs already use fresh ULIDs for exactly this reason (:289).
**Evidence**: Inferred from :291, :303, :313, :319 and journey 7 (:107).
**Suggestion**: Key spills, snapshots and archives by a ULID or content digest that the entry or checkpoint item names.

### 5. [warning] The Single-cell stale mode breaks its own type rule, breaks read-your-writes, and loses authentication after 60 s
**Location**: DESIGN.md:177, :180, :321, :382, :1096-1098
**Finding**:
- **Type contradiction.** :177 says `authorize` and every list read "accept nothing else" than `Fresh`, and `authorize` takes `&Fresh` (:382). :321 says stale mode authorizes from the warm owner fold. Either stale mode serves only anonymous public reads, or the type guarantee is gone.
- **Read-your-writes breaks.** Stale mode triggers per tail read when one takes more than 200 ms. A slow or throttled read, most likely on the hottest repositories, can serve a fold that lacks a push the same user just got "ok" for. CI started by that push's webhook can then fail with "not our ref".
- **Identity is in the same impaired Region.** A Single install's `global` table is a plain table in that Region, edge positives live 60 s (:343), and a hot-swapped process starts with an empty cache. After 60 s, authenticated users stop resolving.
- **The gate cannot see this.** The Milestone 1 gate tests stale mode with "No REST" and no `authorize`, which arrives in Milestone 3 (:1112).

**Evidence**: Inferred from the cited lines.
**Suggestion**: Make stale mode a cell-wide mode switched by a signed flag, not a per-request timeout. Give it an explicit `StaleAuth` path that keeps edge entries past their TTL. Re-gate it in Milestone 3 with token-authenticated private fetches.

### 6. [warning] The hot-chain read budget counts requests, not bytes
**Location**: DESIGN.md:466, :568-570, :1086
**Finding**: Every serving node, up to 32, reads every new entry once. At the stated ceiling of 1,000 entries/s of 1 KB, that is 32 MB/s, or about 8,000 strong read units/s. Split across two Regions it is still about 4,000 per replica partition, against a limit of 3,000. Inline packs of up to 16 KiB, and bodies up to 350 KB, multiply that by the serving-set size, while the threshold for inlining looks only at the write budget (:568). Paired cells have no stale mode, so a throttled tail read becomes a failed read. The Milestone 0 throttle test (:1086) does not fix the number of readers or the entry sizes, so it cannot catch this.
**Evidence**: Measured: a Query is charged on the "total size of all items", rounded up to 4 KB (read-write-operations doc), and a partition allows 3,000 read units/s (research/dynamodb.md:17). The arithmetic is inferred.
**Suggestion**: Base the serving cap and the inlining rule on readers × bytes per second. Use 32 readers and real entry sizes in the Milestone 0 test.

### 7. [warning] Journey 7: after a full restore, the `dr` replica belongs to the old tables
**Location**: DESIGN.md:1001, :1003, :1005, :107
**Finding**: The restore cutover turns Streams, PITR and deletion protection back on, but it does not recreate the MREC `dr` replica of the new tables. The eu-central-1 replica still follows the pre-restore tables, which include the deleted `acme`. When the 3-hour impairment comes, the runbook says to promote after 1 hour. That promotion either restores the mistake or has no target until a replica rebuild finishes, which is a control-plane step that takes time proportional to data size. Also, the five tables are restored independently to second `t`, so an entry written just before `t` whose `Dirty` marker was written after `t` loses its marker. Nothing runs `Remark { since: t - 24h }` to recover it.
**Evidence**: Inferred from :1001 and :1003.
**Suggestion**: Make re-adding the replica and a `Remark` run explicit cutover steps. Refuse `DrPromote` against tables that the current `restore_epoch` does not name.

### 8. [warning] Dispatchers in both Regions update the same `Dirty` items at the same time on an MRSC table
**Location**: DESIGN.md:535, :524-529
**Finding**: Both Regions' dispatchers read identical streams "at once" and raise the same `Dirty.max_seq` items, while consumers delete those items conditionally. On MRSC, a concurrent change to one item from another Region fails with `ReplicatedWriteConflictException`. Here that is the normal path, not an edge case. Conflict retries spend the 10% retry budget, and the conflict-rate deploy gate (:856) will see the noise.
**Evidence**: Measured: per AWS, a write "fails with a `ReplicatedWriteConflictException` when it attempts to modify an item that is already being modified in another Region" (V2globaltables_HowItWorks).
**Suggestion**: Run each chain's dispatching in its preferred Region, with the other Region taking over only on lag, or key markers by Region.

### 9. [warning] Journey 6's containment claims do not hold
**Location**: DESIGN.md:106, :347, :446, :856
**Finding**:
- **The ceiling is shared.** A table's maximum read throughput applies to the whole replica. The per-node budget of 200 misses/s, summed over all canary edges, can reach that maximum, and then every front cell's misses and write re-checks in that Region are throttled. The claim that "other cells and older nodes see no added load" is false.
- **Rollback fights the format rule.** If a new entry format escapes, old nodes return 503 and the gate swaps everything back to the old binary automatically (:856). That leaves no node able to apply those chains, which contradicts "rollback is refused".
- **Metadata formats can stall git.** Both tiers must advance past every entry (:446), so an unknown metadata event kind stalls the core tier and git, not just lists.

**Evidence**: Inferred. The AWS docs measured above list the on-demand maximum read throughput as a per-replica table setting.
**Suggestion**: Bound the per-node budget × fleet size below the table maximum, or partition the budget per front cell. Store a tier tag outside the encoded body so an old node can skip an unknown metadata event, and make the swap-back aware of enabled formats.

### 10. [warning] One home cell that is down stops notifications for whole repositories
**Location**: DESIGN.md:559, :549
**Finding**: The source cell keeps its `Dirty` marker "until every destination acknowledges", and each consumer has one cursor per chain. A popular repository has watchers in every cell, so one down home cell stops its notifications for all of them, service-wide. That is wider than the "home cell is a dependency for its own accounts" radius stated at :273.
**Evidence**: Inferred from :549, :559, :530.
**Suggestion**: Keep a cursor per (consumer, chain, destination cell), so a down cell holds back only its own deliveries.