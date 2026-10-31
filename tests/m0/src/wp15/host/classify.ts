// Classification of the actor from the token and headers (design/DESIGN.md, Shape, the agent block, `ActorKind` and
// `AdmitClass`; design/AGENT-LAYER.md, `AgentApp`). Pure.

import type { ActorKind, AdmitClass, AgentApp, AgentTask, Authz, Credential, Fold, Principal, Provenance } from "./model.ts";
import { DEMOTE_PER_MIN } from "./model.ts";
import { classes } from "./provenance.ts";

export const agentApp = (authz: Authz, app: number | null): AgentApp | null =>
  app == null ? null : authz.agentApps.find((a) => a.app === app) ?? null;

/** Resolves a session string the way the host does: a UUID, `<app_id>.<name>`, or an App's own bare `name`. */
export function resolveSession(raw: string, callerApp: number | null): string {
  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(raw)) return raw;
  if (/^\d+\./.test(raw)) return raw;
  return callerApp != null ? `${callerApp}.${raw}` : raw;
}

/** The task a session names, looked up on the fold that owns tasks (the base's in fork mode). */
const taskOf = (folds: (Fold | null)[], s: string | null): AgentTask | null => {
  if (!s) return null;
  for (const f of folds) if (f && f.tasks[s]) return f.tasks[s];
  return null;
};

/**
 * `kind` is the refusal binding. `admit` is the admission lane. Rules, in the design's words:
 * - An App is `ActorKind::Agent` when it is a GraftyardRun App, or a flagged `AgentApp` with `confine`.
 * - A flagged App without `confine` is `Bot`, though its commands take the Agent admission class.
 * - `Graftyard MCP` and user tokens are never `ActorKind::Agent`.
 * - Admission is Agent when the actor is an `AgentApp` (MCP included), when an App token's push carries a classing
 *   trailer, or when a user token sends `X-Graftyard-Agent-Session` (which grants nothing). A trailer never classes a
 *   user-token push on its own.
 * `requester` comes from the task row, never from anything the caller sends.
 */
export function classify(cred: Credential, sessionHeader: string | null, authz: Authz, fold: Fold, base: Fold | null,
                         provenance: Provenance[] = [], at = 0): Principal {
  const p = classify0(cred, sessionHeader, authz, fold, base, provenance);
  // M0-WP13: the token's own ceiling and repository scope, and DEMOTE_PER_MIN from the fold's per-actor count.
  if (cred.t === "Installation") { p.perms = cred.perms ?? null; p.repos = cred.repos ?? null; }
  if (cred.t === "Pat") p.perms = cred.perms ?? null;
  if (cred.t === "UserToServer") p.repos = cred.repos ?? null;
  if (p.id.t === "User") {
    const n = (fold.writes?.[`u${p.id.user}`] ?? []).filter((t) => at - t < 60_000).length;
    if (n > DEMOTE_PER_MIN) { p.demoted = true; p.admit = "Agent"; }
  }
  return p;
}

function classify0(cred: Credential, sessionHeader: string | null, authz: Authz, fold: Fold, base: Fold | null,
                   provenance: Provenance[]): Principal {
  const folds = [base, fold];
  if (cred.t === "AppJwt") {
    return { id: { t: "App", app: cred.app, installation: 0 }, app: cred.app, kind: { t: "Bot" }, admit: "Bot", cred: cred.t,
             mcpWrite: null, sessionHeader, name: authz.apps[cred.app]?.slug ?? `app-${cred.app}` };
  }
  if (cred.t === "Installation") {
    const aa = agentApp(authz, cred.app);
    const slug = authz.apps[cred.app]?.slug ?? `app-${cred.app}`;
    const id = { t: "App" as const, app: cred.app, installation: cred.installation };
    let kind: ActorKind = { t: "Bot" };
    if (aa?.kind === "GraftyardRun") {
      const task = taskOf(folds, cred.task);
      kind = { t: "Agent", agent: { app: cred.app, task: cred.task, requester: task?.requester ?? null } };
    } else if (aa?.kind === "ThirdParty" && aa.confine) {
      const s = sessionHeader ? resolveSession(sessionHeader, cred.app) : null;
      const task = taskOf(folds, s);
      const own = task && task.kind === "External" && task.app === cred.app ? task : null;
      kind = { t: "Agent", agent: { app: cred.app, task: own ? own.session : null, requester: own?.requester ?? null } };
    }
    const admit: AdmitClass = aa || classes(provenance) ? "Agent" : "Bot";
    return { id, app: cred.app, kind, admit, cred: cred.t, mcpWrite: null, sessionHeader, name: slug };
  }
  if (cred.t === "UserToServer") {
    const aa = agentApp(authz, cred.app);
    const login = authz.users[cred.user]?.login ?? `user-${cred.user}`;
    const admit: AdmitClass = aa || sessionHeader ? "Agent" : "Human";
    return { id: { t: "User", user: cred.user }, app: cred.app, kind: { t: "User" }, admit, cred: cred.t,
             // The host reads WriteScope from the scoped token's `Tokens` row; a token without a row (WP10's prototype
             // tokens) falls back to its claim.
             mcpWrite: aa?.kind === "Mcp" ? ((cred.jti ? authz.tokens?.[cred.jti]?.write : cred.write) ?? "Read") : null, sessionHeader,
             name: aa?.kind === "Mcp" ? `${login} via Graftyard MCP` : login };
  }
  const login = authz.users[cred.user]?.login ?? `user-${cred.user}`;
  return { id: { t: "User", user: cred.user }, app: null, kind: { t: "User" }, admit: sessionHeader ? "Agent" : "Human",
           cred: cred.t, mcpWrite: null, sessionHeader, name: login };
}
