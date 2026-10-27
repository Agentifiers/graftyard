# AWS and GitHub list prices for a Graftyard BYOC install

Collected 2026-10-24. Every AWS number in `prices.json` comes from the AWS Price List bulk API (`https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/<service>/<version>/<region>/index.json`) unless its `source` names a pricing page. Each entry's `source` is the exact versioned file it came from. Prices are identical in us-east-2, us-west-2, and us-east-1 unless a line says otherwise. The extraction script is `/tmp/graftyard-pricing/extract.py`; it fails loudly if a usage type matches more than one price.

## EC2 and EBS
- Linux on-demand per hour: i8g.xlarge $0.3432, i8g.2xlarge $0.6864, i8g.4xlarge $1.3728, m8g.xlarge $0.17952.
- c8g.large is $0.07948 in us-east-2 and $0.07976 in us-west-2 and us-east-1.
- The us-east-2 values match the JSON the EC2 pricing page loads (`b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/ec2/...`).
- Instance store: i8g.xlarge 1 x 937 GB, i8g.2xlarge 1 x 1,875 GB, i8g.4xlarge 1 x 3,750 GB (NVMe SSD). m8g and c8g are EBS only.
- Compute Savings Plan, 1-year, No Upfront, per hour: i8g.xlarge $0.267, i8g.2xlarge $0.534, i8g.4xlarge $1.068 in all three Regions. That is about 22% off on-demand. Source is the `AWSComputeSavingsPlan` bulk file.
- EBS gp3: $0.08 per GB-month, $0.005 per provisioned IOPS-month above 3,000, $0.04 per provisioned MiB/s-month above 125.

## Aurora DSQL
- DPU: $8.00 per 1M DPUs (bulk lists $0.000008 per DPU). Storage: $0.33 per GB-month.
- Free tier: first 100,000 DPUs and 1 GB-month each month, per AWS Organizations management account.
- Multi-Region billing, from the pricing page FAQ and the billing docs:
  - Each Region bills the read, write, and compute DPUs of requests that originate there.
  - Every write replicated to a peered Region adds MultiRegionWriteDPU equal to the originating WriteDPU. It is billed in the originating Region, not the peer. The docs say "a transaction that accrues 8 WriteDPU incurs an additional 8 MultiRegionWriteDPU" in a two-active-plus-witness setup.
  - There is no separate data transfer charge for replication; it is inside MultiRegionWriteDPU.
  - Storage is billed in every Region holding a peered cluster, including clusters in Pending Delete state.
  - "You are not charged for DPUs or storage for your witness Region, as it does not execute transactions or store a data replica."
  - Inter-AZ replication inside a Region is free.
- Pricing page example 2 (two active Regions, N. Virginia takes all writes): Region 1 used 2M WriteDPU, 2M MultiRegionWriteDPU, 0.5M ReadDPU, and 1.5M ComputeDPU (6M total). Region 2 used 1M ReadDPU and 1.312M ComputeDPU. Each Region paid for 25 GB-month. Total $87.996.
- CDC: billed as StreamDPU at the normal DPU rate. `StreamDPU = Bytes Streamed x 0.0000023283`. Kinesis Data Streams is billed separately.
- Aurora DSQL usage may be eligible for Database Savings Plans (1-year). No rate collected.
- Backup through AWS Backup is supported. Warm storage $0.10 per GB-month, cold $0.03 per GB-month, warm restore $0.02 per GB. Cross-Region copy between these three Regions is $0.02 per GB. Logically air-gapped vault warm storage is $0.115 per GB-month.

### DPU formulas and examples (docs, quoted)
- `ComputeDPU = Total Compute time (in seconds)`.
- `WriteDPU = max(Bytes Written, 1024) x 0.00004883`. Rows under 128 bytes bill as 128. Writes also incur a ReadDPU primary-key check.
- `ReadDPU = max(Bytes Read, 2048) x 0.00000183105`. Reads under 128 bytes per storage partition round up to 128.
- Point lookup, 5 rows of 100 bytes, 3 ms: ComputeDPU 0.003, ReadDPU 0.00375, total 0.00675 DPU.
- Filtered scan of 100 rows of 100 bytes, 8 ms: ComputeDPU 0.008, ReadDPU 0.01831, total 0.02631 DPU.
- Single 100-byte insert, 8 ms: ComputeDPU 0.008, ReadDPU 0.00375, WriteDPU 0.05, total 0.06175 DPU.
- Bulk insert of 100 rows of 100 bytes, 80 ms: ComputeDPU 0.08, ReadDPU 0.00375, WriteDPU 0.625, total 0.70875 DPU.
- EXPLAIN ANALYZE VERBOSE samples: a 120-row index-only SELECT showed 0.05919 DPU. A 3-row INSERT showed 0.03732 DPU before the transaction minimums (Read 0.00375, Write 0.05) apply.
- Pricing FAQ: "100K DPUs were equivalent to ~700,000 TPC-C transactions" with a 95/5 read/write mix.
- Billed transaction floor: any write transaction pays at least WriteDPU 0.05 plus ReadDPU 0.00375. At $8 per 1M DPUs that is about $0.00000043 before compute.

## S3 Standard
- Storage $0.023 per GB-month for the first 50 TB ($0.022 next 450 TB, $0.021 above).
- PUT, COPY, POST, and LIST cost $0.005 per 1,000. GET costs $0.0004 per 1,000.
- Replication: you pay destination storage, replication PUTs at destination rates, and inter-Region transfer out at the source Region rate (see data transfer). Replication Time Control adds $0.015 per GB, and its metrics bill as CloudWatch custom metrics.
- S3 to EC2 (or any AWS service) in the same Region is free. The S3 and EC2 pricing pages both say so. NAT gateway or interface endpoint processing still applies if the traffic goes through one.

## Data transfer
- Internet egress from each Region: $0.09 per GB for the first 10 TB, $0.085 for the next 40 TB, $0.07 for the next 100 TB, $0.05 above 150 TB per month. The first 100 GB per month is free, aggregated across all Regions and services.
- Inter-Region per GB: us-east-2 to us-west-2 is $0.02, and us-west-2 to us-east-2 is $0.02. us-east-2 to us-east-1 is $0.01, and us-east-1 to us-east-2 is $0.01. us-east-1 and us-west-2 are $0.02 each way. Inbound is free.
- Inter-AZ in one Region: $0.01 per GB in each direction. That is $0.02 per GB crossing, since both sides bill. Source is the EC2 on-demand pricing page and the `DataTransfer-Regional-Bytes` usage type.

## Network Load Balancer
- $0.0225 per NLB-hour and $0.006 per NLCU-hour.
- An NLCU is the highest of three dimensions, averaged over the hour.
- TCP NLCU: 800 new connections per second, 100,000 active connections sampled per minute, or 1 GB processed per hour.
- TLS NLCU: 50 new connections per second, 3,000 active connections, or 1 GB per hour.
- UDP NLCU: 400 new flows per second, 50,000 active flows, or 1 GB per hour.
- Each in-use public IPv4 address costs $0.005 per hour. That applies to internet-facing NLB nodes and NAT gateways.

## NAT gateway and VPC endpoints
- NAT gateway: $0.045 per hour and $0.045 per GB processed, on top of any data transfer.
- Interface endpoint (PrivateLink): $0.01 per endpoint per AZ per hour. Processing is $0.01 per GB for the first 1 PB per month.

## KMS
- $1 per customer managed key per month, prorated hourly.
- Multi-Region keys bill each primary and each replica at $1 per month.
- The first and second rotations add $1 per month each.
- Symmetric requests cost $0.03 per 10,000. Asymmetric requests other than RSA 2048, such as ECC Sign, cost $0.15 per 10,000. RSA 2048 asymmetric requests cost $0.03 per 10,000.
- The first 20,000 requests per month are free across all Regions.

## CloudWatch
- Custom metrics: $0.30 per metric-month for the first 10,000. Then $0.10 to 250,000, $0.05 to 1M, and $0.02 above.
- Alarms: $0.10 per standard-resolution alarm metric-month, $0.30 high resolution.
- Logs: Standard class ingestion $0.50 per GB. Storage $0.03 per GB-month.
- Free tier: 10 metrics, 10 alarms, 5 GB ingestion, and 5 GB-month storage.

## Route 53 and ARC
- Hosted zone: $0.50 per month for the first 25, then $0.10.
- Standard queries: $0.40 per 1M for the first 1B per month. Alias queries to AWS resources are free.
- Health checks on AWS endpoints: the first 50 in an account are free, then $0.50 per check-month. Each optional feature (HTTPS, string matching, fast interval, latency) adds $1.00 per month. An HTTPS check with string matching on an AWS endpoint is therefore $2.00 per month inside the free 50, or $2.50 after.
- Non-AWS endpoints: $0.75 per check-month plus $2.00 per optional feature.
- ARC routing-control cluster: $2.50 per cluster-hour (about $1,825 per 730-hour month), with at most 2 per account. ARC Region switch plan: $70 per plan per month.

## OpenSearch Service
- The best-practices docs say to avoid T2 and t3.small in production and that "`r6g.large` instances are an option for small production workloads". They also recommend the latest generation.
- Hourly prices: r6g.large.search $0.167, r7g.large.search $0.178, r8g.large.search $0.196, m7g.large.search $0.135, m8g.large.search $0.15.
- gp3 storage: $0.122 per GB-month. Provisioned IOPS are $0.008 per IOPS-month and throughput $0.064 per MB/s-month.
- The sizing docs recommend at least three data nodes, plus three dedicated master nodes for production.

## SES
- Outbound email costs $0.10 per 1,000 recipients, plus $0.12 per GB of attachments.

## GitHub (comparison)
- GitHub Enterprise Cloud: "Starting at $21 USD per user/month". Includes 50,000 Actions minutes per month. From github.com/pricing.
- GitHub Enterprise Server has no separate list price. The GitHub Enterprise plan uses unique-user licensing that covers both Cloud and Server. From docs.github.com, Combined GitHub Enterprise cloud and server use.
- Actions hosted runners per minute: Linux 2-core x64 $0.006, Linux 2-core arm64 $0.005, Linux 1-core slim $0.002. Usage rounds up to whole minutes. From docs.github.com, Actions runner pricing.

## Not verified or left out
- Compute Savings Plan 3-year and Database Savings Plan rates for DSQL were not collected.
- No AWS blog gives per-operation DPU figures beyond the docs examples above. "Monitor Amazon Aurora DSQL costs with the Aurora DSQL console and CloudWatch alarms" only restates the rates.
- The github.com pricing page shows "for the first 12 months *" next to the $21. The footnote text was not in the fetched HTML, so whether the price changes after 12 months is unverified.
- Regional NAT gateway (a newer product) is listed at $0.045 per hour and per GB. Its per-AZ billing rules were not checked.

## Sources
- AWS Price List bulk API, per service: AmazonEC2, AWSComputeSavingsPlan, AuroraDSQL, AWSBackup, AmazonS3, AWSDataTransfer, AWSELB, AmazonVPC, awskms, AmazonCloudWatch, AmazonRoute53, AWSR53AppRecoveryController, AmazonES, AmazonSES.
- https://aws.amazon.com/rds/aurora/dsql/pricing/
- https://docs.aws.amazon.com/aurora-dsql/latest/userguide/billing-metering.html
- https://aws.amazon.com/s3/pricing/ and https://aws.amazon.com/ec2/pricing/on-demand/
- https://aws.amazon.com/elasticloadbalancing/pricing/
- https://aws.amazon.com/privatelink/pricing/ and https://aws.amazon.com/vpc/pricing/
- https://aws.amazon.com/kms/pricing/
- https://aws.amazon.com/route53/pricing/ and https://aws.amazon.com/application-recovery-controller/pricing/
- https://docs.aws.amazon.com/opensearch-service/latest/developerguide/bp.html and sizing-domains pages
- https://aws.amazon.com/ses/pricing/
- https://github.com/pricing, https://docs.github.com/en/billing/reference/actions-runner-pricing, https://docs.github.com/en/billing/concepts/enterprise-billing/combined-enterprise-use

## Cheaper options (small installs)
Prices are Linux on-demand per hour from the AWS Price List bulk files (AmazonEC2 20261023221355, AWSComputeSavingsPlan 20261023221239, AmazonES 20261020160512). us-east-2 and us-west-2 match except c8g.

| Instance | vCPU | Memory | us-east-2 | us-west-2 |
| --- | --- | --- | --- | --- |
| m8g.medium | 1 | 4 GiB | $0.04488 | $0.04488 |
| m8g.large | 2 | 8 GiB | $0.08976 | $0.08976 |
| c8g.medium | 1 | 2 GiB | $0.03974 | $0.03988 |
| c8g.xlarge | 4 | 8 GiB | $0.15896 | $0.15952 |
| r8g.large | 2 | 16 GiB | $0.11782 | $0.11782 |
| t4g.medium | 2 | 4 GiB | $0.0336 | $0.0336 |
| t4g.large | 2 | 8 GiB | $0.0672 | $0.0672 |
| t4g.xlarge | 4 | 16 GiB | $0.1344 | $0.1344 |

Compute Savings Plan effective hourly rates in us-east-2, with the discount against on-demand:

| Instance | On-demand | 1yr No Upfront | 1yr All Upfront | 3yr No Upfront | 3yr All Upfront |
| --- | --- | --- | --- | --- | --- |
| m8g.large | $0.08976 | $0.06594 (26.5%) | $0.06155 (31.4%) | $0.04552 (49.3%) | $0.04131 (54.0%) |
| m8g.xlarge | $0.17952 | $0.13189 (26.5%) | $0.1231 (31.4%) | $0.09104 (49.3%) | $0.08261 (54.0%) |
| t4g.large | $0.0672 | $0.0485 (27.8%) | $0.0452 (32.7%) | $0.0334 (50.3%) | $0.0303 (54.9%) |
| i8g.xlarge | $0.3432 | $0.267 (22.2%) | $0.2492 (27.4%) | $0.1904 (44.5%) | $0.1727 (49.7%) |

- The bulk file lists All Upfront rates per hour. They are the effective hourly rate, so the upfront payment is that rate times the hours in the term.
- T4g burstable. Baseline is 20% per vCPU for t4g.medium, 30% for t4g.large, and 40% for t4g.xlarge (EC2 burstable credit table). T4g launches in Unlimited mode by default. Surplus credits cost $0.04 per vCPU-hour for Linux in both Regions. The charge applies only when the 24-hour average CPU exceeds the baseline.
- OpenSearch on-demand, us-east-2: t3.small.search $0.036 (2 vCPU, 2 GiB), t3.medium.search $0.073 (2 vCPU, 4 GiB), m7g.medium.search $0.068 (1 vCPU, 4 GiB), r7g.medium.search $0.089 (1 vCPU, 8 GiB).
- The OpenSearch best-practices page says "Avoid using T2 or t3.small instances for production domains because they can become unstable under sustained heavy load." The supported-instance-types page says T3 types need a domain without standby and 10 or fewer instances. They also do not support UltraWarm, cold storage, or Auto-Tune. The best-practices, sizing, dedicated-master, and instance-type pages give no production warning for m7g.medium or r7g.medium.
- gp3 includes a baseline of 3,000 IOPS and 125 MiB/s in the storage price (EBS General Purpose volumes page). Only IOPS and throughput above that baseline are billed.
- Not verified. Savings Plan rates for us-west-2 and for the other small types (m8g.medium, c8g, r8g, t4g.medium, t4g.xlarge) were not collected. Neither were OpenSearch Reserved Instance rates.
- Sources: https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/burstable-credits-baseline-concepts.html, https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/burstable-performance-instances-unlimited-mode-concepts.html, https://docs.aws.amazon.com/opensearch-service/latest/developerguide/supported-instance-types.html, https://docs.aws.amazon.com/opensearch-service/latest/developerguide/bp.html, https://docs.aws.amazon.com/ebs/latest/userguide/general-purpose.html
