# The AWS design, the alternative Graftyard did not choose

Graftyard's design of record runs only on Cloudflare (`design/DESIGN.md`). Before it existed, the design run produced a complete design on AWS for the same goals: GHES 3.22 wire compatibility, uptime first, zero lost acknowledged pushes, then scale. This page summarizes that AWS design, prices it beside the Cloudflare design, and names the conditions under which it would be the better choice.

The full AWS design is `history/DESIGN.md`. Its reviews are under `history/review/`, and the arena rounds that produced it are under `history/arena/`.

## What the AWS design is

The AWS design is one Rust binary, pinned upstream git, a React web app, and two AWS stores, DynamoDB and S3 (`history/DESIGN.md`, Summary). Each repository and fork network has a gap-free chain of entries in a DynamoDB table, and each owner has two chains, `Authz` and `Identity`. The commit point is one conditional put of a chain's next entry, made after every byte the entry names is already in S3. No database transaction, SQL server, connection pool, or outbox sits under git, pull requests, issues, or authorization. Every read is a node-local fold of chains in SQLite on NVMe, proven current by a strongly consistent GetItem on the chain's next key. Each repository fold has a core tier (refs, rules, collaborators, pull request heads, review verdicts, required checks) that alone gates git, merges, and authorization, and a metadata tier (issues, comments, labels) that loads lazily. A cell is one table set, one bucket set, and one node fleet that share nothing with other cells (`history/DESIGN.md`, Shape). Managed has many cells under a worldwide MRSC `global` table on three continents that holds logins, token hashes, SSH fingerprints, installations, and the slot map. An install is one cell. A Paired cell uses MRSC global tables in two Regions plus a witness Region. A Single cell uses plain tables in any of 34 Regions, with an optional MREC `dr` replica in another Region. Managed, BYOC, and self-hosted ship one signed bundle, and an in-account `operator` role runs every lifecycle step from signed intents (`history/DESIGN.md`, "Deployment modes" and "Install operations"). Paired is the managed and BYOC default, and Single is the self-hosted default.

## Durability and availability

A Paired cell acknowledges a write only after a second Region holds it, so an acknowledged write survives the permanent loss of a Region with no failover step (`history/DESIGN.md`, Summary, citing `research/uptime.md#29`, `#37`, `#38`). Paired cells stay inside one continent. Only the managed `global` table spans continents (`design/DESIGN.md`, "Compared with the AWS design").

The durability table in `history/DESIGN.md`, "Availability", gives these RPO and RTO values:

| Durability class | RPO | RTO |
| --- | --- | --- |
| Paired cell, Region loss | 0 | About 2 minutes, health checks only [guess] |
| Managed `global`, Region loss | 0 | 0 on the two other continents |
| One repository, operator error | Any seq in the last 35 days | Minutes [guess] |
| A deleted repository or org | 0 within 90 days | Minutes [guess] |
| Single with `dr`, Region loss | Seconds for entries and inline packs (`research/uptime.md#30`). Up to 15 minutes for stored packs [unverified RTC SLA] | The runbook's wait (up to 1 hour) plus about 60 minutes of promotion, which launches nodes [guess] |
| Single without `dr`, Region loss | The newest cross-Region backup copy [unverified for DynamoDB] | Hours |

The SLO targets are per cell and per month, for Paired cells (`history/DESIGN.md`, "SLOs"). They rest on the 99.999% Global Tables SLA that every table in a Paired cell's Regions qualifies for (`research/dynamodb.md#22`). A minute is bad when more than 5% of a journey's requests in the cell fail, or when the external probe fails.

| Journey | Paired target |
| --- | --- |
| git fetch and clone | 99.99% |
| git push acknowledged | 99.95% |
| Pull request merge | 99.95% |
| REST and GraphQL reads | 99.99% |
| REST and GraphQL writes | 99.95% |
| Web page load | 99.9% |
| Actions job start, webhook first attempt within 60 s | 99.9% |
| Identity writes | 99.95% |
| Edge token resolution | 99.99% |
| Acknowledged push durability | 100% |

Single cells lower the 99.99% rows to 99.95%, because their tables carry the 99.99% Standard SLA and only fetches and reads have a stale mode [inference]. In stale mode a Single cell serves fetches and reads from warm folds for a signed limit (default 2 hours) and answers writes with 503. A Paired cell has no stale mode while both replica Regions serve. Token revocation binds writes within 5 s and reads within 60 s (`history/DESIGN.md`, Summary).

The one failure no bulkhead covers is a simultaneous AWS-side DynamoDB regression in both Regions of every cell (`history/DESIGN.md`, "Open questions and risks").

For comparison, the Cloudflare design claims RPO zero and an RTO of about 11.5 s for the loss of any one region, continent, or site release [inference] (`design/DESIGN.md`, Cost and Downtime). Its Milestone 0 passes that RTO only if a new leader commits and serves within 13 s at p99, restated by the operator to the measured 11.0 to 12.9 s.

## Cost

Monthly list prices in US dollars for one install. Both designs are priced on the same workload, which includes 8 pushes, 30 CI jobs, 15 fetches, and 2,500 API requests per developer per day.

| Setup | 50 developers | 300 developers | 2,000 developers |
| --- | --- | --- | --- |
| Cloudflare default, one account and three sites (the base) | $236 | $1,013 | $9,323 |
| Cloudflare default with the agent layer | $257 | $1,341 | $12,254 |
| AWS, one Region | $1,287 | $2,573 | $12,432 |
| AWS, Paired (two Regions plus a witness) | $2,292 | $4,466 | $20,636 |
| AWS `scrappy`: small-install profile, one Region with a cold recovery Region | $226 | not priced | not priced |
| AWS `scrappy-paired`: small-install profile, two Regions plus a witness | $388 | not priced | not priced |

Sources: `research/costs.md`, "The Cloudflare design by company size" (Cloudflare base, AWS one Region, AWS Paired), "The Cloudflare agent layer" (base with agents), and "Cheaper setups for 50 developers" (`scrappy`, `scrappy-paired`). The AWS one-Region and Paired rows also appear in `history/DESIGN.md`, "Deployment modes", and the Cloudflare base row in `design/DESIGN.md`, Cost.

The `paired-design` detail row in `research/costs.md` reads $2,301 at 50 developers, $9 above the table. `history/scripts/cost_small.py` produces that row and prices more KMS keys per Region than the table's script, as `research/costs.md` states for the $1 one-Region gap [inference: the same cause across three Regions].

Read the table with these limits, all from `research/costs.md`:

- AWS figures exclude the vendor's fee, CI runner compute, an AWS Support plan, and tax. Workload and node sizes are guesses until Milestone 0 and a load test measure them.
- Cloudflare figures exclude an Enterprise contract, which has no list price and which managed cells and pushes over 200 MB need. The Spectrum add-on fee is priced at $0 because no list price was found [unverified].
- The agent-layer row excludes runner compute and model compute.
- The AWS figures include no agent layer. The AWS design was never priced with agents, so compare the AWS rows with the Cloudflare base row.
- `scrappy` is not RPO zero for a Region loss, because its recovery Region is cold. `scrappy-paired` is RPO zero across two Regions. The Cloudflare base is RPO zero for the loss of any one of its three sites, which sit on three continents.

At 50 developers, AWS `scrappy` costs less than the Cloudflare base, but a Region loss is not RPO zero for it. At 300 and 2,000 developers the Cloudflare base costs less than either AWS setup ($1,013 against $2,573 and $9,323 against $12,432 for one Region). The fold audit's refolds, priced at a measured row size, briefly made the base the costlier one at 300 developers ($3,726); since they stage snapshots as pages instead of restoring them into scratch tables, the line is $2.26 and $9.29 a month there (`decisions/DECISIONS.md`, "`refold-stream`"; `decisions/OPEN-QUESTIONS.md`, K10). Compare base with base: the AWS rows carry no agent layer. Beside them, the Cloudflare default with the agent layer at 2,000 developers ($12,254) costs less than AWS one Region without agents ($12,432). It cost a little more ($12,802) while every entry was priced at the 17 member rows a repository `DefineLabel` writes; most entries are Actions and Identity entries, which write 8.3, so the rows are now priced by family and kind (`decisions/DECISIONS.md`, "`k12-rows`"; `decisions/OPEN-QUESTIONS.md`, K12).

## The differences that decide between them

These rows come from `design/DESIGN.md`, "Compared with the AWS design".

| Difference | AWS design | Cloudflare design |
| --- | --- | --- |
| Commit point | One conditional put on an MRSC DynamoDB chain item | A Raft quorum of three Durable Objects on three site scripts. Graftyard's maintainers own the consensus protocol, with transfer and `suspect` |
| RPO zero across regions | From the store's 99.999% SLA | From Graftyard's own protocol. No Cloudflare SLA covers Durable Objects |
| Surviving a continent | Managed `global` only. Paired cells stay in one continent | Every `Tri` chain |
| Provider-wide outage | AWS Regions fail separately | Cloudflare fails globally. Only a read-only lifeboat serves |
| Push size | Unbounded by the front | 100 MB on Pro, 200 MB on Business, 5 GB on Enterprise, refused at the edge with a message that names SSH where the install has it on, and otherwise Enterprise or splitting the push, since native SSH needs Enterprise (`tests/m0/results/m0-wp7.md`) |
| SSH path | NLB to nodes | HTTPS by default. Native SSH on Enterprise, Spectrum to Tunnel to containers; on Pro and Business an opt-in Tunnel public hostname that each client reaches with `cloudflared access ssh` (`FRAME.md`, "Operator decision, 2026-10-31, SSH ingress") |
| Largest repository | Node NVMe | 20 GB shard disk, about 15 GB of packs |
| Token revocation | 5 s for writes, 60 s for reads | Immediate for writes, 10 s for reads |

## Why the Cloudflare design is the design of record

The operator asked for a Cloudflare-only design on 2026-10-26. `FRAME.md`, "Operator decision, 2026-10-26, Cloudflare", records the request verbatim: "note all these in the docs. then, use again and come up with what this would look like entirely cloudflare native. only cloudflare technologies. they have stuff like cloudflare basin, etc." At that point the run read the request as asking for a comparison, not a replacement [inference], and kept the AWS design as it was.

The operator's next answers were all about the Cloudflare design and its agent layer. `FRAME.md`, "Operator decision, 2026-10-26, agents answered and graftyard", records them verbatim: "1 - offered is fine 2 - yes, include leases and tasks in v1 3 - sounds good as you decided 4 - default is fine 5 - sounds good 6 - sounds good 7 - ssh through spectrum read all fixes. do one final pass at everything. use for this, then clean up all notes so it is a single comprehensive folder of notes, cleaned, ready for an implementation by other agents. we are going to call this 'graftyard'." Answer 7 keeps SSH through Spectrum, a Cloudflare product. The same section records the run's reading that the Cloudflare design is the design of record, because every answer is about it [inference].

The operator never said to use Cloudflare instead of AWS. Choosing the Cloudflare design as the design of record is the run's reading of the operator's answers, not an operator statement. The choice does not rest on a review score either. In the Cloudflare arena the cross-judge scored the AWS design 42.5 as an ineligible reference, against 41.5 for the three-site Raft candidate that became the base (`design/DESIGN.md`, "Synthesis decision").

## When the AWS design would be the better choice

Each condition below would favor the AWS design. Only condition 5 holds today, for BYOC customers on Pro who need SSH from stock clients with no `cloudflared`.

1. **The Cloudflare consensus gate fails and its fallback is not acceptable.** Cloudflare Milestone 0 runs 48 hours of Porcupine-checked histories under dropped calls, member resets, and leadership transfers. Any linearizability violation, or a `Tri` commit p99 above 1 s, makes candidate 1 the Cloudflare base (`design/DESIGN.md`, "Synthesis decision", flip condition 1). Candidate 1 still acknowledges on one region's copy after any failover. An operator who needs RPO zero across regions after that flip has it in AWS Paired, where the store provides it [inference].
2. **Writes must survive a provider-wide outage.** Cloudflare fails globally, the lifeboat is read-only, and managed users have no service during a Cloudflare-wide outage (`design/DESIGN.md`, "Tradeoffs accepted"). AWS Regions fail separately, so a Paired cell keeps writing through the loss of one Region.
3. **The commit point must rest on a provider SLA.** AWS Paired gets RPO zero from the 99.999% Global Tables SLA. On Cloudflare no SLA covers Durable Objects, and Graftyard's own Raft carries that promise.
4. **Pushes above 200 MB are needed without an Enterprise contract.** Pro caps HTTPS pushes at 100 MB and Business at 200 MB, and Enterprise raises the cap to 5 GB (`design/DESIGN.md`, "Compared with the AWS design"). Enterprise has no list price (`research/costs.md`). The AWS front does not bound push size.
5. **SSH fails Cloudflare Milestone 0 on the customer's plan.** If Spectrum cannot reach a Tunnel origin on Pro or Business, SSH is dropped or needs Enterprise there (`design/DESIGN.md`, "Tradeoffs accepted"). On Pro this has happened for native SSH: the Spectrum API refuses a Tunnel origin, so native SSH needs Enterprise (`tests/m0/results/m0-wp7.md`); Business is not run. Pro and Business installs may turn on SSH through `cloudflared access ssh`, which needs `cloudflared` on every client and its own Milestone 0 row (B0-24a). The AWS design serves SSH from an NLB to its nodes with no client helper.
6. **A repository is larger than a Cloudflare shard can hold.** A git cache shard has a 20 GB disk, which holds about 15 GB of packs. The AWS design keeps packs in S3 and caches them on node NVMe.
7. **A 50-developer install accepts that a Region loss is not RPO zero.** AWS `scrappy` costs $226 a month against the Cloudflare base's $236, with a cold recovery Region instead of RPO zero.

The AWS design has its own gate. If one chain on MRSC fails the append gate (p99 above 150 ms, or under 200 entries a second), the AWS base flips to the earlier Aurora DSQL design (`history/DESIGN.md`, Milestone 0 flip conditions). Anyone who revives the AWS design starts with that measurement.

## Where to read more

- `history/DESIGN.md`: the full AWS design, including "Failure modes", "Build order", "Tradeoffs accepted", "Alternatives considered", and "Open questions and risks".
- `history/arena/round4/SYNTHESIS.md`: why the chain-per-aggregate foundation won round 4, and its eight grafts.
- `history/review/round4-judge.md` and `history/review/round4-foundation-attack.md`: the judge's scoring and the attack that tied the two leading AWS candidates.
- `history/review/cloudflare-judge.md`: the Cloudflare arena judgment that scored the AWS reference.
- `research/costs.md`: every cost figure on this page, with line items.
