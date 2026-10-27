Of the ten steps, four still work, so I have 9 findings below: 3 blockers, 5 majors and 1 minor. The four that work are onboarding the BYOC install, the customer revoking the deploy role mid-rollout, a week-long vendor outage, and leaving BYOC to self-host. Each works because stages run in-account, status documents are read over `s3:GetObject`, `Halt`/`Resume` exist, and the vendor-dependency table covers the outage and the exit (DESIGN.md:976-986, 1041-1043) [inferred]. The steps that fail are creating the Paired tables on the first apply, losing a Region for good and re-protecting it, the format and refold rollout, and the Sydney install.

### 1. [blocker] The deploy policy's data-plane deny stops the first apply from creating global tables
- **Step**: onboarding a Paired install, or a Single install with `dr`.
- **Location**: DESIGN.md:57-59, 966, 1062. AWS docs: globaltables-security.html, "Permissions for creating global tables".
- **Gap**: The deploy policy denies "every DynamoDB data-plane read or write on the install's tables" (DESIGN.md:966) [measured]. AWS requires `Query`, `Scan`, `GetItem`, `PutItem`, `UpdateItem`, `DeleteItem` and `BatchWriteItem` on the table in each new replica Region for MREC and MRSC alike, and an explicit deny beats any allow [measured]. The first apply "runs from the deploy tool or the customer's CLI, because no operator exists yet" (DESIGN.md:1062), but line 59 says the operator role creates the tables [measured: the two lines contradict]. If the deploy role applies, the MRSC trio and the MREC `dr` replica fail [inferred]. If the deploy role instead launches the one-shot deployer, it must pass the deployer role. That role's boundary carries only delete denies, so the read deny becomes hollow [inferred].
- **Suggestion**: Have the first apply create only the control buckets, the keys and a deployer launcher that the customer authorizes once. Every table then comes from `apply:infra`. State which role holds the data-plane permissions that replica creation needs.

### 2. [blocker] Nothing says the install's signing keys are multi-Region, so losing their Region freezes every intent
- **Step**: losing a Region for good; `dr promote`; status after the loss; mTLS certificates.
- **Location**: DESIGN.md:920, 944, 1030, 1066, 1070. AWS docs: multi-region-keys-overview.html.
- **Gap**: The config key and the customer-only key are "two KMS asymmetric keys that the bootstrap creates" (DESIGN.md:920), with no Region given. Only the CMK is called multi-Region (DESIGN.md:944, 1070) [measured]. The status-signing key and the key wrapping the CA are likewise unstated (DESIGN.md:1030, 1066) [measured]. AWS lets you create asymmetric multi-Region signing keys, but "you cannot convert an existing single-Region key" [measured]. If the keys sit only in the bootstrap Region and that Region dies, consequences follow:
  - The customer cannot sign `DrPromote`, which is the one moment it is needed.
  - The peer Region's operator cannot sign status documents or issue mTLS certificates.
  - No `Halt`, rollout or re-protection intent can ever be signed again.
  These consequences are [inferred].
- **Suggestion**: Have the bootstrap create all three keys as multi-Region keys, with replicas in every install Region and in `dr`. Have the preflight check this. Drop the [unverified] tag at DESIGN.md:432: AWS states that each multi-Region key works on its own [measured].

### 3. [blocker] No procedure re-protects a Paired install after it loses a Region for good
- **Step**: losing one Region of a Paired install permanently, then re-protecting it.
- **Location**: DESIGN.md:11, 105, 962, 999, 1062, 1070, 1197, 912, 1359-1384.
- **Gap**: The design names the gap itself: "re-protection is a planned copy into a new table set, as for managed `global` (open question)" (DESIGN.md:1070). The open-questions list does not include it, and the Milestone 5 gate re-protects only managed `c1` (DESIGN.md:1197) [measured]. Journey 5 re-protects managed cells by slot moves (DESIGN.md:105), and an install has none (DESIGN.md:962). So no copy-while-serving mechanism exists. The options are a seal-and-handover inside one cell that is not designed, or a write freeze sized by the table [inferred].
  - **Customer key needed.** New table names, bucket names and Regions need customer-only-key config (DESIGN.md:920), so the vendor SRE cannot re-protect on its own [measured].
  - **Every-Region steps block.** Intents go to "every install Region" (DESIGN.md:999). `publish:image` copies into every Region, and `formats:retire` needs "every launch template at N" (DESIGN.md:1062). Each of these fails or stalls on the dead Region, and the OpenTofu state still holds the dead Region's resources. That contradicts "rollouts resume within 1 hour" [inferred].
  - **Worse than Single while degraded.** "Paired cells have no stale mode" (DESIGN.md:11). With one Region gone, an ordinary outage in the witness Region stops writes, and fresh reads stop with them. A Single install would at least keep serving stale reads [inferred].
- **Suggestion**:
  - Add a `RepairPair` intent signed with the customer-only key. It would create a new MRSC trio, copy each chain with a per-chain `Sealed` handover between table sets, swap Regions in the config, and remove the dead Region from the target and the OpenTofu state.
  - Enable stale mode while a Paired install runs on one replica plus the witness.
  - Add re-protection to the quarterly drill (DESIGN.md:912) and to a milestone gate.

### 4. [major] Denying "deletion protection changes" means denying `UpdateTable`, which breaks uninstall, restore and infra changes
- **Step**: bootstrap deploy policy; uninstall; restore cutover; `apply:infra`.
- **Location**: DESIGN.md:966, 1068, 1076. AWS Service Authorization Reference, `list_dynamodb.html`.
- **Gap**: DynamoDB has no IAM condition key for deletion protection. The keys are `Attributes`, `EnclosingOperation`, `FullTableScan`, `LeadingKeys`, `PartiQLSelect`, `ReturnConsumedCapacity`, `ReturnValues` and `Select` [measured]. So the deny can only be a deny on `dynamodb:UpdateTable`, and it sits in the operator's and the deployer's boundary too (DESIGN.md:966) [measured]. That blocks three things [inferred]:
  - the `Uninstall` stage, which "disables deletion protection" (DESIGN.md:1076);
  - the restore cutover, which re-enables Streams and deletion protection (DESIGN.md:1068);
  - any `apply:infra` change to warm or maximum throughput, or replica updates, which need `UpdateTable` on the source table [measured].
- **Suggestion**: Put only `DeleteTable`, `DeleteTableReplica` and `DeleteGlobalTableWitness` in the boundaries. Move unprotecting into `tributary uninstall --finalize`, run with the customer's admin role.

### 5. [major] The deny list misses indirect destruction and indirect reads
- **Step**: deploy policy for DynamoDB, KMS, S3 and EC2.
- **Location**: DESIGN.md:966, 350. AWS docs: globaltables-security.html, "Replication service-linked role" and "How global tables use AWS KMS".
- **Gap**: Only `kms:DisableKey` and deletes are denied (DESIGN.md:966) [measured]. The holes:
  - **KMS access.** If DynamoDB loses access to an MRSC replica's customer-managed key, the replica errors and after 7 days is "archived or become[s] permanently inaccessible" [measured, AWS]. `kms:PutKeyPolicy` and `kms:RevokeGrant` are not denied [measured].
  - **Table resource policy.** A table resource policy that denies the replication service-linked role does the same after 7 days [measured, AWS]. `dynamodb:PutResourcePolicy` is not denied [measured].
  - **S3 bucket settings.** An S3 lifecycle expiration rule, a versioning suspension, or a `PutBucketPolicy` that drops the `If-None-Match` deny (DESIGN.md:350) all destroy or unprotect data without any delete call [inferred].
  - **Indirect reads.** PassRole to EC2 and Auto Scaling for `trib-` roles, plus `RunInstances` or launch-template user data, runs code as a node role that holds data access. The `ssm:*` denies do not cover this [inferred].
- **Suggestion**: Deny these actions on the install's keys, tables and buckets, and keep launch templates with the deployer role only. Alternatively, reword the promise as hygiene, since the vendor's release key already runs vendor code in the account.

### 6. [major] Format enablement and the `ReleasePointer` have no stated signer
- **Step**: a rollout in waves that adds an entry format.
- **Location**: DESIGN.md:920, 922, 1024-1029, 1041, 1062.
- **Gap**: The design says three incompatible things [measured]:
  - "`formats:enable` signs a `formats` config" (DESIGN.md:1062).
  - "The operator role signs no intent or config" (DESIGN.md:920, 1041).
  - The `formats` list is "signed only in the release" (DESIGN.md:920).
  The `ReleasePointer` (`min_release`, `restore_epoch`, `promotion_epoch`) is written to control buckets with no signature. Nodes obey it, and a higher `promotion_epoch` "stops writing at once" (DESIGN.md:1024-1029) [measured]. Nothing limits the deploy role's writes to `intents/` and `releases/` [measured absence]. One object written by the deploy role could therefore halt all writes or rewind the release [inferred].
- **Suggestion**: Give the operator a pointer-signing key that cannot sign intents, and have nodes verify the pointer and the `formats` state with it. Restrict the deploy role's bucket policy to `intents/*` and `releases/*`.

### 7. [major] The core-tier refold rehearsal sits in the vendor-run preflight, which cannot do it
- **Step**: a fold-schema refold in an install.
- **Location**: DESIGN.md:927, 964, 1283. AWS docs: backuprestore_IAM.html.
- **Gap**: The preflight "rehearses the refold on a PITR copy" (DESIGN.md:964). A PITR restore needs "read and write permissions on the target table" [measured, AWS], and reading `log` is exactly what the deploy role is denied. A large restore also outlives the one-hour session [inferred].
- **Suggestion**: Make the rehearsal an operator `Drill` stage that reports to the status document, and have the rollout gate on that result.

### 8. [major] The Sydney customer gets Single, and the vendor SRE cannot fail it over
- **Step**: onboarding a customer in ap-southeast-2, which is outside the 15 MRSC Regions.
- **Location**: DESIGN.md:69, 354, 1003, 1072, 1333, 1383; `research/dynamodb.md#25`.
- **Gap**: The Paired default fails preflight, as it should [measured]. The fallbacks are poor:
  - A Paired trio through Singapore and Tokyo takes the data out of Australia [inferred].
  - Single with a `dr` replica (Melbourne is my guess for the second Region) has nonzero RPO for packs, and its recovery launches capacity, so it misses fact 9 (DESIGN.md:354, 1333) [measured].
  - `DrPromote` is signed by the customer-only key alone (DESIGN.md:1003), yet the 2am runbook has "the operator" promote (DESIGN.md:1072) [measured].
- **Suggestion**: State the offer and SLO for BYOC Regions outside the MRSC list. Either let the customer pre-authorize the config key for `DrPromote`, or make a customer on-call holder of the customer-only key a condition of onboarding.

### 9. [minor] The Streams interface endpoint must be configured by URL; private DNS does not work
- **Step**: bootstrap and preflight of the Streams endpoint.
- **Location**: DESIGN.md:664, 964. AWS docs: privatelink-streams.html.
- **Gap**: AWS says PrivateLink for Streams does not support private DNS, and clients must use the `vpce-…streams.dynamodb.<region>.vpce.amazonaws.com` URL [measured]. Each Region's endpoint URL therefore has to be config that nodes load, not only something the preflight checks [inferred].
- **Suggestion**: Add the endpoint URL per Region to the config the customer key covers. Have the preflight call `DescribeStream` through that URL.