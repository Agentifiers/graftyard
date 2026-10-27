I recommend Candidate A as the base. It is the only one of the three that makes identity writes worldwide while keeping the baseline's watermark machinery, and it needs three concrete fixes before it is safe. I made no repo edits. I read the AWS pages listed under "Fact checks" on 2026-10-24. Line numbers refer to each candidate's `DESIGN.md` and to the baseline `DESIGN.md`.

## Scores (1 to 5)

| Criterion | A | B | Baseline |
| --- | --- | --- | --- |
| 1. Worldwide identity writes | 4 | 4 | 2 |
| 2. Guarantees kept | 4 | 3 | 5 |
| 3. Correct use of DynamoDB | 4 | 5 | 3 |
| 4. Residency honesty | 5 | 4 | 2 |
| 5. Simplicity, including BYOC and self-hosted | 2 | 1 | 5 |
| 6. Feasibility | 3 | 4 | 4 |
| Total | 22 | 21 | 21 |

Evidence (measured from the text unless marked):
- **A1.** Token, membership and grant writes stay inside the home continent, and claims pay one round trip (A 23, 37, 321). EU-homed roots return 503 for hours after a European loss (A 69-70). An NA-homed org's membership changes stop during an NA outage, whoever makes them.
- **B1.** Every write survives any continent's loss (B 37). Every write also pays a round trip to another continent for each MRSC step, plus a 0.5 to 1.5 s coverage wait, North America included (B 258).
- **Base1.** "Identity writes ... need `global`", which lives in North America only (baseline 11, 1085).
- **A2.** `Lookup::Absent` still rests on commit-ordered DSQL logs (A 218). It adds `Blind` and `Quiet` (A 195-198). An unknown login now depends on three logs, two of them across oceans (A 322). There is an unstated gap in `Quiet`, covered under A's flaws.
- **B2.** B states its weakening: "no" holds only against acknowledged writes, for up to 6 s (B 148, 259). It also adds a same-Region S3 read on every projection miss (B 127-129). That breaks the baseline rule that `route` is "Pure... no network I/O" (baseline 251).
- **Base2.** The baseline is the reference definition (baseline 285, 467).
- **A3.** MRSC is used only for single-item claims and leases. MREC is used only for rows with one writer each. A refuses to build watermarks on streams, with citations (A 10-12, 182).
- **B3.** Each property B relies on has a citation: no transactions, so one fence item per change; the two-readers-per-shard limit, so relays; 24 h retention; identical MRSC stream order; MREC for presence rows only (B 9-12, 154, 156).
- **Base3.** It uses no DynamoDB. Its rejection reason, "nothing requires surviving a continent loss", is now out of date (baseline 1110).
- **A4.** It gives exact `Open` and `Resident` lists with reasons, names the escrow of EU secrets to North America as a cost, and flags GDPR (A 255-259, 347).
- **B4.** It is honest, but every public account's password and 2FA secrets sit in three continents (B 199).
- **Base4.** It states that identity facts are not resident and leaves acceptability as an open question (baseline 1085, 1132).
- **A5.** Managed gains 3 DSQL authorities, the MRSC registry, an MREC board in about 12 Regions, leases, escrow, `reconstitute`, `Handoff` sagas and resident partitions. The authority collapses to a `core` schema, but every install gains two DynamoDB tables and a dr claim audit (A 268-276).
- **B5.** DynamoDB replaces the `global` schema in every mode. Every install also runs relays, feed and board buckets, compactors, ClockBound and two lease kinds, even on a self-hosted evaluation node (B 205-219).
- **Base5.** The `global` store is a schema in `core` for BYOC and self-hosted (baseline 951, 967).
- **A6.** Milestone 0 extends the existing gate to three Region sets plus an MRSC FIS test (A 308). It states no duration or pass and flip numbers, and cost appears only as an open question (A 346).
- **B6.** A two-day authority gate with numeric pass conditions and an explicit flip back to the baseline plus a claim registry (B 246).
- **Base6.** A one-day change-log gate with a named flip (baseline 13, 1019).

## A (geo-homed authorities under an MRSC claim registry)

Strongest ideas:
- **(+) "The registry commits, the authorities publish"** (A 218). A claim is one MRSC conditional put, but it becomes visible only through the home DSQL log. The freshness proof therefore never touches a DynamoDB stream. This is the cleanest use of MRSC in either candidate.
- **(+) Writer leases on MRSC fence off a cut-off continent automatically** (A 221). Geography-tagged tokens keep "no such token" dependent on one log (A 172-177, 322).

Most serious flaws:
- **(−) `Quiet` rests on a client-side deadline, not a server fence.** The bound is "lease expired more than run_tx's 30 s deadline" (A 194, 221). `run_tx` only "refuses to open with under 2 s of writer lease left" (A 143). DSQL allows transactions of up to 5 minutes (`research/uptime.md#27`). A paused EU writer could therefore commit after survivors have declared EU `Quiet` and answered "no". After a cut heals, that is a wrong negative answer. [inference]
- **(−) New worldwide couplings and machinery.**
  - Every identity write on Earth depends on the registry's quorum through leases. "A lease bug stops identity writes" everywhere (A 232, 323).
  - Every follower tails three logs, two across oceans (A 222).
  - Five new stateful components produce 24 failure rows. The board's own estimate is "tens of thousands of dollars per month" (A 346).

AWS facts A gets wrong:
- "replicas cannot be added or removed later" (A 10) is imprecise. AWS forbids adding a replica or deleting a single replica, but allows deleting two to convert the table to single-Region. B 9 has the same wording. (measured)
- The BYOC claim that "the boundary forbids IAM writes", so a new grant is needed (A 278), is probably wrong. The deploy role's default `PowerUserAccess` explicitly allows `iam:CreateServiceLinkedRole`, and the workload boundary applies to the workload roles that deploy creates (`research/byoc-grounding.md#4`-`#6`). [inference about boundary scope]
- The role name A marks unverified is `AWSServiceRoleForDynamoDBReplication`. (measured)

## B (whole global tier in DynamoDB global tables)

Strongest ideas:
- **(+) The proof moves to the writer.** `write` acknowledges only after every holder of a deny lease has covered the change or lost its lease. `NoProof` is private (B 14, 124-129, 148). This replaces the missing commit order with a protocol the type system enforces.
- **(+) A fence item per change.** It gives multi-item changes one commit point and one stream record (B 68-71, 147). The `org#O` fence also closes an admin write-skew gap that the baseline could not prevent (B 147). The closed `Key` list and the `PresenceRow`-only MREC rule make the choice of store a property of the type (B 54-58, 154).

Most serious flaws:
- **(−) Every identity write is slow and globally coupled.** Each MRSC step waits on another continent, and the coverage wait adds up to 6 s (B 258). "One stuck lease holder can delay every identity write" (B 260). An org fence caps changes at "a few per second per org" (B 262), which a large SCIM sync will hit.
- **(−) It does not collapse for installs, and it reads S3 on misses.** DynamoDB plus relays, buckets, compactors and ClockBound run in every mode (B 205-219). The edge reads its publisher's head in S3 on every projection miss (B 29, 35, 127-129), so scanner traffic reaches a store, which is the pattern B itself cites as `#47` when rejecting other designs.

AWS facts: every DynamoDB claim I checked is correct. "A witness has no stream" (B 212) is [inference]. B's "no new grant is needed" (B 219) is correct for `PowerUserAccess`.

## Fact checks (read on 2026-10-24)

- **MRSC writes with one other Region reachable: confirmed.** "The local Region accepts all read and write operations as long as there is a second Region that hosts a replica or witness to establish quorum. If a second Region isn't available, the local Region can only service eventually consistent reads." A Region that returns rejects writes and strong reads until it catches up ([best-practices page](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/bp-global-table-design.html)).
  - The same page still says "MRSC global tables can't span Region sets" and lists 3+4+3 Regions. That is stale.
  - The [HowItWorks page](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/V2globaltables_HowItWorks.html) lists 15 Regions "including Regions on different continents". The [What's New post of 2026-09-23](https://aws.amazon.com/about-aws/whats-new/2026/09/dynamodb-mrsc-additional-regions/) adds Canada (Central), Stockholm, Spain, Mumbai and Singapore.
  - There is no Sydney, Calgary or Hong Kong, so A 278 and B 9 are right.
- **MRSC streams: confirmed.** They are off by default, can be enabled per replica, and "Streams records on MRSC replicas are identical for every replica, including Stream record ordering". The stream definition is never synchronized between replicas (HowItWorks).
  - The [Streams page](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Streams.html) orders records per item only, keeps them 24 h, and warns that more than two readers per shard can be throttled.
  - [`ApproximateCreationDateTime`](https://docs.aws.amazon.com/amazondynamodb/latest/APIReference/API_streams_StreamRecord.html) is "rounded down to the closest second".
  - Neither candidate claims a cross-item watermark, which is correct.
- **MREC conflict rules: confirmed.** Per-item "last writer wins" on an internal timestamp. Conditional writes are evaluated against the Region's own copy. MREC transactions are atomic only in the Region that ran them, so other replicas can see partial writes (HowItWorks).
- **MRSC limits: confirmed.** Exactly three Regions. No transaction APIs. No TTL and no LSIs. `ReplicatedWriteConflictException` can be retried. The table must start empty (HowItWorks).
- **DSQL Region sets: confirmed as A 9 states.** "Aurora DSQL currently doesn't support cross-continent multi-Region clusters" ([DSQL guide](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/what-is-aurora-dsql.html)).
- **BYOC service-linked role:** [`PowerUserAccess`](https://docs.aws.amazon.com/aws-managed-policy/latest/reference/PowerUserAccess.html) allows `iam:CreateServiceLinkedRole`. The role is [`AWSServiceRoleForDynamoDBReplication`](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/globaltables-security.html).

## Base: A

A keeps the baseline's invariants: the `GlobalStore` contract scoped per geography, the version-newer rule for appliers, `Lookup::Behind` on watermarks, `revoke` coverage, the 30 s fail-closed rule, and `NameEpoch`. A future maintainer extends familiar local rules. B asks them to maintain a novel lease, relay and ClockBound protocol that is woven into every write and every mode, and it weakens criterion 2. The baseline cannot meet criterion 1, whatever is added to it. [inference]

## Grafts into A

1. **Required fix for `Quiet`:** set the bound to lease expiry plus DSQL's 5-minute server-side transaction limit plus the clock margin, not the 30 s client deadline. Alternatively, have the writer abort when its lease expires before COMMIT. Either way, cite `research/uptime.md#27`.
2. **From B, the `org#O` fence:** every org-scoped `auth-*` transaction bumps the org row's version, so DSQL's write-write check serializes an admin's removal against that admin's concurrent grant (B 147).
3. **From B, ClockBound** for lease expiry and for `CLOCK_MARGIN` (B 12, 121), replacing the assumed 50 ms margin.
4. **From B, the shape of the authority gate:** numeric pass and flip conditions, a checker for missing acknowledged changes, and FIS isolation of each registry Region in turn (B 246).
5. **From B, the target preflight** that rejects Paired Regions outside the MRSC 15 (B 221). Drop A's extra IAM bootstrap grant (A 278), because `PowerUserAccess` already covers it.

## Drops from A

- **Drop the MREC freshness board** (A 182-198, 270). Keep the baseline's S3 board (baseline 222-225). That removes a DynamoDB store from every install and an estimated cost of tens of thousands of dollars a month. Each member then reads a few more S3 objects across oceans each second. [guess on cost parity]
- **Drop the MREC dr replica** of the Regional BYOC registry and the claim audit that depends on it (A 269, 276). In single-Region installs, put the registry as a table in the `core` `authority` schema behind the same `Registry` trait. BYOC and self-hosted then collapse to the baseline's shape. MRSC stays for managed and for Paired BYOC.
- **Defer** rehome and cross-geography transfer sagas (`Owned::Handoff`, A 157-160) to a later milestone. Keep only the subset that `reconstitute` needs.

## Flip condition

- **The unknown:** can one DSQL multi-Region authority serve about 130 followers on other continents, each polling every 100 ms (about 1,300 queries per second), within the 10,000-connection and 100-new-connections-per-second quotas? Its log must stay gap-free in commit order, cross-ocean p99 apply lag must stay at or under 1.5 s, and a usable snapshot time must be exposed to serve as the watermark (A 345, baseline 642-643).
  - If it cannot, A's three-log freshness fails and B becomes the base. B's freshness needs no commit-ordered log.
- **Cheapest test, about 1 to 2 days on a scratch account:**
  1. Run the baseline's change-log gate (baseline 1019) on an eu-central-1 and eu-west-1 cluster with an eu-west-3 witness.
  2. Use 200 writers on 64 head rows.
  3. Add 65 followers each in us-east-2 and ap-northeast-1, polling every 100 ms.
  4. Assert no `seq` gaps and that `at` never decreases.
  5. Report p99 apply lag, the `40001` retry rate and connection errors.
  6. Ask AWS in parallel whether a DSQL read exposes its snapshot time.