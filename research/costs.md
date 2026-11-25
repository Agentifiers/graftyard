# Cost reference

`scripts/costs_report.py` writes this file from `scripts/cost_model.py`, `history/arena/round4/candidate-3/cost.py`, `history/scripts/cost_small.py`, `scripts/cost.py`, and `scripts/cost_agents.py`. Do not edit it by hand. To change a number, change a script or a price and run `python3 scripts/costs_report.py`.

Every figure outside the Cloudflare sections is a monthly AWS list price in US dollars for one install, with us-east-2 as the home Region, us-west-2 as the peer, and us-east-1 as the witness. Prices come from `research/prices.json`, read from the AWS Price List files on 2026-10-24. No figure includes the vendor's fee, CI runner compute, an AWS Support plan, or tax. Workload and node sizes are guesses until Milestone 0 and a load test measure them.

## Workload the model assumes

Each developer generates, per day, 8 pushes, 30 CI jobs with one checkout each, 15 fetches, 2,500 API requests, 20 issue and pull request writes, and 10 emails.

| Developers | Git data | Actions storage | Node type in the model | Nodes per Region |
| --- | --- | --- | --- | --- |
| 50 | 30 GB | 400 GB | i8g.xlarge | 3 |
| 300 | 300 GB | 4,500 GB | i8g.2xlarge | 3 |
| 2,000 | 4,000 GB | 30,000 GB | i8g.4xlarge | 6 |

## The design by company size

The design is `history/DESIGN.md` as of round 4. The DSQL design is the round 1 to 3 design, with its two known overcharges fixed (backups kept as hourly, daily, and weekly copies, and 16 outbox partitions).

| Developers | One Region | Two Regions (Paired) | DSQL design, one Region | DSQL design, two Regions |
| --- | --- | --- | --- | --- |
| 50 | $1,287 | $2,292 | $1,433 | $2,485 |
| 300 | $2,573 | $4,466 | $3,103 | $5,062 |
| 2,000 | $12,432 | $20,636 | $16,384 | $24,879 |

## Where a 50-developer bill goes

This is the design as written, in one Region, from `PYTHONPATH=scripts python3 history/scripts/cost_small.py --detail design`. It is $1 more than the one-Region figure above, because `history/scripts/cost_small.py` prices six KMS keys per Region and `history/arena/round4/candidate-3/cost.py` prices five.

| Line item | Per month | Share |
| --- | --- | --- |
| EC2 nodes (3 x i8g.xlarge, ondemand) | $752 | 58% |
| OpenSearch (code and issue search) | $207 | 16% |
| NAT gateways (3) and public IPv4 | $123 | 10% |
| CloudWatch metrics, alarms, logs | $76 | 6% |
| Network Load Balancer | $22 | 2% |
| Interface endpoint for DynamoDB Streams | $22 | 2% |
| DynamoDB Streams reads (dispatcher) | $16 | 1% |
| EBS gp3 root and cache volumes | $12 | 1% |
| KMS keys and requests | $10 | 1% |
| Data transfer out to the internet (developers) | $9 | 1% |
| S3 Actions cache, artifacts, logs | $9 | 1% |
| Same-Region transfer to CI runners (install side) | $9 | 1% |
| S3 requests (packs, snapshots, status, Actions) | $6 | 0% |
| Inter-AZ transfer inside the install | $4 | 0% |
| Route 53 zone, health checks, queries | $3 | 0% |
| S3 git packs (copies and noncurrent versions) | $3 | 0% |
| SES email | $2 | 0% |
| DynamoDB reads (strong tail reads, cache misses) | $1 | 0% |
| DynamoDB writes (chains, consumers, Actions) | $1 | 0% |
| DynamoDB storage and PITR | $1 | 0% |
| S3 fold snapshots and chain archive | $0 | 0% |
| Total | $1,288 | 100% |

## Cheaper setups for 50 developers

Each row changes the lines its configuration names and keeps the rest. Rows from `s-net` down use the small-install profile in `history/DESIGN.md` ("Install operations"). Run `PYTHONPATH=scripts python3 history/scripts/cost_small.py --detail <config>` for the line items of any row.

| Config | Configuration | Per month | Per developer |
| --- | --- | --- | --- |
| `design` | Design as written: 3 i8g.xlarge, 3 NAT gateways, 2-node search, full metrics | $1,288 | $26 |
| `nodes` | Right-size nodes: 3 m8g.large with 100 GB gp3 cache each | $750 | $15 |
| `nat` | Plus one NAT gateway instead of three | $677 | $14 |
| `search` | Plus search on one m7g.medium.search node | $522 | $10 |
| `lean` | Plus lean CloudWatch metrics (all SLO alarms kept) | $488 | $10 |
| `lean-1yr` | Lean, 1-year Compute Savings Plan, no upfront | $436 | $9 |
| `lean-3yr` | Lean, 3-year Compute Savings Plan, no upfront | $392 | $8 |
| `lean-t4g` | Lean on burstable t4g.large, 3-year plan | $365 | $7 |
| `lean-dr` | Lean, 3-year plan, plus a cold recovery Region | $405 | $8 |
| `lean-dr-warm` | Lean, 3-year plan, plus a recovery Region with one warm node | $480 | $10 |
| `lean-paired` | Lean, 3-year plan, two Regions plus a witness (Paired) | $674 | $13 |
| `paired-design` | Paired as written | $2,301 | $46 |
| `s-start` | Scrappy start: lean-dr above | $405 | $8 |
| `s-net` | Plus public subnets, no NAT, no Streams interface endpoint | $355 | $7 |
| `s-search` | Plus no OpenSearch: issue search in SQLite FTS5, code search by git grep | $303 | $6 |
| `s-obs` | Plus logs to S3, 12 alarms on the SLO metrics | $270 | $5 |
| `s-t4g` | Plus 3 x t4g.medium (burstable, unlimited), on-demand | $235 | $5 |
| `scrappy` | Plus dispatcher polls each shard once a second | $226 | $5 |
| `scrappy-3yr` | Scrappy on a 3-year plan [inferred rate] | $189 | $4 |
| `scrappy-dns` | Scrappy without the NLB: Route 53 health-checked records to node IPs, on-demand | $195 | $4 |
| `scrappy-paired` | Scrappy, two Regions plus a witness, on-demand | $388 | $8 |
| `floor` | Floor: one t4g.medium in one AZ, cold recovery Region, no uptime target | $121 | $2 |

## The small-install profile, line by line

This is the `scrappy` row: three t4g.medium nodes in three AZs, on-demand, with a cold recovery Region.

| Line item | Per month | Share |
| --- | --- | --- |
| EC2 nodes (3 x t4g.medium, ondemand) | $74 | 33% |
| Network Load Balancer | $22 | 10% |
| Public IPv4 (3 nodes, 3 NLB) | $22 | 10% |
| EBS gp3 root and cache volumes | $19 | 8% |
| KMS keys and requests | $16 | 7% |
| CloudWatch metrics, alarms, logs (logs in S3) | $10 | 4% |
| Data transfer out to the internet (developers) | $9 | 4% |
| S3 Actions cache, artifacts, logs | $9 | 4% |
| Same-Region transfer to CI runners (install side) | $9 | 4% |
| DynamoDB Streams reads (dispatcher) | $8 | 3% |
| S3 requests (packs, snapshots, status, Actions) | $6 | 3% |
| Inter-AZ transfer inside the install | $4 | 2% |
| Route 53 zone, health checks, queries | $3 | 1% |
| dr: S3 replicated packs and snapshots | $3 | 1% |
| S3 git packs (copies and noncurrent versions) | $3 | 1% |
| dr: S3 replication transfer | $2 | 1% |
| SES email | $2 | 1% |
| DynamoDB reads (strong tail reads, cache misses) | $1 | 0% |
| DynamoDB writes (chains, consumers, Actions) | $1 | 0% |
| dr: DynamoDB replica writes | $1 | 0% |
| DynamoDB storage and PITR | $1 | 0% |
| dr: DynamoDB replica storage and PITR | $1 | 0% |
| dr: failover health check and alarm | $1 | 0% |
| dr: no-promotion fence reads | $0 | 0% |
| S3 fold snapshots and chain archive | $0 | 0% |
| Total | $226 | 100% |

The fixed part of this bill (load balancer, public IPv4, KMS keys, CloudWatch, and Route 53) is $73 a month. It does not shrink with fewer developers, which is why no reliable setup costs much less than the `scrappy` row.

## Lessons from the DSQL design

The first cost model found three costs that the DSQL design hid.

| Developers | DSQL design, two Regions, fixed | Extra for hourly full backups kept 35 days | Extra for 256 outbox partitions polled each second |
| --- | --- | --- | --- |
| 50 | $2,485 | $1,182 | $643 |
| 300 | $5,062 | $6,698 | $643 |
| 2,000 | $24,879 | $55,160 | $643 |

- DSQL backups through AWS Backup are always full copies (https://docs.aws.amazon.com/aws-backup/latest/devguide/backup-aurora.html). Hourly backups kept 35 days hold 840 copies, which the table's middle column prices.
- Polling 256 outbox partitions every second costs the same at 50 developers as at 2,000, which the last column prices.
- A Route 53 Application Recovery Controller cluster costs $1,825 a month, more than a whole small install. The current design uses Route 53 failover records instead.

## The Cloudflare design by company size

`design/DESIGN.md` is the design built only on Cloudflare products. Its figures come from `python3 scripts/cost.py`, on the same workload, at Cloudflare list prices (the `cf.*` entries in `research/prices.json`). They exclude an Enterprise contract, which has no list price and which managed cells and pushes over 200 MB need. Per-operation constants in that script are guesses until its Milestone 0.

| Developers | Cloudflare default (one account, three sites) | Cloudflare, account-separated sites | Cloudflare `Solo` (evaluation, no SLO) | AWS design, one Region | AWS design, Paired |
| --- | --- | --- | --- | --- | --- |
| 50 | $238 | $248 | $133 | $1,287 | $2,292 |
| 300 | $1,016 | $929 | $599 | $2,573 | $4,466 |
| 2,000 | $9,335 | $9,232 | $5,395 | $12,432 | $20,636 |

At 50 developers the cheapest reliable setups are the Cloudflare default at $238 (RPO zero across three continents, on the Pro plan with a 100 MB HTTPS push cap and an unpriced Spectrum add-on), AWS `scrappy` at $226 (one Region with a cold recovery Region, so a Region loss is not RPO zero), and AWS `scrappy-paired` at $388 (RPO zero across two Regions).

## Where a 50-developer Cloudflare bill goes

This is the Cloudflare default, from `python3 scripts/cost.py --detail small design`.

| Line item | Per month | Share |
| --- | --- | --- |
| Containers memory (provisioned) | $157 | 66% |
| Zone plan (Pro list; Business, Enterprise, and managed cells not priced) | $25 | 11% |
| Containers disk (provisioned) | $9 | 4% |
| R2 storage (pack copies, clone cache, Actions) | $9 | 4% |
| Durable Objects SQLite storage | $8 | 3% |
| Vault account (plan, locked R2 copy, writes) | $6 | 3% |
| Workers Paid subscriptions (install) | $5 | 2% |
| Runner account (Workers Paid; runner compute excluded) | $5 | 2% |
| ReplaceMember and RestorePlacement (one site loss a year, amortized) | $5 | 2% |
| Email Sending | $4 | 2% |
| Durable Objects requests | $2 | 1% |
| R2 Class A | $1 | 1% |
| Fold live-row check, weekly per voter (K11) | $1 | 0% |
| Inbox Durable Objects (notifications, feeds, cross-repository lists) | $1 | 0% |
| Spare sites (eeur, wnam, oc; scripts and empty buckets) | $0 | 0% |
| Containers vCPU (active) | $0 | 0% |
| Basin Pipelines (audit and SLO history) | $0 | 0% |
| Queues (webhooks, notifications, indexing) | $0 | 0% |
| Fold audit refolds (repository chains that moved since their last match) | $0 | 0% |
| Workers requests | $0 | 0% |
| Workers CPU (graft-front, edge, sites) | $0 | 0% |
| Durable Objects duration | $0 | 0% |
| Durable Objects SQLite rows written | $0 | 0% |
| R2 Class B | $0 | 0% |
| Containers egress by site region and leader split | $0 | 0% |
| Containers: hot pool kept warm for hot networks, working hours | $0 | 0% |
| Spectrum SSH data | $0 | 0% |
| Spectrum Worker application fee [unverified, no price published] | $0 | 0% |
| Analytics Engine (per-repo counters, SLO points) | $0 | 0% |
| Workers Logs ingest and storage | $0 | 0% |
| Total | $238 | 100% |

## The Cloudflare agent layer

`design/AGENT-LAYER.md` adds agents as users: 3 agent tasks per developer per day, each with 5 pushes, 60 REST calls, 2 clones, 1 draft pull request, and 3 CI jobs. Its figures come from `python3 scripts/cost_agents.py`, which reruns `scripts/cost.py` with that workload and prices the new pieces, including the extra warm git cache shards for each size's largest network. Runner and model compute are excluded, and opt-in Code Mode is priced separately in that script.

| Developers | Cloudflare default | Agent layer added | Default with agents | Added share |
| --- | --- | --- | --- | --- |
| 50 | $238 | $22 | $259 | 9% |
| 300 | $1,016 | $350 | $1,366 | 34% |
| 2,000 | $9,335 | $3,101 | $12,436 | 33% |
