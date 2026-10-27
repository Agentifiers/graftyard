# Interrogate verdict on cloudflare/DESIGN.md

## Intent

> `cloudflare/DESIGN.md` builds Tributary only on Cloudflare technologies. Every chain is a Raft group of three SQLite-backed Durable Objects on three site scripts in three regions of one account, and a push is acknowledged once its entry is on two sites in two regions and its pack is in two R2 buckets. Uptime ranks first, zero lost acknowledged pushes second, and scale third. It keeps GHES 3.22 wire compatibility and ships managed, BYOC, and self-hosted from one artifact.

## Reviewers

Each seat was told the foundation was open to attack. All three seats run the same model (anthropic/claude-opus-5-5), so their agreement is weak evidence. Each seat label also wrote one arena candidate in a separate fresh agent: `pstack-panel-a` wrote candidate 1 (the flip target), `pstack-panel-b` candidate 2, and `pstack-panel-c` candidate 3 (the base). So the base's author walked self-hosted, and the flip target's author attacked the foundation. The cross-judge, the integrator, and the resolver all ran on `pstack-judge` (`review/trail-review-6.md` flag 4).

- Seat A, `pstack-panel-a`, attacked the foundation (`review/cloudflare-foundation-attack.md`). 12 findings: 1 blocker, 4 major, 7 minor. Verdict: holds with changes.
- Seat B, `pstack-panel-b`, walked a BYOC install and a managed cell as the vendor SRE (`review/cloudflare-byoc-walk.md`). 11 findings: 1 blocker, 8 major, 2 minor. Verdict: holds with changes, and managed needs a stated cell topology.
- Seat C, `pstack-panel-c`, walked a self-hosted install on three Linux hosts and a BYOC lifeboat (`review/cloudflare-selfhost-walk.md`). 12 findings: 1 blocker, 9 major, 2 minor. Verdict: holds with changes, and self-hosted on customer hardware is not real.

No seat recommended changing the base. Seat A tried and could not, because candidate 1 with its standby fix still acknowledges on one region's copy after any failover.

## Act on

Each item is resolved in `cloudflare/DESIGN.md` under "Review resolutions". The text here is the contract the revision follows.

**CF-ACT-1. Leadership leaves a site before the site hurts it.** Raised by A (F1, F2), B (2), and C (F6). A site deploy, a secret change, and a Workers runtime update each reset every member on the site, and a gray regional incident keeps a sick leader alive on one follower's ack. Each costs up to 10 s of no reads and writes per group led there, about one bad minute per site deploy and about 13 a month against a 4.3-minute read budget.
- Add Raft leadership transfer (`TimeoutNow` to the most caught-up follower).
- Add a signed `SiteMode { site, Drain | Normal }` entry in the `Release` group. Members on a drained site hand off leadership within one lease, refuse to campaign, and keep voting and storing.
- Every operation that creates a new site-script version (code, secret, variable, binding, container config) is a site-day operation: drain, deploy that script at 10%, 50%, then 100%, then undrain.
- A leader steps down by transfer when its commit error rate or storage confirm latency over 10 s passes a compiled threshold. The edge reports missed member deadlines to a follower on another site (candidate 1's `suspect`). A follower that gets reports from two colos within 10 s stops granting the leader promises.
- Pre-vote refuses a candidate on a drained site or one that reset in the last 5 minutes.
- `MemberState` persists `led_term`. A restarted member that led the current term resumes it after one AppendEntries round that re-establishes its lease. A replaced instance cannot append, because requests that touch storage on it are stopped.
- Each group has a preferred site (the owner's home). Transfer restores it after recovery.
- Milestone 0 adds two rows. A site deploy under 100 writes/s passes with zero failed writes and commit p99 at most 1 s. Injecting 30% errors on one site's member calls, and separately on its storage writes, passes if every group led there moves within 20 s and changes leader at most twice in an hour.
- The Summary, the regional rows in Downtime, and the "stalled leader" line state the gray-failure case. Each stays `mitigated` until those rows pass.

**CF-ACT-2. Operations run inside the account.** Raised by B (1, 6). No Worker can deploy or roll itself back, so "watchdog swap-back" names no actor, and a bad edge version at a 10% split survives a 41-hour API outage. Several runtime jobs need API tokens the design says it does not hold.
- A stable `trib-front` Worker forwards to the edge over a service binding and sets `Cloudflare-Workers-Version-Overrides` from a signed pointer in the `Release` group. Each edge deployment keeps N and N+1 uploaded, so swap-back is a signed entry with no API call.
- One stateless `trib-ops` script, with no member classes, executes signed intents. It deploys only versions uploaded beforehand whose release signature verifies. It owns every Cron Trigger that calls the Cloudflare API and every rotatable secret, held in Secrets Store. It uses a customer-created deploy token and a separate read token for R2 and Analytics.
- Container image changes ship on a different day from member code, with `--containers-rollout=none` on the code deploy.
- A failed or erroring verification is a no-op plus an alarm. Only a positive mismatch read twice 24 h apart starts any repair.
- A revocation runbook: revoke the vendor token, submit `Halt` signed by the customer key through the edge, then `Resume` or `Abort`, which `trib-ops` executes.
- A rotation runbook per secret, and custody of the installation-token signing key in `trib-ops`, wrapped by a Secrets Store key.
- The containment rows that say "watchdog" or "automatic swap-back", and the lines that say nothing at runtime reads API tokens, name these actors.

**CF-ACT-3. A `verify` refusal deposes the leader.** Raised by A (F3). Today a refused entry blocks every later entry in the group, so a bad release on site A is a write outage for every group A leads.
- When a follower refuses an entry, the leader truncates its uncommitted tail, transfers leadership to the refusing follower, and persists a flag that keeps it from leading until its release changes.
- A member truncates an undecodable uncommitted tail at boot.
- After undrain, the new site leads 1% of its groups for its first hour, then the rest.
- The next site rolls only after a full day with zero refusals.
- Row "record shape the previous release cannot read" becomes `contained`. It states the residual: once two sites run N+1, they can commit what N cannot read, and recovery is a roll forward.

**CF-ACT-4. One entry in flight per group, with group commit.** Raised by A (F4). Pipelined entries break `decide` and follower `verify`, and without pipelining the stated 300 entries/s cannot happen.
- State one entry in flight per group. Every command that arrives during a round goes into the next entry, up to 1.5 MB, and each command's `decide` sees the fold plus earlier commands in the same entry.
- Followers apply entry k before verifying k+1, which the append for k+1 makes possible because it carries commit index k.
- The Scale row becomes about 10 entries/s per group with many commands per entry. The Milestone 0 gate becomes at least 300 commands/s at commit p99 at most 150 ms.

**CF-ACT-5. The partial-failure arithmetic and the RTO are restated.** Raised by A (F5, F6).
- A push makes three member calls (token, `Authz`, commit). At 22% errors one attempt succeeds 45% of the time and three attempts 84%. Commit-path member calls are exempt from the 10% retry budget and retry at most twice each, which gives 96%. The June 2025 row states that this class breaks both monthly SLOs.
- Milestone 0's partial-failure row injects failures into storage writes, which reset objects, and counts whole pushes.
- A commit-path member call has a 1 s deadline, hedged to the other follower. Campaigns are request-driven through `suspect`, never alarm-driven.
- Write and read RTO are the same: at most about 11.5 s (promise remaining, the deadline, and two rounds). The RPO and RTO table's "reads at most 9 s" is corrected.

**CF-ACT-6. Downtime labels match shared layers, and placement is measured directly.** Raised by A (F7, F8).
- The runtime-cohort row becomes `mitigated`, because cohorts are by customer and the three sites share one account. The binding-path row becomes `mitigated`, because object routing is shared. The third-party storage row becomes `mitigated`.
- The account-separated option contains account faults to one voter, except the fourth account holding the zone and edge, and the text says so.
- The WAF-block gate is replaced by a written answer from Cloudflare on the internal route of a Durable Object call, requested before Milestone 1.
- Placement is checked by latency. At join and daily, each member takes the median of 20 binding round trips to each other voter, and `Placement::new` refuses a pair under 30 ms [guess threshold]. `/cdn-cgi/trace` is dropped. Jurisdictions stay where residency allows. The hint table lives in callers, so a wrong hint is an edge failure row, not a site one.

**CF-ACT-7. Membership changes and `ReplaceMember` are complete.** Raised by A (F9) and B (3).
- Under `Joint`, leases and read-index need promises from a majority of both configs.
- Pre-vote, and votes refused from non-members of the latest committed config.
- The "2-of-3 meanwhile" line is corrected. Sweeper lag is an alarmed metric, and `ReplaceMember` copies single-copy packs first.
- Every `Fresh` read touches SQLite after its lease check, with no in-isolate fold cache.
- `ReplaceMember` runs only on a signed `SiteLost` intent or after 24 h of zero acks from a site, never on elevated errors, which drain covers.
- Each install pre-creates one spare site script per continent at install (for example eeur, wnam, oc), with no members and no warm containers, so replacement needs no control-plane call and stays on the lost site's continent.
- Packs are copied once per network, not per group. `RestorePlacement` moves groups back to their home sites after recovery. Removed members run `deleteAll()` after 35 days.

**CF-ACT-8. Stale mode needs the signed flag.** Raised by A (F10). Automatic entry fires mostly during partitions, when a minority member would serve reads that miss acknowledged merges. Stale mode starts only by signed flag, and the text states that stale reads break read-your-writes across clients.

**CF-ACT-9. Git caches are shards with a stated count, and runners leave the account.** Raised by B (4, 5).
- `GitCache` becomes `GitCacheShard { site, shard }`, with networks mapped by rendezvous hashing, an LRU on disk, and a count per tier. "Per (network, site)" is deleted. The scheduling policy is stated.
- Milestone 0 measures the daily working set in bytes per tier, and `cost.py` derives the shard count from it.
- Hosted runners run in a separate runner account, in managed and in BYOC. Runners reach the Actions broker over HTTPS and need no member binding. Git cache vCPU is reserved per site through `max_instances`, sized so any two sites carry all networks. An alarm fires at 70% of account vCPU, and the quota request is on the day-0 list.

**CF-ACT-10. A Cloudflare-only vault survives the account.** Raised by B (7). One leaked or misused deploy token can delete all three sites, and no RPO row covers it.
- A vault account that the install's tokens cannot reach runs a Worker that pulls the change feed and packs over HTTPS with an org-signed export token, and writes them to R2 under bucket lock rules.
- The vault is required for managed and BYOC. The RPO table gets a row for account compromise or a destructive deploy, with RPO equal to vault lag.
- Deployment modes lists the deploy token's exact permissions. Milestone 0 checks that an account-owned token can deploy containers.

**CF-ACT-11. Managed cells have a stated topology.** Raised by B (8). Bindings cannot cross accounts, so one edge on `tributary.dev` cannot bind members in many cell accounts.
- A front account holds the zone, `trib-front`, the edge, and the global `Names` and `Tokens` groups on its own three sites. It calls each cell's edge Worker over signed HTTPS. Inside a cell, members replicate over bindings.
- The text states that a partial proxy failure on a cell hostname cuts that cell's client traffic, and that replication inside the cell continues [inference].
- One account for every managed user was considered and rejected, because it puts every managed user inside one account-scoped fault.

**CF-ACT-12. Push size and SSH meet the user.** Raised by B (9).
- Milestone 0 runs the push, chunked-push, and Spectrum-to-Tunnel rows on Pro and on Business as well as Enterprise.
- The edge counts streamed bytes and refuses at its own cap, below the plan limit, with a receive-pack `report-status` message that names SSH.
- The sweeper aborts orphan multipart uploads.
- Install preflight reads the zone plan and the largest pack per imported repository and refuses a plan whose cap is below it.
- If Spectrum cannot reach a Tunnel origin on Pro or Business, SSH needs Enterprise for BYOC, and the text says so.

**CF-ACT-13. `cost.py` prices what the customer is billed for.** Raised by B (10). Container egress is priced by each site's region and the leader split. The Spectrum add-on is an [unverified] line. Spare sites, `ReplaceMember`, the vault, and the runner account are priced. A note says pushes over 200 MB need an Enterprise contract on top of the table.

**CF-ACT-14. Install operations are written down.** Raised by B (11). A new "Install operations" subsection under Deployment modes covers:
- `tributary install` creating the `Release`, `Names`, and `Tokens` groups from a customer-signed genesis;
- `tributary admin bootstrap` minting a one-time sign-in link through `trib-ops`;
- custody of the vendor and customer intent keys;
- Terraform owning zone, DNS, buckets, queues, Spectrum, Tunnel, and WAF, and Wrangler alone owning the front, edge, site, and ops scripts;
- a failed colo or latency check retried with a new member generation, and a bucket-recreate path;
- an `oc` site listing `["OC", "APAC"]` for containers and not counting as a separate region.

**CF-ACT-15. Self-hosted means the customer's own Cloudflare account.** Raised by C (F1, F2, F3, F5, F12). The on-premises mode rests on `localDisk` storage that workerd marks experimental, a `localDocker` engine documented for development and testing only, one event-loop thread per host, and four services workerd does not have. `FRAME.md` defines self-hosted as the customer's own account, and the Cloudflare decision asks for the modes "as far as Cloudflare allows them".
- Self-hosted is the customer operating the same artifact in its own Cloudflare account with no vendor token, which is BYOC with intents signed only by the customer.
- workerd on customer hardware is supported only for evaluation (under the `evaluation` flag, like `Solo`) and for the lifeboat. The text states the reasons with citations.
- The Stack row for workerd reads "runtime GA; `localDisk` experimental; `localDocker` development only", used for the lifeboat and evaluation.
- The operator decides whether that reading stands. "Open questions and risks" asks one question covering every off-Cloudflare part: may Tributary run workerd plus Docker on non-Cloudflare hardware for (a) a read-only lifeboat, (b) evaluation, (c) production? The design's default answer is yes, yes, no.

**CF-ACT-16. The lifeboat is correct when it is needed.** Raised by C (F4, F7, F9, F10, F11) and A (F12).
- `trib-blob` writes a temp file, fsyncs it, links it to the final key (failing if the key exists), and fsyncs the directory before any 200. Reads verify `PackRef.checksum`, and a weekly scrub repairs from a sibling copy.
- The workerd version is pinned in the signed artifact. A workerd version change rebuilds the lifeboat on a fresh directory. `uniqueKey` and the key modifier are listed as backed-up state.
- Lifeboats report their decoder release, and `formats:enable` waits for every registered lifeboat. A lifeboat that meets an undecodable entry halts and alarms, and never skips.
- `tributary lifeboat enroll`, run by each developer while Cloudflare is up, writes the existing token into gh's hosts file and a git credential helper for the lifeboat host. The usage snippet shows it. Lifeboat reads are opt-in per client, and `actions/runner` and apps do not follow them.
- A BYOC lifeboat holds every `Tokens` and `Identity` shard. A managed lifeboat would accept only tokens minted for it, by org.
- The certificate comes from ACME DNS-01 against the zone, renewed 30 days before expiry.
- Lag is stated as "typically about 30 s; published". The lifeboat serves its cursor age in `/api/v3/meta` and an `X-Tributary-Lifeboat-Lag` header, alarms past 5 minutes, and pulls log tails from group leaders through the edge when the R2 feed is stale.
- The lifeboat is named as the only off-Cloudflare component in production. The second DNS provider suggestion is dropped. No managed lifeboat runs unless the operator decides otherwise.

**CF-ACT-17. A second flip condition.** Raised by A (F11). Before Milestone 1, ask Cloudflare in writing whether a Durable Object's storage followers span regions. If they span at least two, candidate 1 with its `Alone` fix and `suspect` becomes the base for single-continent (`Na3`-class) and cost-sensitive installs.

## Consider

- Seat C's `FailureDomain::Declared` placement and the full on-premises install text (F2, F3) are needed only if the operator answers "production" to CF-ACT-15's question. They are recorded in `review/cloudflare-selfhost-walk.md` as the work that answer would require.
- Seat C's per-host sizing for 2,000 developers on workerd (F5, about 15 TB of disk and several processes per host) has the same condition.

## Noted

- Seat A found the Durable Object a sound Raft acceptor: uniqueness is enforced on storage access, and the output gate holds replies until writes are confirmed. No seat disputed it.
- Seat B confirmed that per-script deploys reassign versions only for that script's objects, which is the basis of site-per-day rollouts.
- All three seats reproduced every figure in the cost table by running `cloudflare/cost.py`.

## Dismissed

No whole finding was dismissed. Each finding cites lines of `cloudflare/DESIGN.md` and a doc page, a research finding, or arithmetic, and each one checked out against the text. The first pass did soften or leave out seven proposed fixes without saying so, which trail review 6 caught. One of those, the semaphore Durable Object, is dismissed in "Follow-up after trail review 6".

## Agreement map

All three seats found that nothing moves leadership off a site (CF-ACT-1). A and C both raised the "only Cloudflare" scope of the lifeboat (CF-ACT-15, CF-ACT-16). Every other act-on item came from one seat. Seat B alone found the missing release executor, the cross-account managed topology, the vault gap, and the git cache cardinality. Seat A alone found the pipelining contradiction, the `verify` refusal deadlock, and the partial-failure arithmetic. Seat C alone found the workerd evidence and the lifeboat's format lag.

## Follow-up after trail review 6

`review/trail-review-6.md` flag 3 found seven reviewer fixes that the act-on list softened or left out, while "Dismissed" said none. Their disposition, applied in `cloudflare/DESIGN.md` ("Review resolutions", "After trail review 6"):

- Automatic drain on per-site SLO burn (A F1, B 2). Applied: `trib-ops` drains one site at a time with a drain-only key, reading error rates from members over bindings.
- The 1% leadership sample for the new site's whole first day, not its first hour (A F3). Applied.
- A whole-install restore procedure and a drill (C F8, BYOC half). Applied in "Install operations", with a Milestone 7 gate for vault and lifeboat-export restores.
- Disks that honor cache flushes for `trib-blob` (C F4). Applied for lifeboat hosts.
- A 1% first step in every site day (B 2). Applied.
- Container image storage counted against the 50 GB account limit (B 10). Applied in Scale.
- A semaphore Durable Object for `durable_object`-policy container classes (B 5). Dismissed, because every container class in the design uses the `default` policy with `max_instances`.

A re-walk of the revised design by a reviewer that wrote no candidate is in `review/cloudflare-rewalk.md`.

## Re-walk act-on items

The re-walk (`review/cloudflare-rewalk.md`, `reviewer` agent, which wrote no candidate or review) found CF-ACT-2, 3, 7, and 10 partial and raised 10 new findings: 1 blocker, 7 major, 2 minor. Each cites lines and a failure path, and each fix below is accepted as the re-walk wrote it unless noted.

**CR-ACT-1. A refusal deposes the leader only when the refuser is older.** (Finding 1, blocker.) The leader steps down only when the refusing follower runs an older release than the leader and the refused entry is uncommitted. On any other refusal (same or newer release, or a committed entry) the leader keeps leading, marks the refuser as unable to follow that group, alarms, and `trib-ops` halts the site day. A transfer target must be undrained and hold the leader's last index, and a transfer that has not completed within one `LEASE` is abandoned and the leader resumes. `no_lead_until` also clears when the `Release` group records a rollback of the refusing site. Milestone 0 adds an N+1 follower that refuses valid N entries at 10% of a site day, and the two-site residual, each passing only if no group lacks a leader for more than one lease. The text states that the two-site residual runs on two members with no spare voter until the third site rolls forward.

**CR-ACT-2. Keys live in the `Release` group, and the vendor holds no Workers permission.** (Finding 2.) Add `ReleaseEntry::Keys { customer, vendor: Option<KeyId>, release, drain }`, signed by the current customer key and by a new customer key when it changes, and refuse any entry signed by a key absent from the latest `Keys`. The revocation runbook signs `Keys` with `vendor: None` between `Halt` and `Resume` or `Abort`. The vendor writes signed bundles into one install bucket with an R2 token scoped to that bucket, and `trib-ops` uploads them with the customer's deploy token after the release signature verifies. The rotation runbook covers every key through `Keys`. Milestone 5's gate refuses a vendor-signed intent after `Keys { vendor: None }`.

**CR-ACT-3. `FormatsEnable` works without a vendor.** (Finding 3.) It takes the release-signing key, which every mode registers, plus the customer key. Releases that may enable a format carry it pre-signed with the release-signing key. The text says the release-signing key signs releases and the vendor key signs only vendor intents.

**CR-ACT-4. Spares are bound from install.** (Finding 4.) Every site script, the edge, and the Workflows script bind the member classes and buckets of all active, spare, and residency sites at install, and the compiled hint table covers the spares. Milestone 6's drill runs `ReplaceMember` with every account API token revoked and passes only if no script version is created.

**CR-ACT-5. The vault is install-scoped and restores to a chosen cutoff.** (Finding 5.) The vault export is signed by the customer key and covers every `ChainId`, including `Release`, `Names`, `Tokens`, `Identity`, and user-owned repositories. Managed tenants keep per-org tokens for their own exports. Restore step 1 names an operator-chosen cutoff minute, and step 2 uses the newest snapshot at or before it plus the tail up to it. The RPO row adds the compromise case. Milestone 7's restore gate checks a PAT, an SSH key, a user-owned repository, and an org name after the restore.

**CR-ACT-6. Site days gate on follower health and roll back without a human.** (Finding 6.) Each deploy step and the 1% day gate on N+1 members' apply lag, apply and verify exceptions, and ack rate against N members of the same site, plus zero `STEP_DOWN` trips on N+1 leaders. Each release intent carries a pre-signed rollback `SiteDay { site, version: N }`, which `trib-ops` runs on a gate breach. Milestone 0 adds a release whose followers throw on one event kind, stopped at 1% and rolled back with no failed write.

**CR-ACT-7. `trib-front` and `trib-ops` have release paths.** (Finding 7.) Add `Intent::FrontRoll` and `Intent::OpsRoll`, each run at 1% for a day and then 100% with N kept in the deployment, and state that a control-plane outage freezes a bad `trib-front` percentage. The edge calls `trib-ops` with a version override from a signed `OpsPointer`. The unwrapped signing key is cached per `trib-ops` isolate for at most an hour. Failure modes gain a row for token minting.

**CR-ACT-8. The lifeboat never sees a main-host token or holds a production DNS token.** (Finding 8.) `tributary lifeboat enroll` mints a per-developer `lifeboat:read` token that the main edge refuses, and the lifeboat stores only those hashes. `_acme-challenge` for the lifeboat host is delegated by CNAME to a validation zone in a separate account, whose DNS token is all the host holds. The lifeboat reads the export bucket with an R2 read-only token scoped to it. All three are in the rotation runbook, and the exposure text names them.

**CR-ACT-9. Automatic drain is measured by callers and can preempt a site day.** (Finding 9, minor.) Edges and members on the other two sites count missed deadlines per target site into a health object on their own site, and `trib-ops` drains when both report the burn. An automatic drain of a sick site preempts a site day by undraining the rolling site under the 1% lead limit first. A site day blocked an hour by an API failure undrains its site under the same limit. Milestone 0 adds a gray site during another site's day.

**CR-ACT-10. Lifeboats are registered and removable.** (Finding 10, minor.) `tributary lifeboat init` registers each lifeboat with its own key in a customer-signed entry, `LifeboatDecoder` is accepted only under that key, `LifeboatRemove { host }` is signed by the customer key, and an alarm fires when a registered lifeboat has not reported for 7 days.
