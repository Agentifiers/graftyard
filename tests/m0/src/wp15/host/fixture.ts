// The M0-WP10 bed: one install's Authz rows, a base repository, and its workspace fork, with the tasks, leases, policy,
// and rulesets the B0-36 and B0-37 rows need. Shared by the host's `/m0/setup`, the drivers, and the unit tests.

import type { AgentTask, Authz, Credential, Fold, Lease, OrgPolicy } from "./model.ts";

export const HOST = "graft-wp15-host.<workers-subdomain>.workers.dev";
export const BASE = "acme/mono";
export const FORK = "acme-agents/mono";

export const APP = { agent: 1, workspace: 2, claude: 3, cursor: 4, mcp: 5, insights: 6 } as const;
export const INST = { agent: 101, workspace: 102, claude: 103, cursor: 104, mcp: 105, insights: 106, wide: 107 } as const;
/** M0-WP13: a repository inside graftyard-agent's installation that is no task's repository. */
export const OTHER = "acme/other";
/** M0-WP13: the hooks Worker (run/agent.ts deploys it as graft-wp13-hooks); each App's webhook URL is a path on it. */
export const HOOKS_HOST = "graft-wp15-aux.<workers-subdomain>.workers.dev/hooks";
export const MCP_CLIENT = { id: "Iv1.graftyardmcp", secretLabel: "mcp-client-secret" } as const;
const ALL = { contents: "write", pull_requests: "write", issues: "write", statuses: "write", checks: "write", actions: "write", administration: "write" } as const;
export const USER = { alice: 10, bob: 11, carol: 12, mallory: 13, dave: 14 } as const;

/** Task sessions. T1 is the agent under test, T2 holds the Exclusive lease, T3 is cancelled during the run (fork mode),
 *  T4 is completed, and the External tasks belong to the confined vendor App. */
export const T1 = "11111111-1111-4111-8111-111111111111";
export const T2 = "22222222-2222-4222-8222-222222222222";
export const T3 = "33333333-3333-4333-8333-333333333333";
export const T4 = "44444444-4444-4444-8444-444444444444";
export const X1 = `${APP.claude}.fix-1`;
export const X2 = `${APP.claude}.old-1`;
export const LEASE_PATHS = ["pkg/auth/**"];

export function authz(host = HOST): Authz {
  return {
    host,
    apps: { [APP.agent]: { slug: "graftyard-agent" }, [APP.workspace]: { slug: "graftyard-workspace" }, [APP.claude]: { slug: "claude" },
            [APP.cursor]: { slug: "cursor" }, [APP.mcp]: { slug: "graftyard-mcp" }, [APP.insights]: { slug: "graftyard-insights" } },
    agentApps: [
      { app: APP.agent, kind: "GraftyardRun", confine: null },
      { app: APP.workspace, kind: "GraftyardRun", confine: null },
      { app: APP.claude, kind: "ThirdParty", confine: "refs/heads/claude/**" },
      { app: APP.cursor, kind: "ThirdParty", confine: null },
      { app: APP.mcp, kind: "Mcp", confine: null },
    ],
    // Permissions from design/AGENT-LAYER.md, "First-party Apps". The vendor and the flagged App asked for broad
    // permissions, so their requests reach `decide`. INST.wide is graftyard-workspace on an installation an admin
    // widened to every permission (a bad-config case), used where the real ceiling refuses before `decide` runs.
    installations: { [INST.agent]: { app: APP.agent, active: true, perms: { contents: "read", issues: "write", pull_requests: "write", checks: "read", statuses: "read", actions: "read" } },
                     [INST.workspace]: { app: APP.workspace, active: true, perms: { contents: "write" } },
                     [INST.wide]: { app: APP.workspace, active: true, perms: ALL },
                     [INST.claude]: { app: APP.claude, active: true, perms: ALL },
                     [INST.cursor]: { app: APP.cursor, active: true, perms: ALL },
                     [INST.mcp]: { app: APP.mcp, active: true, perms: { contents: "write", issues: "write", pull_requests: "write", checks: "read", statuses: "read", actions: "read" } },
                     [INST.insights]: { app: APP.insights, active: true, perms: { checks: "write", issues: "write", pull_requests: "read" } } },
    users: { [USER.alice]: { login: "alice" }, [USER.bob]: { login: "bob" }, [USER.carol]: { login: "carol" },
             [USER.mallory]: { login: "mallory" }, [USER.dave]: { login: "dave" } },
    workspaceApp: APP.workspace,
    controllerApp: APP.agent,
    // Webhooks as the manifests in design/AGENT-LAYER.md, "First-party Apps", subscribe them. The flagged App asks for
    // every event, `graftyard_task` included, so the first-party-only rule is what keeps that delivery from it.
    hooks: { [APP.agent]: { url: `https://${HOOKS_HOST}/${APP.agent}`, events: ["issues", "issue_comment", "pull_request", "pull_request_review", "check_run", "status", "graftyard_task"] },
             [APP.claude]: { url: `https://${HOOKS_HOST}/${APP.claude}`, events: ["issues", "issue_comment", "pull_request"] },
             [APP.cursor]: { url: `https://${HOOKS_HOST}/${APP.cursor}`, events: ["*", "graftyard_task"] },
             [APP.insights]: { url: `https://${HOOKS_HOST}/${APP.insights}`, events: ["issues", "pull_request"] } },
    tokens: {},
  };
}

/** Installation repositories for M0-WP13 (fork mode: graftyard-workspace is installed only on the workspace org). */
export function withRepos(a: Authz): Authz {
  const r: Record<number, string[]> = { [INST.agent]: [BASE, OTHER, FORK], [INST.workspace]: [FORK], [INST.claude]: [BASE, OTHER],
    [INST.cursor]: [BASE, OTHER], [INST.mcp]: [BASE, OTHER], [INST.insights]: [BASE] };
  for (const [i, repos] of Object.entries(r)) a.installations[i].repos = repos;
  return a;
}

/** The org's `OrgPolicy`: `agents_enabled` and `code_mode` off by default; alice is the org admin. */
export const orgPolicy = (): OrgPolicy => ({ org: "acme", agentsEnabled: false, codeMode: false, workspace: "Fork", admins: [USER.alice] });
/** `McpGrant` rows: bob's grant allows WriteApproveMerge, carol's allows Write. */
export const MCP_GRANTS: Record<string, { user: number; write: "Read" | "Write" | "WriteApproveMerge" }> = {
  "g-bob": { user: USER.bob, write: "WriteApproveMerge" }, "g-carol": { user: USER.carol, write: "Write" } };

const task = (session: string, kind: AgentTask["kind"], state: AgentTask["state"], app: number, inst: number, at: number): AgentTask =>
  ({ session, kind, state, prefix: kind === "GraftyardRun" ? `refs/heads/graftyard/${session}/` : null, requester: USER.alice,
     app, installation: inst, updatedAt: at });

export function baseFold(at: number): Fold {
  const lease: Lease = { id: "01JLEASE00000000000000000T2", holder: { actor: { t: "App", app: APP.agent, installation: INST.agent }, app: null, session: T2 },
                         scope: { paths: LEASE_PATHS, refs: [] }, mode: "Exclusive", intent: "auth rewrite", granted: at, expires: at + 3_600_000 };
  return {
    repo: BASE, network: "n1", defaultBranch: "refs/heads/main", forkOf: null, refs: {},
    tasks: { [T1]: task(T1, "GraftyardRun", "InProgress", APP.agent, INST.agent, at), [T2]: task(T2, "GraftyardRun", "InProgress", APP.agent, INST.agent, at),
             [T3]: task(T3, "GraftyardRun", "InProgress", APP.agent, INST.agent, at), [T4]: task(T4, "GraftyardRun", "Completed", APP.agent, INST.agent, at),
             [X1]: task(X1, "External", "InProgress", APP.claude, INST.claude, at), [X2]: task(X2, "External", "Cancelled", APP.claude, INST.claude, at) },
    leases: [lease],
    policy: { ci: "Approve", labels: ["agent:ok", "needs-review"], extraPaths: ["out/**"], extraHumanApprovals: 0, leases: "AgentsExclusive" },
    prs: {}, nextPr: 1,
    rulesets: [{ id: 7, refs: ["refs/heads/main"], bypass: [APP.agent, APP.workspace, APP.claude, APP.cursor] },
               { id: 8, refs: ["refs/heads/graftyard/**", "refs/heads/claude/**"], bypass: [APP.agent, APP.workspace, APP.claude] }],
    collaborators: { [`u${USER.alice}`]: "admin", [`u${USER.bob}`]: "write", [`u${USER.carol}`]: "write", [`u${USER.dave}`]: "write", [`u${USER.mallory}`]: "read" },
  };
}

export function forkFold(): Fold {
  return { repo: FORK, network: "n1", defaultBranch: "refs/heads/main", forkOf: BASE, refs: {}, tasks: {}, leases: [],
           policy: { ci: "Approve", labels: [], extraPaths: [], extraHumanApprovals: 0, leases: "Advisory" },
           prs: {}, nextPr: 1, rulesets: [], collaborators: {} };
}

/** Every credential the drivers use, by label. */
export const CREDS: Record<string, Credential> = {
  // Graftyard-run: graftyard-agent for API actions, graftyard-workspace for pushes, both carrying T1's task claim.
  "grun-agent": { t: "Installation", app: APP.agent, installation: INST.agent, task: T1 },
  "grun-ws": { t: "Installation", app: APP.workspace, installation: INST.workspace, task: T1 },
  "grun-ws-notask": { t: "Installation", app: APP.workspace, installation: INST.workspace, task: null },
  "grun-ws-done": { t: "Installation", app: APP.workspace, installation: INST.workspace, task: T4 },
  "grun-ws-t3": { t: "Installation", app: APP.workspace, installation: INST.workspace, task: T3 },
  "grun-agent-t3": { t: "Installation", app: APP.agent, installation: INST.agent, task: T3 },
  "grun-ws-t2": { t: "Installation", app: APP.workspace, installation: INST.workspace, task: T2 },
  "grun-wide": { t: "Installation", app: APP.workspace, installation: INST.wide, task: T1 },
  // Confined vendor App (ActorKind::Agent through `confine`).
  "vendor": { t: "Installation", app: APP.claude, installation: INST.claude, task: null },
  // Flagged App without confine (Bot, Agent admission), MCP user-to-server token, plain user tokens.
  "flagged": { t: "Installation", app: APP.cursor, installation: INST.cursor, task: null },
  "mcp-bob": { t: "UserToServer", user: USER.bob, app: APP.mcp, write: "Write" },
  "mcp-bob-am": { t: "UserToServer", user: USER.bob, app: APP.mcp, write: "WriteApproveMerge" },
  "alice": { t: "Pat", user: USER.alice },
  "bob": { t: "Pat", user: USER.bob },
  "carol": { t: "Pat", user: USER.carol },
  "dave": { t: "Pat", user: USER.dave },
  // M0-WP13.
  "mallory": { t: "Pat", user: USER.mallory },
  "vendor-user": { t: "UserToServer", user: USER.carol, app: APP.claude, write: null },
  "jwt-agent": { t: "AppJwt", app: APP.agent }, "jwt-workspace": { t: "AppJwt", app: APP.workspace },
  "jwt-claude": { t: "AppJwt", app: APP.claude }, "jwt-cursor": { t: "AppJwt", app: APP.cursor }, "jwt-insights": { t: "AppJwt", app: APP.insights },
  "insights": { t: "Installation", app: APP.insights, installation: INST.insights, task: null },
  "bob-meta": { t: "Pat", user: USER.bob, perms: { metadata: "read" } },
  "bob-fg": { t: "Pat", user: USER.bob, perms: { metadata: "read", contents: "write", issues: "write", pull_requests: "write" } },
  "mcp-client": { t: "UserToServer", user: USER.bob, app: APP.mcp, write: null },
};
