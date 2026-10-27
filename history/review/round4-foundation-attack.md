### Verdict summary

Measured: the judge put candidate 3 (C3) ahead of candidate 1 (C1) at 41 to 36. The 5-point gap came from exactly two criteria: Downtime 5 against 4 (3 points) and Build risk 4 against 3 (2 points) (`review/round4-judge.md:11-17`). The review fixes cancel both. Inferred: with the same fixes applied to C1, the two now tie at about 36. The aggregated baseline scores about 32 to 33. The shared core of C1 and C3 holds: one conditional put on MRSC, packs in 2 of 3 S3 buckets, and markers. What no longer has evidence behind it is the choice of C3's chain-plus-node-fold form over C1's form.

**Census of mechanisms that exist only because of the chain-per-aggregate shape** (measured from text; the C1 comparison is inferred):
- **Fold engine.** SQLite per chain and tier, checkpoints, archive, refold and audit (DESIGN.md:477-514, 1317).
- **Tier split,** and merges on a cold node waiting for the metadata tier (508, 1323).
- **Read-side machinery for hot chains.** One preferred tail reader per AZ, peer fetch, a serving cap counted in readers times bytes, an inline rule tied to the read budget (492-496, 512), and a coalescing window for huge owners (430).
- **Forks across chains.** A fork spans three chains (RepoName, ForkAdded, Genesis) with a finisher and an abort path (213). GC reads every member chain's fresh fold (340).
- **Slot moves chain by chain,** using per-chain `Sealed`/`Unsealed` entries (276-289).
- **Epoch in the entry key, and gap truncation at `dr` promotion** (135-137, 1310). This is better than C1's last-writer-wins head (C1:570).
- **`RequiredCheck` entries** with append, prune and backfill (160, 467).
- **`Mirror` chains** that copy entries with seq and digest, and a format gate that counts every cell reading a mirror (131-133, 922).
- **Shared with C1, so not shape-specific.** Markers, cursors and Remark (C1:177-195), the seal fence (C1:111), stale mode (C1:244), and a hosting-cell authorization copy behind a fail-closed lease (C1:213-223).
- **Only C1 needs these.** View `catch_up` on the write path, a 3-step `write_item` repaired by a pump, and a 1,000-entry list scan budget (C1:150-168).

### 1. [major] The Downtime margin rested on a property C3 never had, and the `Mirror` fix rebuilt C1's lease
**Step:** Rubric criterion 1, C3 against C1 with the same fixes. **Location:** `review/round4-judge.md:35`; C3:199; `review/round4-review-a.md:24`; `review/round4-review-c.md:15`; DESIGN.md:302, 428, 864; C1:216-223, 291.
**Gap:** The judge gave C3 a 5 because authorization had "no freshness timer" and "identity … failure stay[s] inside the owner's cell" (measured). Two reviewers then showed that every fork, and every repository in a moved slot, read its owner's chain across cells (measured). FD-ACT-6 fixed this with a `Mirror`: a 30 s fail-closed lease per owner per hosting cell, and revocations acknowledged only after every hosting cell applies them (measured, 428). That is C1's `RvLease` keyed by home cell instead of by continent, with 30 s in place of 5 s (inferred). If C1 gets the same fix (revocation logs partitioned by home cell), both have the same blast radius, so C3's Downtime drops to 4 and ties C1's (inferred).
**Suggestion:** Rewrite DESIGN.md:1234 and 1345. Neither stated reason for beating C1 holds any more. Record the actual tie-break instead: per-repository chains rather than a per-network head, and epoch keys rather than last-writer-wins at promotion.

### 2. [major] A stall or bad deploy in one cell can stop writes in other cells, which the design never states
**Step:** Blast radius of a bad deploy (rubric criterion 1). **Location:** DESIGN.md:569, 588, 829, 866, 918, 302.
**Gap:** Mirror entries travel the whole event path: Streams, the dispatcher, the `Dirty` marker, the consumer, then PrivateLink to the hosting cell (measured, 588). The dispatcher alarm fires at 5 minutes of lag (569), but a stall over 30 s already fails writes closed in every hosting cell (inferred). The design never says what "refresh" means for an owner with no new entries; neither "heartbeat" nor a definition appears anywhere (measured). The bad-deploy row lists only "wrong authorization" (866). It does not list a stopped consumer, which breaks the claim that the cell is the blast radius (302). The judge charged C1 with this same flaw: "a bad revocation-log publisher" (judge:29).
**Suggestion:** The home cell's sender writes a timed heartbeat per (home cell, hosting cell) pair, independent of Streams. Leases run from that heartbeat. `Mirror` code ships in the last wave, as `trib-global` already does. Add a failure row for "Mirror stopped".

### 3. [major] The Build-risk margin has reversed
**Step:** Rubric criterion 4. **Location:** `review/round4-judge.md:51-53`; DESIGN.md:188-213, 276-289, 428, 477-514, 584, 1310.
**Gap:** The judge counted 5 protocols for C3 and 11 for C1 (measured). The final design has about 13 (measured from text):
- commit and retry
- `fresh` with peer fetch
- the fold engine and its tiers
- markers and Remark
- the `Mirror` with its ack-wait and lease
- finishers for operations that span chains
- chain-by-chain handover
- stale mode
- the local-replica token re-check
- epoch truncation at promotion
- `RequiredCheck` pruning
- the fleet-wide format gate
- planned copies to re-protect after a Region loss

If C1 grafts `global` and the `Mirror`, it drops its revocation logs, projections, continental tables and `reconstitute`/`rejoin` (C1:199-240), leaving about 8 or 9 (inferred). Both now score about 3 (inferred).
**Suggestion:** Keep this census as a CI-checked list in "Tradeoffs accepted", so further growth is visible.

### 4. [major] `fresh` rests on an AWS guarantee that is not documented; C1's head read does not
**Step:** Fact check on the freshness primitive. **Location:** DESIGN.md:139, 194-196, 217, 1362; C1:244; https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html
**Gap:** I re-read the AWS page today. It promises only that strong reads on MRSC "always return the latest version of an item" (measured). C3 proves a chain fresh with a strong range Query (`sk > last`) that must see items created since the last read. That is unverified (judge fact check 8). C1 proves freshness with a strong GetItem on one item, which is exactly what AWS documents (measured).
**Suggestion:** Keep the chain, and make `fresh` a strong GetItem on the exact key `e#E#(applied+1)`, the key whose `attribute_not_exists` put is the commit point (139). Repeat until that key is absent, and use Query only to fetch entries in bulk. Inferred: this costs the same 1 read unit and is gap-free by construction (208). Read the current epoch from the control bucket. Add a phantom-read test to Milestone 0.

### 5. [major] The `Mirror` is weaker than C1 on grants and on revocation
**Step:** Rubric criterion 3. **Location:** DESIGN.md:428, 1294, 1331, 1372; C1:236.
**Gap:** Grants reach hosting cells "within seconds" (measured). So a user added to a team who then pushes to an org-owned fork can be wrongly refused (inferred). C1 checked again on every deny, so a deny was never wrong about an acknowledged grant (measured, C1:236). The revocation 204 waits for every hosting cell with no limit (1372). One cell that is down therefore stalls an emergency member removal (inferred). Reads in a cut-off cell also stay stale indefinitely (428).
**Suggestion:** Graft C1's verify-on-deny: on a deny, make one coalesced strong read of the home `Authz` tail. Cap the 204 wait at the 30 s lease, because a cell that misses the revocation fails closed by itself. Close reads after a signed limit. Apply the lease only to decisions that read a mirrored fact (installations, teams, org roles, rulesets). Owner and collaborator checks read the repository chain (inferred). That removes most user-owned forks from the blast radius.

### 6. [major] The fold engine is now the only thing separating C3 from C1, and Milestone 0 tests only C3's side
**Step:** The flip rule. **Location:** DESIGN.md:877, 914, 1153, 1157, 1323, 1370; C1:162-165, 402.
**Gap:** After an AZ loss, metadata tiers reload at a capped rate, and merges on cold nodes wait for them (measured). Merge and list SLOs therefore take a hit that C1's DynamoDB views avoid (inferred). C1's index lists, though, stop at a 1,000-entry scan budget and filter in memory (measured). Lists with several filters on large repositories could then return short pages (inferred). Today only a failure of C3's fold gate can flip the base.
**Suggestion:** Add C1-style index lists to Milestone 0 on the same synthetic repository, with a page-completeness check. Also measure merge p99 on a cold node after an AZ drop. Choose the list engine from those numbers and keep the chain commit core either way. Move review verdicts into the core tier (1370).

### 7. [minor] The baseline still loses after the same fixes; no simpler foundation has evidence behind it
**Step:** The baseline and simpler alternatives. **Location:** DESIGN.md:1328, 1344, 1347-1350, 1367; baseline:1350; FRAME.md:29.
**Gap:** The design's own text says one MRSC `global` table replaces the baseline's identity protocols (1344). With that graft, the baseline's Build score rises to about 3 (inferred). It still carries the DSQL schema-change and connection incident classes, and it reaches only 20 Regions, so the Zurich install that BYOC and self-hosted need fails (measured, 1344). That gives about 32 to 33 (inferred). Both the baseline and this design run one AWS-managed store under every cell (1328, 1367). The difference in that risk is one of degree, not kind (inferred). Each simpler alternative examined either needs a second mechanism for RPO 0, or puts lists back on eventually consistent indexes (1347-1350, measured).
**Suggestion:** None for the store choice itself. Score the fleet-wide store risk the same way for all three designs.

**HOLDS WITH CHANGES** (per-key `fresh` probe; a defined `Mirror` heartbeat and a narrower lease scope; verify-on-deny with a capped revocation ack; a Milestone 0 race between the list engines; selection rationale rewritten). Strongest reason: both of the judge's reasons for C3 over C1 are gone, because the `Mirror` brought back C1's fail-closed lease and the protocol count grew past C1's. The pick survives only as a tie, held by per-repository chains and epoch-keyed promotion.