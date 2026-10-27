# Tributary design frame

Tributary is a placeholder codename for an AWS-native code-hosting product that is wire compatible with GitHub. This file is the shared brief for every agent in the design run. The run produces a design. Nobody writes product code in this run.

## The request

The operator asked for an AWS-native GitHub alternative with three deployment modes: self-hosted, bring-your-own-cloud (BYOC), and a hosted managed service. The operator ranks uptime first and wants excellence everywhere else. Wire compatibility with GitHub matters. Rust is the likely service language. React is the likely web frontend. GPUI, the Rust UI framework that Zed uses, is a candidate for a native app.

## Deployment modes

The run reads the three modes as follows. A candidate design may challenge a reading if it says why.

| Mode | Who operates it | Where it runs | Namespace |
| --- | --- | --- | --- |
| Managed | The vendor | Vendor AWS accounts | One global, public, github.com-style namespace shared by every user and org |
| BYOC | The vendor, through scoped cross-account roles | The customer's AWS account | The install's own namespace, as on GHES |
| Self-hosted | The customer | The customer's AWS account | The install's own namespace, as on GHES |

### Operator decision, 2026-10-24

The operator wrote, verbatim, "yes it needs to have org and repo name uniquess as we will support public github style stuff". The run reads that as follows: the managed service supports public, github.com-style use. User and org logins are globally unique and share one namespace. Repository names are unique per owner. The run also infers, without the operator saying so, that the managed service has public repositories with anonymous read, forks and pull requests across owners, stars, follows, and search across everything a caller can see, and that BYOC and self-hosted installs keep their own namespace, the way each GHES instance does. This replaces the earlier reading of managed mode as many isolated tenants with one hostname each.

### Operator decision, 2026-10-24, later

The operator said "you can use dynamodb global tables if that helps worldwide". This answered the open decision that the namespace's `global` tier lived only in North American Regions, because Aurora DSQL multi-Region clusters cannot span continents (`review/round2-judge.md`, fact flags). DynamoDB global tables come in two forms: multi-Region eventual consistency (MREC), which any number of Regions can join, and multi-Region strong consistency (MRSC), which spans exactly three Regions and can cross continents (`research/uptime.md#29`, `#30`, `review/round2-judge.md`).

### Operator decision, 2026-10-24, last

The operator wrote, verbatim, "also do a pass at the design and ensure byoc self installs would work too." The run reads "byoc self installs" as both BYOC installs and self-hosted installs, because the frame names both as deployment modes and either reading needs the same install machinery. Under that reading, a design passes only if every stateful component states its form in each mode (`scripts/check_design.py`, P8) and a self-hoster and a vendor SRE can walk through install, upgrade, recovery, and exit against the text (`review/selfhost-walk.md`, `review/byoc-walk.md`, and their re-walks).

The vendor already runs a BYOC platform for another product. `research/byoc-grounding.md` describes its install model, release signing, and fleet rollouts. Reuse that machinery unless a design shows why a git host cannot.

### Operator decision, 2026-10-24, on the store

Asked whether the design needs Aurora DSQL or could run entirely on DynamoDB, the operator wrote, verbatim, "im not trying to port but rather build an entire new, better scalable github that has less downtime". The run reads this as follows. No foundation is fixed because an earlier round chose it, including the store. Downtime and scalability outrank build convenience when a foundation is compared, so keeping invariants inside database transactions counts only as far as it lowers outage risk. An all-DynamoDB design was never built in this run, so the store decision is open until one is designed and judged against the current design on those two goals [inference].

### Operator decision, 2026-10-26, Cloudflare

The operator wrote, verbatim, "note all these in the docs. then, use again and come up with what this would look like entirely cloudflare native. only cloudflare technologies. they have stuff like cloudflare basin, etc." The run reads this as two asks. First, record the cost findings from the cost questions in the docs (`research/costs.md`, `DESIGN.md`). Second, design a second, separate version of Tributary that uses only Cloudflare products, with the same goals: GHES wire compatibility, less downtime first, scale second, and managed, BYOC, and self-hosted modes as far as Cloudflare allows them. The AWS design in `DESIGN.md` stays as it is. The Cloudflare design lives in `cloudflare/DESIGN.md` [inference: "what this would look like" asks for a comparison, not a replacement]. Whether a product called "Cloudflare Basin" exists is checked in `research/cloudflare.md` before any design names it.

**Pending operator decisions for the Cloudflare design.** The design adopts a default for each until the operator answers. Each default is the run's choice, not the operator's.

1. May Tributary run workerd plus Docker on non-Cloudflare hardware for (a) a read-only lifeboat, (b) evaluation, (c) production? Default: yes, yes, no. Under it, self-hosted means the customer's own Cloudflare account, and the lifeboat is the only off-Cloudflare component in production (`cloudflare/DESIGN.md`, "Deployment modes").
2. May managed users get a lifeboat on vendor hardware for Cloudflare-wide outages? Default: no, so managed has no service during one (`cloudflare/DESIGN.md`, "Tradeoffs accepted").
3. Is a home-grown Raft over Durable Objects acceptable as the commit point? Default: yes, gated by Milestone 0's 48-hour linearizability run, with candidate 1 as the fallback.
4. Should BYOC require the Enterprise plan? Default: no. Pro carries 100 MB HTTPS pushes and Business 200 MB, and pushes over 200 MB need Enterprise.

### Operator decision, 2026-10-26, agents

The operator wrote, verbatim, "read this. https://blog.cloudflare.com/next-git-platform-on-cloudflare why not artifacts? how can we take it a step further and make sure this is agent-friendly?" The run reads this as two asks. First, re-examine the rejection of Cloudflare Artifacts against current facts (`research/artifacts-recheck.md`). Second, design an agent layer that makes AI coding agents first-class users without changing any GHES 3.22 response for clients that do not opt in (`cloudflare/AGENT-LAYER.md`) [inference: "agent-friendly" adds users, it does not relax wire compatibility].

**Pending operator decisions for the agent layer.** Each default is the run's choice, not the operator's.

1. Should Tributary run its own agents (a controller, an agents account, Sandboxes, and model keys), or only host third-party agents? Default: offered, off until an org enables it. Model tokens dominate its cost, about $182,400 a month at 2,000 developers at a guessed $2 a task, against a $9,417 base (`cloudflare/cost_agents.py`).
2. Are leases, a live who-is-working-on-what feed, and a task API must-haves for the first release? Default: no. A yes flips the agent layer's base to arena candidate C (`arena/cloudflare-agents/SYNTHESIS.md`).
3. Should agent pushes get CI without a human click by default? Default: no (`ci: Approve`, as on GitHub). `AutoReadOnly` is opt-in per repository. Under the default, a 500-agent fleet needs about 75 approvals a minute.
4. Should metadata child groups (base Milestone 7) move earlier so a 500-agent fleet can run on one repository at agent-layer launch? Default: no. Fleets of that size wait for child groups (`cloudflare/DESIGN.md`, Scale).
5. Is a 25% test-merge budget acceptable? Default: yes. It sets 7 shards per warm site at a 500-agent fleet's peak, about $990 a month if held, and leaves most `mergeable` polls null under load.
6. In managed mode, per-tenant App keys or per-tenant agents accounts? Default: per-tenant App keys. Accounts isolate more and cost an account each.
7. Keep SSH through Spectrum, or serve HTTPS only? Default: keep SSH, subject to Milestone 0's Spectrum-to-Tunnel row on each plan.

### Operator decision, 2026-10-26, agents answered and graftyard

The operator wrote, verbatim, "1 - offered is fine 2 - yes, include leases and tasks in v1 3 - sounds good as you decided 4 - default is fine 5 - sounds good 6 - sounds good 7 - ssh through spectrum read all fixes. do one final pass at everything. use for this, then clean up all notes so it is a single comprehensive folder of notes, cleaned, ready for an implementation by other agents. we are going to call this 'graftyard'."

The run reads the answers as follows. Decisions 1, 3, 4, 5, and 6 keep their defaults. Decision 2 overrides its default, so leases and a task API ship in the first release of the agent layer. The run reads "leases" to include the read side that lets an agent see other agents' leases, which the earlier question called a live feed [inference]. Decision 7 keeps SSH through Spectrum. "Use for this" reads as "use poteto mode for this". The project is renamed Graftyard, and the deliverable is one folder, `<vendor-workspace>/graftyard/`, that implementing agents can build from. The Cloudflare design is the design of record, because every answer above is about it [inference]. The AWS design becomes a summarized alternative inside that folder. This working folder stays in place as the archive and is not deleted, since deleting it is the operator's call.

During the run the operator added, verbatim, "rename tributary-design directory to simply 'graftyard'". The workspace was renamed to `<vendor-workspace>/graftyard/` the same day, so the sentence above about keeping a separate working folder no longer holds. The renamed folder is the deliverable. It is restructured in place, and the design record that implementers do not need moves into its `history/` folder, with nothing deleted.

## What wire compatible means

An unmodified client works after its user points it at a Tributary host, the same way they point it at GitHub Enterprise Server (GHES) today. `research/clients.md` lists the clients that decide the question. `research/wire-census.md` and `research/actions-protocol.md` count the surfaces those clients touch.

## Quality attributes in priority order

1. Availability of git reads and writes, pull request review and merge, and the REST and GraphQL APIs.
2. Durability. The system never loses an acknowledged push.
3. Wire compatibility.
4. Latency of git operations and page loads.
5. Cost and operational burden in each deployment mode.

## Definition of done

`DESIGN.md` is done when every predicate below holds. `scripts/check_design.py` checks the mechanical ones.

- P1. Every surface in `research/wire-census.md` has a compatibility tier and a named conformance oracle, which is an unmodified real client that must pass.
- P2. Each user journey has an SLO with an error budget. Each stateful component lists its failure modes (instance, availability zone, region, AWS control plane, bad deploy, bad config, noisy tenant), the blast radius of each, and the mechanism that contains it.
- P3. The three deployment modes ship as one artifact, and a table states exactly what varies between them.
- P4. Rust type sketches name the core data shapes: tenancy and cell placement, repository storage and ref transactions, identity and tokens, the issue and pull request model, the Actions run and job state machines, and the event outbox.
- P5. The design records at least two structurally distinct whole-system alternatives and why each lost.
- P6. A build order names the first verifiable milestone and the conformance gate that proves it.
- P7. The design survives an adversarial multi-model review, and the design resolves every finding the lead marks "act on".

## Research files

| File | Question it answers |
| --- | --- |
| `research/wire-census.md` | How big is each GitHub wire surface, and which cross-cutting conventions must every endpoint follow? |
| `research/actions-protocol.md` | What does an unmodified `actions/runner` and the Actions toolkit need from a server, and where does GitHub control the version? |
| `research/git-storage.md` | How do large git hosts store repositories and keep refs consistent, and what can AWS storage and the Rust git ecosystem offer? |
| `research/uptime.md` | Which AWS mechanisms and recent outages should shape the availability design, and what do GitHub's incidents teach? |
| `research/byoc-grounding.md` | What does the vendor's existing BYOC platform already provide? |
| `research/clients.md` | How does each important client target GHES, and which hostname or version heuristics does it apply? |

## Run plan

The lead runs these phases in order. Each phase ends in a check before the next starts.

1. Research. Six read-only slices write the files above. `scripts/census.py` counts the wire surfaces and writes `research/surfaces.json`.
2. Sketch. Three runners each design the whole system around a different storage and consistency bet, per `arena/TASK.md`. `scripts/check_design.py` checks each candidate's structure.
3. Judge. A cross-judge scores the candidates against `review/arena-rubric.md`. The lead reads every candidate end to end, picks a base, and records the pick in `arena/SYNTHESIS.md`.
4. Graft. The lead folds the strongest ideas from the other candidates into `DESIGN.md`.
5. Interrogate. Three reviewers attack `DESIGN.md`. The lead sorts findings into act on, consider, noted, and dismissed in `review/interrogate.md`.
6. Revise. The lead resolves every act-on finding in `DESIGN.md` and reruns `scripts/check_design.py DESIGN.md --final`.
7. Audit. The lead checks `decisions.tsv` against the transcript, and a reviewer on another seat reads the trail.

The run stops at the design. Nobody builds the product.
