// M0-WP10: the agent block's types (design/DESIGN.md, Shape, "Agent load and agent safety"), in TypeScript, with the
// design's names. Rust enums become tagged unions with a `t` field. Pure data: no I/O, no clock.

export type AppId = number;
export type UserId = number;
export type InstallationId = number;
export type Oid = string;
export const ZERO: Oid = "0".repeat(40);

// ---- Actors ----
export interface AgentActor { app: AppId; task: string | null /* SessionId string form */; requester: UserId | null }
export type ActorKind = { t: "User" } | { t: "Bot" } | { t: "Agent"; agent: AgentActor };
export type AdmitClass = "Human" | "MergeQueue" | "Bot" | "Agent";
export type AgentKind = "GraftyardRun" | "ThirdParty" | "Mcp";
/** Install setting on `Authz(Genesis.account)`. `confine` is a `RefPattern`. */
export interface AgentApp { app: AppId; kind: AgentKind; confine: string | null }
export type Perm = "contents" | "pull_requests" | "issues" | "statuses" | "checks" | "actions" | "administration" | "metadata";
export type PermSet = Partial<Record<Perm, "read" | "write">>;
export type WriteScope = "Read" | "Write" | "WriteApproveMerge";
export type ActorId = { t: "User"; user: UserId } | { t: "App"; app: AppId; installation: InstallationId };

// ---- Tasks ----
export type TaskKind = "GraftyardRun" | "External";
export type TaskApiState = "Queued" | "InProgress" | "Completed" | "Failed" | "Idle" | "WaitingForUser" | "TimedOut" | "Cancelled";
export const FINAL_STATES: readonly TaskApiState[] = ["Completed", "Failed", "TimedOut", "Cancelled"];
/** The core-tier part of `AgentTask`, plus `updated_at` for TASK_IDLE. `session` is the SessionId string form: a UUID,
 *  or `<app_id>.<name>` for an External task. */
export interface AgentTask { session: string; kind: TaskKind; state: TaskApiState; prefix: string | null; requester: UserId | null;
                             app: AppId; installation: InstallationId; updatedAt: number;
                             /** M0-WP13: the metadata tier. `creator` is an actorKey, `snapshot` the create (or reassign) index. */
                             creator?: string; issue?: number | null; snapshot?: number; prompt?: string | null; pr?: number | null;
                             summary?: string; createdAt?: number; repo?: string }
export const TASKS_OPEN_PER_REQUESTER = 100;
export const TASKS_OPEN_PER_REPO = 2_000;
export const PROMPT_MAX = 64 << 10;
export const DEMOTE_PER_MIN = 60;

// ---- Issues, comments, and check runs (M0-WP13; the parts the trust view and the task rules read) ----
export interface IssueRev { index: number; title: string; body: string; actor: string }
export interface AssignedEntry { index: number; assignees: string[] /* logins, `<slug>[bot]` for Apps */; actor: string }
export interface Issue { number: number; author: string; revisions: IssueRev[]; assigned: AssignedEntry[]; createdAt: number }
export interface IssueComment { id: number; issue: number; author: string; body: string; createdAt: number }
export interface CheckRun { id: number; sha: Oid; name: string; author: string; title: string; summary: string; text: string }

// ---- Leases ----
export interface LeaseHolder { actor: ActorId; app: AppId | null; session: string | null }
export type LeaseMode = "Shared" | "Exclusive";
export interface LeaseScope { paths: string[] /* PathGlob */; refs: string[] /* RefPattern */ }
export interface Lease { id: string; holder: LeaseHolder; scope: LeaseScope; mode: LeaseMode; intent: string; granted: number; expires: number }
export type LeaseEnforce = "Advisory" | "AgentsExclusive";

// ---- Policy, gate, PR metadata ----
export interface AgentPolicy { ci: "Approve" | "AutoReadOnly"; labels: string[]; extraPaths: string[]; extraHumanApprovals: number; leases: LeaseEnforce }
export interface AgentGate { pr: number; head: Oid; satisfiedBy: UserId }
export interface AgentMeta { app: AppId; session: string | null; requester: UserId | null; repo: string }

// ---- validate's outputs ----
export interface ProtectedHits { refname: string; paths: string[] }
export interface ChangedPaths { refname: string; paths: string[]; truncated: boolean }
export type TrailerKey = "GraftyardSession" | "ClaudeSession" | "AmpThreadId" | "CoAuthoredBy" | "AssistedBy";
export type Source = { t: "Claimed"; key: TrailerKey; value: string } | { t: "AiNote"; note: Oid } | { t: "Session"; session: string };
export interface Provenance { commit: Oid; sources: Source[] }

// ---- Constants ----
export const SESSION_REFS = "refs/graftyard/sessions/";
/** Host-owned namespace: `decide` refuses every update under it except appends by `graftyard-workspace`. */
export const HOST_REFS = "refs/graftyard/";
export const PROTECTED_PATHS: readonly string[] = [".github/workflows/**", ".github/actions/**", ".gitmodules", "CODEOWNERS",
  ".github/CODEOWNERS", "docs/CODEOWNERS", "**/AGENTS.md", "**/CLAUDE.md", "**/GEMINI.md", ".github/agents/**",
  ".github/copilot-instructions.md", ".github/instructions/**", ".claude/**", ".cursor/**", ".cursorrules",
  ".codex/**", ".mcp.json", ".vscode/**"];
export const PATHS_MAX = 1_000;
export const EXCLUSIVE_MAX = 4 * 3_600_000;
export const TASK_IDLE = 86_400_000;
export const NOTES_REF = "refs/notes/ai";

// ---- The fold a group's `decide` reads (the parts these rows touch) ----
export interface Ruleset { id: number; refs: string[] /* RefPattern */; bypass: AppId[] }
export interface Pr { number: number; headRef: string; headOid: Oid; base: string; draft: boolean; open: boolean; merged: boolean;
                      meta: AgentMeta | null; gate: AgentGate | null; author: string /* actorKey */;
                      /** Every `AgentGate` row written on this PR (one per qualifying APPROVE), for the approval count. */
                      approvals?: AgentGate[]; title?: string; body?: string;
                      /** actorKey of everyone who moved the head ref while the PR was open. */ pushers: string[] }
export interface Fold {
  repo: string; network: string; defaultBranch: string; forkOf: string | null;
  refs: Record<string, Oid>;
  tasks: Record<string, AgentTask>; leases: Lease[]; policy: AgentPolicy;
  prs: Record<string, Pr>; nextPr: number; rulesets: Ruleset[];
  collaborators: Record<string, "read" | "write" | "admin">;
  /** M0-WP13. `index` is the group's last committed entry; each committed command advances it. */
  index?: number; issues?: Record<string, Issue>; comments?: IssueComment[]; checkRuns?: CheckRun[];
  /** Committed write commands per actorKey in the trailing minute (`at` values), for DEMOTE_PER_MIN. */
  writes?: Record<string, number[]>;
  /** Host-set pauses: a non-writer edited the task's issue after assignment (cleared by a writer's reassignment). */
  paused?: Record<string, number /* the edit's index */>;
  /** M0-WP15: the last pushed PathSet per agent ref, for the prototype's `graftyard/overlap` check (apps.ts). */
  pathSets?: Record<string, { paths: string[]; at: number }>;
}
/** `OrgPolicy` on the org's chain (design/DESIGN.md, Shape, agent block, after the entries). */
export interface OrgPolicy { org: string; agentsEnabled: boolean; codeMode: boolean; workspace: "Fork" | "InRepo"; admins: UserId[] }
/** Rows of the install's `Authz(Genesis.account)` chain that every App command lists in `deps()`. */
export interface Authz {
  host: string;
  apps: Record<string, { slug: string }>;
  agentApps: AgentApp[];
  /** Per installation: whether it is active, and its granted permissions (the ceiling a token cannot exceed). */
  installations: Record<string, { app: AppId; active: boolean; perms: Partial<Record<Perm, "read" | "write">>; repos?: string[] }>;
  /** M0-WP13: each App's webhook URL and subscribed events (GHES manifest), and its webhook secret's label. */
  hooks?: Record<string, { url: string; events: string[] }>;
  /** The install's controller App (`graftyard-agent`), the only receiver of the `graftyard_task` delivery. */
  controllerApp?: AppId;
  /** M0-WP13: the `Tokens` rows of scoped MCP tokens, by `jti` (grant and `WriteScope`), read by the host, not the claim. */
  tokens?: Record<string, { user: UserId; grant: string; write: WriteScope }>;
  /** `McpGrant` rows on users' `Identity` chains, by grant id. */
  grants?: Record<string, { user: UserId; write: WriteScope; client?: string; redirect_hosts?: string[]; generation?: number; revoked?: boolean; created_at?: string }>;
  users: Record<string, { login: string }>;
  /** The install's `graftyard-workspace` App. */
  workspaceApp: AppId;
}
/** `deps` folds the leader read once through `fresh`: the install's Authz chain, and the base repository's fold when a
 *  token's `task` claim names it (fork mode, `Push::deps()`). */
export interface Deps { authz: Authz; base: Fold | null; org?: OrgPolicy }

// ---- Credentials and principals ----
/** What the edge parsed from the token. `Installation` is a stateless `ghs_` with its claims. */
/** M0-WP13 adds the claims a minted token carries: `repos` and `perms` (SHARED's `InstallationClaims`), `jti` (the
 *  `Tokens` row of a scoped MCP token, which holds its grant and `WriteScope`), and fine-grained `perms` on a PAT.
 *  `AppJwt` is an App's own JWT, which only mints installation tokens. */
export type Credential =
  | { t: "Installation"; app: AppId; installation: InstallationId; task: string | null; repos?: string[]; perms?: PermSet; jti?: string; exp?: number }
  | { t: "UserToServer"; user: UserId; app: AppId; write: WriteScope | null; jti?: string; repos?: string[] }
  | { t: "Pat"; user: UserId; perms?: PermSet; jti?: string }
  | { t: "AppJwt"; app: AppId };
export interface Principal { id: ActorId; app: AppId | null; kind: ActorKind; admit: AdmitClass; cred: Credential["t"];
                             mcpWrite: WriteScope | null; sessionHeader: string | null; name: string;
                             /** M0-WP13: the token's own ceiling (null: the role's or installation's), and repository scope. */
                             perms?: PermSet | null; repos?: string[] | null; demoted?: boolean }

// ---- Commands ----
export interface RefUpdate { name: string; old: Oid | null; new: Oid | null }
/** Per-update facts from `validate` that `decide` needs beside the three named outputs. */
export interface UpdateFacts { name: string; ff: boolean /* old is an ancestor of new, or a create */; missing: boolean;
                              /** M0-WP13: heads of open PRs with `AgentMeta` into this ref that the new tip contains. */ contains?: Oid[] }
export type Surface = "git" | "rest" | "graphql" | "task-api";
export type Cmd =
  | { t: "Push"; via: Surface; updates: RefUpdate[]; atomic: boolean; changed: ChangedPaths[]; protected: ProtectedHits[];
      provenance: Provenance[]; facts: UpdateFacts[] }
  | { t: "Review"; pr: number; event: "APPROVE" | "COMMENT" | "REQUEST_CHANGES" }
  | { t: "Merge"; pr: number; sha: Oid | null; mergeOid: Oid | null }
  | { t: "ReadyForReview"; pr: number }
  | { t: "RulesetEdit"; id: number }
  | { t: "BranchProtectionEdit"; branch: string }
  | { t: "RepositoryDispatch" }
  | { t: "WorkflowDispatch"; workflow: string; ref: string }
  | { t: "WorkflowRerun"; run: number }
  | { t: "ReleaseCreate"; tag: string }
  | { t: "LabelWrite"; issue: number; labels: string[] }
  | { t: "StatusCreate"; sha: Oid; context: string }
  | { t: "CheckRunCreate"; sha: Oid; name: string; output?: { title: string; summary: string; text: string } }
  | { t: "BranchRename"; from: string; to: string }
  | { t: "HookEdit" }
  | { t: "DeployKeyEdit" }
  | { t: "PrOpen"; head: string; base: string; draft: boolean; body?: string }
  | { t: "TaskCancel"; session: string }
  // M0-WP13: the task API and the issue commands its rules read. `id` is the host-minted UUID, passed in so decide is pure.
  | { t: "TaskCreate"; kind: TaskKind; id: string; issue: number | null; prompt: string | null; name: string | null;
      requester: string | null /* login, External only */; summary: string }
  | { t: "TaskPatch"; session: string; state: TaskApiState | null; pr: number | null; summary: string | null }
  | { t: "IssueOpen"; title: string; body: string }
  | { t: "IssueEdit"; n: number; title: string | null; body: string | null }
  | { t: "Assign"; n: number; assignees: string[] }
  | { t: "CommentCreate"; issue: number; body: string };

export type RefusalRule = "Approve" | "Merge" | "ReadyForReview" | "Bypass" | "Dispatch" | "Rerun" | "Release" | "Label"
  | "GraftyardStatus" | "Rename" | "Hook" | "DeployKey" | "OutsidePrefix" | "NoLiveTask" | "Protected" | "Leased" | "HostOwned"
  | "GateUnsatisfied" | "McpScope" | "DraftOnly" | "Stale" | "NotFound" | "Missing" | "Atomic" | "NotAllowed"
  // M0-WP13 task rules. `AgentsDisabled`, `Invalid`, and `TaskCap` are 422; `TaskCreator` and `TaskCaller` are 403;
  // `TaskFinal` and `TaskPaused` are 409.
  | "AgentsDisabled" | "TaskCreator" | "TaskCaller" | "Invalid" | "TaskCap" | "TaskFinal" | "TaskPaused"
  /** The token's repository list does not include the repository (GitHub answers 404). */
  | "Scope"
  /** Refused by the token's permission ceiling before `decide` runs (not an agent rule). */
  | "Ceiling";
export interface Refusal { rule: RefusalRule; text: string }

export type Event =
  | { t: "Refs"; updates: RefUpdate[]; actor: string }
  | { t: "Provenance"; list: Provenance[] }
  | { t: "PathSet"; refname: string; paths: string[]; truncated: boolean }
  | { t: "PrSync"; pr: number; head: Oid; pusher: string }
  | { t: "Review"; pr: number; event: string; by: string }
  | { t: "AgentGate"; gate: AgentGate }
  | { t: "PrMerged"; pr: number }
  | { t: "PrReady"; pr: number }
  | { t: "PrOpened"; pr: Pr }
  | { t: "Labeled"; issue: number; labels: string[] }
  | { t: "Status"; sha: Oid; context: string }
  | { t: "Recorded"; what: string }
  | { t: "TaskUpdated"; session: string; state: TaskApiState }
  | { t: "LeaseRelease"; ids: string[] }
  | { t: "TaskCreated"; task: AgentTask }
  | { t: "TaskPatched"; session: string; state: TaskApiState | null; pr: number | null; summary: string | null; paused?: boolean; snapshot?: number }
  | { t: "IssueOpened"; issue: Issue }
  | { t: "IssueEdited"; n: number; title: string | null; body: string | null; actor: string; nonWriter: boolean }
  | { t: "Assigned"; n: number; assignees: string[]; actor: string }
  | { t: "Commented"; comment: IssueComment }
  | { t: "CheckRunStored"; run: CheckRun }
  /** The first-party `graftyard_task` delivery, emitted in the task's own entry, sent only to the controller. */
  | { t: "Deliver"; event: string; action: string; session: string };

export type RefOutcome = { name: string; ok: true } | { name: string; ok: false; refusal: Refusal };
export type Decision =
  | { ok: true; events: Event[]; refs?: RefOutcome[] }
  | { ok: false; refusal: Refusal; refs?: RefOutcome[] };

export const actorKey = (id: ActorId): string => id.t === "User" ? `u${id.user}` : `a${id.app}i${id.installation}`;
