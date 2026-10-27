# Round 4 rubric: the foundation of a new GitHub

Judge-only. Candidates never see this file. Score each candidate and the baseline (`DESIGN.md` as it stands) from 1 to 5 on each criterion, multiply by the weight, and sum. Maximum 50.

The operator's goal, verbatim: "build an entire new, better scalable github that has less downtime". Downtime comes first and scale second. Convenience of building counts only through outage risk and build risk.

Treat two known defects in the baseline as already fixed, and do not score them: DSQL backups kept as 840 hourly full copies (fix: hourly for a day, daily for a week, weekly for a month), and 256 outbox partitions polled every second in installs (fix: 16 partitions with batched lease renewal). Both come from `scripts/cost_model.py` and `research/dynamodb.md`.

## Criteria

1. **Downtime, weight 3.** For each GitHub incident class in `research/uptime.md` findings 40 and 43 to 49, does the foundation remove it, contain it to a cell or a repository, or only mitigate it? Weigh by incident-minutes: database about 2,500, capacity about 1,350, config about 1,260 (finding 43). Also score Region loss (RPO and RTO per class), what keeps serving while each store is impaired, and the blast radius of a bad deploy and a bad config. 5 means every database-class incident is removed or contained by construction and Region loss loses no acknowledged write. 1 means the design adds a shared store whose failure stops core surfaces everywhere.
2. **Scale, weight 2.** The busiest single repository it carries (pushes, merges, bot ref updates, clones), with the limiter named and the number labeled; fleet scale to 3.35 billion pushes a month; fan-out; hot keys. 5 means every ceiling is named, numbered, and above GitHub's observed peaks, with a sharding answer for each hot key. 1 means an unbounded hot key or a fleet-wide serialization point.
3. **Correctness, weight 2.** Never loses an acknowledged push. Merge atomicity for same-repository and fork pull requests. Read-your-writes on the endpoints that `gh`, GitHub Desktop, VS Code, and Actions rely on (journeys 2 and 3). Revocation bound. Idempotency under retries. 5 means each guarantee has one mechanism that a reader can point at, and every weakening is stated. 1 means a journey contradicts a stated guarantee.
4. **Build and operate risk, weight 2.** How much novel distributed protocol the team must write and prove (sagas, fences, relays, materializers, leases), the number of stores to run, how testable the invariants are, and whether Milestone 0 can falsify the foundation cheaply. 5 means few protocols, each with a test that fails loudly. 1 means correctness rests on several unproven protocols interacting.
5. **Install fit, weight 0.5.** Managed, BYOC, and self-hosted forms per component; Regions each mode can run in; backups and point-in-time restore; what a self-hoster operates.
6. **Cost, weight 0.5.** Monthly AWS cost at 50, 300, and 2,000 developers against the baseline's `scripts/cost_model.py` figures after the two fixes above.

## Fact checks

Check every AWS fact a candidate's score depends on against the cited source or the AWS documentation, especially DynamoDB transaction and MRSC limits, per-partition throughput, Streams retention and readers, DSQL limits, and SLAs. A false fact that a criterion rests on caps that criterion at 2 until fixed. List each check as confirmed, refuted, or unverified, with the URL.

## Output

A table of scores per criterion per design, weighted totals, the fact checks, the most serious flaw in each design, what each loser does better than the winner (graft candidates), and a recommended base with the single condition that would flip it.
