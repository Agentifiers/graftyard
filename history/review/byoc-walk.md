I found 12 gaps: 2 blockers and 10 majors. The design's BYOC story depends on two things the existing machinery can't do as specified. Paired rollouts take longer than a one-hour chained session, and the observer role can't read any Tributary route. Customer 1's Region trio passes both region checks. Taking DESIGN.md:291 and `review/round3-judge.md:69-77` as given, us-east-2/us-west-2 with a us-east-1 witness is inside one DSQL Region set and inside MRSC's list (measured from those texts). I also read `deployments/infrastructure/bootstrap/customer/main.tf` and `variables.tf` to check the bootstrap claims, and AWS's IAM roles page (https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles.html) for the role-chaining cap.

### 1. [blocker] Paired rollouts take longer than the one-hour chained session, and nothing resumes them
**Step**: rolling a release in waves; node image change; vendor-to-customer role chaining on long rollouts.
**Location**: DESIGN.md:58, 1002, 1014, 1040, 1226; `research/byoc-grounding.md#4`, `#21`, `#35`, `#36`, `#48`; `deployments/infrastructure/bootstrap/customer/main.tf:104-117`
**Gap**:
- **The cap applies (measured).** The deploy role sets `max_session_duration = 43200` (main.tf:107). Its trust policy names a single principal, and that principal is the vendor's role, so assuming the customer role is role chaining. AWS's IAM roles page says chaining caps the session at one hour "regardless of the maximum session duration configured for individual roles".
- **The design undercounts the work.** DESIGN.md:1226 calls the cap an open question and says a rollout of "three AZs fits, but only barely". A Paired install has nodes in both cell Regions (1040), so it has six AZ steps. Each step can include a 10-minute clone drain and is followed by a gate (1002).
- **The result (inferred).** The six AZ steps alone take at least an hour. That is before throttled backfills (1006), OpenTofu applies, image publication, `e2e:public`, and `recordInstallation` (`#27`). Node image changes, which replace instances per AZ (1002), take longer still.
- **No recovery path.** The existing tool never cancels or resumes (`#35`), and a partial deploy leaves a mix of old and new components (`#36`). The design defines no resumption point inside `roll:nodes`.
- **No runner either.** <vendor-fleet-app> "records operator intent only, and nothing deploys" (`#21`). So "the vendor then runs the same `deploy` command from its pipeline" (58) really means an engineer runs a runbook by hand, per customer and per release, under this cap.

**Suggestion**: Run long stages inside the customer account:
- The tool writes a signed, create-only rollout plan to the install's control bucket.
- An in-account controller (the `worker` role, under a lease row) runs the AZ steps, gates, and migrations, and persists a cursor.
- The vendor tool only submits the plan, then polls with fresh short sessions, re-assuming per stage.
- Every stage must be idempotent and resumable.
- Size the Milestone 6 gate (1121) on a two-Region Paired install.

### 2. [blocker] The observer role can't read any Tributary route, and the default gates have nothing to check
**Step**: observing health through the observer role only; gates between <vendor-fleet-app> waves.
**Location**: DESIGN.md:769, 773, 1014, 1041, 1049; `research/byoc-grounding.md#7`, `#17`, `#18`, `#19`, `#31`, `#32`, `#33`; main.tf:27, 279-286
**Gap**:
- **No authorization path.** The observer role may only call `execute-api:Invoke` on `GET /v1/system/observer/*` in `var.aws_region` (main.tf:27, 284-285; measured). Today API Gateway checks that IAM permission (`#18`). Tributary serves the observer routes from `trib-ops` in the Rust binary (769), and DESIGN.md:773 lists no API Gateway. A process behind an NLB cannot verify SigV4 for `execute-api` (inferred). So the new `storage`, `dr`, and `identity` routes (1014) are either unauthenticated or unreachable.
- **One Region only.** The route ARN covers a single Region, so a Paired install's peer Region is never observed. When the home Region fails, observation fails with it.
- **The default gates don't fit Tributary.** The defaults are `record-at-release`, `lambda-code-matches`, `components-healthy`, `no-alarms`, and `liveness` (`#31`). `components` covers only ECS services and Lambda functions (`#19`), and Tributary has neither (inferred from 744-752).
- **The installation record has no store.** It lives in a DynamoDB control table (`#17`), but Regional installs claim "no DynamoDB" (1041, 1049).
- **No proof of rollout.** No route reports per-node binary, image, or git version, so no gate can prove "every node runs N".
- **Every wave becomes manual.** An unverifiable check needs an operator acknowledgement, and automatic advance never passes one (`#32`).
- **Private installs fail the probe.** The health probe is an unauthenticated GET to each declared endpoint (`#33`), which fails when the customer's NLB is private.

**Suggestion**:
- Pick one authorization path. Either deploy an API Gateway with AWS_IAM auth and a VPC link to the NLB in each cell Region, with the observer policy covering every install Region, or switch the observer to `s3:GetObject` on a signed status prefix in each Region's control bucket.
- Add a `nodes` route reporting instance, AZ, Region, binary digest, AMI, git version, and role-process state.
- Store the installation record in `core`.
- In the Tributary contract, replace `lambda-code-matches` and `components-healthy` with a `nodes-at-release` check.
- Allow liveness to be probed through the observer route for private installs.

### 3. [major] The deploy role holds full data-plane and destructive power, and the bootstrap changes the design names are the wrong ones
**Step**: bootstrap grants and the permissions boundary (service-linked roles, PassRole for EC2 and Auto Scaling, KMS customer-managed keys).
**Location**: DESIGN.md:36, 58, 748, 996, 1051, 1053, 1221; `research/byoc-grounding.md#4`-`#6`, `#36`; main.tf:19, 28, 126-208, 136-140; `variables.tf:74-77`
**Gap**:
- **(a) Unbounded power.** `PowerUserAccess` is attached by default (variables.tf:77; measured), and only IAM is fenced (main.tf:126-208). Inferred from the managed policy's `NotAction` on `iam:*`, `organizations:*`, and `account:*`, the vendor can therefore:
  - connect to DSQL as admin (`dsql:DbConnectAdmin`);
  - read pack objects in S3;
  - call `kms:Decrypt` and `kms:ScheduleKeyDeletion` on the customer CMK, unless its key policy excludes the role;
  - delete DSQL clusters, the registry table, and buckets.

  DESIGN.md:58 says only that the customer "grants nothing else", and never states what the vendor can see. Because rollback destroys resources that exist only in the newer config (`#36`), destruction can happen by accident, not only on purpose.
- **(b) The quickstart doesn't run as written.** Name prefixes are hardcoded with no `trib-` (main.tf:19), and there is no `product` variable. Yet the quickstart passes `-var product=tributary` (36).
- **(c) The named PassRole change isn't the one needed.** "PassRole for Auto Scaling" (1053) isn't needed: Auto Scaling acts through its service-linked role, and an instance profile needs PassRole to `ec2.amazonaws.com`, which is already allowed (main.tf:28; inferred). What's actually missing is a PassRole target for `fis.amazonaws.com`, which the drills need (996; inferred). The DSQL CDC feed for `dr-apply` may need one too (1221; guess).
- **(d) Service-linked roles depend on PowerUserAccess.** The inline policy grants only the OpenSearch service-linked role (main.tf:136-140). The DSQL, Auto Scaling, ELB, and DynamoDB replication roles come only from `PowerUserAccess`. A customer who narrows `deploy_managed_policy_names` breaks the first deploy.

**Suggestion**:
- Ship a Tributary bootstrap, or add a `product` variable that selects prefixes and PassRole services.
- Grant the service-linked roles explicitly.
- Replace `PowerUserAccess` with a scoped Tributary deploy policy.
- Add denies the deploy role can't remove: `dsql:DbConnect*`, pack and control `s3:GetObject`, data-CMK `Decrypt`/`ScheduleKeyDeletion`/`DisableKey`, `dsql:DeleteCluster`, `dynamodb:DeleteTable`, and `s3:DeleteBucket`.
- Migrations already run in the `worker` role (748), so the deploy role doesn't need DSQL access for them.
- State the vendor's data-plane visibility in the Deployment modes table.

### 4. [major] Incident response has no channel between a read-only observer and an all-powerful deploy role
**Step**: incident response with no data-plane access.
**Location**: DESIGN.md:51, 676, 918, 964, 1002, 1183, 1028; `research/byoc-grounding.md#7`, `#21`
**Gap**: The design gives an operator these BYOC-relevant actions:
- replay a parked outbox row (676);
- recover a parked replica partition, where "Swap-back restores apply" (964);
- promote a Regional install to its `dr` Region with an ARC routing-control flip plus the mandatory `ref_log` reconcile (918);
- revoke a `kid` (1183);
- run `tributary admin bootstrap` (51);
- trigger a hot swap or swap-back on nodes (1002).

None has a BYOC execution path. The observer is GET-only (`#7`), and <vendor-fleet-app> only records intent (`#21`). The vendor can either act through `PowerUserAccess` (SSM or `DbConnectAdmin`, which is full data-plane access and conflicts with finding 3) or not act at all.

The Regional RTO of about 60 minutes (918) assumes someone flips ARC. An ARC cluster in the customer account also appears in no component row (inferred).

**Suggestion**: Define an `ops` command path:
- Commands are vendor-signed with a separate KMS key and written create-only to the control bucket.
- The `worker` role executes them from an allowlist: `replay-parked`, `unpark`, `swap-back`, `promote-dr`, `revoke-kid`, `rotate-epoch`.
- Each command is audited, visible to the customer, and optionally needs customer approval.
- Name who flips ARC in BYOC, and add the ARC cluster to the component table.

### 5. [major] Paired BYOC requires MRSC for no benefit, so the second customer is rejected unnecessarily; Regional preflight is undefined
**Step**: onboarding a customer whose Regions are outside the MRSC list or lack Aurora DSQL.
**Location**: DESIGN.md:58, 1033, 1041, 1049, 1051, 1188, 1217, 1221, 1228, 47; `review/round3-judge.md:69-77`
**Gap**:
- **The rejection.** A trio such as ca-west-1 with two other Regions from its DSQL set is rejected, because ca-west-1 is "in DSQL's sets but not in MRSC's list" (1051).
- **What MRSC buys an install: nothing (inferred).** An install has one geography (1033, 1049), and writer leases exist to fence a continent cut off from the others, which can't happen inside one geography. Paired `core` is already a multi-Region DSQL cluster with RPO zero, and Regional installs already keep `claim` in `core`'s `authority` schema (1041). That table would give Paired installs the same linearizable claims.
- **What MRSC costs an install.** It adds a third store and a Region-list restriction. Its replica set is fixed, so a Region change needs a `RegistryGeneration` copy (1051), and BYOC has no fleet control plane to run one (1029).
- **Regions without DSQL.** The design answers these: no install (1188, 1217).
- **No Regional preflight.** Only the Paired preflight is described. Nothing checks that the home and `dr` Regions both have DSQL, that DSQL CDC is available (1221, unverified), that `i8g` is offered in three AZs (47), that ClockBound is supported (1228), or that vCPU quotas suffice.

**Suggestion**:
- Keep the registry in `core.authority.claim` for every install, Paired included, and keep MRSC only for managed.
- The Paired preflight then checks only that the trio is inside one DSQL Region set.
- Add a Regional preflight covering the checks above.
- State in Usage what a customer in a DSQL-less Region is told.

### 6. [major] Migration stage order and the contract precondition are unspecified, and partial rollouts make contract unsafe
**Step**: schema migration with expand and contract.
**Location**: DESIGN.md:748, 1002, 1006, 1008, 1012, 1014, 1042; `research/byoc-grounding.md#27`, `#49`
**Gap**:
- **No order.** The stages `migrate:expand`, `migrate:contract`, and `roll:nodes` are named (1014) but not placed in the 27-stage order.
- **No precondition.** Nothing says what `migrate:contract` must verify first. The window "N runs against schema N-1 and N+1" (1006) holds only if no process that reads the dropped shape is left:
  - nodes in either Region;
  - the `dr-apply` group (748);
  - swapped-back AZs (1002).
- **The existing guard doesn't transfer.** The tool's fence is a time fence on the Lambda timeout (`#49`), which says nothing about long-lived processes.
- **Partial rollouts make it worse (inferred).** After an abort, an insufficient-capacity failure, a revocation, or a session expiry, the next deploy can run contract while an N-1 AZ is still serving.
- **No trigger.** The design doesn't say how a deploy stage invokes the runner that lives in `worker` (748).
- **No shadow for BYOC authority changes.** The BYOC authority is a schema inside `core` (1042), so its DDL gets none of the two-week shadow testing that 1008 requires.

**Suggestion**:
- Fix the order: expand → `roll:nodes` across all Regions and the `dr` group → soak → contract, removing only the previous release's deprecated shapes.
- Gate contract on the `nodes` route (finding 2) showing every process at version N or later and no swap-back pending.
- Persist the backfill cursor in a `core` row.
- Have the in-account controller from finding 1 trigger the runner.
- Name a CI replay fixture as the BYOC substitute for the authority shadow.

### 7. [major] Rollback across a contract barrier: the machinery computes a different barrier than the design states, and tool rollback destroys newer resources
**Step**: rolling back across a contract barrier.
**Location**: DESIGN.md:1006, 1027, 1051; `research/byoc-grounding.md#13`, `#25`, `#26`, `#34`, `#36`
**Gap**:
- **Different barrier rule (measured).** Barriers are a git diff today: any added `moved {` block, any change under `packages/db/src/records/`, or any added or changed `*-migration.ts` (`#26`).
- **No field to carry the phase.** Barriers are computed at publish against the newest earlier signed release (`#25`), and record v1 has no phase field (`#13`). "A release declares its phase" (1006) therefore needs a record and compat change the design doesn't specify, and every OpenTofu refactor with a `moved` block becomes a barrier.
- **Pinned customers lose rollback (inferred).** For a customer pinned to N-2 (1027), a hotfix's `comparedWith` link points at the newest release, not theirs. That breaks the chain, and a gap in the chain blocks rollback (`#34`).
- **Rollback destroys resources.** The rollback runs the older OpenTofu config, which destroys resources that exist only in the newer config (`#36`), for example a `dr` cluster, a bucket, or a key. The registry's `prevent_destroy` (1051) makes the rollback plan fail outright.
- **No roll-forward lane.** After a barrier, rolling forward is the only fix, but BYOC releases wait behind managed soak (1027).

**Suggestion**:
- Add `phase` to record v2 and have compat read it.
- Compute `comparedWith` per release line.
- Remove resources only two releases after the code stops using them, as with contract.
- Turn on deletion protection for DSQL, S3, and KMS.
- Define an expedited roll-forward lane with a typed acknowledgement.

### 8. [major] Node image and git version changes have no delivery path into the customer account
**Step**: a node image or git version change.
**Location**: DESIGN.md:47, 54, 749, 1002, 1018, 1040; `research/byoc-grounding.md#46`
**Gap**:
- **The AMI can't be in the bundle (inferred).** The bundle "includes the node image" (1018), but a signed file bundle cannot carry an AMI. Existing practice keeps AMIs outside `release.json`, shares them per account and Region, and re-encrypts `aws/ebs` AMIs (`#46`).
- **Self-healing depends on the vendor.** If the Auto Scaling group launches from a vendor-shared AMI, the promise that the group replaces dead nodes (54) depends on the vendor keeping it shared. Offboarding or losing the vendor account stops replacements (inferred).
- **Key policy gap (inferred from AWS's Auto Scaling docs).** EBS encrypted with a customer CMK needs a key-policy entry for `AWSServiceRoleForAutoScaling`, which the bootstrap doesn't add.
- **Capacity during replacement.** Replacement launches against customer quotas and `i8g` capacity in two Regions. With `perAz: 1` (47), replacing a node empties that AZ and its warm cache, and no surge rule is stated.
- **Two answers for git.** Git is "shipped inside the node image and the release bundle" (749), so it's unclear whether a git change is a hot swap or an instance replacement.
- **Gates can't check the image.** No image id is in the signed record.

**Suggestion**:
- Put per-Region AMI ids and the image digest in the signed record.
- Add a `publish:image` stage that copies the AMI into the customer account in each Region, re-encrypted with the customer CMK.
- Grant the Auto Scaling service-linked role use of the CMK.
- Replace instances with an instance refresh at minimum 100% and maximum 200% healthy, after a quota preflight.
- Ship git in the bundle so it can be hot-swapped, and keep the image to the OS only.

### 9. [major] Certificate and key rotation has no owner in BYOC
**Step**: certificate and key rotation.
**Location**: DESIGN.md:58, 478, 490, 528, 752, 970, 1004, 1025, 1029, 1030, 1093, 1143
**Gap**:
- **No issuer.** `tributary-fleet` "provisions certificates" and is managed-only, and it is "Not deployed" in BYOC (752, 1029). Nothing issues or renews a certificate for a delegated subzone (1025).
- **No path for uploaded certificates.** Nodes read certificates from the control bucket and local disk (970, 1143), and nothing says how a customer upload gets there.
- **Expiry goes unseen.** The expiry check is "alarm 30 days before expiry, and a CI test fails" (1004). Vendor CI never sees a customer certificate, and the alarm has no named recipient.
- **Key epochs never rotate.** They carry `not_after` (478; "the one cell's key epochs", 1030), but there is no rotation trigger, cadence, or overlap rule. When a key expires, every `ghs_` token, every `GITHUB_TOKEN`, and OIDC (1093) stop working.
- **No internal CA.** The front assertion key and mTLS between Regions (490) need one, and none is named.
- **Rotation may wait on the vendor.** If rotation rides the signed config waves (1004), a customer pinned at N-2 or a vendor outage delays it.

**Suggestion**:
- Run an in-install `cert` task: ACME DNS-01 against the delegated Route 53 subzone, renew at 30 days, write to each Region's control bucket.
- Give customers a writable prefix for uploaded certificates, which nodes validate on load.
- Rotate key epochs automatically. Publish a new verifying key at least one token lifetime before signing with it, and retire the old one after the last token's `exp`.
- Send expiry alarms to both the observer and a customer SNS topic.

### 10. [major] The customer revoking the deploy role mid-rollout leaves no safe stopping point and no takeover path
**Step**: the customer revokes the deploy role in the middle of a rollout.
**Location**: DESIGN.md:58, 1002, 1012, 1014; `research/byoc-grounding.md#17`, `#23`, `#30`, `#33`, `#35`, `#36`
**Gap**: The design doesn't cover this case. The automatic abort and swap-back live in the tool's gates (1002). If credentials are revoked mid-AZ, the likely end state is (inferred):
- new processes serving ungated, and the kept binary never swapped back;
- an OpenTofu lock left in the state bucket;
- an expand or backfill half done;
- an instance refresh still running on the AWS side;
- no installation record, which is written only after verification (`#17`), so <vendor-fleet-app> shows the install as `unknown` (`#33`).

The customer has no documented way to finish or revert. The self-hosted path (`#23`) needs registry access and the deployer image, and the design doesn't promise either to BYOC customers.

**Suggestion**:
- Define safe points after each AZ gate.
- Add a node-local watchdog: a new process that hasn't received "gate passed" within T swaps itself back.
- Make the in-account controller (finding 1) own in-flight state, so a revocation stops only new submissions.
- Document customer takeover: re-point `deploy_principal_arn`, use registry read access granted at onboarding, then run `deploy --resume`.

### 11. [major] Losing a Paired install's home Region strands the deploy and observe machinery
**Step**: operating after Region loss in a Paired install.
**Location**: DESIGN.md:7, 291, 1029, 1037, 1041, 1051, 1191; `research/byoc-grounding.md#8`, `#17`, `#27`; `variables.tf:11-12`; main.tf:27, 53-55
**Gap**: Serving survives on us-west-2 plus the us-east-1 witness. Everything else sits in the single `aws.region` (`#8`):
- the state bucket ("the state bucket lives here", variables.tf:11-12);
- the observer ARN (main.tf:27);
- the installation record (`#17`);
- the ECR copies (`#27`).

If that Region is lost for good, the vendor can neither deploy nor observe, including the deploy needed to restore redundancy. Restoring redundancy needs a new peer, and MRSC can't add a replica, so it needs a `RegistryGeneration` copy (291, 1051) with no BYOC control plane to run it (1029). No BYOC procedure or RTO for re-pairing is given.

**Suggestion**:
- Replicate state to the peer Region and allow a `stateRegion` failover in the target.
- Run an observer in each cell Region and keep the record in `core`.
- Add a `repair:pair` stage. Whether a DSQL cluster can be re-paired is unverified; ask AWS.
- Finding 5 removes the MRSC regeneration step entirely.

### 12. [major] A week-long vendor outage, and leaving BYOC, expose vendor dependencies the design never lists
**Step**: the vendor control plane unreachable for a week; the customer leaving BYOC to self-host.
**Location**: DESIGN.md:58, 442, 751, 996, 1004, 1047, 1083, 1167; `research/byoc-grounding.md#14`, `#15`, `#46`, `#47`
**Gap**: "The install keeps serving with the vendor unreachable" (58) is asserted but not traced. These dependencies are unaccounted for:
- **(a) Actions mirror.** The vendor syncs the `actions` mirror org and the signed pin map (1083), with no BYOC channel defined. A push into the customer's namespace is a data-plane write. During an outage new action tags fail to resolve, and after leaving BYOC they stop for good.
- **(b) Alerts.** Alarms reach only the vendor's observer, and the customer has no alert route.
- **(c) Signing.** The design doesn't say who signs the config bundles and the stale-fetch limit (442, 1004). If the vendor does, the customer can't change config during an outage or after leaving.
- **(d) Drills.** "A failed practice run or drill blocks deploys" (996), but nobody runs drills in BYOC. Either deploys always block, or the stale-fetch fallback is never exercised (1167).
- **(e) Leaving BYOC.** These are all vendor-held:
  - shared AMIs (`#46`);
  - registry access and retention (`#14`, `#15`), with a prod registry that doesn't exist yet (`#47`);
  - the optional vendor runner account (751, 1047), which has no self-hosted equivalent;
  - <vendor-fleet-app> retirement and observer removal.

**Suggestion**: Add a vendor-dependency row to the Deployment modes table, stating each item's outage behavior and offboarding step. Then:
- Run mirror sync in-install against a cached pin-map key.
- Offer a customer-owned config signing key.
- Add a customer SNS topic for alarms.
- Run drills from the in-account controller and report the results through the observer.
- Write an offboarding runbook: copy AMIs, grant the customer's CI registry read access, hand over mirror sync, remove the observer.