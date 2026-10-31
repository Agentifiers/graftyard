// M0-WP12: the bed for the layer rows. One fleet of A1 agents of one actor kind (build/BUILD-PLAN.md, L0-12, "run once
// per actor kind"), their credentials as the edge would verify them, their tasks, refs, and lease scopes, and the seed
// rows of `Authz(Genesis.account)` and the repository's core tier. Pure, so the runner and the load client share it.

import type { AgentAppRow, Task, Who, XEv } from "./lease.ts";

export type Kind = "grun" | "vendor" | "flagged" | "mcp" | "human";
export const KINDS: Kind[] = ["grun", "vendor", "flagged", "mcp", "human"];
/** Which kinds are `ActorKind::Agent`: a Graftyard-run App and a confined vendor App. */
export const AGENT_KINDS: Kind[] = ["grun", "vendor"];
export const APP = { grun: 101, vendor: 201, flagged: 301, mcp: 401, insights: 102 } as const;
export const INST = { grun: 1001, vendor: 2001, flagged: 3001, mcp: 4001 } as const;
const userOf = (kind: Kind, a: number) => (kind === "mcp" ? 50_000 : 60_000) + a;
export const ADMIN = 1;

/** The task an agent works for (its lease's `session`). */
export function session(kind: Kind, a: number): string {
  switch (kind) {
    case "grun": return `t${a}`;
    case "vendor": return `${APP.vendor}.t${a}`;
    case "flagged": return `${APP.flagged}.t${a}`;
    case "mcp": return `${APP.mcp}.t${a}`;
    case "human": return `h.t${a}`;
  }
}
/** The verified credential and session header of agent `a`'s token. */
export function who(kind: Kind, a: number): Who {
  switch (kind) {
    case "grun": return { cred: { t: "Installation", app: APP.grun, installation: INST.grun, task: session(kind, a) } };
    case "vendor": return { cred: { t: "Installation", app: APP.vendor, installation: INST.vendor, task: null }, session: `t${a}` };
    case "flagged": return { cred: { t: "Installation", app: APP.flagged, installation: INST.flagged, task: null }, session: `t${a}` };
    case "mcp": return { cred: { t: "UserToServer", user: userOf(kind, a), app: APP.mcp }, session: session(kind, a) };
    case "human": return { cred: { t: "User", user: userOf(kind, a) }, session: session(kind, a) };
  }
}
/** The ref agent `a` pushes: inside its task's prefix, its App's `confine`, or anywhere for the other kinds. */
export function ref(kind: Kind, a: number): string {
  switch (kind) {
    case "grun": return `refs/heads/graftyard/t${a}/task`;
    case "vendor": return `refs/heads/claude/${a}/task`;
    case "flagged": return `refs/heads/cursor/${a}/task`;
    default: return `refs/heads/u${a}/task`;
  }
}
/** Each agent's own Exclusive scope. */
export const scope = (a: number) => `pkg/${a}/**`;
export const changedPath = (a: number, n: number) => `pkg/${a}/f${n % 7}.go`;
export function task(kind: Kind, a: number): Task {
  const s = session(kind, a);
  switch (kind) {
    case "grun": return { session: s, kind: "GraftyardRun", state: "InProgress", prefix: `refs/heads/graftyard/t${a}/`, app: APP.grun, installation: INST.grun, requester: ADMIN };
    case "vendor": return { session: s, kind: "External", state: "InProgress", prefix: null, app: APP.vendor, installation: INST.vendor, requester: ADMIN };
    case "flagged": return { session: s, kind: "External", state: "InProgress", prefix: null, app: APP.flagged, installation: INST.flagged, requester: ADMIN };
    case "mcp": return { session: s, kind: "External", state: "InProgress", prefix: null, app: APP.mcp, installation: INST.mcp, requester: userOf(kind, a) };
    case "human": return { session: s, kind: "External", state: "InProgress", prefix: null, app: APP.grun, installation: INST.grun, requester: userOf(kind, a) };
  }
}
/** Another task's Exclusive scope, held by a Graftyard-run task, that the conflicting writes target. */
export const VICTIM = { session: "victim", scope: "victim/**", path: "victim/auth/session.go" };
export const victimWho = (): Who => ({ cred: { t: "Installation", app: APP.grun, installation: INST.grun, task: VICTIM.session } });
export const RACES = 20;
export const grunTask = (s: string): Task => ({ session: s, kind: "GraftyardRun", state: "InProgress", prefix: `refs/heads/graftyard/${s}/`, app: APP.grun, installation: INST.grun, requester: ADMIN });

const AGENT_APPS: AgentAppRow[] = [
  { app: APP.grun, kind: "GraftyardRun", confine: null },
  { app: APP.vendor, kind: "ThirdParty", confine: "refs/heads/claude/**" },
  { app: APP.flagged, kind: "ThirdParty", confine: null },
  { app: APP.mcp, kind: "Mcp", confine: null },
];

/** Seed commands for one group: `Authz` rows, `AgentsExclusive`, the fleet's tasks and users, the victim and idle
 *  tasks, and `prs` open PRs, each approved with green checks, for the merge queue. Chunked under the entry cap. */
export function seed(kind: Kind, agents: number, prs: number, extra: XEv[] = []): XEv[][] {
  const base: XEv[] = [
    ...AGENT_APPS.map((row) => ({ t: "AgentApp" as const, row })),
    ...Object.entries(INST).map(([k, installation]) => ({ t: "Install" as const, installation, app: APP[k as keyof typeof INST], active: true, push: true })),
    { t: "Policy", leases: "AgentsExclusive" }, { t: "Collab", user: ADMIN, perm: "admin" },
    { t: "TaskCreated", task: grunTask(VICTIM.session) }, { t: "TaskCreated", task: grunTask("idleA") }, { t: "TaskCreated", task: grunTask("idleB") },
    // L0-12's racing pairs, r<k>a and r<k>b for k < RACES.
    ...Array.from({ length: RACES * 2 }, (_, i) => ({ t: "TaskCreated" as const, task: grunTask(`r${i >> 1}${i % 2 ? "b" : "a"}`) })),
    ...extra,
  ];
  const out: XEv[][] = [base];
  let chunk: XEv[] = [];
  const add = (e: XEv) => { chunk.push(e); if (chunk.length >= 400) { out.push(chunk); chunk = []; } };
  for (let a = 0; a < agents; a++) {
    add({ t: "TaskCreated", task: task(kind, a) });
    if (kind === "mcp" || kind === "human") add({ t: "Collab", user: userOf(kind, a), perm: "write" });
  }
  for (let n = 1; n <= prs; n++) {
    add({ t: "PrOpened", n, head: `h${n}`, agentApp: null });
    add({ t: "Review", n, reviewer: ADMIN, state: "APPROVED", commit: `h${n}` });
    add({ t: "Check", sha: `h${n}`, name: "ci", conclusion: "success" });
  }
  if (chunk.length) out.push(chunk);
  return out;
}
