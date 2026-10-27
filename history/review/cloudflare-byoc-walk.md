# Cloudflare design, BYOC and managed-cell operator walk (panel seat B)

Subject: `cloudflare/DESIGN.md` (742 lines, read whole) and `cloudflare/cost.py` (run). Docs were fetched as `index.md` on 2026-10-26. Labels: [measured] I read or ran it, [inferred] my reasoning from what I read, [guess] my estimate.

## 1. Verdict

**Holds with changes. For managed, change the cell shape.** The three-site Raft base over cross-script Durable Object bindings survives the walk. Nothing I walked loses an acknowledged push through the protocol itself [inferred]. The walk breaks on operations instead. The design moved the AWS design's lifecycle steps onto Cloudflare deploys, but it dropped the in-account operator that made those steps safe (`DESIGN.md` lines 1003, 1050 to 1052, against `cloudflare/DESIGN.md` lines 508 and 531) [measured]. The result is a set of containment rows ("watchdog swap-back", "automatic swap-back", "ReplaceMember re-protects") that name an actor no Cloudflare mechanism provides [inferred]. There is also no way to move leadership off a site. So every site-day deploy and every gray regional incident hits the third of groups led from that site [inferred]. Managed has a deeper problem. It puts one cell per account behind one hostname, but bindings cannot cross accounts, so the managed front door is undefined (finding 8) [inferred]. Required changes: an in-account deployer plus a runtime site drain, a defined `GitCache` cardinality with a recomputed bill, a stateless ops script that holds every API token, a Cloudflare-only vault account outside the install token's reach, and a stated managed topology.

**Walk index.** Each step, whether `cloudflare/DESIGN.md` tells the SRE what to do, and where the gap is treated.

| Step | Design tells the SRE? | Finding |
| --- | --- | --- |
| Day 0: account, plan, Containers, R2 | Partly: lines 547 to 552 list them. No first admin, no `Release` genesis, no key custody | 11 |
| Day 0: account token | Says "an account-owned API token" (551). Permissions, holder, and rotation unstated | 6, 7, 11 |
| Day 0: Terraform plus Wrangler | One sentence (531). It does not say which tool owns site scripts | 2, 11 |
| Day 0: three site scripts, buckets, location checks | Commands (35 to 39) and checks (110, 274). Failure handling unstated | 11 |
| Day 0: Spectrum | "Pro is the floor" (538, 549). The paid add-on and the Tunnel origin type are unstated | 9, 10 |
| Day 0: container sizing | Contradictory (224 against 370 and 371) | 4, 5 |
| Site-per-day upgrade, failing mid-roll | Says "watchdog" (460, 508) but gives it nothing to act with | 1, 2 |
| Regional DO incident, 6 hours | Claims about 15 s (9, 419). No drain, and no `ReplaceMember` trigger | 2, 3 |
| `ReplaceMember` | Mechanism only (329). The target site, threshold, move-back, and cleanup are unstated | 3 |
| Dashboard and API outage during upgrade | "Rollouts wait" (507). A bad version already at a split cannot be removed | 1 |
| Account token rotation | Absent | 6 |
| Container vCPU quota | One row (478) with no enforcement mechanism | 5 |
| Push over plan body limit | "State their cap" (368) | 9 |
| Billing against `cost.py` | Table reproduces. Several lines are mispriced or missing | 4, 10 |
| Customer revokes vendor token | "Keeps serving" (554). No mid-roll runbook | 1, 6, 7 |
| Managed cell | One account per cell (94, 535) behind one hostname (18) | 8 |

## 2. Findings

### 1. Blocker. Release control has no executor in the account, so swap-back, rollback, and mid-roll recovery all need the Cloudflare API from outside

**Rests on:** lines 41, 344 to 348 (347: "Swap-back is automatic"), 460, 484, 506 to 509 (508: "Each site script applies one release per day under its own watchdog"), 531, 541, 554.

**Scenario.** The SRE runs `tributary release roll 2026.44 --one-site-per-day` (line 41).
- *Monday.* `wrangler deploy` of `trib-site-a` activates the new Worker version and then fails pushing the git image. The rollouts doc says "The Worker is active before the image and rollout steps begin. These steps are not transactional" (https://developers.cloudflare.com/containers/configuration/rollouts/) [measured]. Site A now runs member code N+1 against git image N. No line in the design covers that mixed state [measured, by reading 342 to 348 and 460].
- *What the watchdog can do.* A Worker cannot redeploy or roll back itself without an API token. Rollback is "using Wrangler or the Cloudflare dashboard" (https://developers.cloudflare.com/workers/versions-and-deployments/rollbacks/) [measured]. Gradual deployments have no automatic rollback; the doc says to "Roll back ... if you notice issues" (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/) [measured]. The edge's "automatic swap-back" (347, 484) also needs a metric read, and Analytics Engine is read through a bearer-token SQL API (https://developers.cloudflare.com/analytics/analytics-engine/sql-api/) [measured]. So every swap-back is an API call made by whoever holds a token [inferred].
- *Tuesday, a 2023-11-02-class API outage (about 41 h, `research/cloudflare-uptime.md` finding 2) during the roll, with the edge at a 10% split on a bad version.* "Rollouts wait" (507). The bad 10% also stays for 41 h. If it fails its requests, each minute has 10% failures. That is above the 5% bad-minute threshold (line 422), so about 2,460 bad minutes against a 4.3-minute monthly read budget [inferred arithmetic].
- *The customer revokes the vendor token mid-roll.* Site A is on N+1, and B and C are on N. The design gives no step for the customer to finish or abort (541, 554) [measured]. The AWS design had a runbook for this exact case: revoke, then `Halt` signed with the customer-only key (`DESIGN.md` line 1050) [measured]. Line 531 says the BYOC machinery "transfer[s] unchanged". But that machinery drove the AWS stages from an in-account operator role, and the Cloudflare port has none [measured].

**Fix.**
1. Add a stable `trib-front` Worker that changes about once a year. It forwards every request to the edge over a service binding and sets `Cloudflare-Workers-Version-Overrides` from a signed pointer it reads from the `Release` group. Keep both N and N+1 in every edge deployment (N+1 at 100%, N at 0%), so swap-back is a signed `Release` entry with no API call. Version overrides reach versions "even those set to serve 0% of traffic" and work across service bindings (https://developers.cloudflare.com/workers/versions-and-deployments/version-overrides/) [measured].
2. For site scripts, the runtime lever is the site drain in finding 2.
3. Add an in-account `trib-deployer` (a Worker plus a Workflow, no Durable Object member classes). It holds a customer-created account token in Secrets Store, deploys only versions that were already uploaded with `wrangler versions upload` and whose release signature verifies, and acts only on intents signed by the vendor or customer key.
4. Ship each site's container image change on a separate day from its member code, with `--containers-rollout=none` on the code deploy.
5. Add a revocation runbook: revoke, submit `Halt` signed by the customer key through the edge, then `Resume` or `Abort` executed by `trib-deployer`.
6. Rewrite rows 460, 484, and 506 to 509 to name these actors.

### 2. Major. Nothing moves leadership off a site, so each site-day deploy and each gray regional incident stalls or degrades the third of groups led there

**Rests on:** lines 9, 98 to 99, 166 to 168, 288, 378 to 380, 398, 419, 455, 457.

**Scenario, deploy.** A plain deploy assigns every Durable Object of the script the new version, and "The Durable Object will only be reset when it is assigned a different version" (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/with-durable-objects/) [measured]. So on B's day, every member on B resets at once [inferred]. In-flight requests on the leaders there fail. Followers then cannot campaign until their promise lapses (`ELECTION_AFTER` = 10 s, line 167), and `MemberState` (164 to 165) persists no leadership [measured]. That gives about 10 to 15 s with no `fresh` read and no commit for every group led from B [inferred].

The deploy is not the only trigger. Any new version of a site script does the same: `wrangler secret put` "creates a new version of the Worker and deploys it immediately" (https://developers.cloudflare.com/workers/configuration/secrets/) [measured], and so does a `terraform apply` that re-uploads a site script (line 531) [inferred]. A loop that touches all three scripts resets every member of every group within seconds [inferred].

The release tag `2026.44` suggests weekly releases [guess]. At one or two bad minutes per site-day, that is 12 to 24 bad minutes a month against the 4.3-minute read budget (lines 426 to 427) [inferred arithmetic, assuming the edge fails rather than holds requests].

**Scenario, the 6-hour WEUR incident.** The real event was "Elevated error rates" from 17:51 to 00:09 (`research/cloudflare-uptime.md` finding 20) [measured], not a clean loss. A leader on B with a 20% storage error rate still renews its 9 s lease through any one follower ack. It keeps leading for 6 hours, and about 20% of its groups' requests fail [inferred]. Line 391 says "a stalled leader loses its lease", which covers stalls and not flaky leaders [measured]. The 15 s claim (lines 9, 419) holds only for a clean failure [inferred].

**Scenario, leader drift.** Raft elects whoever wins, and the design states no leader preference (288 assumes A) [measured]. After a few deploys and incidents, about two thirds of groups are led from another continent than their users. Every `fresh` read then pays an intercontinental round trip (380) [inferred].

**Fix.**
1. Add Raft leadership transfer (`TimeoutNow` to a caught-up follower).
2. Add a signed `SiteMode { site, Drain | Normal }` entry in the `Release` group. Members on a drained site hand off leadership within one lease, refuse to campaign, and keep voting and storing, so the quorum is unchanged.
3. Each site-day runs drain, then a gradual deploy of that site at 1%, 10%, 50%, 100% (assignment per object is stable, as the doc above guarantees), then undrain.
4. Every operation that creates a site-script version (secret, var, binding, container config, Terraform) is a site-day operation. Terraform must never own a site script.
5. Add a self-step-down rule: a leader whose own storage or RPC error rate exceeds 5% over 30 s transfers leadership. Add an automatic drain when a site's member error rate crosses a threshold.
6. Add a per-group preferred site (the owner's home), restored by transfer after recovery.
7. Change line 9 to state the gray-failure case.

### 3. Major. `ReplaceMember` has no trigger, no target in a default BYOC install, breaks the continent claim, and never moves back or cleans up

**Rests on:** lines 35 to 39, 274, 329, 373, 436 to 437, 443, 457.

**Scenario.** During the 6-hour WEUR incident the SRE must decide whether to run `ReplaceMember`. The design gives no threshold for "site loss" against a degraded site [measured, lines 329 and 457]. Its target is "D (wnam)" (329), but a default BYOC install has only A, B, and C (35 to 38) [measured]. Creating D needs `tributary sites add`, which is Wrangler, Terraform, and API calls, so it is impossible during a control-plane outage [inferred]. D in wnam with A in enam puts two voters in North America. `Placement::new` allows that because it refuses only one region (131) [measured]. But line 437's "Chains on `Tri`, continent loss: RPO 0" is then false until someone moves back, and no move-back exists [inferred].

The replacement does not buy durability during the incident. With B out, every new entry is already on A and C [inferred from 175 and 269]. What it costs:
- up to 1 million Workflows (373);
- a copy of every pack of each group's networks, done per group (329), so a fork network of 100 repositories is copied 100 times, with the extra copies refused by `If-None-Match` and each attempt still billed as a Class A operation [inferred];
- removed members on B left holding SQLite storage, billed at $0.20 per GB-month, because no cleanup step exists [inferred from the DO pricing doc].

**Fix.**
1. Trigger `ReplaceMember` only on a signed `SiteLost` intent, or on 24 h of zero acks from a site. Never trigger it on elevated errors, because drain (finding 2) covers those.
2. Replace within the same continent from a pre-created spare site per continent (B with eeur, A with wnam, C with oc). Create the spares at install so no control-plane call is needed.
3. Copy packs once per network, not per group.
4. Add `RestorePlacement`, which moves each group back to its home sites after recovery.
5. Run `deleteAll()` on removed members after 35 days, matching the restore window in line 442.
6. Add the spare sites to `cost.py`.

### 4. Major. `GitCache` cardinality contradicts itself, and the per-network reading breaks the quota and the bill by about two orders of magnitude

**Rests on:** lines 224 to 227 ("A container-backed Durable Object per (network, site)"), 340, 370 to 371, 471 ("One container's repositories", plural), 475, 584 to 585, 594 to 596; `cost.py` lines 50 to 51 and 141.

**Scenario.** On day 0 the SRE must set `max_instances` and ask for a quota.
- *Per-network reading.* Each container has its own Durable Object (https://developers.cloudflare.com/containers/platform/pricing/) [measured]. Read literally, line 224 means one container per active network per site. `max_instances` "Defaults to 20" and errors beyond it (`research/cloudflare.md`, Containers) [measured]. A 50-developer install that fetches more than 20 networks in a day on one site then fails clones on day 1 [inferred]. Each warm (`sleepAfter` 24 h) standard-2 costs $41.63 a month and each standard-4 $82.52 [measured arithmetic from doc prices, 730 h]. So 20 networks on two sites cost about $1,665 a month against the $98 container line in `--detail small design` [measured run plus arithmetic]. At 2,000 developers, 1,500 vCPU allows at most 375 standard-4 instances, which is 187 networks warm on two sites [inferred arithmetic].
- *Pooled reading* (12 instances, lines 370 and 371). The warm disk is 12 x 20 GB = 240 GB against 4,000 GB of git at the large tier (`scripts/cost_model.py` TIERS) [measured]. That is 6%. The claims that warm pools "carry a day of load" through a control-plane outage (340, 475) and that "each hot network is warm on two sites" (472) then rest on an unmeasured working set [inferred].

**Fix.**
1. Define `GitCache` as `GitCacheShard { site, shard: u16 }`, with networks mapped by rendezvous hashing, an LRU on disk, and a stated count per tier.
2. State the scheduling policy. The `durable_object` policy is beta and "does not support `max_instances`" (https://developers.cloudflare.com/containers/configuration/scheduling-policy/) [measured].
3. Add a Milestone 0 measurement of the daily working set in bytes per tier.
4. Make `cost.py` derive the instance count from that working set.
5. Delete "per (network, site)" from line 224, or price it.

### 5. Major. The account vCPU quota is shared with hosted Actions sandboxes, and "a third per site" has no enforcing mechanism

**Rests on:** lines 71, 104, 370, 478, 499 to 502, 540.

**Scenario.** Two orgs each start a 256-job matrix on hosted runners (Sandboxes, line 71). Sandboxes inherit Containers limits (`research/cloudflare.md` line 11) [measured]. At standard-4, that is 2,048 vCPU, above the 1,500 vCPU account limit (https://developers.cloudflare.com/containers/platform/limits/) [measured, arithmetic]. New `GitCache` starts on all three sites then fail, so cold clones fail everywhere [inferred]. The same happens when a region loss forces the surviving sites to start caches [inferred].

Line 478's "per-site vCPU budget of a third of the account limit" has no mechanism behind it. `max_instances` is per class under the `default` policy only, and Sandboxes count against the same pool [inferred from the scheduling-policy doc]. Per-org caps (501) are stated in jobs, not vCPU [measured]. Raising `max_instances` is a site-script deploy, which resets members (finding 2) [inferred].

**Fix.**
1. Put hosted runners in a separate runner account in managed and in BYOC. Runners speak HTTPS to the Actions broker and need no member binding [inferred]. The AWS design already used separate runner accounts (`DESIGN.md` line 650) [measured].
2. Reserve git cache vCPU per site through `max_instances` on `default`-policy classes, sized so any two sites can carry all networks.
3. Add a Tributary-side semaphore Durable Object for any `durable_object`-policy class.
4. Add an alarm at 70% of account vCPU.
5. Add the quota request to the day-0 checklist.

### 6. Major. The runtime does hold Cloudflare API tokens, so rotation, revocation, and API outages reach it, and verification has no fail-static rule

**Rests on:** lines 78, 110, 146 to 147, 260, 274, 403, 507 ("Nothing at runtime reads them"), 554.

**Scenario.** Several runtime jobs need tokens or rotatable secrets [inferred unless marked]:
- The daily placement re-verification (274) reads R2 bucket `location` through the REST API (110).
- `place_learners` (146 to 147) reads Analytics Engine counters through a bearer-token API [measured doc, above].
- cloudflared in the SSH fronts needs a Tunnel token.
- Stateless installation tokens (260) need a signing key. The AWS design wrapped that key in KMS (`DESIGN.md` line 437) [measured], and the Cloudflare design names no custody.

None of these has a rotation step [measured, grep for "rotat" finds nothing]. If the vendor token is the one these crons use, the customer revoking it silently stops placement and learner checks. If the cron reads an API 403 or 5xx as "misplaced", it fires `ReplaceMember` across the install [inferred]. Rotating any secret held in a site script is a site-script version, so it resets members (finding 2).

**Fix.**
1. Add one stateless `trib-ops` script with no Durable Object member classes. It owns every API-calling Cron Trigger and every rotatable secret, and reads them from Secrets Store (open beta, https://developers.cloudflare.com/secrets-store/, off the request path).
2. Give it a customer-owned, least-privilege token for R2 read and Account Analytics read, separate from the deploy token.
3. Make any verification failure a no-op plus an alarm. Only a positive mismatch read twice, 24 h apart, starts replacement.
4. Write a rotation runbook per secret: roll the token, then `wrangler versions secret put` on `trib-ops` or the edge only.
5. Name custody for installation-token signing keys: in `trib-ops` or the edge, wrapped by a key in Secrets Store.
6. Correct lines 507 and 554 to say which runtime jobs call the API.

### 7. Major. One leaked or misused account token can destroy all three sites, and the RPO table has no row for it

**Rests on:** lines 104, 436 to 447, 542, 551, 672.

**Scenario.** The vendor's token needs Workers edit to deploy sites [inferred]. With it, an attacker or a buggy vendor pipeline can deploy code that calls `deleteAll()` in every member on all three scripts. It can also remove the cross-binding and deploy a class tombstone, which "removes its namespace and **all of its stored data permanently**" (https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/) [measured]. The cross-script bindings force the tombstone to wait until no other Worker binds the class, which blocks a single accidental deploy but not a deliberate sequence [measured, same page].

Line 104 lists the leaked token as an account-scoped fault, but the RPO table (436 to 447) promises "0" with no row for it [measured]. The lifeboat, the only copy outside the account, is only "Recommended" for BYOC (542) [measured]. A second gap: the account-token compatibility matrix lists Workers, R2, DO, Spectrum, and Tunnels but not Containers (https://developers.cloudflare.com/fundamentals/api/get-started/account-owned-tokens/) [measured]. Whether an account token can push and deploy container images is unverified.

**Fix.**
1. Add a Cloudflare-only vault: a second account that the install's tokens cannot reach. A Worker there pulls the lifeboat change feed and packs over HTTPS with an org-signed export token, and writes them to R2 under bucket lock rules.
2. Make the vault or a lifeboat required for BYOC and managed.
3. Add an RPO row: "Account compromise or destructive deploy: vault lag, about 30 s; total loss without one".
4. List the deploy token's exact permissions in "Deployment modes".
5. Add a Milestone 0 check that an account-owned token can run `wrangler deploy` with containers.

### 8. Major. Managed cells are one account each behind one hostname, but bindings cannot cross accounts

**Rests on:** lines 18, 94, 106, 265, 370, 374, 535, 538.

**Scenario.** The vendor SRE adds cell 2 in a new account (94, 370, 374). Managed is one global namespace on `tributary.dev` (line 18, `FRAME.md` line 21) [measured]. A zone lives in one account, and routes and bindings reach only that account's Workers. A service binding's target "must be on your Cloudflare account" (https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) [measured]. So the edge for `tributary.dev` cannot bind cell 2's members. Line 265's "Every group is reachable from every edge" is false across cells [inferred].

Line 538's "The cell account's zone" implies a zone per cell, which breaks the single hostname [inferred]. Global `Names` and `Tokens` groups (265) need a home that every cell's edge reaches [inferred]. If instead managed is one account, it meets account limits: 500 Workers per account (https://developers.cloudflare.com/workers/platform/limits/) [measured], 250 Cron Triggers, 1,500 vCPU, and 300 Workflow creations a second (`research/cloudflare.md`) [measured]. And one account becomes the blast radius for every managed user [inferred].

**Fix.** State the managed topology. Option (a): a front account holding the zone, the edge, and the global `Names` and `Tokens` groups, calling cell accounts over the signed-HTTPS member path, with the December-2025-class exposure stated for cross-cell hops. Option (b): one managed account per continent group, with an Enterprise limit raise named as a prerequisite. Either way, rewrite 94, 265, 370, 374, and 538 to match.

### 9. Major. A push over the plan's body limit fails with a bare 413, and the remedy and its test plan target the wrong plan

**Rests on:** lines 20, 298, 368, 538, 549, 594, 604, 619 to 620, 652.

**Scenario.** On the $168 Pro install (594), a developer pushes 150 MB over HTTPS. If the front enforces the limit on chunked bodies, which is unverified (`research/cloudflare-git.md` line 94), the Worker never runs. git prints `error: RPC failed; HTTP 413` with no `remote:` text [inferred]. So "installs on lower plans state their cap" (368) has no channel to the user [inferred]. If the front cuts the body mid-stream, the edge has already opened multipart uploads in three buckets, and nothing aborts them [inferred].

SSH is the natural escape, since Spectrum is layer 4 with no Worker body limit [inferred]. But:
- Spectrum on Pro and Business is a "Paid add-on" with "TCP: No" (https://developers.cloudflare.com/spectrum/protocols-per-plan/) [measured].
- A Tunnel virtual-network origin requires "Application type must be TCP or UDP" (https://developers.cloudflare.com/spectrum/reference/configuration-options/) [measured], so the Pro or Business SSH app may not accept the Tunnel origin [inferred].
- Milestone 0 runs SSH and push tests on "an Enterprise trial zone" (604) and records the chunked case only on Business (620) [measured].

**Fix.**
1. Run the Milestone 0 push, chunked-push, and Spectrum-to-Tunnel rows on Pro and on Business.
2. Have the edge count streamed bytes and refuse at its own cap below the plan limit, with a receive-pack `report-status` naming SSH.
3. Make the sweeper abort orphan multipart uploads.
4. Add a preflight that reads the zone plan and the largest pack per repository and refuses Pro for imports above the cap.
5. If Spectrum cannot reach the Tunnel on Pro or Business, state that SSH needs Enterprise for BYOC.

### 10. Minor. `cost.py` misprices or omits lines a BYOC customer will be billed for

**Rests on:** lines 566 to 596, 671; `cost.py` lines 155, 159, 161.

**Evidence.**
- *Egress region.* Container egress is priced at the North America and Europe rate for every site (`cost.py` line 155). Site C is apac, at $0.04 to $0.05 per GB with 500 GB included (https://developers.cloudflare.com/containers/platform/pricing/) [measured]. An APAC-led install at 2,000 developers moves 63,251 GB a month, which costs $2,510 to $3,138 against the modeled $1,556. That is 16 to 27% on the $5,845 total [measured run of `workload()` plus arithmetic].
- *Spectrum add-on.* The Spectrum paid add-on is not priced, only the overage (`cost.py` line 159) [measured]. The product page renders only allowance and overage, so the base fee is unverified [measured].
- *Workflows.* They are fixed at $0 (`cost.py` line 161), but finding 3's copies are billed [inferred].
- *Removed members* keep SQLite storage (finding 3).
- *Image storage.* The 50 GB per account includes rollback images (https://developers.cloudflare.com/containers/platform/limits/) [measured].

**Fix.** Price egress by each site's region and the leader distribution. Add the Spectrum add-on as an unverified line. Price `ReplaceMember` and the spare sites. Add a "BYOC quote" note that 2 GB pushes need an Enterprise contract on top of the table.

### 11. Minor. Day 0 lacks the first admin, the `Release` group genesis, key custody, tool ownership, and the failure path for placement checks

**Rests on:** lines 35 to 42, 110, 114, 140, 274, 509, 531, 544 to 553.

**Evidence.**
- *First admin.* No step creates the first site admin or configures SAML or OIDC [measured, by grep]. The AWS design has `tributary admin bootstrap` (`DESIGN.md` line 439) [measured].
- *`Release` group.* `formats:enable` needs "Two keys" (509), but no line says which keys, who holds them, or how the `Release` group's first config is written [measured].
- *Placement checks.* The design does not say what happens when they fail. A hint is honored only on the first `get()`, and a bucket's location is fixed at creation (`research/cloudflare.md`) [measured]. So recovery means new member generations or a new bucket name and a site config change, which is a site deploy [inferred].
- *OC sites.* Containers in `OC` are "Limited capacity" and "cannot be used exclusively" (https://developers.cloudflare.com/containers/concepts/placement/) [measured]. Site F = oc (94) and `Apac2w` (140) cannot pin containers to one region as "Site" (117) requires [inferred].

**Fix.** Add an "Install operations" section that covers:
- `tributary install` creating the `Release`, `Names`, and `Tokens` groups from a customer-signed genesis;
- `tributary admin bootstrap` minting a one-time sign-in link through `trib-ops`;
- custody of the vendor and customer intent keys;
- Terraform owning zone, DNS, buckets, queues, Spectrum, Tunnel, and WAF, and Wrangler alone owning the edge, site, ops, and deployer scripts;
- retry with a new generation on a failed colo check, and a bucket-recreate path;
- a rule that an `oc` site lists `["OC", "APAC"]` and is not counted as a separate region for containers.

## 3. Checked and found sound

- `cost.py` reproduces every figure in the cost table ($168, $938, $5,845 default; $180, $947, $5,835 separated; $118, $756, $4,386 Solo) and the large-tier line items in lines 584 to 588 [measured, ran `python3 cloudflare/cost.py` and `--detail`].
- Per-script site isolation is real: a deploy reassigns versions only for that script's objects, and assignment is stable within a deployment (gradual-deployments with-durable-objects doc) [measured]. Finding 2 leaves the site boundary intact and only adds drain to it [inferred].
- Cross-script binding makes an accidental class deletion impossible in one deploy, because the tombstone is rejected while another Worker binds the class (migrations doc) [measured].
- The 2-of-3 bucket rule before the ack, plus inline packs up to 1 MiB, keeps every acknowledged pack on two regions through the 6-hour WEUR walk [inferred from lines 213 to 215 and 291 to 293].
- `ENAM`, `WEUR`, and `APAC` are valid exclusive container regions, so the default A, B, C sites can be pinned (placement doc) [measured].
- Request paths never call the vendor. Serving continues after the vendor's token is revoked, provided finding 6 moves the runtime crons to a customer-owned token [inferred].
