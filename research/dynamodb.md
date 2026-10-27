# DynamoDB facts for a DynamoDB-only Graftyard

Facts only, read on 2026-10-24. Doc pages were fetched through their `.md` form and are cited by their canonical `.html` URL. Scratch copies, the Price List files and the script that appended the `ddb.*` entries to `prices.json` are in `/tmp/graftyard-ddb/`. DG means `https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/`.

## 1. Transactions

1. TransactWriteItems "groups up to 100 write actions in a single all-or-nothing operation. These actions can target up to 100 distinct items in one or more DynamoDB tables within the same AWS account and in the same Region. The aggregate size of the items in the transaction cannot exceed 4 MB." Source: DG/transaction-apis.html [doc]
2. TransactGetItems "groups up to 100 `Get` actions together ... The aggregate size of the items in the transaction can't exceed 4 MB." Source: same. [doc]
3. Cost is 2x. "DynamoDB performs two underlying reads or writes of every item in the transaction: one to prepare the transaction and one to commit the transaction." A canceled transaction "still consumes the underlying read or write capacity". Source: same. Units: "Transactional write requests require two write request units to perform one write per second for items up to 1 KB." Source: DG/Constraints.html [doc]
4. Isolation is serializable between a transaction and GetItem, PutItem, UpdateItem, DeleteItem and other transactions. It is read-committed against Query, Scan and BatchGetItem as a unit, and BatchWriteItem as a unit is "NOT Serializable". Source: DG/transaction-apis.html, isolation table. [doc]
5. MREC: "DynamoDB transaction operations ... are only atomic within the Region where the operation was invoked. Transactional writes are not replicated as a unit across Regions". Source: DG/V2globaltables_HowItWorks.html [doc]
6. MRSC: "Global tables configured for multi-Region strong consistency (MRSC) do not support transaction operations, and will return an error if those operations are invoked on an MRSC replica." Source: same. [doc]

## 2. Items, partitions, indexes, reads

7. "The maximum item size in DynamoDB is 400 KB, which includes both attribute name binary length (UTF-8 length) and attribute value lengths". Source: DG/Constraints.html [doc]
8. "Every partition in a DynamoDB table is designed to deliver a maximum capacity of 3,000 read units per second and 1,000 write units per second." Source: DG/bp-partition-key-design.html. Adaptive capacity can isolate one hot item but caps it at "the partition maximum of 3,000 RCUs and 1,000 WCUs". Source: DG/burst-adaptive-capacity.html [doc]
9. Per-table throughput defaults to 40,000 read and 40,000 write units in both modes, adjustable. Source: DG/ServiceQuotas.html [doc]
10. "You can define up to 5 local secondary indexes per table." and "There is a default quota of 20 global secondary indexes per table." Source: DG/ServiceQuotas.html. The GSI quota is adjustable ("Global Secondary Indexes per table | Each supported Region: 20 | Yes", code L-F7858A77). Source: https://docs.aws.amazon.com/general/latest/gr/ddb.html [doc]. No LSI row appears in that quota table, so I read the LSI limit as fixed. [inferred]
11. "All reads from GSIs and streams are eventually consistent." and "Strongly consistent reads from a global secondary index or a DynamoDB stream are not supported." Source: DG/HowItWorks.ReadConsistency.html [doc]
12. Query has no snapshot, even inside one page. "The isolation level is read-committed between any transactional operation and any read operation that involves multiple standard reads (`BatchGetItem`, `Query`, or `Scan`). If a transactional write updates an item in the middle of a `BatchGetItem`, `Query`, or `Scan` operation, the subsequent part of the read operation returns the newly committed value (with `ConsistentRead`)". Source: DG/transaction-apis.html. Pages are separate requests chained by `LastEvaluatedKey`/`ExclusiveStartKey` with a 1 MB cap. Source: DG/Query.Pagination.html [doc]. I found no read-timestamp or snapshot parameter in these pages. [inferred]

## 3. Streams and TTL

13. Retention: "stores this information in a log for up to 24 hours". Ordering: "For each item that is modified in a DynamoDB table, the stream records appear in the same sequence as the actual modifications to the item", and ordering is per item, "not across an entire partition". Each record "appears exactly once in the stream." Source: DG/Streams.html [doc]
14. Readers: "No more than two processes at most should be reading from the same stream's shard at the same time. Having more than two readers per shard can result in throttling." Source: same. [doc]
15. MRSC Streams are supported but off by default. "You can enable Streams on an MRSC replica. Streams records on MRSC replicas are identical for every replica, including Stream record ordering." MREC replicas always have Streams on and "cannot be disabled", and their records may differ across replicas. Source: DG/V2globaltables_HowItWorks.html [doc]
16. Since 2026-10-15 a consumer can start a shard at a timestamp with the `AT_TIMESTAMP` iterator. "There is no additional charge for this feature beyond standard GetRecords API request charges." Source: https://aws.amazon.com/about-aws/whats-new/2026/10/amazon-dynamodb-streams-timestamp-processing/ [doc]
17. TTL on MRSC: "Global tables configured for multi-Region strong consistency (MRSC) do not support configuring Time To Live (TTL) deletion." Source: DG/V2globaltables_HowItWorks.html [doc]
18. Other MRSC limits from the same page. "Local secondary indexes (LSIs) are not supported for MRSC global tables." "A MRSC global table must be deployed in exactly three Regions." It can be created only from an empty table, and "You cannot add additional replicas to an existing MRSC global table." Concurrent cross-Region writes to one item fail with `ReplicatedWriteConflictException`. [doc]

## 4. Backups

19. PITR provides "up to 35 days of recovery points at a per second granularity". Source: DG/Point-in-time-recovery.html. The period is configurable: "You can set the recovery period for continuous backups to be any number between 1 and 35 days." Restore "always restores to a new table". Source: DG/PointInTimeRecovery_Howitworks.html [doc]
20. PITR pricing basis: "DynamoDB charges for PITR based on the size of each DynamoDB table, including table data and local secondary indexes. Changing the recovery window (for example, from 35 days to 1 day) doesn't reduce the price." Source: same. [doc]
21. Price List, us-east-2 and us-west-2 alike: "$0.20 per GB-Month of storage used for DynamoDB PITR backup", "$0.10 per GB-Month of storage used for on-demand backup", "$0.15 per GB of data restored". Source: Price List files in section 7. [doc]

## 5. SLAs

22. DynamoDB (updated May 14, 2025): "at least 99.99% if the Standard SLA applies, or (b) at least 99.999% if the Global Tables SLA applies". The Global Tables SLA applies only "if all of your DynamoDB tables in the applicable AWS region are part of Global Tables ... and you make reasonable attempts to failover". The SLA has no separate MREC or MRSC terms, and availability is measured over 5-minute intervals. Source: https://aws.amazon.com/dynamodb/sla/ [doc]
23. Aurora DSQL (updated May 27, 2025): "at least 99.999% if the Multi-Region SLA applies, or (b) at least 99.99% if the Single-Region SLA applies". Multi-Region counts a 1-minute interval as down only when "all active peered Regions of the Multi-Region Cluster were Unavailable". Source: https://aws.amazon.com/rds/aurora/dsql/sla/ [doc]

## 6. Regions

24. DynamoDB endpoints list 36 Regions. Two are GovCloud, which leaves 34 in the commercial `aws` partition, including me-south-1. China Regions sit in a separate partition and are not counted. Source: https://docs.aws.amazon.com/general/latest/gr/ddb.html, counted by script. [measured]
25. MRSC is in 15 Regions: us-east-1, us-east-2, us-west-2, ca-central-1, eu-west-1, eu-west-2, eu-west-3, eu-central-1, eu-north-1, eu-south-2, ap-northeast-1, ap-northeast-2, ap-northeast-3, ap-south-1, ap-southeast-1. Since 2026-09-23, "You can now create an MRSC global table in any three-Region configuration across the 15 supported AWS Regions ... This includes configurations that span North America, Europe, and Asia Pacific." Sources: https://aws.amazon.com/about-aws/whats-new/2026/09/dynamodb-mrsc-additional-regions/ ; DG/V2globaltables_HowItWorks.html [doc]
26. Aurora DSQL single-Region is in 20 Regions: us-east-1, us-east-2, us-west-2, ca-central-1, ca-west-1, ap-east-1, ap-southeast-4, ap-south-1, ap-northeast-3, ap-northeast-2, ap-southeast-1, ap-southeast-2, ap-northeast-1, eu-central-1, eu-west-1, eu-west-2, eu-west-3, eu-south-2, eu-north-1, sa-east-1. Source: https://docs.aws.amazon.com/aurora-dsql/latest/userguide/what-is-aurora-dsql.html (matches https://docs.aws.amazon.com/general/latest/gr/dsql.html) [doc]
27. DSQL multi-Region works only inside one Region set. North America has us-east-1, us-east-2, us-west-2, ca-central-1 and ca-west-1. Asia Pacific has ap-east-1, ap-south-1, ap-northeast-3, ap-northeast-2, ap-southeast-1 and ap-northeast-1. Europe has eu-central-1, eu-west-1, eu-west-2, eu-west-3, eu-south-2 and eu-north-1. "Aurora DSQL currently doesn't support cross-continent multi-Region clusters." Source: same page. [doc] Sydney, Melbourne and São Paulo appear in no set, so I read them as single-Region only. [inferred]

## 7. Prices (us-east-2 and us-west-2 are identical)

Source: `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonDynamoDB/20260911124422/{us-east-2,us-west-2}/index.json`, published 2026-09-11. Standard table class. Entries are in `prices.json` as `ddb.*`. [doc]

| Item | USD | Price List description |
| --- | --- | --- |
| On-demand write request unit | 0.625 per 1M | "$0.625 per million write request units (Ohio)" |
| On-demand read request unit | 0.125 per 1M | "$0.125 per million read request units (Ohio)" |
| Replicated write request unit (rWRU) | 0.625 per 1M | "$0.625 per million replicated write request units (Ohio)" |
| Provisioned WCU | 0.00065 per hour | "$0.00065 per hour for units of write capacity beyond the free tier" |
| Provisioned RCU | 0.00013 per hour | "$0.00013 per hour for units of read capacity beyond the free tier" |
| Provisioned replicated WCU (rWCU) | 0.00065 per hour | "$0.00065 per hour for units of replicated write capacity beyond the free tier" |
| Standard storage | 0.25 per GB-month | "beyond first 25 free GB-Months" |
| PITR | 0.20 per GB-month | see finding 21 |
| On-demand backup | 0.10 per GB-month | see finding 21 |
| Restore, also charged when adding a replica Region | 0.15 per GB | "$0.15 per GB of data restored" |
| Streams read request unit | 0.20 per 1M | "$0.0000002 per DynamoDB Streams read request unit beyond free tier", after 2.5M free per month |

28. MREC and MRSC use the same write units: "The replicated write units (rWCUs and rWRUs) used to bill for write operations are identical for both multi-Region strong consistency (MRSC) and multi-Region eventual consistency (MREC) modes." A witness incurs no rWRU, storage or replication transfer cost. rWRUs are charged "in every Region containing a replica table", and GSI updates are billed as ordinary WRUs in each replica Region. Source: DG/global-tables-billing.html [doc]
29. Replication transfer is free: "DynamoDB does not charge cross-Region data transfer fees for replicating data between the Regions of a global table." Source: same. Other cross-Region traffic, such as DynamoDB in one Region read by EC2 in another, "is charged on both sides of the transfer". Source: https://aws.amazon.com/dynamodb/pricing/on-demand/ [doc]
30. Streams GetRecords made by Lambda triggers (except on Lambda Managed Instances) and by global table replication are not charged. Source: same pricing page. [doc]

## Not verified

- Whether the GSI quota can go above 20 in practice, or to what ceiling. The quota table says only "Adjustable: Yes". The local AWS CLI binary fails to run (wrong CPU type), so I could not query Service Quotas directly.
- That no What's New post after these doc snapshots adds MRSC transactions or TTL. Two searches found none, and the current DG pages still say both are unsupported.
