### 1. [blocker] `dr` promotion can lose acknowledged writes, and nothing decides when traffic moves
**Step**: DynamoDB in eu-central-2 impaired for 3 hours at 2am, then `dr promote` and failback.
**Location**: DESIGN.md:1070, 1026-1027, 1205, 137, 217, 1074, 1310.
**Gap**:
- **Health check undefined.** Traffic moves "by failing the home Region's Route 53 health check" (1070). Nothing says what that check probes, or how the operator fails it (measured).
  - If it probes the deep endpoint that appends to DynamoDB, the record fails over within a minute of the impairment. That is before any promotion, so clients reach a standby that is not serving and miss stale mode.
  - It also fails back by itself once eu-central-2 heals, which sends clients to the old-epoch home (inferred).
  - If the operator fails it by API call, Route 53's control plane is on the recovery path (inferred).
  - The customer's zone may not be in Route 53. The previous design had a fallback for that; this one doesn't (measured, by grep).
- **The fence is only a poll.** Home nodes stop writing when they "read a higher `promotion_epoch` in any control bucket" (1026-1027). No interval is stated, and no put checks it (137, 217). When DynamoDB recovers at 5am, stale nodes can acknowledge epoch-1 pushes (inferred).
- **Milestone 6 accepts that loss.** Its gate expects "the old home keeps writing through a partition … the fork recorded as stated loss" (1205). Acknowledged writes made after the promotion are not bounded by the stated RPO, which breaks "never lose an acknowledged push" (measured against FRAME.md:44).
- **Late entries vanish.** Epoch-1 entries that replicate after the promotion sort below the `e#2#…` tail key (217). Nothing applies them or adds them to the recorded loss (inferred).

**Suggestion**:
- Make the fence a lease. Home puts require a "no promotion" read from the `dr` control bucket within L seconds, and promotion waits L plus clock error before it takes writes.
- Point the failover health check at a CloudWatch alarm on a metric that only `DrPromote` and `DrFailback` publish, so it is sticky and data-plane only.
- Re-scan the old epoch past each cut after the home Region reconnects, and record what arrives late.
- Change the Milestone 6 gate to "zero acknowledged entries lost after promotion."

### 2. [major] There is no way to add `dr` to a running install, and failback uses steps that are denied or don't replicate
**Step**: The evaluation install grown to three nodes, then adding `dr` (eu-central-1), promotion, and failback.
**Location**: DESIGN.md:36, 944, 1076, 1070, 354, 920, 966.
**Gap**:
- **No procedure.** Nothing says how to set `dr` on an existing install, although the `dr` Region is customer-signable config (920; measured, by grep).
- **Keys.** The CMK is multi-Region only "for Single with `dr`" (944). KMS cannot convert a single-Region key to multi-Region (measured: AWS KMS docs), so every encrypted table, object, and wrapped key would need re-encryption.
- **Old objects never reach `dr`.**
  - S3 live replication copies only new objects (measured: S3 Batch Replication docs). Promotion truncates each chain "at its first missing pack" (354), so almost every chain would be cut at its oldest pack (inferred).
  - Failback has the same problem: objects that are already replicas are not replicated again without Batch Replication (measured: S3 docs).
- **Failback is blocked.** It "rebuilds the old home … from a fresh replica [inference]" (1070). That needs the stale eu-central-2 replica deleted, and every workload boundary denies deleting "DynamoDB tables and their replicas" (966) (measured contradiction).
- **Signing keys.** The Region of the config key and the customer-only key is never stated (920, 966). If they live only in eu-central-2, losing that Region means nobody can sign `DrPromote` (inferred).

**Suggestion**:
- Always create the CMK, the config key, and the customer-only key as multi-Region keys.
- Add an `EnableDr` stage that runs Batch Replication and gates `dr-fresh` on a complete pack inventory.
- Specify `DrFailback` step by step, including a scoped exception for deleting the replica.

### 3. [major] The permissions boundary contradicts Uninstall and Restore, and tables created at runtime have no owner
**Step**: PITR restore and uninstall.
**Location**: DESIGN.md:966, 1068, 1076, 1062, 924.
**Gap**:
- **Denied steps the design requires.**
  - The boundary on the operator denies "deletion protection changes" (966). Yet `Uninstall` "disables deletion protection" (1076), and the restore cutover re-enables it (1068) (measured).
  - Restore leaves five deletion-protected tables that no role may delete (inferred).
  - Each core-schema rehearsal leaves a full PITR copy behind (924) (inferred).
- **Outside OpenTofu.** Restored tables, re-created `dr` replicas, and the Paired copy set are not in OpenTofu state. The next `apply:infra` still manages the old names, and `--finalize` "destroys the install" from state, missing all of them (inferred).
- **Settings a restore drops.** AWS says tags and IAM policies, CloudWatch alarms, Streams, TTL, deletion protection, and PITR must be set again by hand on a restored table (measured: AWS restore docs). The cutover re-sets only Streams, PITR, and deletion protection (1068) (measured).
- **Uninstall leaves data.** It never mentions the 35-day system backups or AWS Backup recovery points (inferred).

**Suggestion**:
- Scope the deny to *disabling* protection on live tables named in signed config.
- Have `Restore` and `--finalize` adopt or import runtime tables into state, and retire superseded tables under a customer-only intent.
- Re-apply tags, alarms, and resource policies at cutover.

### 4. [major] The core-tier refold rehearsal has no runner, and core-tier serving during a refold is unspecified
**Step**: Upgrades N to N+1 and N to N+4 that include a fold-schema change.
**Location**: DESIGN.md:924, 964, 966, 1018-1020, 508, 918.
**Gap**:
- **No runner.** The rehearsal runs "in the preflight" (964), that is, in the deploy tool or customer CI. That principal is denied every DynamoDB data-plane read or write (966). AWS restore needs read and write on the target table (measured: DynamoDB backup IAM docs). No `Stage` variant exists for it (1018-1020) (measured).
- **No serving rule.** "Old file keeps serving until the new one catches up" is stated only for the metadata tier (508, 924). A hot swap reopens "the same SQLite files" (918). So whether git waits on a core refold per chain in a one-cell install is undefined (inferred).

**Suggestion**:
- Add `Stage::RehearseRefold`: the operator restores into a `trib-rehearsal-*` table, gets a deny exception for deleting that prefix, and reports to the status document.
- State that the core tier dual-reads until each chain's refold snapshot is published.

### 5. [major] Paired is impossible in eu-central-2, and a running Single install cannot become Paired
**Step**: Choosing Paired instead.
**Location**: DESIGN.md:56, 943, 962, 964, 1009-1014; research/dynamodb.md#18, #25.
**Gap**:
- **eu-central-2 can't be Paired.** It is not among the 15 MRSC Regions (research#25), so the preflight rejects it. Paired, and also any `dr`, puts the data and witness outside Switzerland. The design never tells a Zurich buyer this (measured; residency impact inferred).
- **No conversion.** MRSC tables can be created only empty (research#18). Planned copies and slot moves exist only in the fleet control plane, which is "Not deployed" in installs (943, 962). No `Command` converts the durability class (1009-1014) (measured).

**Suggestion**:
- State in the quickstart and preflight that Swiss residency means Single without `dr`.
- Add a `Reprotect { to: Paired }` intent that copies and seals inside one cell, plus a Milestone 6 gate for it.

### 6. [major] Air-gapped installs break LFS, caches, artifacts, revocations, and promotion
**Step**: The air-gapped variant (VPC endpoints, including Streams).
**Location**: DESIGN.md:677, 1108, 1110, 1074, 1055-1056, 1070, 920.
**Gap**:
- **Presigned S3 URLs.** LFS, the Actions cache, and artifacts hand clients presigned S3 URLs (677, 1108, 1110). On-prem runners and laptops cannot reach S3 through gateway endpoints, which "do not allow access from on-premises networks" (measured: VPC docs).
- **Cross-Region writes.** Revocation records go to the `dr` control bucket "before they return" (1070). A gateway endpoint "is available only in the Region where you created it" (measured), yet 1070 says no peering is needed. So every revocation hangs (inferred).
- **List not derived.** The "derived" endpoint list (1074) omits AWS Backup and Route 53, which the stage table lists (1055-1056) (measured). Route 53's API has no PrivateLink endpoint (guess).
- **Streams endpoint URL.** Streams PrivateLink does not support private DNS, and clients must use the `vpce-…` URL (measured: AWS docs). That URL is not among the signable settings (920) (measured).

**Suggestion**:
- Proxy LFS, cache, and artifact bytes through the install, or require S3 interface endpoints with private DNS.
- Require inter-Region connectivity or a replicated control bucket when `dr` is set.
- Generate the endpoint list from the stage table, and carry endpoint URLs in signed config.

### 7. [major] The 2am runbook strands a 3-hour impairment, and stale-mode authentication works only per node
**Step**: DynamoDB impaired for 3 hours at 2am (stale mode, authentication, runbook).
**Location**: DESIGN.md:1072, 352, 372-374, 810, 920, 1070.
**Gap**:
- **No `dr`.** The runbook says "keep waiting or restores" (1072).
  - The 2-hour limit ends fetches at 4am. Raising it is allowed (920) but is not a branch of the runbook.
  - An in-Region restore calls the impaired control plane.
  - A cross-Region restore exists in AWS (measured: AWS restore docs), but without `dr` the packs have no copy outside eu-central-2 (inferred).
- **With `dr`.** One hour of waiting plus about 60 minutes of promotion (810) means about 2 hours without writes, not the 60-minute RTO stated (inferred).
- **Per-node token caches.** Expired token entries are kept only in each node's own edge cache (352, 372-374). Behind the NLB, other nodes miss, and a miss cannot reach `global`. A node replaced during the outage serves nothing (inferred).

**Suggestion**:
- Add a runbook branch that raises the stale limit to AWS's projected recovery time.
- Have nodes snapshot positive token entries to S3 every minute for stale mode.
- State the write RTO as the wait plus the promotion.

### 8. [minor] RepoRestore covers 14 days, but backups cover 35
**Step**: An admin deletes an org and force-pushes, then undelete and `RestoredTo`.
**Location**: DESIGN.md:353, 807, 1068.
**Gap**:
- Undelete and `RestoredTo` work within 14 days (353, 807) (measured).
- From day 14 to day 35, the overwritten commits exist only as noncurrent S3 versions. Only the whole-install `Restore` puts those back (1068), so repairing one repository rewinds every repository (inferred).

**Suggestion**: Let `RepoRestore` restore the noncurrent pack versions it needs, so its window matches the 35-day PITR window.

### 9. [minor] AMI delivery for self-hosted, plus small prerequisite gaps
**Step**: Prerequisites, quotas, and preflight; growing to three nodes.
**Location**: DESIGN.md:1064, 932, 1074, 964, 1134, 1076, 54, 1374.
**Gap**:
- **Vendor share.** The source-AMI share and KMS grant need vendor credentials (1064). That step is still unassigned for self-hosters, as the previous rewalk found (measured).
- **Air-gapped images.** Store-image requires the AMI to be shared directly with the account (measured: EC2 docs). A restored AMI gets a new id (measured), while the release names AMIs by id per Region (932).
- **Opt-in Region.** The design itself treats eu-central-2 as opt-in (1134), but the preflight doesn't check that the Region is enabled (964) (measured).
- **Node count.** "Raising `perAz` from 1 to 3" (1076) contradicts `perAz: 1` meaning three nodes (54) (measured).
- **Answerable open question.** Route 53 documents failover in private hosted zones using CloudWatch-alarm health checks, so open question 1374 can be answered (measured: Route 53 docs).

**Suggestion**:
- Name who shares the AMI for self-hosted installs, map AMI ids by digest, and add the opt-in check to the preflight.
- Add an AZ-count field to the target.

**Still holding from the earlier walks** (measured): the SSM bootstrap and `SiteAdmin` (434), first-party OAuth clients seeded for `gh` (760), runners and the actions pack (1064), the `steps[]` chain with a runner-floor preflight (1062), recovery leases kept in control buckets (1043), `restore_epoch` (1028), status documents and Prometheus (1072), GHES import with `legacy_id` (716), and uninstall `--finalize` (1076, but see finding 3).