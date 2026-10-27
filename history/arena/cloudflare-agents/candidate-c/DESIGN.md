# Tributary agent layer, candidate C, an agent-native coordination host behind an unchanged GHES wire

## Summary

Tributary answers the blog's agent questions inside the host. "Who is working on what" is a live feed per repository. "Which agent branches will collide" is computed with `git merge-tree` in the git cache shards, which already hold every branch of a network (`cloudflare/DESIGN.md` lines 304 to 319). "How do reviewers cope with many agent changes" is an opt-in merge gate plus a session record behind every agent commit. "Why was this changed" resolves from a commit to a session through trailers, `refs/notes/ai`, or the pull request. All of it lives in one new Worker script, `trib-agents`, and under `/api/tributary/`. No GHES 3.22 path, default, or response changes for a client that does not opt in ("GHES compatibility") [inference from that table].

Safety comes first. No new component sits on the commit or read path of a GHES request. Every agent write, from git, REST, GraphQL, MCP, or the task API, ends in the base's `commit`, whose `decide` checks rulesets (`cloudflare/DESIGN.md` lines 215 to 228). `trib-agents` holds no member binding, so it cannot write around `decide`. Agent actors can never approve, merge, mark ready, be a bypass actor, or write outside their branch prefix. Coordination is advisory, so when it fails, agents and humans fall back to plain GHES behavior [inference from Shape].

Load. A1 (500 agents on one monorepo) puts 18.9 write commands a second on the repository's group, 6% of the base's Milestone 0 group-commit gate of 300 commands a second (`cloudflare/DESIGN.md` line 765), and 387 requests a second on its leader, 39% of the 1,000 req/s soft limit (`research/cloudflare.md`, Durable Objects). Both come from `cost_agents.py --detail` with [guess] per-task constants. The group is not the bottleneck. The network's git cache shard is, at 174% of one `standard-4`, because the base maps each network to one shard per site (`cloudflare/DESIGN.md` line 313) [guess vCPU per operation]. So I change the base in four places, with numbers in "Journeys" and "Scale". Hot networks spread over K shards per warm site. Group commit admits commands by class, so an agent burst cannot starve merge-queue entries. Hot-network clone-cache entries are written to both warm sites' buckets. Tributary-run agent Sandboxes get their own account. GitHub-style per-reference write scaling (`research/agents.md` finding 3.2) is designed here as delegated ref lanes, including what lanes do to atomic pushes, `ls-refs`, and the merge queue. Lanes are built only if a Milestone 0 row at 5,000 agents fails, because at A1's mix they would move only 4.6 of 18.9 commands off the parent. Pull request state stays with the base branch's group [inference].

Artifacts has no role ("Artifacts' role"). The layer adds $13, $126, and $1,457 a month at 50, 300, and 2,000 developers (`cost_agents.py`). The largest line is container egress for agent clones that miss the clone cache.

## Artifacts' role

None, in this design. The canonical-store rejection stands unchanged. New repositories cap at 1 GB against the 5 GB monorepo, a repository is "a single logical instance", there is no SLA because beta is excluded, and no RPO, RTO, or placement beyond `eu` and `us` is stated (`research/artifacts-recheck.md`, Answer and section 1). Two of the base's objections are weaker than written and should be corrected. v1-only push is not visible to any unmodified client, and `filter` is documented as unsupported only for v1 (`research/artifacts-recheck.md`, implication 3).

The live question is the agent workspace tier (`research/artifacts-recheck.md`, implication 2), a per-task fork with a short-lived token whose result comes back through Tributary's receive path. I reject it for five reasons.

1. Coordination needs every open agent branch in one object store. Prediction runs `merge-tree` inside the network's shard. Branches in per-task Artifacts repositories sit outside it, so each prediction would fetch first, against 200 git requests a second per repository (`research/artifacts-recheck.md` section 1, limits row) [inference].
2. Safety at write time. An Artifacts token covers the whole repository, with no branch, path, or ref scope and no pre-receive veto (`research/artifacts-recheck.md` section 3). A3 needs a prefix-only push that `decide` checks at the moment of the write. A workspace tier only checks at bring-back, after the agent has written what it wanted [inference].
3. Size and fork cost. Forks are asynchronous, return an `objects` count that suggests copying, and are capped at 1 GB like any new repository (`research/artifacts-recheck.md` section 3) [inference]. A 5 GB monorepo cannot be forked at all. A GHES fork in the base already shares the network's packs and keeps its refs on its own group (`cloudflare/DESIGN.md` journey 2), which is the isolation Artifacts sells, at no extra storage (`research/agents.md` 8.4).
4. Install fit. No off-Cloudflare runtime for the binding is known (`research/artifacts-recheck.md` open question 12), so evaluation on workerd and the lifeboat could not carry agent work.
5. A second git store splits provenance, vault, lifeboat, and `RestoredTo`, which cover only Tributary's store (`cloudflare/DESIGN.md` RPO table) [inference].

Cost is not the reason. 6,000 tasks a day at about 6 operations each is about 1.1 million operations, or about $163 a month before storage (`research/artifacts-recheck.md` pricing row) [inference, arithmetic].

**Flip condition.** Artifacts earns one role, a scratch tier for speculative attempts that never become pull requests (best-of-N, `research/agents.md` 3.5), when all three hold. Milestone 0 question 2 shows forks share objects with their source (`research/artifacts-recheck.md` section 4). Cloudflare raises the per-repository cap above the install's largest network in writing. Artifacts reaches GA under an SLA. Discarded attempts would then stay off Tributary's groups and shards, and only the chosen attempt would be pushed through `commit`. Even then no branch that a pull request points at would live in Artifacts [inference].

## Shape

`trib-agents` is one Worker script per install or managed cell. It is not a site script, so its deploys never reset chain members (`cloudflare/DESIGN.md` lines 112, 685). It defines the `RepoCoord` and `TaskAgent` Durable Object classes, the `TaskFlow` Workflow, the MCP handler, and `/api/tributary/v1`. Its objects hold nothing canonical, so it rolls like the edge (1%, 10%, 50%, 100%), with a signed `AgentsPointer` swap-back modeled on `EdgePointer` [inference]. The edge forwards `/api/tributary/*` to it over a service binding, and it calls the edge's handlers back over a service binding for every GHES operation. Tributary-run agents run in Sandboxes in an agent account that is separate from the base's runner account. Products are the base's own (Workers, Durable Objects, Workflows, Queues, Sandbox SDK, R2), with no KV, AI Gateway, Agents SDK, or Artifacts.

```rust
// ---- Actors (chain; base change) ----
pub enum ActorKind { User, Bot, Agent(AgentActor) }
/// Tributary-run agents, plus any app an admin flags as an agent (opt-in). For an `Agent`, `decide` refuses an APPROVE
/// review, `Merge`, ready-for-review, ruleset and bypass-list edits, any ref update outside `prefix` and its own
/// session ref, and changes under PROTECTED_PATHS unless the repository's agent policy allows them.
pub struct AgentActor { pub app: AppId, pub prefix: RefPattern /* refs/heads/tributary/* */, pub requester: Option<UserId> }
pub const PROTECTED_PATHS: [&str; 5] = [".github/workflows/**", "**/AGENTS.md", ".github/agents/**",
                                        ".github/copilot-instructions.md", ".github/instructions/**"];

// ---- Admission (base change to the leader's group commit) ----
pub enum CommandClass { Merge, Human, Bot, Agent, System }
/// Each entry first takes queued Merge and Human commands, then the rest in arrival order. The Agent class may use at
/// most AGENT_SHARE of the group's measured command rate. Over it, receive-pack answers `ng <ref> rate limited` and
/// REST answers 429 with `retry-after`, the shapes GitHub already uses (`research/agents.md` R9).
pub const AGENT_SHARE: f32 = 0.6;                                 // [guess]

// ---- Hot networks (base change to `shard_for`) ----
/// A network whose home-shard work passes HOT_AT for 10 minutes gets K shards per warm site by rendezvous top-K.
/// Pushes validate on the first, upload-pack spreads over all K. `max_instances` per site reserves one spare per hot
/// network, so growing K needs no deploy. K falls after 24 h below HOT_AT / 2.
pub fn shards_for(network: NetworkId, site: SiteId, count: u16, k: u8) -> ArrayVec<GitCacheShard, 4>;
pub const HOT_AT: f32 = 0.7;                                      // share of one instance's vCPU [guess]

// ---- Ref lanes (designed; built only if Milestone 0 row 3 fails) ----
/// A config command on the repository group. Refs under `prefix` hash to `ChainId::Lane(repo, i)`, a three-site group
/// homed with its parent. A push routed under an older `version` is refused by the lane and retried by the edge.
pub struct Delegation { pub prefix: RefPattern, pub lanes: u16, pub version: u32 }
pub enum LaneCommand { Update(Push), Prepare { txn: TxnId, updates: SmallVec<[RefUpdate; 4]>, until: Timestamp },
                       Resolve { txn: TxnId, commit: bool } }
/// `git push --atomic` across groups. Lanes commit `Prepare`, which holds their refs; the parent commits `Decide`
/// with its own updates; lanes commit `Resolve`. A lane past `until` reads the parent's decision through `fresh`.
pub struct Decide { pub txn: TxnId, pub commit: bool, pub own: SmallVec<[RefUpdate; 4]> }
/// `ls-refs` reads the lanes its `ref-prefix` arguments name, then the parent, and shows a prepared ref at its new value
/// only if the parent fold holds `Decide { commit: true }`. A resolved lane implies a decided parent, so reading lanes
/// first gives one atomic snapshot [inference].
pub async fn advertise(parent: &Stub, lanes: &[Stub], req: &LsRefs) -> Result<RefList, GitError>;

// ---- Sessions and provenance (repository group, metadata tier) ----
pub struct SessionId(Uuid);                                       // also the task id for Tributary-run agents
// RepoEvent gains:
//   SessionOpened { id, actor: ActorId, requester: Option<UserId>, issue: Option<IssueNumber>, model: Option<Box<str>>,
//                   instructions: Digest /* the AGENTS.md and profile blobs the agent read at start */ }
//   SessionClosed { id, state: TaskState, pr: Option<PrNumber>, log: Option<Oid> /* blob under SESSION_REFS */ }
pub const SESSION_REFS: &str = "refs/tributary/sessions/";        // advertised only to a ref-prefix that names it
/// Filled by the shard during `validate` from the new commits' trailers and any `refs/notes/ai` update in the same
/// push. A parse failure leaves it empty and never fails the push. `Push` gains `provenance`; `Merged` links the
/// merge or squash commit to the pull request's sessions.
pub struct Provenance { pub commit: Oid, pub sources: SmallVec<[Source; 2]> }
pub enum Source { Trailer { key: TrailerKey, value: Box<str> }, AiNote { note: Oid }, Session(SessionId) }
pub enum TrailerKey { TributarySession, ClaudeSession, AmpThreadId, CoAuthoredBy, AssistedBy }

// ---- Coordination (trib-agents; derived and advisory) ----
/// One object per repository named "coord:{repo}:{n}", n = 0 hinted at the group's home site and n = 1 at its next
/// site. The edge uses n = 0 and moves to n = 1 after 3 missed 300 ms deadlines in 10 s. Either one rebuilds from the
/// repository group's cursor (open pull requests and sessions) and from lease heartbeats.
pub struct RepoCoord;      // SQLite: leases, activity, branch_paths, predictions; hibernatable WebSockets
pub struct Lease { pub id: LeaseId, pub session: SessionId, pub scope: Scope, pub intent: Box<str> /* <= 280 B */,
                   pub expires: Timestamp /* 10 min, renewed by heartbeat */ }
pub enum Scope { Paths(SmallVec<[PathGlob; 4]>), Branch(RefName) }
/// Always grants and reports overlaps. Enforcing a lease would put this object on the push path.
pub struct LeaseAnswer { pub lease: Lease, pub overlaps: Vec<LeaseView> }
pub struct Activity { pub session: SessionId, pub actor: ActorId, pub branch: Option<RefName>, pub pr: Option<PrNumber>,
                      pub paths: PathSummary, pub state: TaskState, pub updated: Timestamp }
pub struct Prediction { pub a: RefName, pub b: RefName, pub a_tip: Oid, pub b_tip: Oid, pub base_tip: Oid,
                        pub verdict: Verdict, pub files: SmallVec<[PathBuf; 8]>, pub at: Timestamp }
pub enum Verdict { Clean, PathOverlap, Conflict, Stale }
impl GitCacheShard {
    /// `git merge-tree --write-tree --name-only` of `head` against up to 8 path-overlapping branches and the base tip.
    /// Runs on the network's second warm site under a compiled 50% vCPU cap, paused while that site leads the network.
    pub async fn predict(&self, head: Oid, base: Oid, against: &[(RefName, Oid)]) -> Result<Vec<Prediction>, GitError>;
}

// ---- Injection defense (beside GHES bodies, never in place of them) ----
pub struct AgentView { pub visible: String, pub hidden: Vec<Hidden>, pub author: Trust,
                       pub comments: Vec<AgentComment>, pub withheld: u32 /* comments by authors without write */ }
/// Removed from `visible`: comments, zero-width and bidi controls, raw HTML. Flagged in place: alt text, link titles.
pub struct Hidden { pub kind: HiddenKind, pub bytes: u32, pub sha256: Digest }   // content withheld
pub enum HiddenKind { HtmlComment, ZeroWidth, BidiControl, RawHtml, ImageAlt, LinkTitle }
pub enum Trust { Writer, Reader, Outside, Agent }                 // permission at read time, and ActorKind

// ---- Tasks, gates, budgets (trib-agents; states copy GitHub's agent tasks API, `research/agents.md` 1.5) ----
pub enum TaskState { Queued, InProgress, Completed, Failed, Idle, WaitingForUser, TimedOut, Cancelled }
pub struct Task { pub id: SessionId, pub repo: RepoId, pub issue: Option<IssueNumber>, pub requester: UserId,
                  pub profile: AgentProfile, pub budget: Budget, pub flow: WorkflowInstanceId }
pub struct Budget { pub model_usd_micros: u64, pub wall: Duration /* <= 6 h Sandbox cap */, pub pushes: u16,
                    pub prs: u8, pub rest_writes: u16 }
/// Gates are `step.waitForEvent` with GATE_TIMEOUT, released by a consumer of the repository group's cursor. The
/// authority is the chain record of the review or approval, never the Workflow.
pub enum Gate { RunWorkflows, ReadyForReview, BudgetRaise, UserReply }
pub const GATE_TIMEOUT: Duration = Duration::from_secs(7 * 86_400);
/// Sandbox egress (`research/agents.md` 6.7). Adds a 1-hour installation token narrowed to the task's repository with
/// contents, pull_requests, and issues write and checks read, minted by `trib-ops`. Allows only the host and the
/// install's configured model endpoints. Charges model spend to the `TaskAgent` before forwarding.
pub async fn outbound(req: Request, task: &TaskAgent) -> Result<Response, Denied>;

// ---- Remote MCP (trib-agents) ----
pub const MCP_PATH: &str = "/api/tributary/mcp";
pub const MCP_VERSIONS: [&str; 2] = ["2026-07-28", "2025-11-25"];   // stateless first; Streamable HTTP, no SSE
/// OAuth 2.1 at /api/tributary/oauth with PKCE S256, RFC 8707 `resource`, RFC 9728 metadata, and CIMD client ids.
/// Stored hashed in the `Tokens` groups, never KV. Accepted only at MCP_PATH.
pub struct McpGrant { pub user: UserId, pub client: Url, pub audience: Url, pub repos: SmallVec<[RepoId; 8]>,
                      pub perms: PermSet, pub expires: Timestamp /* 1 h; refresh 30 d, rotated on use */ }
/// Per call, the grant intersected with the tool's declared permissions and the user's current `Authz`, handed to
/// the edge handler in process. Writes then reach `commit` like any REST write.
pub fn narrow(g: &McpGrant, tool: &ToolSpec) -> CallerScope;
```

**Invariants.**

- The edge never awaits `trib-agents`, `RepoCoord`, a Workflow, or a prediction for any `/api/v3`, `/api/graphql`, or git request [inference from the routing above].
- Every agent write reaches `commit`. `trib-agents` has no member binding.
- An `Agent` never approves, merges, marks ready, bypasses, or writes outside its prefix and its own session ref (`decide`).
- Coordination state is rebuildable from the chain and heartbeats within 60 s [guess].
- Sessions, provenance, and logs are chain entries and packs, so the vault, lifeboat, `RestoredTo`, and export carry them (`cloudflare/DESIGN.md` RPO table).
- GHES `body` and `body_html` are never rewritten (`research/agents.md` R12).

## Journeys

**A1. Fleet on one monorepo.** The group runs on `Tri` with its leader on A (enam). Rates below come from `cost_agents.py --detail` [guess inputs].

| Load | Rate | Lands on | Headroom |
| --- | --- | --- | --- |
| Agent branch pushes (500 every 120 s) | 4.17/s | Repository group leader, one command each | Part of the 18.9/s total |
| PrSync, draft PR opens, REST writes, check writes, sessions | 4.17, 0.42, 4.17, 3.75, 1.25/s | Same group | Part of the total |
| Merge queue and bots (base) | 0.17 and 0.83/s | Same group | Admitted first in every entry |
| All write commands | 18.9/s | About 10 entries/s at one in flight (`cloudflare/DESIGN.md` line 465) | 15.9x against the 300/s gate |
| Inline pack bytes | 183 KB/s | Entries of at most 1.5 MB | 82x against 15 MB/s |
| Leader requests (CI ref reads 33, human API 200, agent reads 31, push ref reads 4, read-index 100, commands 19) | 387/s | Leader member | 2.6x against 1,000; learners take reads above 500/s |
| CI clones (2,000/min) on 250 new agent tips/min | 33/s, 8 clones per tip | Clone cache, so at best 87.5% hits [inference] | Misses land on shards |
| Home-site shard work (validation 2.3, misses 4.2, other 0.5 vCPU-s/s) | 6.96 vCPU-s/s | One `standard-4` in the base | 174%, so K = 3 per warm site at 70% |
| Conflict prediction | 3.33 vCPU-s/s | Second warm site | 2 of its 3 shards |
| `RepoCoord` | 13.5 req/s | One object | 74x |
| Agent Sandboxes plus agent CI jobs | About 500 plus 375 concurrent [guess] | Agent and runner accounts, 1,500 vCPU each | Separate accounts, so neither starves the other |

Writes to different refs never conflict in `decide`, because each `expect` is checked per ref (`research/agents.md` 8.3). They share only the pipeline. A synchronized burst of 500 pushes at 40 KB inline fills about 14 entries, so the last push acks after about 1.4 s, while each entry still carries any queued merge first [inference]. At 5,000 agents the same mix is 180 commands/s, 60% of the gate, which is Milestone 0 row 3. The merge queue is unchanged. `parent_tip` chaining and the tree-diff check run on the parent leader as in the base, at 0.17 merges a second.

**A2. Third-party agent vendor.** An admin follows the vendor's manifest link (for example Claude Code's, `research/agents.md` 2.1), and Tributary creates the app as GHES 3.22 does (R1). The vendor mints installation tokens narrowed by `repositories` and `permissions`, receives signed webhooks (R2), reads and writes check runs, and works through REST alone, which matters because Claude's hosted proxy refuses GraphQL (2.2). At the end it calls `DELETE /installation/token`, which commits a revocation on the token's shard, so the next write is refused (`cloudflare/DESIGN.md` journey 4). The vendor writes no Tributary-specific code. Tributary still sees its work. Branch prefixes such as `claude/` and trailers such as `Claude-Session` (5.5, 5.6) open inferred sessions in `RepoCoord`, and the feed shows them. If the admin flags the app as an agent (opt-in), the `Agent` invariants and budgets apply to it. Self-hosted and BYOC installs keep rate limits off by default, as GHES does, and need a Tunnel or IP allowlist for the vendor's inbound calls (R9, R23).

**A3. Tributary-run agent on an issue.**

1. A user assigns the issue to `tributary-agent[bot]` through `POST /repos/{o}/{r}/issues/{n}/assignees` or GraphQL `replaceActorsForAssignable` with `agentAssignment` (`research/agents.md` 1.3, 1.4). `suggestedActors(capabilities: [CAN_BE_ASSIGNED])` lists it once the app is installed, and never lists `copilot-swe-agent` (R5).
2. The assignment commits on the repository group. A cursor consumer starts `TaskFlow`, whose first step commits `SessionOpened`. `TaskAgent` starts a Sandbox in the agent account and sets a 60 s heartbeat alarm under the 6-hour cap (R21).
3. The Sandbox clones and pushes through `outbound`, which holds the token, so the sandbox never sees one (`research/agents.md` 6.7). Every commit carries `Tributary-Session: https://<host>/_tributary/sessions/<id>`. `decide` refuses any ref outside `refs/heads/tributary/<issue>-<id>/` and any change under `PROTECTED_PATHS`.
4. The agent opens a draft PR through REST with the session link in the body. The audit event `pull_request.create` carries `actor_is_agent` and `agent_session_id` (R6). Actions runs for its pushes wait as `action_required` until a writer approves them, which releases the `RunWorkflows` gate.
5. The agent pushes its log to `refs/tributary/sessions/<id>` and commits `SessionClosed`. It cannot approve, merge, or mark ready (`decide`). Merge also needs `tributary/agent-gate` to succeed, which means an approval by a writer who is neither the agent nor the requester (`research/agents.md` R11).

**A4. Prompt injection.** An outside contributor files an issue whose body hides a payload in an HTML comment, as in "Comment and Control" (`research/agents.md` 4.11). A Tributary agent reads the issue only through `AgentView`. The comment is removed and listed as `Hidden { kind: HtmlComment, bytes, sha256 }`. The author is marked `Outside`, and comments from authors without write access are withheld, the two mitigations GitHub applies (4.12). The blast radius is bounded by the host in five ways [inference from Shape]. The sandbox holds no credential. Egress reaches only the host and the model endpoint. The token covers one repository. Writes stay inside the agent's prefix and outside workflows and instruction files. Actions secrets stay unreachable until a writer approves the run. The agent cannot merge, and its budget caps spend. Five things cannot be prevented. A payload in visible text or in repository files, including `AGENTS.md` on the default branch, still reaches the model. A subtly harmful diff still needs a human reviewer. Repository content can still leak into the agent's own PR, which only repository readers see. The model provider sees the content by design. A third-party agent reads the raw GHES `body` unless it calls `AgentView` itself.

**A5. Coordination.** Agent X (Tributary-run, MCP) takes a lease on `src/billing/**` with intent "fix rounding". Agent Y (a vendor app with no Tributary code) pushes `cursor/billing-fix` touching `src/billing/rate.rs`.

1. Y's push commits as usual. The cursor sends the event to `RepoCoord`, which records Y's changed paths and finds X's lease and X's open branch overlapping.
2. The second warm site's shard runs `merge-tree` of Y's tip against X's tip and the base tip, and reports `Conflict` in `rate.rs` within the 30 s target (Milestone 0 row 6).
3. X learns through the feed and the `coord_conflicts` tool. Y learns through a `tributary/coordination` commit status on its head, if the repository opted in, and Cursor reads checks (`research/agents.md` 2.5). Without the opt-in, Y learns only at merge.
4. At merge, the first PR through the merge queue lands. The second becomes unmergeable and leaves the queue, exactly as on GHES. `parent_tip` and the tree-diff check keep the base tip correct (`cloudflare/DESIGN.md` lines 242 to 245). The prediction turns `Stale`, reruns against the new base tip, and the feed asks the second agent to rebase.

**A6. Why.** Six months later a reviewer runs blame on a line and gets a commit. `GET /api/tributary/v1/repos/{o}/{r}/commits/{sha}/provenance` reads the metadata fold. A squash commit maps to its PR through `Merged`, and the PR maps to its sessions. A pushed commit maps through its `Tributary-Session`, `Claude-Session`, or `Amp-Thread-ID` trailer, or its `refs/notes/ai` note (`research/agents.md` 5.5, 5.8). The session record gives the requester, issue, model, a digest of the instructions the agent read, each gate and who released it, and the log blob, all from chain state and packs. Retention defaults to 13 months [guess]. A vendor's session URL may be dead by then, and Tributary cannot prevent that.

**A7. Remote MCP client.** The client POSTs to `https://<host>/api/tributary/mcp` and gets 401 with `WWW-Authenticate` naming `/.well-known/oauth-protected-resource/api/tributary/mcp` (RFC 9728). That document names the authorization server at `/api/tributary/oauth`. The client identifies itself with a CIMD URL, since dynamic registration is deprecated in spec 2026-07-28 (`research/agents.md` 7.1), and runs the authorization code flow with PKCE and `resource` set to the MCP URL (RFC 8707). The user signs in through the install's SSO and picks repositories and permissions. The token's audience is the MCP URL, so `/api/v3` refuses it. Each of the 30 stateless tool calls narrows the grant by the tool's needs and the user's current `Authz`, then calls the edge handler in process. Tools are curated (about 15, covering repository reads, `AgentView`, PRs, checks, coordination, provenance, and tasks), not Code Mode, which is experimental (6.10). A session costs about $0.000035 at list prices (`cost_agents.py`).

**A8. Installs and incidents.** BYOC and self-hosted run the same `trib-agents` script, agent account, and MCP endpoint in the customer's accounts, with no runtime call to the vendor ("Deployment modes"). During a gray regional DO incident like WEUR's 6 h 18 min (`research/cloudflare-uptime.md` finding 20), A1 behaves as follows.

- If the follower site is gray, the leader commits with the third site. One enam to apac round of about 180 ms [guess] gives about 5 entries a second, still enough for 18.9 commands a second at about 4 commands an entry [inference]. Prediction pauses if its shard is there, and the feed marks results `Stale`.
- If the leader site is gray, step-down and `suspect` move the group within the 20 s target, which stays `mitigated` until Milestone 0 (`cloudflare/DESIGN.md` line 426). In that window about 83 agent pushes and 667 CI ref reads may fail and retry, and no acknowledged push is lost. The second warm site carries the 6.96 vCPU-s/s home work on its own K = 3 shards. This design writes hot-network clone-cache entries to both warm sites' buckets, so a gray bucket costs a hedge rather than 33 misses a second on the shards [inference].
- `RepoCoord` objects homed on the sick site move to `n = 1` after 3 missed deadlines and rebuild within 60 s. Sandboxes there miss heartbeats and restart elsewhere from their last push, losing at most 2 minutes of work at A1's push interval. Gate releases wait if Workflows is hit, as it was in Denver (finding 20), and no merge proceeds without its chain approval.

## Scale

| Quantity | Value | Limit | Headroom or gate |
| --- | --- | --- | --- |
| Commands on the A1 group | 18.9/s; 180/s at 5,000 agents | 300 commands/s Milestone 0 gate | 15.9x; 1.7x at 5,000 |
| Leader requests, A1 | 387/s | 1,000 req/s soft per object | 2.6x, learners above 500 reads/s |
| Home-shard vCPU, A1 | 6.96 vCPU-s/s | 4 vCPU per `standard-4` | K = 3 at 70%; 2,000 developers need K = 2 on their largest network (`cost_agents.py`) |
| Lanes, if built | Move pushes and session-log pushes, 4.6 of 18.9/s | Parent keeps PRs, checks, and merges | 24% relief, which is why they wait for row 3 |
| `RepoCoord` per repository | 13.5 req/s at A1; feed coalesced every 1 s | 1,000 req/s per object | 74x |
| `TaskFlow` creations | 0.42/s at A1, and about 0.42/s at 2,000 developers' peak (6,000 tasks in 8 hours, doubled) [guess] | 100/s per workflow, 300/s per account (`research/agents.md` 6.5) | Over 200x |
| Waiting gates | Under 1,000 [guess] | "millions" waiting (6.5) | Not a limit |
| Agent Sandboxes | About 500 at A1 | 1,500 vCPU per account | Own account; quota request on day 0 |
| MCP | 6.25 calls/s at A1 | Stateless Worker | Per-grant limit of 10 calls/s [guess] |
| Session logs | 1.2 GB/day at 2,000 developers | Inline under 1 MiB, else a stored pack | As any push |

Cost grows sub-linearly in agents per repository for the coordination parts, because `RepoCoord` and lanes are per repository and hibernate when idle, and linearly for clones and pushes [inference from `cost_agents.py`].

## GHES compatibility

| Surface | Path or field | Tag |
| --- | --- | --- |
| Agent assignment | REST `POST .../issues/{n}/assignees`; GraphQL `agentAssignment` on four mutations | wire-compatible |
| Agent discovery | `suggestedActors(capabilities: [CAN_BE_ASSIGNED])` lists `tributary-agent` once installed | additive |
| Audit fields | `actor_is_agent`, `agent_session_id`, `programmatic_access_type` | wire-compatible |
| Agent actor rules | `decide` refusals for `ActorKind::Agent` only; third-party apps only if flagged | additive |
| Workflow approval for agent pushes | Run status `action_required` | wire-compatible |
| Agent merge gate | Commit status `tributary/agent-gate`; required for Tributary agents' PRs, opt-in elsewhere | additive, opt-in |
| Overlap notice to other agents | Commit status `tributary/coordination` | opt-in, off by default |
| Provenance trailers and notes | `Tributary-Session` trailer; `refs/notes/ai` carried (R7) | wire-compatible |
| Session refs | `refs/tributary/sessions/*`, advertised only to a `ref-prefix` naming it | opt-in |
| Ref lanes, if built | Same `ls-refs` and atomic results through `advertise` and `Decide` | wire-compatible |
| Agent-class admission | `ng ... rate limited`, 429 with `retry-after` | wire-compatible |
| Task API | `/api/tributary/v1/repos/{o}/{r}/tasks[/{id}]`, states from 1.5 | additive |
| Activity feed | `/api/tributary/v1/repos/{o}/{r}/activity` and its WebSocket | additive |
| Leases and predictions | `/api/tributary/v1/repos/{o}/{r}/leases`, `/conflicts` | additive |
| Provenance and session logs | `/api/tributary/v1/repos/{o}/{r}/commits/{sha}/provenance`, `/sessions/{id}[/log]` | additive |
| Injection view | `/api/tributary/v1/repos/{o}/{r}/issues/{n}/agent-view` and the PR and comment equivalents | additive |
| Agent policy and budgets | `/api/tributary/v1/repos/{o}/{r}/agent-policy` | additive |
| Remote MCP | `/api/tributary/mcp` | additive |
| OAuth for MCP | `/api/tributary/oauth/*`, `/.well-known/oauth-protected-resource/api/tributary/mcp`, `/.well-known/oauth-authorization-server/api/tributary/oauth` | additive [unverified that GHES 3.22 serves nothing there] |
| Web pages | `/_tributary/sessions/<id>`, a provenance panel in blame | additive |
| Not served | `/api/v3/agents/*`, the Copilot CLI offline proxy, rewritten `body` fields, markdown conversion on API paths | conflicting, so absent |

## Failure modes

Each cell gives blast radius, then containment. Base components keep their rows in `cloudflare/DESIGN.md`.

| Component | instance | data-center | region | provider-wide | control-plane | bad-deploy | bad-config | noisy-tenant |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Admission and hot shards | One shard. Others of K and the second warm site serve; rehydrate within 120 s | As instance | One warm site. The other is sized for home work; prediction pauses | As base git caches | Spare `max_instances` reserved, so K grows with no deploy; starts unknown | Ships on site days, gated on agent 429 rate at 1% | A low `AGENT_SHARE` slows agents only, never merges or humans | One fleet holds at most 60% of its group |
| Ref lanes, if built | As chain groups | As chain groups | As chain groups; prepared refs resolve from the parent after `until` | As chain groups | None; groups are named on first use | `Prepare` and `Decide` formats behind the two-release rule | Wrong `version` makes the lane refuse; the edge retries | A hot prefix gets more lanes |
| `RepoCoord` | One repository's feed for a restart | DO storage followers | Its repositories move to `n = 1`, rebuilt within 60 s | Feed down; GHES unaffected | None | `AgentsPointer` swap-back; rebuild from cursor | Wrong hint costs latency only | 1,000 req/s per object, 1 s coalescing, 2,000 sockets [guess] |
| Conflict predictor | One job; rerun on next push or 5-minute sweep | As instance | Paused, results `Stale` | None produced | None | Ships with the git image on container days; advisory | CPU cap compiled at 50% | 8 pairs per push, per-network budget |
| Sessions and provenance | As chain groups and pack buckets | As base | As base | As base | None | Parse failure leaves provenance empty; push succeeds | Short retention deletes logs, recoverable for 35 days by `RestoredTo` | Logs capped at 50 MB [guess] |
| Agent runtime (`TaskFlow`, `TaskAgent`, Sandbox, `outbound`) | Restart from last push | As instance | Tasks restart elsewhere, at most 2 min lost | No new tasks; vendor agents unaffected by this layer | New Sandboxes may not start [unknown]; tasks queue | Roll gated on task failure rate; image on its own day | Bad allowlist denies egress, so tasks fail closed | Per-org concurrency and budgets in its own account |
| Approval gates | Instance waits durably | As instance | Release delayed | Gates stuck; no merge without chain approval | None | A stuck Workflow leaves tasks `Idle`; merges depend on `decide` | Policy off leaves the admin's GHES rules | Creation 100/s per workflow |
| MCP and OAuth | Stateless retry | Anycast | Token reads on `Tri` groups | Down with everything | None; CIMD documents cached 24 h | Swap-back; conformance row per step | A wrong audience refuses all tokens, closed | 10 calls/s per grant [guess] |

## Deployment modes

| Component | Managed | BYOC | Self-hosted |
| --- | --- | --- | --- |
| `trib-agents`, `RepoCoord`, `TaskAgent` | One per cell; the front edge routes `/api/tributary/*` to the owner's cell edge | One per install account | Same as BYOC, no vendor token |
| Agent Sandboxes | A vendor agent account per cell | A customer-owned agent account ($5) | Same as BYOC |
| Model endpoints | Vendor-configured | Customer-configured in `outbound`'s allowlist | Same as BYOC |
| MCP and OAuth | On the front host; grants in the front `Tokens` groups | On the customer's zone | Same as BYOC |
| Base changes | Site days through `trib-ops` | Same | Same |
| Evaluation | `Solo` on Cloudflare runs everything | Same | workerd runs `RepoCoord`, provenance, and MCP; tasks need Workflows ("externally-supplied") and containers (`localDocker` is development-only), so none run (`cloudflare/DESIGN.md` lines 659 to 662) |
| Lifeboat | None by default | Serves provenance and session logs read-only, because they are chain state | Same as BYOC |

## Cost

`python3 arena/cloudflare-agents/candidate-c/cost_agents.py` imports `cloudflare/cost.py`, reruns its model with the agent workload folded into `ACTIVITY`, and adds the new components. Runner and model compute are excluded. Agent traffic is HTTPS, so the base's Spectrum delta is removed.

| Line | 50 | 300 | 2,000 |
| --- | --- | --- | --- |
| Base lines moved by agent load (rows written, R2 Class A, container egress and vCPU, vault, others) | $2 | $68 | $668 |
| Agent clones, container egress and vCPU on clone-cache misses | $3 | $41 | $537 |
| Hot-network shards (one extra `standard-4` on each warm site at 2,000) | $0 | $0 | $165 |
| Session logs, 13 months, three copies plus vault | $1 | $4 | $27 |
| Conflict prediction, `RepoCoord`, fan-out | $1 | $4 | $30 |
| Agent account, Workflows, `TaskAgent`, `outbound`, MCP | $6 | $9 | $30 |
| **Added per month** | **$13** | **$126** | **$1,457** |

That is 5%, 10%, and 16% of the base, or under a cent per agent task. A1 at 500 agents needs one more hot shard per warm site, $165 a month [inference from the script]. New lines ignore free allowances, which overstates them at 50 developers. The largest line rests on a 70% clone-cache hit rate and a 200 MB average agent clone at 2,000 developers [guess], both Milestone 0 measurements.

## Build order

### Milestone 0 rows for the agent layer

| Measurement | Pass | Fail |
| --- | --- | --- |
| 1. A1 replay on one `Tri` group with 500 agents pushing every 120 s, 18.9 commands/s, 10 merges/min, and 33 ref reads/s | Agent push commit p99 at most 400 ms; merge p99 within 10% of a run without agents | Push p99 over 2 s, or merge p99 up 50% |
| 2. Burst of 500 pushes in 1 s with merges queued | Every push acknowledged within 5 s; every merge in one of the next 2 entries | Any lost acknowledgment, or a merge delayed over 2 s |
| 3. 5,000 agents, 180 commands/s on one group | Commit p99 at most 150 ms | Over 400 ms, which builds lanes as Milestone A6 |
| 4. Hot network with 6.96 vCPU-s/s on K = 3 `standard-4` per warm site, then one warm site killed | Clone time to first byte p99 at most 1 s, validation p99 at most 2 s, before and after | Time to first byte p99 over 5 s |
| 5. Agent clone bytes, clone-cache hit rate, and vCPU per merge-tree | Recorded; `cost_agents.py` updated | None. Results set K and the clone line |
| 6. Prediction on 1,000 replayed branch pairs of a real monorepo | 100% of textual conflicts on current tips, at most 5% false conflicts, push to verdict p95 at most 30 s | Any missed conflict on current tips, or p95 over 120 s |
| 7. `RepoCoord` at 50 req/s with 2,000 hibernating WebSockets | Feed p99 at most 2 s, no `overloaded` | Any `overloaded` |
| 8. `RepoCoord` `n = 0` failing every call | Edge on `n = 1` within 10 s, leases restored within 60 s | Over 120 s |
| 9. 500 hidden payloads (comments, zero-width, bidi, raw HTML) through `AgentView` | 100% removed or flagged; GHES `body` byte-identical to a 3.22 fixture | Any payload in `visible`, or any `body` difference |
| 10. A Sandbox tries to read a token, push outside the prefix, edit `.github/workflows`, reach an unlisted host, and approve its own PR | 0 of 100 succeed, each | Any |
| 11. Agent ruleset bypass through git, REST, GraphQL, MCP, and the task API | 0 of 100 | Any |
| 12. MCP tokens with wrong audience, expired, revoked, or presented to `/api/v3` | 100 of 100 refused | Any accepted |
| 13. Workflows made unreachable while an agent PR awaits approval | 0 merges without the chain approval; release within 10 s p99 of the review once back | Any unapproved merge |
| 14. Gray site in A1, with 30% errors on one site's member calls, DOs, and bucket | At least 99% of agent pushes succeed within 3 attempts; feed back within 60 s | Under 96%, or any lost acknowledgment |
| 15. Lanes, only if built, with atomic pushes across parent and lane under leader kills and 1 million `ls-refs` reads | Zero torn snapshots; zero refs held 10 s past `until` | Any |

### Milestones

| Milestone | Built | Gate |
| --- | --- | --- |
| A1, after base Milestone 3 | `ActorKind::Agent` rules, protected paths, `tributary/agent-gate`, `action_required`, admission classes | Rows 1, 2, 10, and 11 rerun on the real code |
| A2 | Provenance in `Push`, sessions, hidden refs, provenance API, blame panel | 10,000 replayed agent commits all resolve to a session or PR, and a vault restore keeps them |
| A3 | `RepoCoord`, feed, leases, predictor, hot-network K | Rows 4, 6, 7, and 8 |
| A4 | `TaskFlow`, `TaskAgent`, agent account, `outbound`, budgets, gates | A3 end to end; row 13; budget overshoot at most one model call per task |
| A5 | MCP handler, OAuth, CIMD, tools | Row 12; the local GitHub MCP server and Claude Code's GHES manifest flow pass as oracles (`research/agents.md` 8.1) |
| A6, only if row 3 fails | Lanes, `Decide`, `advertise` | Row 15 and the base's 48-hour linearizability run with lanes |

## Tradeoffs accepted

- Leases are advisory. Enforcing them would put `RepoCoord` on the push path, a new shared dependency the uptime goal forbids. Agents that ignore leases learn at prediction time or at merge.
- Prediction is exact only for textual conflicts on current tips. Semantic conflicts that merge cleanly are not found.
- Prediction borrows the second warm site's idle CPU and pauses during a site loss, so coordination degrades first.
- Lanes are designed and not built. At A1 they move only 24% of commands, because pull request records stay with the base branch's group. If row 3 fails, the next ceiling after lanes is PR metadata, which the base's metadata child groups (Milestone 7) would have to carry [inference].
- Lanes, if built, turn rare cross-prefix atomic pushes into two-phase commits with two extra rounds, about 160 ms on `Tri` [inference].
- Hot networks cost one or two more warm shards per site, $165 a month at 2,000 developers.
- Agent-class admission returns 429 to agents under a burst, so agents retry while merges and humans do not wait.
- Third-party agents get raw GHES bodies unless they call `AgentView`. Rewriting `body` would break the wire.
- Budgets meter model spend only for Tributary-run agents, whose egress passes `outbound`. Vendor model spend is invisible to the host.
- `tributary/agent-gate` is required by default only for PRs by Tributary agents, so Renovate and Dependabot automerge keep working (`research/agents.md` R11).
- Session logs live in git packs, so deleting one is a ref deletion plus GC, not an instant purge.
- The MCP handler is written in Rust against the stateless spec rather than with `createMcpHandler`, so the team tracks spec revisions itself.

## Alternatives considered

- **Artifacts as the agent workspace tier.** Rejected in "Artifacts' role", with its flip condition.
- **Lanes on by default for every agent prefix.** Rejected because they add 2PC, fan-out reads, and groups for 24% relief at A1.
- **Moving agent PR records into lanes too.** It would offload nearly all 18.9 commands, but PR numbers are allocated per repository and `gh pr list` would fan out to every lane. It is a redesign of the base's PR model.
- **Enforced leases checked in `decide`.** Rejected because a `RepoCoord` outage would then stall pushes.
- **One fork per task (`research/agents.md` 8.4).** It spreads refs across groups for free, but GHES clients and vendors expect a branch in the base repository, and fork PRs change Actions secrets behavior. Vendors may still do it, and it already works.
- **`workers-oauth-provider` with KV grants.** Rejected because KV failed in June 2025 and lags up to 60 s (`research/cloudflare-uptime.md` findings 9 and 38). Grants live in `Tokens` groups.
- **AI Gateway `agent_id` spend limits (R19).** Not in `research/cloudflare.md`, and limited to 20 rules per gateway and eventually consistent (`research/agents.md` 6.8). `TaskAgent` meters per task exactly.
- **Agents SDK and `McpAgent`.** `McpAgent` is deprecated, and the Agents SDK states a 1 GB per-agent limit against 10 GB per object (6.1, 6.2). Plain Durable Objects do the job.
- **Code Mode over the GHES OpenAPI.** Experimental (6.10), and it would expose every write. Curated tools instead.
- **Rewriting `body` to strip hidden content for all clients.** Conflicting with GHES (R12).

## Open questions and risks

- The largest base change and the largest cost line both rest on [guess]ed vCPU per operation and clone-cache hit rate (rows 4 and 5).
- Does the base's 300 commands/s gate hold with inline packs near 1 MiB? A1's average is 40 KB [guess], but agents that commit generated files may not be.
- Where do Workflows instances and Sandboxes run during a regional incident? Neither is placement-pinned here, and Denver listed both Workflows and Containers (`research/cloudflare-uptime.md` finding 20).
- Which Workflows concurrency figure binds, 50,000 or 10,000 (`research/agents.md` could-not-verify 8)? Waiting gates may count.
- Is the Sandbox outbound handler billed as Worker requests? `cost_agents.py` assumes yes [unverified].
- Does GHES 3.22 serve anything at the `/.well-known` OAuth paths used for MCP [unverified]?
- Exact `AgentAssignmentInput` fields in 3.22 were not read in full [unverified].
- Do clients tolerate hidden refs that a `ref-prefix` can still name? No unmodified client asks for `refs/tributary/` [inference].
- Is 13-month session-log retention acceptable to customers whose logs may contain secrets? A per-org policy exists, and the default is a [guess].
- The hot-key clone-cache copy in two warm buckets is a small base change whose cost is under $5 a month [inference] and is not in the script.
