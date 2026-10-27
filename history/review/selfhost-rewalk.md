Seven of the 12 findings are resolved and five are partial. None is still fully open. The revision also introduced or left one blocker and four majors, listed below. Line numbers are `DESIGN.md` unless a file is named. Tags: (m) means I read it in the cited text, (i) means I inferred it, (g) means an AWS fact I did not check.

| Finding | Verdict | Evidence |
| --- | --- | --- |
| 1. Recovery Region is mandatory and untested | RESOLVED | 161 makes `dr` an `Option`, 31 corrects the inventory, 1108 adds a preflight for every Region and 1241 adds a promotion gate in Milestone 6 (m). Cross-Region CDC is still [unverified] at 959, but that is now an optional path with a gate (i). |
| 2. Nobody signs config | PARTIAL | 1059 and 1110 split the trust roots and list the settings a customer can sign. But 1059 says only the customer can use the config key in self-hosted, while the operator-run restore "re-signs the new endpoints with the config key" (1165). Promotion also has to repoint nodes (959, 1167), so the signing step at 2am has no owner (i). |
| 3. No path for actions, runner packages or air-gapped installs | PARTIAL | The actions pack, import, served runner tarballs, early warning of the next runner floor and the air-gapped section are at 1073, 1161, 1199, 1171 and 1241 (m). But `uses:` still resolves only from the `actions` mirror org (1203), so `aws-actions/*` and other publishers have no source in an install. ARC scale sets are still T1 and Milestone 7 (1199, 1243) (m). |
| 4. No backup or restore | RESOLVED | AWS Backup, noncurrent S3 versions kept for the backup retention, and `admin restore` are at 1165, 475 and 975, and Milestone 0 asks AWS about DSQL backup coverage (1227) (m). Restore has new defects, in N2. |
| 5. No monitoring and no 2am runbook | RESOLVED | CloudWatch, Prometheus, SNS and the decision tree are at 1169. Observation moves to status documents and the installation record moves to `core` (1155). The weekly drill runs on one node (1169) (m). |
| 6. KMS keys unusable in the `dr` Region | RESOLVED | 1085 requires a multi-Region CMK for Regional installs with `dr`. 1167 pre-stages keys, certificates and host keys, rotates key epochs at promotion and defines failback (m). |
| 7. No first admin, instance sign-in or site admin | RESOLVED | `SiteAdmin` and `InstanceAuth` are at 525-527. The SSM bootstrap and SES or SMTP are at 572. Seeding in "each authority" is at 532 and 573, and the preflight checks SES (1108) (m). |
| 8. N to N+4 upgrades and one-node upgrades | PARTIAL | The `steps[]` chain and journal, retained contract-phase releases, roll-forward-only recovery and the one-node outage are at 1067, 1159, 1112, 1333 and 1241 (m). The runner floor under a skip and the per-step infrastructure apply are still open (N3). |
| 9. GHES import | RESOLVED | New ids with `legacy_id`, the archive mapping and credential re-issue are at 864. Milestone 6 gates the import with instance SAML (1241) (m). |
| 10. AMI outside the signed release | PARTIAL | Each release now carries per-Region AMI ids and the image digest (1073), plus `import-ami` (1161) and a copy test in Milestone 0 (1227) (m). Copying a vendor-owned, vendor-encrypted AMI still needs a vendor share and KMS grant per account and Region (byoc-grounding#46), and nothing assigns that step. AMI retention is not tied to `retention=customer` (1112, 1118) (i). |
| 11. Prerequisites and preflight | RESOLVED | 1108 checks DSQL, CDC, instance type, quotas, SES or SMTP and certificate format in every Region. 959 adds a single DNS change when the zone is not in Route 53 (m). |
| 12. Evaluation profile and uninstall | RESOLVED | 1173 and 1095 define both (m). One leftover: the weekly drill takes "one node out of rotation" and a failed drill blocks rollouts (1169), so a one-node install either goes down weekly or can never upgrade (i). |

### New findings

**N1. [blocker] Recovery needs the operator lease that lives in the broken `core` cluster.**
- **Breaks:** DSQL impaired at 2am, and `dr` promotion.
- **Lines:** 1127, 1139-1142, 1043-1045, 1134, 962, 1169, 1269.
- **Gap:**
  - Every stage, including `SwapBack`, `Config` (for example raising the 2-hour stale limit), `Restore` and `DrPromote`, runs under a lease row and cursor in `core` (m).
  - The operator is part of `worker`, and workers are "shed first" (1269) (m).
  - So the runbook's three branches (swap back, wait past 2 hours, restore) cannot start while `core` is impaired (i).
  - No line says where the `dr` operator keeps its lease or cursor (1045) (m).
- **Fix:**
  - Recovery-class commands take a lease and keep their journal in the acting Region's control bucket, written with `If-Match`.
  - Nodes load signed config straight from the bucket, with no operator.
  - Reserve one `core` connection for the operator.
  - Add a gate that runs swap-back and config apply with `core` blackholed.

**N2. [major] Restore rewinds state that nodes and the projection still hold.**
- **Breaks:** backup and restore.
- **Lines:** 1165, 476, 1266, 1106, 959, 960, 1241.
- **Gap:**
  - Restored `ref_shard` versions go backwards. `refresh` reads only rows newer than a cache's last version, so warm caches keep serving undone refs and can miss new pushes (i).
  - Revocations made after `t` come back. The revocation-record replay covers only `dr` promotion (1106) (m/i).
  - Line 960 says nothing in recovery calls a DSQL management API, but restore creates clusters (i).
  - Milestone 6 has no restore gate (m).
- **Fix:**
  - Add a `restore_epoch` that makes nodes drop reftables and auth snapshots.
  - Restart shard versions above the pre-restore maximum.
  - Always write revocation records to the home control bucket, and replay them on restore.
  - Gate restore under load in Milestone 6.

**N3. [major] The N to N+4 chain has no infrastructure apply step.**
- **Breaks:** upgrade from N to N+4.
- **Lines:** 1159, 1141, 1069, 1073, 1161, 1199.
- **Gap:**
  - Each release carries an OpenTofu module, but no `Stage` applies it. 1159 has the operator fetch each "deployer image" without saying who runs it (m).
  - If N+4's module is applied up front, it removes resources that are only two releases unused, while N's code still uses them (i).
  - If each step is applied in-install, worker nodes need the deploy policy's IAM (i).
  - Four floor raises collapse the one-release warning into a few hours of soak, so runners with updates disabled are refused (i).
- **Fix:**
  - Add an `ApplyInfra{step}` safe point, run by the submitter's tool or by a separate role that is assumed only for that stage.
  - Have the chain preflight check registered runners against the final step's floor.

**N4. [major] Uninstall cannot run as an operator stage.**
- **Breaks:** uninstall.
- **Lines:** 1127, 1138, 1173, 1110, 36.
- **Gap:**
  - The stage's cursor lives in the `core` cluster it deletes, and it runs on nodes it must terminate (i).
  - The deploy policy denies deleting stateful resources (1110), and the operator's permissions are not stated (m).
  - The bootstrap was applied with an admin's local state (byoc-grounding#22a), so the operator cannot "destroy the bootstrap last" (i).
- **Fix:**
  - Limit the operator stage to export and disabling protections.
  - The customer then runs a `tributary uninstall --finalize` step with its own admin role: destroy, empty the buckets, delete replica keys and then primary keys, then destroy the bootstrap.

**N5. [major] The air-gapped endpoint list leaves out services the revision's own stages call.**
- **Breaks:** an air-gapped install doing backups, `admin bootstrap` or `dr`.
- **Lines:** 1171, 1165, 572, 959, 1372.
- **Gap:**
  - AWS Backup is missing, which the Backup and Restore stages need (m).
  - SSM's `ssmmessages` and `ec2messages` endpoints are missing (g).
  - `dr-apply` in eu-central-1 must reach the home Region's DSQL and CDC, which Region-local interface endpoints do not provide without inter-Region peering (g).
  - ARC routing-control endpoints are public (g).
- **Fix:**
  - Derive the endpoint list from the stage table.
  - Require inter-Region connectivity when `dr` is set.
  - Have the preflight probe every endpoint.
  - Extend the air-gapped Milestone 6 gate to a backup and a promotion.