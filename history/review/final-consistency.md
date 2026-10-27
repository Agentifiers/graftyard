# Final pass: consistency (FinalConsistency, reviewer, read-only)

I found 13 problems: 7 major, 6 minor, and no blockers. The worst ones would push implementers into broken code: the agent-layer rows that `decide` depends on have no fold tier, the 8 GB child-group trigger fires when a busy repository's storage is already full, and the types the agent layer adds were never added to the type definitions themselves.

**What matches.** I ran all four script commands and checked every cost, rate and size figure against their output. Every figure in `cloudflare/DESIGN.md`, `cloudflare/AGENT-LAYER.md` and `FRAME.md` matches the scripts. The only differences are roundings: 10.2 for 10.19 vCPU, "about 3,000" for 2,975 agents, and 301 for 300.8 vCPU. The agent block in Shape is byte-identical in both documents. These operator decisions are reflected correctly: all four earlier Cloudflare defaults, and agent-layer answers 2, 3, 5, 6 and 7. Answers 1 and 4 are not (G-6 and G-9).

**Cleanup.** `cloudflare/__pycache__` did not exist when I finished. I deleted `scripts/__pycache__`, whose timestamp (17:01:26) matches my first script run; Python recreates it on the next run.

---

**G-1. Major. The agent rows that `decide` reads have no tier, and the only tier stated breaks the core-tier rule.**
- **Where:** `cloudflare/AGENT-LAYER.md:373`; `cloudflare/cost_agents.py:440-445`; `cloudflare/DESIGN.md:289, 299, 304-307, 342-344, 587`; `design/SHARED.md:415-421, 431`.
- **Gap:**
  - Push and merge checks need four kinds of rows: `AgentTask` ("a Tributary-run push with no live `AgentTask`", DESIGN:289), `Lease` (the `AgentsExclusive` refusal, :342-344), `AgentGate` (:304-307) and `AgentPolicy`.
  - SHARED says the core tier "alone gates fetches, pushes, merges, and authorization" and lists none of these rows (:415-421, :431).
  - The layer's Scale row puts task rows in the "Metadata tier, 8 GB child-group threshold" (:373), and the script counts "PRs, provenance, and tasks" as metadata growth (:443).
- **Why it breaks:** above 8 GB "the metadata tier moves to a child group" (DESIGN:587). Operator decision 4 makes child groups the precondition for A1-sized fleets.
  - After that move, the live-task check either refuses every Tributary-run push or needs a read of another group on every push.
  - "Cancelling or finishing a task releases its leases in the same command" (:299) would then span two groups, which one entry cannot do.
- **Fix:**
  - Split `AgentTask` into a small core part (`session`, `kind`, `state`, `prefix`, `requester`, `app`, about 100 B) and a metadata part (`prompt`, `summary`, `pr`).
  - Put `Lease`, `AgentGate`, `AgentPolicy` and `AgentMeta`'s App flag in the core tier. Keep `PathSet`, provenance and the feed ring in metadata.
  - Add these rows to `tier_of` and to SHARED's core-tier list, then update AGENT-LAYER:373 and the script's tier-growth term.

**G-2. Major. The 8 GB child-group threshold is defined two ways, and the reading the numbers use fires at the 10 GB `SQLITE_FULL` limit.**
- **Where:** `cloudflare/DESIGN.md:587, 597, 914`; `cloudflare/AGENT-LAYER.md:367, 634`; `cloudflare/cost_agents.py:132, 445`.
- **The two readings:**
  - Milestone 0 (DESIGN:914) treats 8 GB as member SQLite: it passes "under 8 GB" and fails "over 8 GB without a child group taking the tier".
  - The script (`CHILD_GROUP_GB = 8  # metadata tier size`), DESIGN:597 and AGENT-LAYER:367 and :634 treat 8 GB as the size of the metadata tier.
- **Evidence** (my arithmetic from the `--a1` terms: 4 GB tier, 2.0 GB log, 1.18 GB of rows at 30 days, 1.2 GB steady check rows, 0.77 GB a month of growth):
  - **Tier reading:** the tier reaches 8 GB at (8 − 4 − 1.2) / 0.77 = 3.6 months. The member then holds 8 + 2.0 + 0.01 ≈ 10.0 GB, so the move starts at the write-failure limit. AGENT-LAYER:634 says the same: "about 10 GB with a full log, the `SQLITE_FULL` limit".
  - **Member reading:** 6.01 + 1.17t = 8 gives about 1.7 months, not "about 4".
- **Prior disposition:** `review/cloudflare-agents-interrogate.md:111` (trail review 7, flag 2) settled this with "wait for child groups". That holds only if the Milestone 7 trigger fires below the cap, so the disposition is incomplete.
- **Fix:** define one constant in base Shape, measured on member SQLite bytes, e.g. `CHILD_GROUP_AT = 8 GiB` member bytes, which caps the tier at 10 GB − `LOG_MAX_BYTES` − core − margin. Use it in `cost_agents.py` and restate the month count in DESIGN:597 and AGENT-LAYER:367 and :634.

**G-3. Major. Types the agent layer extends are not extended where they are defined, and DESIGN:463-469 still says they are reused "without change".**
- **Missing variants and fields:**
  - SHARED's `RepoEvent` (SHARED:165-183) has no assignment or issue-edit variant. The design needs both: the "`Assigned` entry" (DESIGN:293, AGENT-LAYER:313) and pausing a task on a non-writer's title or body edit (AGENT-LAYER:242).
  - `LeaseAcquire`, `LeaseRenew` and `LeaseRelease` (DESIGN:327-328) are named but never defined: no TTL request of 60 to 3,600 s, no scope, mode, intent or task.
  - There are no events for creating, changing or cancelling an `AgentTask`, or for writing `AgentGate`, `AgentPolicy` or `PathSet`.
  - `AuthzEvent` lacks `AgentApp`, which AGENT-LAYER:131 calls "an `Authz` entry".
  - `IdentityEvent` lacks `McpGrant` rows (AGENT-LAYER:213-215).
  - `InstallationClaims` (SHARED:289-290) has no `task` claim, though DESIGN:293 and 297-298 rely on one.
  - `Push` (SHARED:211-212) has no `provenance` or `protected` field, and the Usage call site (DESIGN:61) builds `Push` without them.
  - `Consumer` (SHARED:531) has no insights consumer.
  - The base's `ReleaseEntry` and `Intent` (DESIGN:367-384) lack `McpPointer`, `ControllerPointer`, `McpRoll` and `ControllerRoll`, which AGENT-LAYER:235-236 says the base gains.
- **Fix:** add one "Agent-layer entries" block to base Shape (the base wins over SHARED) that defines these variants and fields, each with its `tier_of`. Retitle DESIGN:463 "Reused with the additions below", and point SHARED's blocks at it.

**G-4. Major. Lease-holder identity is undefined and collapses for third-party fleets.**
- **Where:** `cloudflare/DESIGN.md:298, 335, 337, 340, 343`; `cloudflare/AGENT-LAYER.md:274, 290, 371`; `cloudflare/cost_agents.py:67, 88`.
- **Gap:**
  - `LeaseHolder { actor: ActorId, app, task }` uses an `ActorId` type that is defined nowhere.
  - Every rule keys on "holder": `LEASES_PER_HOLDER` = 8 per repository, "Shared conflicts only with another holder's Exclusive lease", `AgentsExclusive` refuses overlap with "another holder's" lease, and "only the holder renews or releases".
  - An external task gets "no prefix and no token" (DESIGN:298), so a third-party App's installation token carries no task.
- **Impact:** A1 is half third-party agents pushing in-repo (`TRIB_RUN_SHARE = 0.5`), with "500 live leases, one per agent". If those agents share one App, they are one holder:
  - The cap of 8 refuses the rest.
  - They never conflict with each other, so `AgentsExclusive` never refuses them.
  - Any one of them can release another's lease.
- **Fix:**
  - Define the holder as (actor, App, session). The session comes from the task claim, or for third-party Apps from `X-Tributary-Agent-Session` bound to an external task the same App created.
  - Require a session on acquire from App tokens. Add a per-App cap below `LEASES_PER_REPO`, and define `ActorId` in terms of SHARED's `Actor`.

**G-5. Major. The lease outage rule has no trigger the group can observe.**
- **Where:** `cloudflare/DESIGN.md:331`; `cloudflare/AGENT-LAYER.md:274, 454-455, 590`.
- **The rule:** "The first entry after the group could not commit extends every lease that was live when commits stopped by the gap".
- **Why it can't work:**
  - The log only gets a `Noop` "first entry of every term" (DESIGN:178).
  - Idle groups must bill zero duration (Milestone 0, DESIGN:902), so there are no periodic entries.
  - The gap between two entries therefore cannot tell an outage from idle time. Taken literally, every quiet gap extends abandoned leases, so they may never lapse on quiet repositories.
  - The Actions precedent it cites (DESIGN:732, "Job leases extend by the outage length on recovery") states no mechanism either, and there expiry is a command (`LeaseExpired`), not an entry-time comparison.
  - The Milestone 0 row (AGENT-LAYER:590) measures "the outage length" from the test harness, not from anything the system records.
- **Fix:** record the outage explicitly.
  - A member that fails a commit-path call persists `stalled_since`. The first entry the group commits afterward carries `Resumed { since }`, which extends only leases with `expires > since`, by `at − since`.
  - For provider-wide outages that no member observed, `trib-ops` commits a signed `OutageCredit { from, to }` from probe history.
  - Alternatively, drop the extension and give holders a renewal grace of `LEASE_TTL / 3` after `Resumed`.

**G-6. Major. Tasks created through the API have no route to the controller, and operator decision 1's "off until an org enables it" appears nowhere.**
- **Where:** `cloudflare/AGENT-LAYER.md:240, 276, 321, 392, 495-503, 623`; `FRAME.md:54, 66`.
- **Gap:**
  - `POST .../tasks` lets a writer create a Tributary-run task from a prompt alone (:276).
  - The controller is "a GitHub App backend reached by webhooks over HTTPS" (:240), and the first release has "no new webhook event" (:392, :623).
  - So nothing tells the controller about Bob's prompt-only task (A3 step 10), or about a cancel ("the controller stops the Sandbox at its next step").
  - The answer "offered is fine" keeps the default "offered, off until an org enables it" (FRAME:54, :66). Neither document states that default. The "Enable agents" checklist has no step that turns Tributary-run agents on for an org or installs `tributary-agent`, and `POST` checks no enable flag.
- **Fix:**
  - Deliver task creation and cancel to the controller from the event pipeline as a first-party-only delivery to its webhook hostname, and state that GHES clients never see it.
  - Add an org setting, off by default, that `POST` checks (422 when off). Add a checklist step and a sentence for managed mode.

**G-7. Major. `AgentTask` has no App field and requires a user requester, so external tasks and the App-scoped endpoints cannot be authorized.**
- **Where:** `cloudflare/DESIGN.md:293-294, 298, 300-302`; `cloudflare/AGENT-LAYER.md:276`.
- **Gap:**
  - The API lists tasks by `app`, lets "the task's App" cancel, and lets "the task's own App" `PATCH` `state`, `pr` and `summary`. The row has no `app` field.
  - `requester: UserId` is mandatory, but an `AgentApp` creates an external task with an installation token, where no user exists. `AgentActor.requester` is optional, which contradicts the row.
- **Fix:** add `app: AppId` (the controller's App for Tributary-run tasks, the creating App for external ones). Make the row's `requester` an `Option<UserId>`, and state who may `PATCH` external tasks.

**G-8. Minor. AWS.md's "with the agent layer" totals predate leases and tasks.**
- **Where:** `alternatives/AWS.md:54, 70`.
- **Evidence:** the row shows $281, $1,521 and $12,146. Those are the base plus the old added totals of $17, $225 and $2,767. `cost_agents.py` now adds $26, $293 and $3,217, and the row's own source has $289, $1,588 and $12,597 (`research/costs.md:186-188`).
- **Effect:** at 2,000 developers the corrected $12,597 is above AWS one-Region's $12,432, which is priced without agents. The stale row implies the opposite order.
- **Fix:** paste the costs.md row and note that the AWS figures include no agent layer.

**G-9. Minor. Statements made stale by the operator's answers and by the leases-and-tasks change.**
- **Where:** `cloudflare/AGENT-LAYER.md:634, 477, 690`.
- **The three stale statements:**
  - **:634.** "Whether to pull child groups earlier is an operator decision". The operator kept the default (FRAME:57, :66).
  - **:477.** "No new component sits on a base commit or read path … the fold additions ride commands the base already writes". Lease and task commands are new, the `AgentsExclusive` check runs inside the push commit path, and held feed polls sit on members.
  - **:690.** The AG-ACT-17 entry still says `planned` comes "from a `tributary-plan` block". The leases change replaced that: `planned` is read from leases (:152-153, :274).
- **Fix:** rewrite :634 as "The operator kept the default (FRAME.md, answer 4): child groups stay at Milestone 7." Restate :477 to cover the new commands and polls, and add "later superseded by LT-ACT-1" to :690.

**G-10. Minor. Two of the feed's lease events contradict the lease rules.**
- **Where:** `cloudflare/AGENT-LAYER.md:178-189, 274, 590`.
- **Gap:**
  - `LeaseChange::Rescoped` is "a renewal that changes scope or mode", but renew "moves `expires` and changes nothing else". The leases contract (`review/cloudflare-leases-tasks.md`) says both, in LT-ACT-1 and LT-ACT-2.
  - `Expired` has no entry behind it ("expiry needs no command"), yet the feed is a fold over the log with a cursor of (group, index). Nothing says when `Expired` is emitted. On a quiet group no later entry exists, so the "each event within 2 s at p99" row (:590) cannot hold for expiries.
- **Fix:** either drop `Rescoped` or define a rescope command with its own conflict check. Specify that `Expired` is emitted by the first entry whose `at` passes `expires`, and tell clients to time expiry themselves from `expires`.

**G-11. Minor. `max_instances` and reserved vCPU leave out the hot pool that Shape includes.**
- **Where:** `cloudflare/DESIGN.md:418, 423-425, 592, 595, 598, 842-846`; `cloudflare/cost.py:125-128`; `cloudflare/cost_agents.py:413-414`.
- **The two versions:**
  - **Shape (DESIGN:418):** "Per site, `max_instances` is the shard count plus the hot pool", with `HOT_POOL = 7`.
  - **Everything else:** `cost.py` sets `max_instances_per_site = n`, i.e. 25. The Cost table says 300.8, and the Scale row says "301 vCPU … 5x".
- **Contradiction:** DESIGN:595 and `--a1` give 384.8 vCPU, which is 3.9x headroom against 1,500, not 5x.
- **Fix:** add the hot pool to `shards()` and the Cost table, restate the DESIGN:592 headroom as 3.9x, and say which instance size the pool uses at 50 and 300 developers.

**G-12. Minor. SHARED keeps the AWS-era rule for unknown events, which contradicts the base's follower check.**
- **Where:** `design/SHARED.md:200, 560`; `cloudflare/DESIGN.md:234-236, 562, 1080`.
- **The two rules:**
  - **SHARED:** "an older release skips an unknown metadata event". The rule comes from AWS-era FD-ACT-12 (`review/interrogate.md:214`).
  - **Base:** a follower's check refuses any entry that does not "decode … use an enabled format", and that refusal deposes the leader (DESIGN:234-236, :562). The lifeboat also halts on undecodable entries (:1080).
- **Fix:** make SHARED say that in Graftyard an entry in a format not yet enabled is refused, and that the tier tag only lets the core tier apply an entry without decoding its metadata body.

**G-13. Minor. Types used in public contracts are undefined.**
- **The types:**
  - `Step` and `Harness` (`cloudflare/AGENT-LAYER.md:268-269`) belong to the versioned public `SessionLog` JSON (`schema: 1`, served by `GET .../tasks/{id}/log` and session pages).
  - `PrAction` (AGENT-LAYER:184) is in the feed JSON.
  - `PathGlob` and `RefPattern` have no glob dialect or overlap rule. Lease conflicts depend on overlap (DESIGN:336-337, :343), and `PROTECTED_PATHS` mixes `**/AGENTS.md` with bare `CODEOWNERS` (DESIGN:309).
  - `Health` (DESIGN:197) is used by `STEP_DOWN` but never defined.
- **Fix:** define each type. State the glob dialect (gitignore-style, anchored unless it starts with `**/`) and how overlap between two globs is decided.