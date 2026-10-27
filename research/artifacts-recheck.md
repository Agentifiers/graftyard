# Cloudflare Artifacts recheck, 2026-10-26

Purpose. Recheck every reason `design/DESIGN.md` ("Alternatives considered", "Cloudflare Artifacts as the git store") gave for rejecting Artifacts, against the docs and status data as of 2026-10-26. Then list what is new, what helps AI coding agents, and what a Milestone 0 must test.

Method. Every page under https://developers.cloudflare.com/artifacts/llms.txt was read as `<url>/index.md` on 2026-10-26, plus the Queues event subscription, delivery, limits, and Wrangler pages, the Workers Builds Artifacts integration page, both blog posts, the Cloudflare Enterprise SLA, the cloudflarestatus.com incidents JSON, the ArtifactFS README, and the git protocol v2 spec. Both blog posts loaded directly this time, so the earlier GitHub mirror is no longer needed.

Labels. [doc] is an official page, quoted. [measured] means I read the raw data myself (status JSON, a spec). [inference] is my reasoning. "Could not verify" means no source answered it.

Inputs rechecked. `research/cloudflare-git.md` findings 3 to 7 and 14, `research/cloudflare.md` (Artifacts row and section), `design/DESIGN.md` lines 856 to 861.

## Answer

Nothing that rejected Artifacts as the canonical git store has changed. The 1 GB cap on new repositories, the single logical instance per repository, the closed engine, beta status, and the prices are all as the design recorded them. Artifacts has no SLA (beta is excluded from Cloudflare's SLA), no stated RPO or RTO, and no placement control beyond `eu` or `us`. In the last 50 status incidents it was listed as degraded twice, both times as a downstream of Durable Objects or a regional network fault [measured].

Two of the design's objections are weaker than written. Upstream git has no protocol v2 push command, so "push is v1 only" does not limit any unmodified client [measured from the git spec, inference about effect]. Artifacts documents `filter` as unsupported only among "optional v1 capabilities". Whether v2 fetch advertises `filter` is undocumented, and Cloudflare's own ArtifactFS relies on blobless clones against Artifacts remotes. That is a Milestone 0 test, not a settled rejection.

What is new is an agent toolkit. It includes programmatic `fork`, `readFile`, commit and tree reads, repo-scoped tokens with 60 s to 1 year TTLs, push events that can start a Workflow directly, Workers Builds deploys, `eu`/`us` jurisdictions, and per-repo GraphQL metrics. It is bounded by 200 control-plane requests per second per namespace, 200 git requests per second per repository, forks that run asynchronously and appear to copy objects, tokens that cover the whole repository (no branch scope), and no merge, compare, pull request, hook, or branch protection API.

## 1. Rejection reasons rechecked

| Assumption in the design | Current fact, quote, and URL | Status |
| --- | --- | --- |
| Per-repository size cap of 1 GB for new repos, against the 5 GB monorepo in journey 1 | "Maximum storage per repository: 1 GB for repos created on or after 2026-09-29. Repos created earlier have a 10 GB limit." Limits page last updated 2026-10-21. https://developers.cloudflare.com/artifacts/platform/limits/ [doc]. The changelog entry of 2026-09-29 says the same and offers no raise path. https://developers.cloudflare.com/changelog/post/2026-09-29-repository-size-limit/ [doc] | Unchanged. The cap still rules out the 5 GB monorepo. No "can be raised" note on the per-repo row, unlike the account row. |
| Push is protocol v1 only | "Push over protocol v2: Not supported. Artifacts does not support v2 receive-pack." Page last updated 2026-04-25. https://developers.cloudflare.com/artifacts/api/git-protocol/ [doc]. The git protocol v2 spec (git 2.56.0, 2026-09-28) defines the commands `ls-refs`, `fetch`, `object-info`, and `bundle-uri`, and no push command. https://git-scm.com/docs/gitprotocol-v2 [measured, read spec] | Fact unchanged. Its weight is lower than the design assumed. Unmodified git pushes with the v0/v1 receive-pack flow to every host, GitHub included, so v1-only push costs a GHES-compatible host nothing [inference]. |
| `filter` (partial clone) is unsupported | "Optional protocol v1 capabilities: Partial. Some optional v1 capabilities, such as `filter` and `include-tag`, are not supported." The v2 row says only "Protocol v2 supports `ls-refs` and `fetch`." Same page [doc]. ArtifactFS "starts with a blobless clone" and "works with Artifacts Git remotes". https://developers.cloudflare.com/artifacts/guides/artifact-fs/ [doc]. Its README adds: "When the remote advertises partial-clone filtering, only commits, trees, and refs are fetched initially; otherwise Git falls back to downloading blobs eagerly." https://github.com/cloudflare/artifact-fs [doc] | Unchanged for v1. Undocumented for v2 fetch, which modern git uses by default [inference]. Could not verify whether Artifacts advertises `filter` over v2. Milestone 0 question 1. |
| Closed engine | The April post describes a Zig engine "compiled to a ~100KB WASM binary" and names only ArtifactFS as open sourced ("we're open-sourcing ArtifactFS"). https://blog.cloudflare.com/artifacts-git-for-agents-beta/ (2026-04-16, modified 2026-07-15) [doc]. ArtifactFS is Apache-2.0 and in "beta release". https://github.com/cloudflare/artifact-fs [doc]. No docs page links an engine repository [doc absence]. | Unchanged. |
| Single logical instance per repo, and that failure domain | "Like Durable Objects, a repo is a single logical instance that Cloudflare can route to from any region." "Cloudflare replicates repo data synchronously across multiple data centers and copies it asynchronously to object storage and snapshots." Last updated 2026-04-25. https://developers.cloudflare.com/artifacts/concepts/how-artifacts-works/ [doc]. The Artifacts status component exists since 2026-07-16. It went `degraded_performance` in "Increased Errors for Durable Objects" ("increase of errors for Durable Objects and downstream services in DEN") from 15:37 to 18:00 UTC on 2026-10-15, and was named in "Network Performance Issues in Ashburn" ("Affected services: Durable Objects, R2, Workflows, Queues, D1 and Artifacts.") from 01:01 to 03:09 UTC on 2026-10-24. https://www.cloudflarestatus.com/api/v2/incidents.json [measured] | Unchanged, and now observed. Artifacts degrades with Durable Objects, as the design predicted [inference]. The 2026-10-22 WEUR DO incident (17:51 to 00:09) does not list the Artifacts component [measured]. Could not verify whether WEUR-homed repos were affected. |
| Auth tokens in KV | "Under the hood, Artifacts also uses R2 (for snapshots) and KV (for tracking auth tokens)." Post dated 2026-04-16, modified 2026-07-15. https://blog.cloudflare.com/artifacts-git-for-agents-beta/ [doc]. No docs page states where tokens live. A `token.revoked` event exists. https://developers.cloudflare.com/artifacts/guides/event-subscriptions/ [doc] | Unchanged as far as any source says. Revocation latency is undocumented. The design's 60 s KV staleness concern (`history/DESIGN.md` "Workers KV for tokens") still applies if KV is still used [inference]. Could not verify. |
| Beta status | "Artifacts is now in open beta" (changelog, 2026-10-01). https://developers.cloudflare.com/artifacts/platform/changelog/ [doc]. The Enterprise SLA excludes issues "Occurring during beta and trial services, unless otherwise agreed to in writing by Cloudflare", and its service-specific SLA list covers only R2, Queues, and Workers. https://www.cloudflare.com/enterprise-support-sla/ (policy date 2026-04-01) [doc] | Unchanged. It moved from private to open beta, not to GA. No SLA covers it. |
| Storage and operation price, $2,000 a month storage and $965 operations at 2,000 developers | "First 10,000 per month + $0.15 per additional 1,000 operations"; "First 1 GB per month + $0.50 per additional GB-mo"; "Replicas do not add storage charges." https://developers.cloudflare.com/artifacts/platform/pricing/ (updated 2026-10-01) [doc] | Unchanged. The design's arithmetic stands. |
| Per-repo and per-namespace request rates | "Git request rate, per artifact: 2,000 requests per 10 seconds per artifact". "Control-plane request rate: 2,000 requests per 10 seconds per Artifacts namespace". https://developers.cloudflare.com/artifacts/platform/limits/ [doc] | Git rate unchanged. The namespace control-plane rate was not in the design's research. It caps create, fork, import, and token minting at 200 per second per namespace on average [inference]. |
| Account caps | "Maximum storage per account: 1 TB (can be raised on request)". "Maximum number of repositories: Unlimited". "Maximum number of namespaces: Unlimited". https://developers.cloudflare.com/artifacts/platform/limits/ [doc]. "Artifacts requires a Workers Paid plan. Usage is not available on Workers Free." https://developers.cloudflare.com/artifacts/ (updated 2026-10-23) [doc] | Unchanged. The design's 4 TB at 2,000 developers needs a raise [inference]. The April promise to bring Artifacts "to the Workers Free plan (with some fair limits)" has not shipped. |

## 2. New since the design's research

### Workers binding (page updated 2026-10-01)

Source for this subsection: https://developers.cloudflare.com/artifacts/api/workers-binding/ [doc].

- Configuration is `"artifacts": [{ "binding": "ARTIFACTS", "namespace": "default" }]`. One binding is scoped to one namespace. Wrangler 4.145.0 or later is required for types.
- Namespace methods: `create(name, { readOnly, description, setDefaultBranch })`, `get(name)`, `list({ limit, cursor })`, `import({ source: { url, branch, depth }, target: { name, opts } })`, and `delete(name)`. "`create()` returns repo metadata including `name`, `remote`, `defaultBranch`, and an initial token."
- `get()` "returns an `ArtifactsRepo` RPC capability that implements `Disposable`" and "Throws if the repo does not exist or is not ready yet." Listed repos carry "a `status` value of `ready`, `importing`, or `forking`."
- Repository methods: `info()`, `createToken(scope?, ttl?)` (scope "default: \"write\""), `listTokens()`, `revokeToken(tokenOrId)`, `fork(name, { description, readOnly, defaultBranchOnly })`, `log({ ref, limit, offset })` ("maximum: 1000", "follows the first-parent chain"), `readCommit(hash)`, `readTree(hash)` ("returns only the tree's immediate children"), `readBlob(hash)`, and `readFile({ ref, path })` ("returns a MIME-typed `Blob`").
- Absent from the binding: merge, diff or compare, ref creation or update, ref listing, search, hooks, and branch protection [doc absence].

### REST API (page updated 2026-10-22)

Source: https://developers.cloudflare.com/artifacts/api/rest-api/ [doc].

- Namespaces have explicit create (with `jurisdiction`), list, get, and delete routes. "The namespace must be empty before you delete it."
- Repo list supports `search`, `sort` by `created_at`, `updated_at`, `last_push_at`, or `name`, and `limit` up to 200.
- Fork returns `ForkRepoResult extends CreateRepoResult { objects: number }`.
- Content routes are `log`, `commit/:hash`, `tree/:hash`, `blob/:hash`, `file?ref=&path=`, and `raw/:ref/:path` ("Returns file bytes with a sniffed `Content-Type`").
- Token TTL: "Minimum 60 (1 minute), maximum 31,536,000 (1 year). Defaults to 86,400 (24 hours)." Token states are `active`, `expired`, and `revoked`. Format is `art_v1_<40 hex>?expires=<unix_seconds>` (git protocol page).
- Delete repo "returns `202 Accepted`". No undelete or restore route is documented [doc absence].
- Import: "Artifacts imports public HTTPS remotes". https://developers.cloudflare.com/artifacts/guides/import-repositories/ [doc]. Error `REMOTE_AUTH_REQUIRED` (10106): "The remote repository requires authentication." https://developers.cloudflare.com/artifacts/api/errors/ [doc]
- Other errors: `MEMORY_LIMIT` (10402) "The operation exceeds service memory limits", and `FORK_IN_PROGRESS` (10303). https://developers.cloudflare.com/artifacts/api/errors/ [doc]
- Wrangler has `artifacts namespaces list|get` and `artifacts repos create|list|get|delete|issue-token` (changelog 2026-05-18). https://developers.cloudflare.com/artifacts/api/wrangler/ [doc]. The dashboard can create, fork, and browse repos and mint tokens (changelog 2026-06-17) [doc].

### Event types and delivery

- Account-level source `artifacts` emits `repo.created`, `repo.deleted`, `repo.forked`, and `repo.imported`. Repository-level source `artifacts.repo` emits `pushed`, `cloned`, `fetched`, `token.created`, and `token.revoked`. Page updated 2026-05-21. https://developers.cloudflare.com/artifacts/guides/event-subscriptions/ [doc]
- The `pushed` payload has `ref`, `before`, `after`, `commits[]` (id, message, `messageTruncated`, author, committer, parents), `totalCommitsCount`, and `commitsTruncated`. Metadata has `eventSubscriptionId`, `eventSchemaVersion: 1`, and `eventTimestamp`. Same page [doc]
- Repo-level events need one subscription per repository on the Queues path. The flags `--source-namespace` and `--source-repo-name` are "required for artifacts.repo source". https://developers.cloudflare.com/queues/reference/wrangler-commands/ [doc]
- A second path skips Queues. A Worker's `triggers.events` entry of type `cf.artifacts.repo.pushed` can target a Workflow, and "When you omit `repo_name`, Cloudflare runs the same Workflow for every push to any repo in your Artifacts namespace." Page updated 2026-10-26. https://developers.cloudflare.com/artifacts/guides/build-and-deploy-on-push/ [doc]
- Delivery. Event subscriptions publish to a queue, and "Queues provides *at least once* delivery by default ... in rare occasions, may be delivered more than once." https://developers.cloudflare.com/queues/reference/delivery-guarantees/ [doc]. Queue bounds are "5,000 messages per second" per queue, 128 KB messages, and retention "Configurable up to 14 days". https://developers.cloudflare.com/queues/platform/limits/ [doc]
- No Artifacts page states event ordering, publish latency, or whether an event can be lost if the publisher fails after the push commits [doc absence]. Could not verify. Events are emitted after the fact. No pre-receive or veto hook exists [doc absence].

### Workers Builds integration

- "Pushes to the production branch run the configured build and deploy commands. The Artifacts integration only supports `main` as the production branch." Non-production branches build Worker Previews. Page updated 2026-10-01. https://developers.cloudflare.com/workers/ci-cd/builds/git-integration/artifacts-integration/ [doc]
- A custom CI path uses the `@cloudflare/ci` SDK inside a Workflow. "each `runner()` call spins up an isolated sandbox and executes a shell command", with dependency caching via sandbox snapshots in R2. https://developers.cloudflare.com/artifacts/guides/build-and-deploy-on-push/ [doc]
- A Workers Builds incident ("Cloudflare Workers build delays") ran 15:12 to 16:03 UTC on 2026-10-26. https://www.cloudflarestatus.com/api/v2/incidents.json [measured]

### Jurisdiction (added 2026-08-13)

- "Artifacts jurisdictions ensure repo data is stored and processed only within a selected location." The options are `eu` and `us`. "You cannot change the jurisdiction after creating the namespace." Omitting it leaves the namespace "unrestricted". https://developers.cloudflare.com/artifacts/guides/data-localization/ [doc]
- No `fedramp` option exists, unlike R2, Durable Objects, and Containers (`research/cloudflare-git.md` findings 40, 41) [doc comparison]. Artifacts' wording ("stored and processed only within") is stronger than the DO page's caveat that a "DurableObjectId will be logged outside of the specified jurisdiction". Could not verify whether Artifacts inherits that caveat.

### Metrics

- The GraphQL dataset is `artifactsEventsAdaptiveGroups`. Fields are `count`, `sum.durationMs`, `avg.durationMs`, and quantiles p25 to p999. Dimensions include `repository`, `eventKind`, `eventType`, and `errorMessage`. "Metrics can be queried for the past 31 days." Event types are `create`, `fork`, `push`, `pull`, `delete`, `storageLimitReached`, `serverError`, `clientError`, and `rateLimited`. https://developers.cloudflare.com/artifacts/observability/metrics/ [doc]
- The April post promised "stored bytes per repo". The metrics page lists no storage field [doc absence]. Could not verify a per-repo size metric.

### Billing date and prices

- Prices are unchanged (section 1). The pricing page says "Cloudflare will begin billing for Artifacts operations and storage on October 14, 2026." https://developers.cloudflare.com/artifacts/platform/pricing/ [doc]. The 2026-10-01 post says "We will begin billing for Artifacts usage on October 15, 2026." https://blog.cloudflare.com/next-git-platform-on-cloudflare [doc]. The two sources disagree by one day. Both dates have passed.
- Billable operations are "such as `create`, `push`, `pull`, and `clone`", and the April post adds forks. Could not verify whether `readFile`, `log`, token minting, or event delivery count as operations.

### SLA

- No Artifacts SLA exists. The Enterprise SLA excludes beta services and lists service-specific SLAs only for R2, Queues, and Workers. https://www.cloudflare.com/enterprise-support-sla/ [doc]

### Durability, replication, placement, RPO, and RTO

- The only statements are on the "How Artifacts works" page (quoted in section 1) and the pricing page ("Storage is replicated by default, and you do not need to manage repository availability or uptime."). https://developers.cloudflare.com/artifacts/platform/pricing/ [doc]
- No page states the number of data centers, whether they span regions, the asynchronous copy lag (an RPO), failover time (an RTO), where a repo is placed, or whether a repo moves after a regional loss [doc absence]. No customer-facing snapshot, restore, or export API exists beyond `git clone` [doc absence].

### Doc inconsistencies found

- The Sandbox example reads `repo.defaultBranch` and `repo.remote` from a `get()` handle. https://developers.cloudflare.com/artifacts/examples/sandbox-sdk-artifacts/ [doc]. The binding page says "Repository metadata is not exposed as properties on `ArtifactsRepo`." [doc]. One of them is wrong.
- The billing date differs by one day between the docs and the blog (above).

## 3. What helps agents, and what bounds it

| Capability | Source | Bound |
| --- | --- | --- |
| Per-task repos. "Create one repo for each unit of autonomous work. If you have `10,000` agents, create `10,000` repos." | https://developers.cloudflare.com/artifacts/concepts/best-practices/ [doc] | Repos and namespaces are unlimited. Creation is capped by "2,000 requests per 10 seconds per Artifacts namespace" (limits page). 10,000 creates or forks in one namespace take at least 50 s [inference]. The docs advise sharding: "When one namespace becomes hot, shard new repos into additional namespaces." |
| Forks from a baseline. `fork(name, { defaultBranchOnly, readOnly })`. The 2026-10-01 post shows forking per task and reading `AGENTS.md` with `readFile`. | Binding page; https://blog.cloudflare.com/next-git-platform-on-cloudflare [doc] | No documented forks-per-repo limit. A fork is asynchronous (`forking` status, `FORK_IN_PROGRESS`) and returns an `objects` count [doc]. That suggests it copies objects rather than sharing them [inference]. Could not verify whether each fork bills its full size. Each fork is capped at 1 GB, like any new repo [inference]. The binding takes no target namespace, yet the `repo.forked` payload carries one [doc]. Could not verify cross-namespace forks. |
| Repo-scoped tokens. `read` or `write`, TTL 60 s to 1 year, list and revoke, and token events. | REST API page; git protocol page [doc] | A token covers the whole repository. No branch, path, or ref-pattern scope exists [doc absence]. An agent with a write token can force-push any ref, and no branch protection or pre-receive hook exists to stop it [inference from absence]. Revocation latency is undocumented. |
| Context reads without a clone. `readFile`, `readTree`, `readBlob`, `log`, and `readCommit`. | Binding page [doc] | `log` returns at most 1,000 commits per call along the first-parent chain only. `readTree` returns one level. There is no diff, blame, or search. The April post's "Repo-level search APIs and namespace-wide search APIs" has not shipped in the docs [doc absence]. |
| Push-driven automation. A `pushed` event starts a Workflow or a Queue consumer. | Event and build guides [doc] | At-least-once delivery. Per-repo queue subscriptions, or one namespace-wide Workflow trigger. Events come after the fact. |
| Agent metadata. "Use git notes to attach prompts, model output, run IDs, or other harness metadata to a commit." | Best practices page [doc] | Notes "live on separate refs". Whether GHES clients read them has no bearing on Artifacts [inference]. |
| Fast large-repo start. ArtifactFS mounts via FUSE and hydrates blobs on demand. | ArtifactFS guide and README [doc] | It needs FUSE on the host. Blob laziness depends on the remote advertising `filter` (section 1). The README reports "`git status` takes ~7s on large repos (5800+ entries)". |
| Hot baseline clones. Many agents cloning one baseline repo. | Limits page [doc] | 2,000 git requests per 10 s per repository is 200 per second. A v2 HTTP clone makes at least three requests (info/refs, `ls-refs`, `fetch`), so one repo serves about 66 clones per second at most [inference]. The design's CI load of 33 clones per second (`history/DESIGN.md` line 864) leaves 2x headroom on one hot repo [inference]. |
| Importing work from GitHub. | Import guide; errors page [doc] | Public HTTPS remotes only, one branch, optional depth. Private repos fail with `REMOTE_AUTH_REQUIRED`. |
| Cost per agent task. | Pricing page [doc] | At 10,000 tasks a day with about 6 operations each (fork, clone, three pushes, one fetch), that is about 1.8 million operations a month, or about $268, before storage [inference, arithmetic]. |

## 4. Open questions for a Milestone 0

1. Does Artifacts advertise `filter` on protocol v2 `fetch`? Run `git clone --filter=blob:none` and `GIT_TRACE_PACKET=1` against a fresh repo. If yes, the partial-clone rejection falls.
2. Does a fork copy objects or share them? Fork a 500 MB repo 10 times and watch account storage and the `objects` field. This decides whether per-task forks are cheap.
3. How long does `fork` take as a function of repository size and object count, and how often does it return `FORK_IN_PROGRESS` or `MEMORY_LIMIT`?
4. What is the largest single push Artifacts accepts, and where does it fail (request body limit, the 128 MB DO memory limit, or the 1 GB cap)? Push a 900 MB pack.
5. Does Artifacts handle what GHES clients send? Test signed pushes (`push-cert`), atomic pushes, `push-options`, force-push, deleting the default branch, and `include-tag` on v1 fetch.
6. What is token revocation latency? Revoke a write token and retry pushes every 100 ms.
7. Are `pushed` events ordered per ref, how late do they arrive, and can a committed push produce no event? Push 10,000 times and reconcile events against `log`.
8. Does a multi-ref push emit one `pushed` event per ref?
9. What happens to a repo when its home data center or region fails? Watch error rates during the next DO regional incident on the status page. Ask Cloudflare for placement, replica count, RPO, and RTO in writing.
10. What counts as a billable operation (`readFile`, `log`, token minting, `info`)? Run a known sequence and read the bill.
11. Can one Graftyard install stay under the namespace control-plane limit while minting a short-lived token per agent push, or does it need a token cache or namespace sharding?
12. Can Artifacts run off Cloudflare at all? Could not verify whether workerd or any self-hosted runtime implements the binding. The Wrangler option `remote = true` ("use the remote Artifacts service in local dev") implies a local emulation exists [inference]. BYOC and self-hosted modes need an answer.
13. Will Cloudflare raise the 1 GB per-repo cap, give it a GA date, or put Artifacts under an SLA in writing? Only Cloudflare can answer.

## Implications [inference]

1. Artifacts still cannot be Graftyard's canonical store. It fails on size and on its single-instance failure domain with no SLA or stated RPO. Missing BYOC or self-hosted parity matters only for workerd evaluation and the lifeboat, because BYOC and self-hosted run in a Cloudflare account the customer owns (`history/review/cloudflare-agents-foundation.md` finding 10, added 2026-10-26).
2. It fits as an agent workspace tier beside the canonical store. Per-task forks or imports with short-lived repo tokens, `readFile` for context, push events into review Workflows, and results brought back through Graftyard's own receive path.
3. Two items in the design's rejection bullet should be corrected. "Push is protocol v1 only" is not a client-visible limit. "`filter` is unsupported" is documented for v1 only.
4. A Graftyard agent tier would have to add what Artifacts lacks: branch-scoped credentials, protected refs, a pre-receive veto, merge and compare, and an ordered ref-update log. Graftyard already owns these for GHES compatibility.

## Could not verify

- Whether Artifacts advertises `filter` over protocol v2.
- Whether forks share or copy storage, and whether they bill their full size.
- Where tokens live today and how fast revocation takes effect.
- Event ordering, latency, and loss on publisher failure.
- Replica count, region spread, RPO, RTO, and post-failure placement.
- Whether Artifacts was affected by the 2026-10-22 WEUR Durable Objects incident, which did not list it.
- Cross-namespace forks.
- Any off-Cloudflare runtime for Artifacts.
