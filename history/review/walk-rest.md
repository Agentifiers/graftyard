# Implementability walk: Milestone 4, Milestone 7, AM4 (WalkRest, poteto-agent, read-only)

I walked 15 work packages across Milestone 4, AM4, and Milestone 7, and found 15 places where I would have had to guess. One is a blocker. The other 14 are major. Every quote below was read in the design files this session. I mark my own reasoning with [inferred].

**Seeded question.** The design does not state a limit on how many requests one Durable Object can hold open, does not cite one, and does not say what happens past it. AGENT-LAYER "Scale" gives the feed's limit as "1,000 req/s soft per object; `FEED_POLLS` 2 per token and 50 per IP". That is a request rate plus a per-client cap, not a per-member cap on held requests. Cloudflare's troubleshooting page (read this session) lists overload only for requests still waiting to be delivered and for "Too many requests for the same object within a 10 second window". It names no cap on held requests. Finding W5 covers the consequences.

## Findings, deepest first

**W1. Blocker. M7-WP4.**
- **Where.** `design/DESIGN.md`, Shape, "Chains" (the `CHILD_GROUP_AT` comment and `ChainId`); "Scale", Member storage; `design/AGENT-LAYER.md`, the feed block.
- **Gap.** The only text is "Member SQLite bytes at which a repository's metadata tier moves to a child group (Milestone 7)". `ChainId` has no child variant. Nothing names the commit point of the move, or how the edge routes a metadata command afterward.
- **Also missing.** Nothing says what happens to an entry that spans tiers, such as `Release`, which is "Metadata tier, plus Core when `tag` is set". It does not say how consumers keep "chain order" across two chains, or what `RestoredTo` and `ResetTo` cover after the split. The feed text says a cursor gets 410 "after ... a move to a child group", which suggests the log itself moves.
- **Smallest addition.** A "Child groups" subsection with these parts:
  - a `ChainId::RepoMeta(RepoId)` variant;
  - a core `MetaSplit { child, at }` entry as the commit point;
  - the child group starting from the metadata snapshot at `at`;
  - the parent refusing metadata commands after the split;
  - core-first commit for commands that touch both tiers, with a named finisher for the metadata half;
  - consumer ordering across the split index, and the scope of repair commands.

**W2. Major. M4-WP3, M4-WP5.**
- **Where.** DESIGN "Failure modes", the row "actions state | instance | Long polls on one broker object"; the Stack row for Durable Objects; SHARED "Actions", Logs.
- **Gap.** The Stack row lists group members, git cache shards, and `Inbox`. It lists no broker class. SHARED says the broker keeps "an in-memory table ... per runner scope". Logs say "Live logs stream over the `FeedStreamUrl` WebSocket", but DESIGN never mentions WebSockets, so nothing receives the runner's stream and passes it to web viewers. Neither object has a key, a home site, or failure rows. The BM-4 gate fails a site mid-run.
- **Smallest addition.** Declare the broker and a live-log relay as site-script Durable Object classes. Give each a key (runner scope, job), a placement rule, a Stack row, and a row for each failure class. Or state that the Actions group leader holds broker state.

**W3. Major. M4-WP3.**
- **Where.** SHARED "Actions runs and jobs"; DESIGN Shape, Identity (`ActionsEvent`).
- **Gap.** `ActionsEvent { job, version, event: JobEvent }` holds only job transitions.
  - SHARED says "a `Creating` run, its jobs, then the flip to `Queued`". Yet `RunStatus` has no `Creating`, and no event creates a run, a job (labels, needs, matrix values, outputs), a runner registration, or a runner session.
  - `Actions(AccountId, u8)` has "16 shards", but no function picks a shard. Nothing says how an org runner's poll sees queued jobs on all 16.
- **Smallest addition.** Add `ActionsEvent` variants for run created, jobs created, run status, and runner registered or removed. Add a shard rule (`actions_shard(run)`) and state how one runner scope reads every shard.

**W4. Major. M4-WP3.**
- **Where.** SHARED "Multi-chain operations"; "Event fan-out", other consumers; "Actions runs and jobs", Completion.
- **Gap.** Two Actions steps write to more than one chain, and "Multi-chain operations" lists neither of them.
  - "`ActionsTrigger` creates runs, jobs, and check suites and runs". That writes the Actions shard and the repository chain.
  - "`completejob` records the job's conclusion, the check-run conclusion, and ... a `RequiredCheck` entry". That writes the same two chains.
  - A crash between the two commits leaves `RequiredCheck { conclusion: None }`, so a protected merge waits forever [inferred].
- **Smallest addition.** A "Workflow run" bullet in that section. The Actions entry is the commit point. `RunOwner` finishes the repository-chain check entries, keyed by job attempt. Cancel is the abort path.

**W5. Major. AM1-WP4 (seeded question), M4-WP3.**
- **Where.** AGENT-LAYER "Scale", Activity feed row; the feed block; DESIGN `LEARNER_AT`.
- **Gap.** No cap on polls held per member, and no defined answer past such a cap.
  - Followers hold the polls, and followers are the commit quorum. An `overloaded` follower can refuse replication calls from the leader, so feed load can stall commits [inferred].
  - Learners are added by request rate, not by how many requests are held. With 5,000 agents each waiting 25 s, the rate is 200/s, under the 500 that adds a learner, while each follower holds about 2,500 polls [inferred arithmetic].
  - Broker long polls have the same gap.
- **Smallest addition.**
  - A `FEED_HELD_MAX` per member [guess].
  - 429 with `Retry-After` past it.
  - The held count feeding learner placement.
  - A Milestone 0 row that holds 5,000 polls and checks commit p99.

**W6. Major. M4-WP1.**
- **Where.** SHARED "Event fan-out", Order and Webhooks; DESIGN "Failure modes", event pipeline rows; `research/cloudflare.md`, Queues.
- **Gap.** SHARED promises that a webhook "never arrives before an earlier one from the same chain". Cursors are per (consumer, chain). Delivery uses "Per-owner queue shards" and a 10 s deadline. The Queues research cites only "at least once" and says nothing about order.
- **Also undefined.**
  - Whether the cursor advances when work is queued or after the delivery attempt.
  - Whether order holds per hook or per chain.
  - The cap's value ("Per-owner concurrency caps are compiled", with no number).
  - If the cursor waits on attempts, one hook that times out at 10 s limits every hook on that chain to 0.1 entries/s [inferred].
- **Smallest addition.** The cursor advances once `HookDelivery` rows are recorded. Order is per (chain, hook), kept by a named per-hook sequencer. The cap is a constant with a value.

**W7. Major. M4-WP1, M4-WP3.**
- **Where.** SHARED "Identity and tokens" ("Actions secrets as ciphertext"; `AppRecord.webhook_secret`, "ciphertext as org webhook secrets are"); `rest:actions` ("secrets (sealed box)"); DESIGN `SecretName`.
- **Gap.**
  - `SecretName` has no key that encrypts or decrypts these secrets.
  - A sealed box needs a private key and a `key_id` held by the server (`GET .../actions/secrets/public-key`). The design names neither.
  - Repository-level and environment secrets have no event at all.
  - Nothing names which component decrypts, either the Webhooks consumer to sign payloads or the code that builds the job message.
- **Smallest addition.** A wrapping key in `SecretName` and a sealed-box key pair per owner, with `key_id` and a rotation rule. Name the components that decrypt. Add `RepoEvent` stored formats for secrets and variables.

**W8. Major. M4-WP1.**
- **Where.** SHARED "Event types" (`RepoEvent`); `HookDelivery`; DESIGN RPO table ("Delivery history | Webhook attempt records | Lost with their site").
- **Gap.** `RepoEvent` has no hook variant, and `HookId` has no record behind it. Org hooks sit inside `OrgPolicy`, which open question F19 already covers, so I exclude them. The site-local store for delivery records is unnamed. That leaves no home for repository hook CRUD, the deliveries list, `.../deliveries/{id}/attempts`, ping, or the retry opt-in.
- **Smallest addition.** `RepoEvent::Hook { id, record: Option<HookRecord> }`, holding url, events, content_type, active, secret ciphertext, and retry opt-in. Name the delivery store and give it failure rows.

**W9. Major. M4-WP2.**
- **Where.** DESIGN Shape, Identity (`UserRecord`, `Inbox`); gate BF-5; SHARED `rest:activity`.
- **Gap.**
  - `Watch` and `Subscription` live "on the actor's group". `Counts` writes only counts.
  - Nothing maps a repository or thread to its watchers. So `Notify`, reading a repository chain, cannot find who to notify about "a comment on a watched issue". `/stargazers` and `/subscribers` have no list either.
  - "rebuilt from them" names no chain set or start index, though logs archive after 30 days.
  - `Inbox` has no failure rows.
- **Smallest addition.**
  - A derived index of watchers and subscribers per repository.
  - A rebuild rule (Identity thread state plus a bounded window of chains).
  - `Inbox` failure rows.

**W10. Major. M4-WP1.**
- **Where.** SHARED "Event fan-out" ("`Search` feeds the search index"); `rest:search`; DESIGN Stack (Queues, "search-index ... fan-out"); "Public service concerns", Residency.
- **Gap.** No product holds the index. The Durable Objects row does not list it, and the failure-mode component list omits it. Residency still requires that "search documents" stay in the residency domain.
- **Smallest addition.** Name the store, for example a Durable Object class with FTS5 per owner or residency domain. Give it a Stack row, a placement rule, and failure rows.

**W11. Major. M4-WP4.**
- **Where.** SHARED "Actions", Hosted runners; DESIGN "Failure modes", actions state bad-config ("caps in the runner account, which reach the broker over HTTPS").
- **Gap.** No component in the runner account is named. Something must learn of a queued `ubuntu-latest` job, mint a JIT config (which needs an admin credential), start a Sandbox, and destroy it afterward. The hosted label names and machine sizes are absent. So is the demand signal, whether a broker endpoint or `workflow_job` webhooks as ARC uses.
- **Smallest addition.** A named Worker in the runner account with its credential, demand API, label and size table, and caps, plus its Stack and failure rows.

**W12. Major. M7-WP5.**
- **Where.** DESIGN Shape, "Sites and placement" ("A system chain's record is in the signed genesis (`Genesis.placements`) and never changes"); "Deployment modes", Evaluation.
- **Gap.** "A Cloudflare-hosted `Solo` install moves to `Tri` through a `Joint` config", but the system chains' records cannot change. "The install's signed `evaluation` flag" has no field in `Genesis` or in any `ReleaseEntry`, and no intent signs the move.
- **Smallest addition.** An intent that rewrites the system-chain records and clears the flag, a field that holds the flag, and the order of group moves.

**W13. Major. M7-WP6.**
- **Where.** SHARED `rest:migrations` ("imports as chunked entries ... keeps a `legacy_id` index").
- **Gap.**
  - Whether imported entries fire webhooks to imported hooks, `ActionsTrigger` runs for each imported push, or `Notify`.
  - Whether issue numbers are kept, since bodies link `#N`.
  - How captures carry the original `created_at` and author.
  - How a merged PR is written, given that `Merged` needs `verified: TreeDiffDigest` and `engine`.
  - Where `legacy_id` lives.
- **Smallest addition.** An `Imported` entry marker that consumers skip, an import-only command set that keeps numbers and original times, and the location of the `legacy_id` table.

**W14. Major. AM4-WP1, AM4-WP2.**
- **Where.** AGENT-LAYER Shape, "Overlaps and prediction" and the feed block; the insights consumer region row; SHARED `Consumer`.
- **Gap.**
  - Check runs post "only when the verdict changes", but no entry or fold row stores a verdict, and `PathSet` has no verdict field.
  - The feed is "a fold over the group's log", yet `ActivityEvent::Verdict` has no source entry. The PR page and `graftyard_overlaps` "read the fold [inference]".
  - The producer contradicts itself. `Consumer::Insights` has its own cursor, while AGENT-LAYER says "PrSync's apply enqueues" jobs. Apply runs on every member, which would enqueue each job three times [inferred].
  - `overlap:<cluster>` has no rule for what a cluster is.
- **Smallest addition.**
  - A verdict entry committed through the edge by the insights consumer.
  - The feed and the "changed" check both read it.
  - One producer for insight jobs.
  - A cluster rule: connected overlapping `PathSet`s, named by the lowest PR, and the label removed when the cluster dissolves.

**W15. Major. M7-WP3.**
- **Where.** SHARED "Public service concerns" ("Limits do not multiply with the number of serving processes"); `rest:rate-limit`; AGENT-LAYER managed limits "keyed per minted token".
- **Gap.** No store holds the counters behind `x-ratelimit-*`, which clients see. The Stack lists rate-limiting rules only for abuse limits. Analytics Engine cannot be read inside a request [inferred].
- **Smallest addition.** Name the counter mechanism and its error bound, with a Stack row.

## Work packages walked

- **M4-WP1.** W6, W7, W8, W10.
- **M4-WP2.** W9.
- **M4-WP3.** W2, W3, W4, W5, W7.
- **M4-WP4.** W11.
- **M4-WP5.** W2, through the live-log relay. The rest walked clean.
- **M4-WP6.** The BM-4 gate depends on W2 to W4.
- **AM4-WP1.** W14. `predict` itself walked clean.
- **AM4-WP2.** W14.
- **AM4-WP3.** Clean apart from one minor gap I left out to stay within 15. `CodeMode.enabled_orgs` is a static field, and no setting or endpoint lets an org opt in.
- **AM4-WP4.** The LM-4 gate depends on W14.
- **M7-WP1.** Walked clean against "Deployment modes" and "Install operations".
- **M7-WP2.** Walked clean. Its open point is already C11.
- **M7-WP3.** W15. F8 and F9 are already open questions.
- **M7-WP4.** W1.
- **M7-WP5.** W12.
- **M7-WP6.** W13.
- **M7-WP7.** Walked clean.
- **M7-WP8.** The BM-7 gate depends on W12.

## Process

**Principle.** Model the Domain set the method. For each package I wrote out the stored types first. Gaps W1, W3, W7, W8, and W14 surfaced as types that would need a variant or field the documents never define.

**Playbook (Investigation).**
- Route through `how`: skipped, because the brief prescribed the walk method.
- Throughput checkpoint: n/a, read-only investigation.
- Output: written in the findings format the brief asked for.
- Unslop: rules applied while drafting. I did not read the skill.

Nothing was edited.