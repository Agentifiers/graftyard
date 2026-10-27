# Round 4 synthesis: the foundation of a new GitHub

## Base

Candidate 3, "every commit is one conditional put". Each repository, fork network, and owner has a gap-free chain in DynamoDB. Every acknowledged change is one conditional put of the next entry, made after the bytes it names are in S3. Every read is a node-local fold of chains, proven current by one strongly consistent tail read that started after the request arrived. Paired cells use MRSC tables. Single-Region installs use plain tables in any of 34 Regions. A worldwide MRSC `global` table holds logins, token hashes, SSH fingerprints, installations, and the slot map in managed mode.

## Scores

| Design | Lead (first read) | Lead (after the judge) | Cross-judge |
| --- | --- | --- | --- |
| Baseline, Aurora DSQL | 36 | 33 | 30 |
| Candidate 1, one head per fork network | 41 | 38 | 36 |
| Candidate 2, home-Region transactions plus a journal | 33 | 31 | 30 (33 before a fact cap) |
| Candidate 3, a chain per repository, network, and owner | 39 | 41 | 41 |

My first read put candidate 1 ahead. I weighted its index items, which need no fold engine, and its atomic fork merges. The judge (`review/round4-judge.md`) put candidate 3 ahead for two reasons I checked and accept.

- Candidate 1's revocation lease makes every cell fail writes closed for a whole continent's repositories when it cannot re-read that continent's revocation logs within 5 s (candidate 1 lines 216 to 220 and 291). That reintroduces the cross-continent coupling the baseline's freshness boards had. Downtime carries weight 3.
- Candidate 3 has one commit primitive (`commit`) and one freshness primitive (`fresh`). Candidate 1 has a sequencer, view catch-up, a crash-repair pump for `meta` writes, revocation logs with leases, projections with verify-on-deny, continental identity tables, and `reconstitute`.

The rubric's tie-breaker (which base a maintainer extends most easily without breaking invariants) also favors candidate 3. A new feature is a new event kind on a chain and a new fold table.

## Convergence

All three runners chose DynamoDB and S3 with no Aurora DSQL, no transactions on the multi-Region path, and a durable log that outlives DynamoDB Streams, with Streams used only to wake consumers. Candidate 3's brief allowed any store, and it rejected DSQL with numbers. This agreement is weak evidence, because every seat runs the same model.

## Grafts

1. **Required checks race-free (from candidate 2).** A conclusion for a check that a branch rule requires is also appended to the repository chain as a `RequiredCheck` entry. A merge's `decide` therefore sees every required conclusion committed before its put. Other check runs stay in the `checks` table.
2. **Repository restore from the chain (from candidate 1, in place of candidate 2's scratch tables).** A `RestoredTo { seq }` entry resets the repository's fold to its state at that seq. Later entries stay in the chain as history, and packs survive under the 14-day GC rule. Repairing one repository needs no table restore.
3. **Write-class token re-check (from candidate 2).** A write-class request whose token entry in the edge cache is older than 5 s re-reads the token strongly in `global`, coalesced per token per node. Token revocation then binds writes within 5 s and reads within 60 s.
4. **Bounded stale fetches in Single cells (from candidate 1 and the baseline).** When a Single cell's tail reads fail, warm folds serve fetches and reads at their last proven seq for a signed limit (default 2 hours), writes get 503, and a weekly drill keeps the mode exercised. Paired cells have no stale mode, because the peer Region serves.
5. **Tiered folds (mine, answering the judge's top flaw).** Each repository fold has a small core tier (refs, rules, collaborators, pull request heads, required checks) that loads first and alone gates git and authorization. The metadata tier (issues, comments, reviews, labels) loads lazily from its own snapshot. A node loss or a new metadata schema therefore never delays git.
6. **Milestone 0 throttle test (from candidate 1).** One chain at 125 puts a second plus 1,800 strong tail reads a second for an hour, with zero sustained throttling.
7. **No routing-control cluster in installs (from the cost estimate).** A Single cell with `dr` promotes through Route 53 failover records and health checks. An ARC routing-control cluster costs $1,825 a month (`research/pricing.md`).
8. **One SLA class per Paired cell (from the judge's fact check 22).** Every table in a Paired cell's Regions is a global table, so the 99.999% Global Tables SLA applies.

## Rejected

- Candidate 1's head per fork network carrying every metadata event. It is one partition that cannot be split (candidate 1 line 537).
- Candidate 1's revocation logs and leases, and its continental identity tables with `reconstitute`. Candidate 3's `global` table removes `reconstitute`, at the cost of one cross-continent round trip on signups, renames, and token creation.
- Candidate 2's journal, home lease, promotion, and standby. They form a home-grown failover protocol that halts the cell whenever the lease cannot renew.
- Candidate 2's eventually consistent directory, and candidate 1's 256 KB inline pack items. S3 placement in two of three buckets already gives RPO 0.

## Flip conditions

- If Milestone 0's fold gate fails (on 200,000 issues and 2 million comments, a `labels=bug&sort=updated` p99 above 200 ms, or a cold 2 GB snapshot load above 120 s), candidate 1 becomes the base with candidate 3's owner-chain authorization.
- If Milestone 0's chain append gate fails on MRSC (p99 above 150 ms, or under 200 entries a second on one chain), Aurora DSQL refs-as-rows (the round 1 to 3 design) remains the base.

## Verification

`scripts/check_candidate.py` passes all three candidates. The judge reran each cost script and reproduced each table. Integration is verified by `scripts/check_design.py --final`, an interrogate pass, and a walkthrough of the seven journeys against the integrated `DESIGN.md`.
