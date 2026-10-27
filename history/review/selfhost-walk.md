I found 12 gaps: 3 blockers, 7 majors and 2 minors. The self-hosted install in eu-west-1 can't be built or run as `DESIGN.md` is written. Three blockers stop it. The default durability class needs a second Region and a replication path nobody has built or tested. Nothing says who signs the config the nodes require. And there is no defined way to get actions or runner packages into the install, so with no outbound internet Actions cannot run at all.

Each claim is labelled: (measured) means I read it in the cited text, (inferred) means I reasoned from the text, (guess) means an AWS fact I did not check.

### 1. [blocker] The default self-hosted class needs a recovery Region fed by a mechanism nothing builds or tests
**Step**: Prerequisites, first deploy, and DR to a second Region.
**Location**: DESIGN.md:157, 46, 1037, 444, 748, 918, 1221, 1105-1123, 996; DESIGN.md:31 against 1039
**Gap**:
- `Durability::Regional { home, dr }` has no single-Region form, so the default self-hosted class must provision a recovery Region (157, 1037) (measured).
- That recovery Region holds warm `core` and `aux` clusters, a recovery bucket, a standing `dr-apply` node group, and an ARC routing control (748, 918, 1039) (measured).
- It all rests on DSQL CDC reaching another Region "one whole source transaction at a time", which the design marks [unverified] (444, 1221) (measured).
- No milestone gate covers it. The only "dr-apply/promot/reconcile" matches are at 145, 444, 748, 918, 925, 1049, 1141, 1191 and 1221, none in Build order (1105-1123). Milestone 4 tests only Paired (measured). The Region-flip drills are managed-only (996) (measured).
- The quickstart still promises "two Aurora DSQL clusters … one S3 bucket" (31). With `drRegion` set it is four clusters, two buckets and a second node group (inferred).
- If CDC cannot cross Regions, every default self-hosted install is unbuildable (inferred).

**Suggestion**:
- Make `dr` optional (`Regional { home, dr: Option<Region> }`). Without it, RPO is the backup interval (see finding 4).
- Add a Milestone 6 gate: promote a Regional install to its `dr` Region under load, then check the stated RPO and the `ref_log` reconcile output.
- Fix the line 31 inventory, and preflight `drRegion` for DSQL, CDC, i8g and an ARC cluster.

### 2. [blocker] Nothing says who signs the config the nodes need, in a mode with no vendor access
**Step**: First deploy (hostname, certificate, cluster endpoints), the 2am stale-fetch limit, and pointing nodes at a restored or promoted cluster.
**Location**: DESIGN.md:9, 442, 921, 928, 1004, 230-232, 355, 1026, 1028
**Gap**:
- Config "ships as a signed config bundle per cell… There is no runtime flag service" (1004) (measured).
- Other things are signed too: the stale-fetch limit ("signed per-cell limit", 9, 442), segment overrides (230-232) and `RegistryGeneration` flips (355) (measured).
- No line says whose key signs these in self-hosted mode. The deploy principal is the customer's CI (1026) and the vendor has no observer (1028) (measured).
- If the vendor's release key signs, the customer can't change these on its own:
  - its own certificate or hostname
  - the DSQL endpoints after a restore or promotion
  - the 2-hour stale limit during an outage (inferred)
- If a customer key signs, nothing provisions it, stores it, or names its trust root on the nodes (inferred).
- Certificate rotation has only a vendor "CI test" (1004) (measured).

**Suggestion**:
- Split the trust roots. The vendor key signs only the release. A customer config key, created by the bootstrap (KMS asymmetric, `trib-` prefixed), signs per-install config.
- Add `tributary config sign/apply` with last-known-good rollback, and list which settings are customer-signable (endpoints, certificate, stale limit).

### 3. [blocker] No defined way to get actions, the pin map or runner packages into a self-hosted or air-gapped install
**Step**: Registering runners and ARC with no hosted runners; the air-gapped variant (action mirror sync, runner version pins).
**Location**: DESIGN.md:107, 1078, 1079, 1083, 1120, 1123, 1040-1047, 773, 80, 969; research/actions-protocol.md#25; research/byoc-grounding.md#22, #14
**Gap**:
- The launch service resolves "every `uses:`" from "the install's `actions` mirror org, which the vendor syncs" (1083) (measured).
- Self-hosted gives the vendor no access, and no sync tool, bundle content or pull path is named (inferred).
- Actions from other publishers have no source at all. Milestone 5's own gate uses `aws-actions/configure-aws-credentials` (1120), which is not in `actions/*` (measured).
- `RunnerRefresh` carries a `download_url` (research/actions-protocol.md#25). The design never says where runner packages are hosted (measured).
- Each release refuses runners below its floor (1078) (measured). An air-gapped or `--disableupdate` ARC fleet therefore breaks on the first upgrade that raises the floor (inferred).
- ARC scale-set endpoints are T1, Milestone 7 (1079, 1123), so a 300-developer ARC shop has no autoscaling at launch (measured).
- Air-gapped installs also lack:
  - an offline `release fetch` from the vendor's S3 registry (byoc#22, #14)
  - a list of required VPC endpoints (DSQL, S3, KMS, STS, ECR, CloudWatch, email)
  - a DR switch that works without public Route 53 health checks (918, 969) (inferred)

**Suggestion**:
- Ship a signed "actions pack" with every release: the `actions/*` mirror, `tributary-actions/*` forks, the pin map and runner tarballs. Add `tributary actions import <pack>` plus an online sync job, as GHES's `actions-sync` does.
- Serve `RunnerRefresh.download_url` from the install. Publish the runner floor one release ahead and warn on runners below the next floor.
- Add an air-gapped section: an offline bundle transfer, required VPC endpoints, and CloudWatch-alarm health checks instead of public Route 53 health checks.

### 4. [major] There is no backup or point-in-time restore for DSQL or S3, and the install has only one cell
**Step**: Backups and point-in-time restore of DSQL and S3.
**Location**: DESIGN.md:920, 934, 431, 440, 1140, 948, 1231, 1170, 1037
**Gap**:
- The word "backup" appears only for the managed registry (948, 1231). Nothing covers DSQL `core` or `aux` (measured, by grep).
- The bad-deploy failure row (920) contains a bad deploy to "one cell, the canary cell first", but a self-hosted install is one cell (1037) (measured).
- A bad migration, a bad GC or a compromised admin can therefore only be undone from backup, and the Regional CDC replica copies the damage (inferred).
- A restore would have to answer things the design never addresses:
  - A restored cluster gets a new endpoint, which runs into the signing gap in finding 2 (inferred).
  - `core` and `aux` must be restored to the same instant, or check relays and run triggers dangle (inferred).
  - Superseded pack bytes leave S3 after 7 days and unreachable objects after 14 (431, 440). S3 versioning keeps deletes 30 days (934). A restore older than 7 days points at deleted objects with no undelete procedure (inferred).
- AWS Backup support for DSQL is cited only in `arena/round2/candidate-3/DESIGN.md:14`, not in DESIGN.md (measured).

**Suggestion**:
- Add a backup and restore section with:
  - AWS Backup plans for both clusters, if AWS supports them [verify]
  - S3 version retention at least as long as the backup retention
  - `tributary admin restore --to <t>`: restore both clusters to one time, re-sign the endpoints, undelete pack versions, and run the daily push-durability replay as verification
- Raise GC and superseded-pack retention to cover the restore window.

### 5. [major] Monitoring is undefined for self-hosters, and there is no 2am runbook for an impaired DSQL
**Step**: Monitoring, and what the operator does at 2am when DSQL is impaired.
**Location**: DESIGN.md:54, 918, 442, 996, 1002, 1014, 1028; research/byoc-grounding.md#17, #18, #21, #51
**Gap**:
- The quickstart says "At 2am the self-hoster has nothing to repair by hand" (54). Yet a Regional `core` impairment stops pushes, merges, the API, the web UI and Actions. Fetches go stale and stop after 2 hours (442, 918) (measured).
- Recovery is a human-triggered ARC flip with up to 15 minutes of push RPO, ref rollbacks and about 60 minutes RTO (918) (measured).
- The design gives no rule for choosing between waiting and promoting, no command, no failback procedure, and no runbook for an AWS-wide engine regression (inferred).
- The observer routes (1014) sit on machinery that is API Gateway plus Lambda (byoc#18). It also writes an installation record to a DynamoDB control table (byoc#17), which contradicts "no DynamoDB" for single-Region installs (1049, 1181) (measured).
- <vendor-fleet-app> is a vendor runbook generator (byoc#21) and the observer is optional in self-hosted (1028) (measured).
- So nothing names the metrics, alarms or SLO burn source that the per-AZ deploy gate (1002) and the operator's pager read (inferred).
- The weekly stale-fetch drill runs on "the canary cell" (442, 996), which a self-hosted install does not have (measured).

**Suggestion**:
- Specify the export contract for self-hosted: CloudWatch metrics and alarms, or Prometheus.
- Say which component serves `/v1/system/observer/*` in installs, and drop or replace `recordInstallation`'s DynamoDB table.
- Write a DSQL-impaired runbook:
  - a decision tree keyed on AWS Health, CDC lag and the stale-limit clock
  - `tributary dr promote` and `tributary dr failback`
  - a customer-run quarterly promotion drill

### 6. [major] Regional recovery cannot decrypt the token and TLS keys if the home Region's KMS is gone
**Step**: DR to a second Region.
**Location**: DESIGN.md:1030, 528, 773, 970, 918
**Gap**:
- Multi-Region CMKs are listed for Paired only. The self-hosted row reads "Customer CMK, multi-Region for Paired" (1030) (measured).
- The cell's Ed25519 signing keys are KMS-wrapped on local disk and unwrapped at node start "through its own Region's KMS endpoint" (528, 773) (measured).
- After a home-Region loss, nodes launched in eu-central-1 could not unwrap them (inferred). That loses `ghs_`/`GITHUB_TOKEN` minting, the OIDC JWKS and assertion signing.
- The recovery bucket's SSE-KMS key, the TLS key and the SSH host keys have no stated copy in the `dr` Region either (inferred).
- Failback, meaning reversing CDC and handling home-only commits inside the RPO window, is absent (inferred).

**Suggestion**:
- Require a multi-Region CMK replica in `dr` for Regional installs, and pre-stage wrapped keys, the certificate and host keys in the `dr` control bucket.
- Add a key-epoch rotation step to promotion, and a failback procedure.

### 7. [major] First admin and first `gh auth login`: no instance-level sign-in, no site-admin role, bootstrap undefined
**Step**: The first site admin and the first `gh auth login`.
**Location**: DESIGN.md:51, 531, 812, 464-489, 333, 337-339, 357, 494, 532, 1033, 125, 773; research/byoc-grounding.md#2
**Gap**:
- `tributary admin bootstrap --first-admin alice` (51) does not say:
  - where it runs
  - how alice gets a first credential
  - how it writes the authority (inferred)
- The authority refuses to commit without a writer lease checked against ClockBound (333, 337-339, 357), so a CLI on a laptop cannot write directly (inferred).
- SSO attaches only to orgs and enterprises "as on GitHub Enterprise Cloud" (531). Instance-wide SAML, LDAP or CAS, which GHES shops use, is absent (measured).
- No site-admin or stafftools role exists in `OrgRole`, `Actor` or `authorize` (464-489). `rest:enterprise-admin` is T1 (812) (measured).
- The device flow's browser sign-in therefore has no defined identity source for 300 users (inferred).
- First-party OAuth clients are "seeded once in `auth-na` (an install's one authority)" (494, 532). But an eu-west-1 install's one geography is Europe (1033), so it would be `auth-eu` (measured contradiction).
- Email goes through SES (125, 773). A new dedicated account starts in the SES sandbox, and SES has account-wide singletons (byoc#2) (guess for sandbox; measured for singletons).

**Suggestion**:
- Define install authentication modes (built-in, SAML, LDAP, OIDC) at instance scope, and a `SiteAdmin` actor with bypass and audit.
- Have `admin bootstrap` run on a node through SSM, minting a one-time sign-in link printed to the operator.
- Fix the `auth-na` wording, and add SES production access (or SMTP) to the prerequisites.

### 8. [major] Upgrades: N to N+4 stepping has no defined mechanics, and a single node can't upgrade without downtime
**Step**: Upgrades from N to N+1 and from N to N+4.
**Location**: DESIGN.md:1012, 1027, 1006, 1002, 749, 1078; research/byoc-grounding.md#14, #15, #16, #25, #34, #35, #36
**Gap**:
- "The deploy tool… steps through every contract boundary in order, automatically" (1012). The existing tool deploys one bundle with that bundle's own pinned deployer image (byoc#16) (measured).
- Stepping therefore means fetching and verifying N+1 through N+4, including deployer images. Nothing says whether intermediate bundles are guaranteed retained (byoc#15). The customer cannot retag them, because writes are publisher-only (byoc#14) (inferred).
- A failure after a contract step (`rollbackBarrier`, 1006) leaves the install between releases. Rollback is barred (byoc#34) and runs the older tool against newer state (byoc#36). No resume or roll-forward procedure is given (inferred).
- Each step can raise the runner floor (1078) (inferred).
- Git is pinned in the node image (749), and image changes "replace instances per AZ through the Auto Scaling group" (1002). A one-node evaluation install therefore takes downtime and loses its cache on any git or OS bump. That contradicts "upgrades the same way, by process handoff on one host" (1012) (measured contradiction).

**Suggestion**:
- Give the release record a `steps[]` chain the tool walks, with a resumable journal.
- Guarantee `retention=customer` for every contract-phase release.
- Define roll-forward-only recovery after a barrier.
- State that single-node node-image upgrades take a short outage, or keep git outside the AMI on a separately updated volume.

### 9. [major] GHES import is T1, in no milestone, and "keep source ids" breaks the id layout
**Step**: Importing from GHES.
**Location**: DESIGN.md:823, 174-181, 1123, 1073
**Gap**:
- `rest:migrations` is T1 and no milestone gate names it. Milestone 7's list omits it (1123) (measured).
- "Single-cell installs keep source ids" (823) conflicts with the id layout (inferred):
  - Repository-scoped ids are `(seq << 16) | slot`, and "every fork carries its slot" (175, 181). An imported fork keeps its GHES id, so its low 16 bits differ from its parent's and the network spans slots.
  - Global ids are `seq * 4 + origin geography` (177), so an imported user id encodes a random geography.
  - The `id_block` allocator must also skip imported ids, which is unstated.
- Nothing says how users, SSH and GPG keys, PATs, LFS objects, release assets, webhooks with secrets, or runner registrations carry over. Nothing addresses instance-SAML login mapping either, given EMU-style `_shortcode` logins (1073) (inferred).

**Suggestion**:
- Import with new ids plus a `legacy_id` index in every mode, as managed already does (823).
- Specify the `ghe-migrator` archive mapping and the credential re-issue flow.
- Add an import gate to a named milestone, because GHES replacement is the self-hosted buyer's first step.

### 10. [major] The node image (AMI) is not in the signed release, and sharing it per account and Region is a vendor step
**Step**: Bootstrap and first deploy; the DR Region launch; the air-gapped variant.
**Location**: DESIGN.md:1018, 749, 1002, 1053; research/byoc-grounding.md#46, open question 6
**Gap**:
- The bundle "includes the node image" (1018), and the release machinery transfers "unchanged" (1053) (measured).
- But the machinery keeps AMIs outside `release.json`. They are shared per customer account and Region with a vendor-run `release share-ami`, and must be re-encrypted if they use `aws/ebs` (byoc#46) (measured).
- A self-hoster therefore needs a vendor action per account and Region, including the `dr` Region. The image that carries the pinned git is not covered by the release signature (inferred).

**Suggestion**:
- Put the AMI, or an AMI build recipe with a reproducible digest, into the signed release.
- Add a customer-side `release import-ami` that copies and re-encrypts with the customer CMK in each install Region, and verifies the digest at boot.

### 11. [minor] Prerequisites are incomplete and nothing preflights the Regional install
**Step**: Prerequisites and quotas (DSQL, NLB, i8g, ACM, DNS, opt-in Regions).
**Location**: DESIGN.md:31, 47, 58, 1051, 80, 748, 1025, 918; research/byoc-grounding.md#2, #6; research/uptime.md#22
**Gap**:
- The preflight exists only for Paired trios (58, 1051). Nothing checks a Regional `home` and `dr` for (measured):
  - DSQL and CDC availability
  - the DSQL cluster quota for four clusters
  - i8g.2xlarge availability, which I have not confirmed in eu-west-1 or eu-central-1 (guess)
- A brand-new dedicated account (byoc#2) starts with low EC2 vCPU and SES limits (guess).
- The edge terminates TLS itself behind a TCP NLB (80, 748). A standard non-exportable ACM certificate therefore can't be used, and the design doesn't say how a customer certificate and key reach the nodes (1025) (inferred).
- The ARC flip (918) assumes Route 53 and an ARC cluster (uptime.md#22). Customer DNS is "customer domain" (1025) and may not be Route 53 (inferred).

**Suggestion**: Publish a prerequisite checklist and extend preflight to Regional installs:
- DSQL, CDC and i8g in both Regions
- vCPU and DSQL cluster quotas
- SES production access
- certificate format (PEM import, or exportable ACM)
- Route 53 or ARC presence, with a documented non-Route-53 DNS fallback

### 12. [minor] Growing from one node to three, and uninstall, are not described
**Step**: A one-node evaluation install growing to three nodes; uninstall.
**Location**: DESIGN.md:1040, 1044, 937, 938, 157, 1051, 934, 823
**Gap**:
- Evaluation is "1 for evaluation" (1040) and "writes one board row" (1044) (measured).
- The design does not say:
  - whether evaluation still needs `dr` (157 makes it mandatory)
  - how the 33% removal cap and 66% per-AZ sizing (937, 938) behave with one node
  - how `nodes.perAz` changes from 1 to 3 (the rendezvous rebalance and cold-cache fill) (inferred)
- Uninstall is absent (measured, by grep). Nothing covers:
  - DSQL deletion protection
  - versioned buckets and the 30-day versions (934)
  - the CMK deletion window
  - the `prevent_destroy` registry table in Paired installs (1051)
  - data export, which depends on T1 migrations (823)

**Suggestion**:
- Add an evaluation profile: one AZ, no `dr`, stated as having no availability SLO, plus a documented `perAz` scale-up and its cache-warm behaviour.
- Add `tributary uninstall`: export, disable protections, empty versioned buckets, schedule key deletion, and destroy the bootstrap last.