# Final pass: correctness and operations (FinalCorrectness, reviewer, read-only)

# Final adversarial pass: Graftyard base, SHARED.md, and agent-layer leases

I found 15 findings: 2 blockers, 11 major and 2 minor. All five seeds are real gaps (FP-7, FP-9, FP-10, FP-11, FP-14). None of these findings repeats an item that `review/cloudflare-interrogate.md` or `review/cloudflare-agents-interrogate.md` already disposed of. FP-6 is close to `review/final-security.md` F1 but is a different defect: F1 is about replaying signed entries, FP-6 is about rolling back below an enabled format, and F1's fix does not cover it. I checked the A1 numbers by running `python3 cloudflare/cost_agents.py --a1` and then deleted `cloudflare/__pycache__`. I edited no files.

**Seeds at a glance**

| Seed | Real gap? | Finding |
| --- | --- | --- |
| 1. Id allocation | Yes. There is no allocator, and managed mode cannot route URLs addressed by id | FP-9 |
| 2. 35-day restore vs 14-day GC | Yes. In practice the restore window is 14 days for force-pushes and branch deletions | FP-7 |
| 3. Restore intents and their key | Yes. Adding them to `Intent` as written would let the vendor sign them | FP-14 |
| 4. Stars, watches, notifications, feeds | Yes. They have no store at all | FP-11 |
| 5. `Tokens` and `Names` entry types | Yes. There are no entry types, and a suspended user's PAT can keep pushing | FP-10 |

---

## Blockers

**FP-1. Blocker. `verify` checks `expect` against the wrong fold, so a normal merge-queue group stalls the group permanently.**
- **Where:** cloudflare/DESIGN.md:218-220 and 233-244; design/SHARED.md:167-168 and 397.
- **Gap:**
  - The leader's `decide` sees "the fold at `applied` plus the earlier commands of the same entry".
  - Followers check that "every `expect` must equal the follower's own fold". The signature is `verify(e, fold: &FoldView)`, and that fold is at k.
  - SHARED says a queue group lands as "one entry [that] carries a `Merged` per pull request". Each `Merged` has `base: RefUpdate`, so PR 2's base update expects PR 1's result.
- **Evidence:**
  - A group of two PRs produces `Merged{1, main t0→g1}` and `Merged{2, main g1→g2}` in one entry. Each follower's fold has `main = t0`, so g1 ≠ t0 and both followers refuse.
  - Both refusers run the same release, so the rule is "the leader keeps leading, adds the refuser to `cannot_follow`". With both followers in `cannot_follow` there is no quorum.
  - With one entry in flight, nothing behind it commits. `cannot_follow` clears only "until their release changes".
  - No gate builds a multi-PR group. M2 tests only a stale `parent_tip`.
- **Fix:**
  - `verify` replays the entry's events in order on a scratch transaction over the fold at k, and checks each `expect` against that running state.
  - If both followers on the leader's release refuse, treat it as a leader defect: truncate the uncommitted tail and alarm.
  - Add an M2 or M0 row: one entry with a 5-PR queue group plus a `PrSync` batch commits on all three sites.

**FP-2. Blocker. `CALL_DEADLINE`, retries, `suspect` and `SiteHealth` treat admission queueing as a dead leader, and the two-colo rule can stop failover entirely.**
- **Where:** cloudflare/DESIGN.md:195-198, 209-215, 256-257, 573, 692, 890, 916-917.
- **Evidence:**
  - The edge-to-leader commit call is a commit-path member call. Journey 5 says "the edge's 1 s deadline misses, it hedges and reports `suspect`".
  - Bursts: 500 pushes against the 150/s agent bucket (0.5 of 300, from `cost_agents.py`) take 3.33 s to acknowledge. About (500−150)/500 = 70% of the commit calls miss the 1 s deadline.
  - Those misses come from many colos. Followers then stop granting promises, and the leader loses its lease, so commits stop for about 11.5 s. This happens again on every backlog.
  - Steady state at 5,000 agents: `AGENT_QUEUE_MAX` of 1,000 drains at 150/s in 6.7 s, so every queued call misses.
  - The edge gives up after 1 s × (1 + `COMMIT_RETRIES`) ≈ 3 s. The designed `ng … retry after` answer after `AGENT_WAIT` (30 s) can never be sent.
  - `SiteHealth` counts these misses against the healthy leader's site, which can trigger automatic drain.
  - The opposite failure: a follower stops promising and campaigns only after "reports from two colos", and "campaigns are request-driven through this call". A group whose clients all enter at one colo (a BYOC company in one city, CI in one cloud region) never elects after its leader's region dies.
  - The M0 RTO row (890) does not specify client colo diversity or hung calls (as opposed to failing calls). The burst and 5,000-agent rows count neither leader changes nor `suspect` reports.
- **Fix:**
  - Split the commit call. The leader answers within `CALL_DEADLINE` with `Admitted{ticket}` or `NotLeader`. The edge then waits for the outcome on a second call whose deadline is `AGENT_WAIT` plus one round.
  - Deadline, hedge, retries, `SiteHealth` and `suspect` apply only to the receipt and to member-to-member calls, never to queue time.
  - The two-colo threshold should only gate withdrawing promises from a leader that is still renewing. A follower whose promise has already lapsed campaigns on any single report, or on its own failed read-index call.
  - M0: the burst and 5,000-agent rows pass only with zero leader changes and zero `suspect` reports. Run the RTO row once from a single colo and once with blackholed calls.

---

## Major

**FP-3. Major. Leader-lease rules allow stale `Fresh` reads during an abandoned transfer and on a new leader. That would trip flip condition 1.**
- **Where:** cloudflare/DESIGN.md:178, 184-186, 202-207, 348-350, 1055.
- **Evidence (abandoned transfer):**
  - During a transfer the leader keeps sending AppendEntries to bring `to` up to date and to the third voter. Each ack is a promise that moves `lease_end` to send time + 8.5 s.
  - Voters "may disregard their promise … for a vote marked as a transfer", with no limit on when.
  - Suppose the third voter acks a round sent at t0+s and then votes for `to`, and the old leader then stops hearing from both. Dropped calls to one site are M0's own fault model.
  - The old leader abandons at t0+9 s and "resumes" with a lease valid until t0+s+8.5 s. It serves stale reads for up to 8.5 s while `to` commits.
  - The same happens if a leader restarts and resumes `led_term` while its earlier `TimeoutNow` is still in flight.
- **Evidence (new leader):**
  - `fresh` serves "from its fold while `now < lease_end − DRIFT`" but never requires an entry of the current term to be committed first.
  - Raft §8 requires the new leader to commit its no-op first (https://raft.github.io/raft.pdf).
  - Otherwise a new leader holding entry k with commit index k−1 serves a fold that is missing acknowledged entry k.
- **Consequence:** either case is "any linearizability violation" in the 48-hour run. That flips the base to candidate 1 over a specification omission.
- **Fix:**
  - AppendEntries carries `transfer: Option<(to, transfer_id)>`, and acks to rounds that carry a transfer count toward no lease.
  - A voter honours a transfer vote only if the last AppendEntries it accepted in that term named the same `transfer_id`.
  - Abandoning a transfer sends `transfer: None` and needs one full round in the leader's term before it serves `fresh`. Persist `transfer_id` in `MemberState`.
  - `fresh` and read-index answer only after the current term's `Noop` is committed and applied.

**FP-4. Major. `RESET_QUARANTINE` leaves no electable member after resets that hit all three sites together.**
- **Where:** cloudflare/DESIGN.md:191, 199, 541, 556, 617, 912.
- **Evidence:**
  - Pre-vote refuses any candidate that reset in the last 300 s.
  - The design itself says a runtime release "reaches all three at once" (617) and that failed storage writes "reset objects" (912).
  - When every member booted within the last 300 s, a group that needs an election cannot get one. That happens when the reset leader cannot resume, for example because both followers withdrew promises after `suspect` reports during its restart, and the design gives that withdrawal no end.
  - The group then has no leader until 300 s after the last reset. That is 26 times the claimed 11.5 s RTO.
  - Transfers to recently reset members are refused the same way.
  - Under June-2025-class storage errors (22%), busy members keep resetting, so the quarantine never clears.
  - No M0 row resets all three sites together.
- **Fix:**
  - Quarantine only members with two or more boots inside the window, or whose last leadership ended by `STEP_DOWN`.
  - A voter waives quarantine after it has seen no leader for 2×`ELECTION_AFTER`.
  - A promise withdrawn after `suspect` reports is restored after 2× the `suspect` window, or on a higher term.
  - Add an M0 row: every member on all three sites reset within 60 s under 100 writes/s, with two-colo `suspect` reports. Pass if every group commits within 12 s.

**FP-5. Major. Spare sites have no release path, so `FormatsEnable` and `ReplaceMember` contradict each other.**
- **Where:** cloudflare/DESIGN.md:107, 121, 136, 539, 599, 743, 934.
- **Evidence:**
  - A spare is a site script with its own `release` but "no members". Site days gate on member metrics (apply lag, ack rate, `STEP_DOWN`), which do not exist on a spare, and nothing rolls spares.
  - `formats:enable` "requires every site's … reported release", but only members report. Spares therefore either block every enable forever or get skipped.
  - If they are skipped: after several releases and one enable, a region is lost, and `ReplaceMember` creates members on a spare still running the install-time release.
  - Those members cannot decode committed entries, so they land in `cannot_follow`. The group stays 2-of-2 for the whole loss, which is the exact case `ReplaceMember` exists for.
  - Spare container classes also point at install-time images, and "Older images are deleted after each site day" (599).
  - The M6 drill runs on spares fresh from install, so it cannot see this skew.
- **Fix:**
  - Every release adds one site day per spare and residency site, gated by a canary group on that site that replays a corpus of current formats.
  - Keep each spare's image until that spare's own site day.
  - `FormatsEnable` requires every site, spares included (reported through the canary).
  - `ReplaceMember` refuses a spare that cannot read every enabled format.
  - Run the M6 drill after two release rolls and one `FormatsEnable`.

**FP-6. Major. Nothing refuses a rollback below an enabled format, so the pre-signed undo or `Abort` can take down every group.**
- **Where:** cloudflare/DESIGN.md:385-388, 743, 809; design/SHARED.md:555-558, 615, 620.
- **Evidence:**
  - SHARED rule 4 says "swap-back below the reading release is refused and pages", and the release registry has a `rollbackBarrier`.
  - The base never wires either in. Undo `SiteDay{site, N}` runs "with no human signature", and `Abort` "redeploys the previous uploaded version of each site".
  - After N+1 runs everywhere and `FormatsEnable` commits, N+1 writes the new format. If `Abort` (the revocation runbook's exit) or a late undo then puts a site back on N, its members cannot decode committed entries in their own logs. Truncate-at-boot covers only uncommitted entries.
  - Those members refuse everything. Once two sites are rolled back, no group has a quorum.
  - The M5 gate runs `Abort` before any enable.
- **Fix:**
  - The `Release` group's `decide` refuses `SiteDay`, `EdgePointer`, `OpsPointer` and `Abort` when the target release's signed record does not read every enabled format, and pages.
  - `Abort` means "go to the newest uploaded release that reads every enabled format".
  - The M5 gate adds: after `FormatsEnable`, both undo-to-N and `Abort` are refused.

**FP-7. Major (seed 2, real). The 35-day `RestoredTo` window does not survive the 14-day GC.**
- **Where:** design/SHARED.md:234, 568, 570-573; cloudflare/DESIGN.md:671, 699, 914.
- **Evidence:**
  - GC deletes objects "unreachable from every member chain's fresh fold for 14 days".
  - `RestoredTo`'s `decide` "checks that each target is reachable from live or cruft packs".
  - SHARED:573 hands pack retention to DESIGN's RPO row, which says only "`RestoredTo` from the log and archives". That covers log entries, not git objects.
  - Example: a force-push on day 0, then `repo-restore --to day −1` on day 20, is refused. In practice the window is 14 days for exactly the force-push and branch-delete cases it exists for.
  - `ResetTo` needs "the newest snapshot at or below `to`", and no snapshot retention is stated.
  - The install cannot read the vault.
  - The M0 hot-group row checks only that archived entries are readable.
- **Fix:**
  - GC roots are the fold's refs plus every ref value named in the last 35 days, kept in a `ref_log` fold table pruned at 35 days. `RestoredTo` reads the same table.
  - Cruft expires 14 days after an object leaves that set.
  - Keep at least one snapshot per tier at or below every index inside the window.
  - The vault deletes a pack no sooner than 35 days after the install's GC deleted it.
  - Add an M0 row: delete a branch and force-push on day 1, restore to day 0 on day 30, and check that the objects come back.

**FP-8. Major. The lease and Actions-lease "outage extension" cannot tell an outage from an idle gap.**
- **Where:** cloudflare/DESIGN.md:326-331, 732, 902; cloudflare/AGENT-LAYER.md:110-114, 185, 274, 590; design/SHARED.md:483-484.
- **Evidence:**
  - The rule is that "the first entry after the group could not commit extends every lease … by the gap".
  - Idle groups write nothing (the M0 idle-cost row requires zero billed duration), and a new leader never saw the attempts that failed. So the gap can only be taken as `at_k − at_{k−1}`.
  - That makes every live lease immortal, because `expires − at` stays constant. A 600 s lease on a repository idle for 20 minutes still answers a conflicting Exclusive acquire with 409.
  - Actions job leases are rewritten only when "less than 5 minutes remain". A dead runner's job then never reaches `LeaseExpired`, and required checks never conclude.
  - "Checked on read" has no entry `at` to compare with. Using the last entry's `at` means idle leases never lapse. Using the leader's clock means a read reports expired (and `LeaseChange::Expired` has no entry that produces it), and a later extension revives the lease.
  - The M0 lease row passes an implementation with immortal leases.
- **Fix:**
  - Extend only by a recorded stall window. It starts at the earliest durable marker: a follower's persisted `suspect` time, a candidate's campaign start, or the leader's persisted `stalled_since` when a command waited past the deadline. It ends at the next committed entry, which carries an explicit `ExtendLeases{by}`.
  - Idle time never counts.
  - Reads compare against the leader's leased clock. The feed emits `Expired` at the first entry with `at ≥ expires`.
  - Add rows: an idle 60 s lease, then an overlapping Exclusive acquire 10 minutes later succeeds; a dead runner's job fails at 10 minutes.

**FP-9. Major (seed 1, real). No id allocator, and managed mode cannot route URLs addressed by id.**
- **Where:** design/SHARED.md:117, 138, 274, 312; cloudflare/DESIGN.md:109, 169, 622.
- **Evidence:**
  - Ids must be `i64` with 0 < id < 2^53 and must never repeat.
  - The AWS source used `(seq << 16) | slot` from per-slot `IdBlock` items (root DESIGN.md:252-253). The extraction dropped slots, and the base names no allocator for account, repo, comment, check-run, run, job, hook, installation or App ids. Yet `RepoName` and signup already need an id before `decide`.
  - Managed routing: the front edge resolves a cell "from `Names`" by login. But pagination `link` headers use `/repositories/{id}/…`, renames return 301 to that path, node ids start with the repo id, and `POST /app/installations/{id}/access_tokens` carries no login.
  - `octokit.paginate` follows those links, so page 2 of every list fails in managed.
  - The "alarm at 50%" (622) has nothing to measure.
- **Fix:**
  - `DbId` = cell (8 bits) | block (29) | offset (16).
  - A per-cell `Ids` group leases blocks to group leaders. Each group commits `IdBlock{base,len}` in its own log before using it, so a new leader resumes from the next free id. Gaps are fine; reuse never happens.
  - Issue numbers stay as per-repository fold counters.
  - The front routes `/repositories/{id}`, `/user/{id}`, node ids and installation ids by the cell bits. Installation → owner comes from FP-10.

**FP-10. Major (seed 5, real). `Tokens` and `Names` have no entry types, and suspension has no home on the write path.**
- **Where:** cloudflare/DESIGN.md:169-170, 176-178, 446-447, 810; design/SHARED.md:164, 194, 198, 239-246, 326; root DESIGN.md:371-376.
- **Evidence:**
  - `EntryBody::Events` takes `Event = Repo | Network | Authz | Identity`. SHARED:198 hands `Tokens`, `Names` and `Actions` entries to the base, which defines only `ReleaseEntry`.
  - `NameShard` is never defined.
  - The AWS `global` table's key kinds no longer have a home: Email uniqueness, the App registry, installation → owner, `Account{suspended, sessions_not_before}`, verifying keys, retired names and former names. The rotation runbook puts install keys "in `Identity`", but no account's `Identity` holds install keys.
  - Suspension: SHARED:326 says it is an "account fact[] that the edge checks". SHARED:194 says `Identity` "never gates a request". The write path reads only "the token and the owner's Authz". So a suspended user's PAT keeps pushing.
  - Creating a PAT or SSH key spans `Identity` and `Tokens` with no commit point or finisher.
- **Fix:**
  - Add `NamesEvent`: `LoginClaimed`, `Account{suspended, sessions_not_before}` keyed by account id, `FormerName`, `Retired`, `Shortcode`, `EmailClaimed`, `AppRegistered`, `InstallationRegistered{id, app, owner}`.
  - Add `TokensEvent`: `Stored{hash, kind, account, scopes, expires, sso_orgs}`, `Revoked`, `SshKey`/`SshKeyRemoved`, `JtiRevoked`, `LifeboatRead`, `VerifyingKey`.
  - Add an `ActionsEvent` built from `JobEvent`, and define `name_shard` and `account_shard`.
  - Write-class requests also read the actor's `Account` row through `fresh`.
  - Credential creation commits first on its `Tokens` shard, which is also the uniqueness point for SSH fingerprints. The `Audit` finisher then completes the `Identity` side.

**FP-11. Major (seed 4, real). Stars, watches, follows, notifications and feeds have no store.**
- **Where:** design/SHARED.md:32, 408, 444, 547; cloudflare/DESIGN.md:88, 93; FRAME.md:21.
- **Evidence:**
  - SHARED keeps them as "per-account records kept off the repository chain". The AWS source held them in DynamoDB work tables (root DESIGN.md:285-286).
  - The base has no store outside chains (KV and D1 are unused), only a notification Queue.
  - Repository objects in T0 `rest:repos` include them: GHES 3.22 docs say "`watchers`, `watchers_count`, and `stargazers_count` correspond to the number of users that have starred a repository" (https://docs.github.com/en/enterprise-server@3.22/rest/activity/watching).
  - Notification read and done state is data the user writes and must survive.
  - FRAME's public managed service implies stars and follows.
- **Fix:**
  - Records a user writes (stars, watches and subscriptions, follows, notification thread state) become `IdentityEvent`s on the actor's group. They gate nothing, so they fit `Identity`'s rule.
  - A consumer writes coalesced count events, in the bot class, to each repository's metadata tier.
  - Notifications, feeds, `/issues` and `/user/repos` live in a derived per-account `Inbox` Durable Object, rebuilt from cursors and merged with `Identity` read state when read. State the lag as a known deviation, and add it to `cost.py`.

**FP-12. Major. How `trib-front` reads the pointer is unspecified, and Cloudflare ignores an override silently.**
- **Where:** cloudflare/DESIGN.md:79, 449, 495, 557, 560-561, 740; cloudflare/AGENT-LAYER.md:237-238.
- **Evidence:**
  - Every request runs `trib-front`, which forwards it "to the edge version named by the `Release` group's `EdgePointer`". There is one group per install, and no read mode, cache bound or fail-static rule is given; only "members cache the pointer".
  - A `fresh` read per request would put all traffic on one Durable Object (1,000 req/s soft limit) and turn its 11.5 s failover into an install-wide outage.
  - Cloudflare's version-overrides page says two things the design does not (https://developers.cloudflare.com/workers/versions-and-deployments/version-overrides/index.md):
    - "A version override will only be applied if the specified version is in the current deployment … routed according to the percentages."
    - "Version overrides only apply to `fetch()`-based service binding calls … no way … via RPC."
  - The design cites that page only for the "0% of traffic" point.
  - So minting through RPC ignores `OpsPointer`, and an `EdgePointer` to a version no longer in the deployment silently does nothing. Neither failure is detected.
  - In managed, nobody is named as reading each cell's `EdgePointer`.
- **Fix:**
  - Cache pointers per isolate with a compiled TTL. Refresh from any member's applied `Release` state (pointers only move forward by index), fail static to the last value, and never block a request on the group.
  - Make every pinned call a `fetch()`-based binding.
  - The edge reports the version it ran via the version-metadata binding, and `trib-front` alarms on a mismatch.
  - Refuse an `EdgeRoll` that would drop a version an undo pointer still names.
  - The managed front reads each cell's pointers over signed HTTPS.

**FP-13. Major. "A revocation binds every write acknowledged after it" is false while commands wait in admission.**
- **Where:** cloudflare/DESIGN.md:256-257, 446-447, 523-528, 586; design/SHARED.md:152; cloudflare/AGENT-LAYER.md:307.
- **Evidence:**
  - The token or `jti` is read at the edge before the command queues, and it is not a `Command` dependency.
  - Agent commands wait up to `AGENT_WAIT` of 30 s (6.7 s in steady state at 5,000 agents).
  - So after `DELETE /installation/token` returns 204 (the agent-layer review's fix AG-ACT-9, revoking a misbehaving agent's token), that agent's already-queued pushes still commit and are acknowledged up to 30 s later.
  - When the `Authz` dependency is read is also unstated. The per-entry estimate of 1/(5+80+5 ms) includes no dependency hop.
- **Fix:**
  - Restate the guarantee as "binds every command decided after it".
  - The leader re-reads the credential's shard for any command that waited longer than `READ_CACHE`.
  - Read each distinct `Authz` dependency once per entry, after the entry's command set is frozen, and add that hop to the entries-per-second math.
  - The M3 gate adds: revoke while 100 pushes are queued, and none is acknowledged at an index after the revocation.

---

## Minor

**FP-14. Minor (seed 3, real). `RepoRestore` and `RepoReset` are missing, and if added as an `Intent` the vendor could sign them.**
- **Where:** design/SHARED.md:568-572; cloudflare/DESIGN.md:362-366, 380-384, 804.
- **Evidence:**
  - SHARED says these are "signed admin intents that only the customer can sign" and hands the keys to the base. The base's `Intent` enum lacks them.
  - The base's rule is "Intents take the vendor or customer intent key", so adding them there makes them signable by the vendor.
  - Also unspecified:
    - how the repository group authenticates a signature that lives on `Release`;
    - how a time `t` maps to an index;
    - idempotence;
    - who runs restores in managed.
- **Fix:**
  - Add `ReleaseEntry::RepoRestore{repos, refs, at, reason}` and `RepoReset{repo, at, reason}` to the customer-key list.
  - `trib-ops` commits `RestoredTo` or `ResetTo` as a command whose dependencies include `ChainId::Release`. Its `decide` checks the `Release` index and the repository, and that index is the request id.
  - `to` is the last index with `at ≤ t`.
  - In managed, the cell's customer key belongs to the operator, so a restore is a support action.

**FP-15. Minor. Archive and snapshot keys are chain positions, with no committed record before followers truncate.**
- **Where:** cloudflare/DESIGN.md:270-272, 397-399, 587; design/SHARED.md:236; review/interrogate.md:206 (FD-ACT-8).
- **Evidence:**
  - SHARED requires keys that are a ULID or digest "named by an entry, never a chain position". The base uses `Archive{chain, from}` and `Snapshot{chain, tier, upto}`, and no `EntryBody` names an archive.
  - Race: a deposed leader's in-flight put of `Archive{from: i}` covering [i, j) lands. The new leader's put for [i, j′) gets 412 from `If-None-Match`. If 412 is treated as "already archived", followers truncate [j, j′), which no archive holds.
  - On A1 the log is 71 GB per 30 days against the 2 GB cap, so less than a day of entries stays live. The lost range is therefore inside the 35-day restore window and the rebuild-from-genesis path.
- **Fix:**
  - Key archives and snapshots by (chain, from, to, digest).
  - Commit an `Archived{key, from, to, digest}` record. Followers truncate only through a committed range they can HEAD in two buckets.
  - Snapshots also go to two buckets.