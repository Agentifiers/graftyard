# Round 3 synthesis: a worldwide global tier

## Base

Candidate 1, geo-homed DSQL authorities under a worldwide MRSC claim registry. The lead scored 25 against 21, with the North American baseline near 22. The cross-judge scored 22, 21, and 21 (`review/round3-judge.md`). Both picked candidate 1 because it is the only shape that makes identity writes worldwide and keeps the commit-ordered DSQL logs that `Lookup::Behind`, `revoke` coverage, and the 30 s fail-closed rule rest on. Candidate 2 survives a continent loss better, but it weakens "no" to acknowledged writes, makes every identity write wait on another continent plus a coverage wait, and runs a novel lease and relay protocol in every install.

## Where DynamoDB global tables help, and where they do not

| Use | Verdict | Reason |
| --- | --- | --- |
| Worldwide uniqueness claims (logins, shortcodes, app slugs, SSH fingerprints, email digests) | MRSC, three Regions on three continents | One conditional write per item is the commit point, and MRSC survives a continent's loss with RPO zero. |
| Writer leases that fence a cut-off continent | MRSC, same table | A lease renewal needs two of three continents, so a minority continent stops committing on its own. |
| Accounts, credentials, memberships, grants, installations | Not DynamoDB | MRSC has no transactions and no stream gives a cross-item commit order. MREC is last-writer-wins and can drop a removal that races a write. These stay in per-continent DSQL authorities. |
| The freshness board | Not DynamoDB | One writer per row would make MREC safe, but S3 boards read across Regions do the job without adding a DynamoDB store to every install. |
| Single-Region BYOC and self-hosted installs | Not DynamoDB | The registry is a table in the `core` `authority` schema behind the same `Registry` trait. |

## Grafts into the base

| Graft | Source |
| --- | --- |
| `Quiet` waits for lease expiry plus DSQL's 5-minute server-side transaction limit plus the clock margin, and a writer aborts any transaction whose lease expires before COMMIT | cross-judge required fix |
| Every org-scoped authority transaction bumps the org row's version, so DSQL serializes an admin's removal against a concurrent grant | candidate 2 |
| ClockBound bounds for lease expiry and `CLOCK_MARGIN` | candidate 2 |
| Milestone 0's authority gate with numeric pass and flip conditions, a checker for missing acknowledged changes, and AWS FIS isolation of each registry Region in turn | candidate 2 |
| Target preflight that rejects Paired Regions outside the MRSC Region list | candidate 2 |

## Drops from the base

| Dropped | Reason |
| --- | --- |
| The MREC freshness board | The S3 board stays, so installs gain no DynamoDB store for it. |
| The MREC dr replica of a Regional BYOC registry and its claim audit | Single-Region installs keep the registry in `core`, so BYOC and self-hosted collapse to their current shape. |
| Rehome and cross-geography transfer sagas (`Owned::Handoff`) at launch | Deferred. Only the subset `reconstitute` needs ships. |
| An extra IAM grant for the replication service-linked role | The deploy role's `PowerUserAccess` already allows `iam:CreateServiceLinkedRole` for `AWSServiceRoleForDynamoDBReplication` (`review/round3-judge.md`, fact checks). |

## Flip condition

Can one DSQL multi-Region authority serve about 130 followers on other continents polling every 100 ms within its connection quotas, with a gap-free commit-ordered log, cross-ocean p99 apply lag at or under 1.5 s, and a usable snapshot time as the watermark? Milestone 0 tests it. If not, candidate 2 becomes the base, because its freshness needs no commit-ordered log.
