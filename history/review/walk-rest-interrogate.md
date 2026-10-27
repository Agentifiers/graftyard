# Third implementability walk: triage and act-on items

`history/review/walk-rest.md` (WalkRest, a fresh poteto-agent on anthropic/claude-opus-5-5) walked Milestone 4, Milestone 7, AM4, and the SHARED sections the first two walks left out. It found W1 to W15 and one minor gap: Code Mode has no opt-in setting. The lead read all of them. Every one is acted on.

This walk was run because trail review 8 (`history/review/trail-review-8.md`, flags 2 and 5) found that the first two walks did not cover these milestones.

Rules for the writers are the same as in `history/review/walk-interrogate.md`:

- edit only the regions you own;
- re-read before every edit;
- do not touch the Build order tables;
- report your gate rows as `Milestone | Measurement | Pass | Fail`, along with any open questions;
- tag invented values [guess].

The caps in `scripts/check_design.py` stay at 1,900 and 1,000 lines. If the base needs more lines, report the count rather than cutting content.

## Writer X1: Actions and events

Owns `design/SHARED.md` "Actions runs and jobs", "Actions", "Event fan-out", and "Multi-chain operations", plus `design/DESIGN.md` Shape "Identity" (the `ActionsEvent` part) and the `actions state` and `event pipeline` rows of "Failure modes".

- **WK-X1 (W2).** Declare two site-script Durable Object classes:
  - the runner broker, keyed by (org, runner scope);
  - a live-log relay, keyed by job, which takes the runner's `FeedStreamUrl` WebSocket and fans it out to viewers.

  Both hold derived state only. Each needs a placement rule (the Actions group's leader site), a Stack row, and a row for every failure class. The broker caps the long polls it holds at a named constant and answers past it as the runner protocol expects. The design must say what the runner does then.
- **WK-X2 (W3).** Add `ActionsEvent` variants for run created, with the run's jobs (labels, needs, matrix values) and its status flips, and for runner registered and removed, and for sessions. Add `actions_shard(run)`, and state how one runner scope's broker reads queued jobs across all 16 shards.
- **WK-X3 (W4).** Add a "Workflow run" bullet to "Multi-chain operations". The Actions entry is the commit point. `RunOwner` finishes the repository-chain check-run and `RequiredCheck` entries, keyed by job attempt. Cancel is the abort path. No crash may leave a `RequiredCheck` with no conclusion.
- **WK-X4 (W6).**
  - A consumer's cursor advances once the `HookDelivery` rows are recorded, never on the delivery attempt itself.
  - Order holds per (chain, hook), kept by a named per-hook sequencer.
  - The per-owner concurrency cap gets a value.
  - A slow hook delays only itself.
- **WK-X5 (W7).**
  - Add a wrapping key to `SecretName`.
  - Add one sealed-box key pair per owner, with a `key_id` and a rotation rule.
  - Add stored formats for repository, environment, and org secrets and variables.
  - Name the components that decrypt: job-message assembly and webhook signing.
- **WK-X6 (W8).**
  - Add `RepoEvent::Hook { id, record: Option<HookRecord> }`, carrying url, events, content_type, active, secret ciphertext, and retry opt-in.
  - Name the site-local delivery store, and add failure rows for it.
- **WK-X7 (W11).** Add a named Worker in the runner account that does the following:
  - learns of queued jobs through a named demand endpoint on the broker;
  - mints JIT configs with a named credential;
  - starts and destroys Sandboxes;
  - uses a hosted label and machine-size table, all [guess], and per-org caps.

  It also needs a Stack row and failure rows.

## Writer X2: storage, scale, and operations

Owns:

- `design/DESIGN.md` Shape: "Chains" (the child group part only), "Identity" (the `UserRecord` and `Inbox` part), and "Sites and placement" (the `Solo` move only);
- `design/DESIGN.md` Stack rows for new classes;
- `design/DESIGN.md` failure rows for the new components;
- `design/SHARED.md` "Public service concerns" and the `rest:migrations`, `rest:search`, and `rest:rate-limit` rows.

**WK-X8 (W1).** Add a "Child groups" part to "Chains", made of:

- `ChainId::RepoMeta(RepoId)`;
- a core `MetaSplit { child, at }` entry as the commit point;
- the child group starting from the metadata snapshot at `at`;
- the parent refusing metadata commands after the split;
- commands that touch both tiers commit core first, with a named finisher for the metadata half;
- consumer ordering across the split index;
- `RestoredTo` and `ResetTo` scope after the split;
- the feed's 410 rule.

**WK-X9 (W9).** Add a derived index of watchers and subscribers per repository and thread, which `Notify` and `/stargazers` and `/subscribers` read. Add the `Inbox` rebuild rule: Identity thread state plus a bounded window of chains. Add failure rows for `Inbox` and the index.

**WK-X10 (W10).** Add a search store: a Durable Object class with FTS5, one per owner or per residency domain. Give it a Stack row, a placement rule that follows residency, and failure rows.

**WK-X11 (W12).**

- A `Genesis` field and a `ReleaseEntry` that holds the `evaluation` flag.
- An intent, signed by the customer key, that rewrites the system-chain placement records from `Solo` to `Tri`, clears the flag, and orders the group moves.

**WK-X12 (W13).**

- An `Imported` marker on entries, which `Webhooks`, `ActionsTrigger`, and `Notify` skip.
- An import-only command set that keeps issue numbers, original times, and authors.
- How a merged PR is imported without a tree-diff check.
- Where the `legacy_id` table lives.

**WK-X13 (W15).**

- Name the counter store behind `x-ratelimit-*` and its error bound.
- Use a Cloudflare product only if `research/cloudflare.md` lists it. Otherwise use a Durable Object class.
- Give the store a Stack row and failure rows.

## Writer Y: agent layer

Owns all of `design/AGENT-LAYER.md` except Build order, plus the agent block in `design/DESIGN.md` Shape. The block must stay byte-identical.

- **WK-Y1 (W5).**
  - Add `FEED_HELD_MAX` per member [guess], with 429 and `Retry-After` past it.
  - The held count feeds learner placement.
  - A feed poll never runs on a member that is not answering replication. Say how: for example, learners serve polls first, and followers serve them only when no learner exists.
  - Recompute any figure that changes with `scripts/cost_agents.py`.
- **WK-Y2 (W14).**
  - The insights consumer commits a verdict entry through the edge.
  - The "changed verdict" check and the feed's `Verdict` event both read that entry.
  - Jobs have one producer, the `Insights` consumer's cursor, and nothing enqueues them from `apply`.
  - Cluster rule: connected overlapping `PathSet`s, named by the lowest PR number. The label is removed when the cluster dissolves.
- **WK-Y3 (minor).** Code Mode opt-in becomes an `OrgPolicy` setting with an endpoint in "API reference", replacing the static `enabled_orgs`.

## Writer Z: integration, after X1, X2, and Y

Owns the Build order sections of both design documents, `build/BUILD-PLAN.md`, `decisions/OPEN-QUESTIONS.md`, `decisions/DECISIONS.md`, and the "Review resolutions" sections. It adds a "Third walk" entry to both.

- Add every reported gate row, each with a measurable Pass and Fail.
- Update the work packages and the coverage table so that every row appears exactly once, and prove it with a script.
- Add the open questions the writers report.
