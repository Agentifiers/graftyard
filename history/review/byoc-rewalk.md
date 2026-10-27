**BYOC re-walk, revised DESIGN.md (seat B, read-only).** Of the 12 BYOC findings, 4 are resolved and 8 are partial. All 8 global-tier findings are resolved. Below the table are 5 new findings, 1 blocker and 4 majors. Line numbers are measured from `DESIGN.md`; reasoning about consequences is marked [inferred].

| Finding | Verdict | Evidence |
| --- | --- | --- |
| BYOC-1 rollout longer than the 1 h chained session | PARTIAL | 1132-1144, 1155, 1241: in-account stages run from intents with short polling sessions, and the Paired revocation case is gated in Milestone 6. OpenTofu applies are still outside any stage (see N1). |
| BYOC-2 observer can't read Tributary | PARTIAL | 1083, 1147-1152, 1155, 1069: status documents read with `s3:GetObject` in every Region, a `nodes` report, the record in `core`, and new gates. The default `liveness` check (an unauthenticated GET, `byoc-grounding#33`) is never replaced, so a private NLB fails it, and that fail cannot be overridden (#32). |
| BYOC-3 deploy role power and bootstrap | PARTIAL | 1110, 1081: `product` variable, explicit service-linked roles, and a scoped policy are in. PassRole still names Auto Scaling, not FIS, and `DisableKey` and admin DSQL connect are not denied. The denies can also be bypassed through intents (N2). |
| BYOC-4 incident channel | RESOLVED | 1136-1138, 959: allowlisted, audited commands, and the `dr` operator flips ARC. Remaining gap: the ARC cluster appears in no component row (1090-1104). |
| BYOC-5 MRSC and preflight | RESOLVED | 1096, 1106, 1108: the claim table lives in `core`, the Paired check is DSQL-set only (ca-west-1 accepted), and every install Region gets a preflight. |
| BYOC-6 migration order and contract | PARTIAL | 1159, 1061: order, the `nodes`-status precondition, and the cursor in `core` are in. Install authority DDL still has no shadow substitute (1063 covers managed only), and the gate ignores nodes that launch later (N3). |
| BYOC-7 rollback across a barrier | PARTIAL | 1061, 1159: `phase` is in the record, and contract releases are retained with roll-forward recovery. `comparedWith` is still not computed per release line, so a customer pinned at N-2 can lose rollback. BYOC releases still have no expedited lane (1082). `prevent_destroy` makes rolling back past an added resource fail rather than "never destroy" it. |
| BYOC-8 image delivery | PARTIAL | 1073, 1110, 1161, 1057: AMI ids and digest are in the record, there is a `publish:image` stage, an Auto Scaling grant on the key, and git ships in the image. Not stated: a surge rule, `perAz: 1` behaviour, and the vendor-side share and KMS grant on the source AMI. |
| BYOC-9 certs and key rotation | RESOLVED | 1059, 1163: ACME renewal, a `certs/` upload prefix, an epoch overlap rule, and alarms to the customer's SNS topic. Remaining gap: the mTLS CA is still unnamed (300, 528). |
| BYOC-10 deploy role revoked mid-rollout | PARTIAL | 1155, 1157, 1241: safe points, the watchdog, and `Resume` are in. The stop semantics contradict each other and there is no Halt command (N4). An OpenTofu lock left behind is unaddressed. |
| BYOC-11 home Region lost | PARTIAL | 1045, 1155: operator lease, intents, status, and observer exist in every Region. The single-Region state bucket and re-pairing are only open questions (1371), with no procedure or RTO. |
| BYOC-12 week-long outage and leaving BYOC | RESOLVED | 1114-1123, 1169, 1051: vendor-dependency table, customer config key, SNS, and drills run by the operator role. Remaining gap: how the install gets registry access while still in BYOC is undefined (1119 covers only after leaving). |
| GT-1 a lost Region makes survivors lag | RESOLVED | 209-228, 1038, 1051: a member that reads its authority is current, and `BoardLease` expiry excuses the dead Region; the drill covers Paired. |
| GT-2 "no" right after a commit | RESOLVED | 233-235, 289, 388, 1233: creates wait for coverage, and the 5 s window is stated. |
| GT-3 revocations lost at a continent cut | RESOLVED | 323, 389, 1000: a `Revocation` item is written to the registry, and appliers and `reconstitute` apply it. |
| GT-4 no fence on a healed authority | RESOLVED | 327, 367-380, 391: `Sealed` first, `(geo_epoch, version)` ordering, and `rejoin`. |
| GT-5 `reconstitute` inputs incomplete | RESOLVED | 366, 371, 374-376, 391: `RecoveryClass`, escrowed hashes and secret ciphertext, and index-cell reassignment. |
| GT-6 one bad board row | RESOLVED | 216-221, 390, 1035-1042: rows past ClockBound are dropped, a lead needs corroboration, and layout, cost, and failure rows are given. |
| GT-7 Regional promotion brings back revocations | RESOLVED | 556-558, 959, 1106, 1167: a `dr` bucket record is written before `covered`, and promotion applies it first. |
| GT-8 Paired installs don't need MRSC | RESOLVED | 329, 1096, 1106, 1108: MRSC is managed-only. |

**New findings**

**N1 [blocker] Nothing owns OpenTofu applies or the release bytes inside the account.** Breaks: image changes, any release that changes infrastructure, a customer `Resume` during a vendor outage, and uninstall.
- The release carries "the OpenTofu module" (1073), and the target still names a `stateBucket` (45). No `Stage` applies it (1141-1142, 1069), so nothing updates the launch template to the new AMI.
- The operator "fetch[es] each release" (1159), but in BYOC nothing grants registry read access (1118-1119).
- [inferred] If the vendor tool keeps running `tofu`, the one-hour cap, the lock left by a revocation, and outage blocking all come back. If the worker role runs it, every serving node holds the deploy role's power.
- `Uninstall` "destroys the bootstrap last" (1173) from the nodes it is destroying.
- Fix:
  - Add `Stage::ApplyInfra`, run as a short-lived in-account task under its own role, not the node instance profile.
  - Have the deploy tool copy verified bundles create-only into `releases/` in each control bucket.
  - Replicate the state bucket to the peer Region.
  - Run uninstall's final destroy from the customer's CLI.

**N2 [major] The "cannot read data or delete stateful resources" promise (1081, 1110) can be bypassed through the shared config key.**
- In BYOC the deploy role may sign with the config key (1059), and config can repoint DSQL endpoints and bucket names (1059).
- The allowlist includes `Uninstall` and `Restore` (1136-1138), which the operator role executes, so the IAM deny never applies.
- `Submitter` is a field inside the signed body (1133), so a vendor signature can claim to be `Customer`. Admin bootstrap goes through SSM (572), and SSM is not denied.
- Fix:
  - Add a customer-only key, required for `Uninstall`, `Restore`, `DrFailback`, and endpoint, bucket, or IdP config.
  - Derive the submitter from which key signed.
  - Put the delete denies in the workload boundary too.
  - Deny the deploy role `ssm:StartSession` and `ssm:SendCommand`.

**N3 [major] The contract gate checks only processes that are running now (1159).** Breaks: migrations across a barrier together with Auto Scaling self-heal.
- The binary is hot-swapped on live instances (1057), and nodes verify only the image digest at boot (1161).
- [inferred] A node replaced, or brought back from an AZ outage (1044), after contract can boot N-1 against the contracted schema.
- Fix: nodes boot the binary named by the release row in `core`, and refuse to serve below the schema's minimum release; contract also requires every group's launch template to be at N.

**N4 [major] A running rollout cannot be halted, and what revocation does is contradictory.**
- The allowlist (1136-1138) has no `Halt` or `Abort`.
- 1155 says revocation "stops new submissions only", while Milestone 6 (1241) expects the rollout to stop at its next safe point.
- An intent signed earlier stays valid until its `not_after` (1132), and the operator cannot observe that the role was revoked.
- Fix: add `Halt { intent }` and `Abort { intent }` (swap back to the last safe point), and make the revocation runbook "revoke the role, then submit `Halt`".

**N5 [major] The operator upgrades itself in the middle of a rollout.**
- 1047 claims "the previous release's operator runs the rollout". But the operator is part of `worker` (1127), and `roll:nodes` hot-swaps those same processes (1057).
- [inferred] The lease can move to an N operator partway through. N-1 and N must then share the `StageCursor` format (1144), and a bad N operator escapes the stated containment.
- Fix: keep operator processes out of `roll:nodes` until after the soak, and put the cursor and journal under the two-release format rule.