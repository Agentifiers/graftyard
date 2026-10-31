// M0-WP15: a GHES 3.22 read surface on the prototype host, enough for unmodified clients (gh, octokit,
// github-mcp-server) and graft-mcp's tools. Shapes follow the GHES 3.22 REST reference for the fields served; fields
// this prototype has no data for are absent, so this is not a GHES byte oracle (results/m0-wp15.md, L0-11).

import type { Host } from "./host.ts";
import { tokenOf, verify } from "./token.ts";
import { tokenLive } from "./oauth.ts";
import type { Credential, Fold } from "./model.ts";

const DOC = "https://docs.github.com/enterprise-server@3.22/rest";
export const json = (status: number, body: unknown, h: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json; charset=utf-8", ...h } });

/** The caller's credential, live, or null. */
export async function caller(h: Host, req: Request): Promise<Credential | null> {
  const tok = tokenOf(req);
  const cred = tok ? await verify(h.secret, tok) : null;
  return cred && cred.t !== "AppJwt" && tokenLive(h, cred) ? cred : null;
}
export function userJson(h: Host, id: number, base: string): Record<string, unknown> {
  const login = h.authz.users[id]?.login ?? `user-${id}`;
  return { login, id, node_id: `U_${id}`, avatar_url: `${base}/avatars/u/${id}`, url: `${base}/api/v3/users/${login}`,
           html_url: `${base}/${login}`, type: "User", site_admin: false };
}
const iso = (ms: number) => new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");
function actorJson(h: Host, key: string, base: string): Record<string, unknown> {
  const a = /^a(\d+)i/.exec(key);
  if (a) { const slug = h.authz.apps[a[1]]?.slug ?? `app-${a[1]}`; return { login: `${slug}[bot]`, id: 900 + Number(a[1]), type: "Bot", site_admin: false, url: `${base}/api/v3/users/${slug}[bot]` }; }
  return userJson(h, Number(key.slice(1)), base);
}
function repoJson(f: Fold, base: string, perm: string | null): Record<string, unknown> {
  const [owner, name] = f.repo.split("/");
  return { id: Math.abs([...f.repo].reduce((a, c) => (a * 31 + c.charCodeAt(0)) | 0, 7)), node_id: `R_${f.repo}`, name, full_name: f.repo, private: true, visibility: "private",
    owner: { login: owner, type: "Organization", url: `${base}/api/v3/users/${owner}` }, html_url: `${base}/${f.repo}`, url: `${base}/api/v3/repos/${f.repo}`,
    fork: f.forkOf != null, default_branch: f.defaultBranch.replace("refs/heads/", ""), has_issues: true, archived: false, disabled: false,
    clone_url: `${base}/${f.repo}.git`, ...(perm ? { permissions: { admin: perm === "admin", maintain: perm === "admin", push: perm !== "read", triage: perm !== "read", pull: true } } : {}) };
}

export async function ghesRoutes(h: Host, req: Request, url: URL): Promise<Response | null> {
  const p = url.pathname, base = url.origin, m = req.method;
  if ((m === "HEAD" || m === "GET") && (p === "/api/v3" || p === "/api/v3/")) {
    // github-mcp-server reads classic scopes from X-OAuth-Scopes on HEAD /api/v3/ (research/agents.md 1.11).
    const c = await caller(h, req);
    const hdr: Record<string, string> = c?.t === "Pat" && !c.perms ? { "x-oauth-scopes": "repo, read:org, read:packages", "x-accepted-oauth-scopes": "" } : {};
    return new Response(m === "HEAD" ? null : JSON.stringify({ current_user_url: `${base}/api/v3/user` }), { status: c ? 200 : 401, headers: { "content-type": "application/json; charset=utf-8", ...hdr } });
  }
  if (m === "GET" && p === "/api/v3/meta") return json(200, { verifiable_password_authentication: false, installed_version: "3.22.0" });
  if (m === "GET" && p === "/api/v3/rate_limit") {
    const r = { limit: 5000, used: 1, remaining: 4999, reset: Math.floor(Date.now() / 1000) + 3600 };
    return json(200, { resources: { core: r, graphql: r, search: { ...r, limit: 30, remaining: 29 } }, rate: r });
  }
  if (m === "GET" && p === "/api/v3/user") {
    const c = await caller(h, req);
    if (!c || (c.t !== "Pat" && c.t !== "UserToServer")) return json(c ? 403 : 401, { message: c ? "Resource not accessible by integration" : "Requires authentication", documentation_url: DOC });
    return json(200, { ...userJson(h, c.user, base), name: h.authz.users[c.user]?.login ?? null });
  }
  let x: RegExpExecArray | null;
  if (m === "GET" && (x = /^\/api\/v3\/repos\/([^/]+)\/([^/]+)(?:\/(issues|pulls|branches|commits\/([^/]+)))?$/.exec(p))) {
    const f = h.folds[`${x[1]}/${x[2]}`];
    const c = await caller(h, req);
    if (!c) return json(401, { message: "Requires authentication", documentation_url: DOC });
    const uid = c.t === "Pat" || c.t === "UserToServer" ? c.user : null;
    const role = f && uid != null ? f.collaborators[`u${uid}`] ?? null : null;
    if (!f || (uid != null && !role) || (c.t === "UserToServer" && c.repos && !c.repos.includes(f.repo)) || (c.t === "Installation" && c.repos && !c.repos.includes(f.repo)))
      return json(404, { message: "Not Found", documentation_url: DOC });
    if (!x[3]) return json(200, repoJson(f, base, role));
    if (x[3] === "issues") {
      const st = url.searchParams.get("state") ?? "open";
      const rows = Object.values(f.issues ?? {}).filter(() => st !== "closed").map((i) => { const r = i.revisions.at(-1)!;
        return { number: i.number, title: r.title, body: r.body, state: "open", user: actorJson(h, i.author, base), labels: [], assignees: [],
          comments: (f.comments ?? []).filter((c2) => c2.issue === i.number).length, created_at: iso(i.createdAt), updated_at: iso(i.createdAt),
          html_url: `${base}/${f.repo}/issues/${i.number}`, url: `${base}/api/v3/repos/${f.repo}/issues/${i.number}` }; });
      const prs = Object.values(f.prs).filter((q) => q.open || st !== "open").map((q) => ({ number: q.number, title: q.title ?? "", body: q.body ?? "", state: q.open ? "open" : "closed",
        user: actorJson(h, q.author, base), labels: [], assignees: [], pull_request: { url: `${base}/api/v3/repos/${f.repo}/pulls/${q.number}` }, html_url: `${base}/${f.repo}/pull/${q.number}` }));
      return json(200, [...rows, ...prs].sort((a, b) => b.number - a.number).slice(0, Number(url.searchParams.get("per_page") ?? 30)));
    }
    if (x[3] === "pulls") {
      const st = url.searchParams.get("state") ?? "open";
      return json(200, Object.values(f.prs).filter((q) => st === "all" || (st === "open") === q.open).sort((a, b) => b.number - a.number).map((q) => ({ number: q.number,
        title: q.title ?? "", body: q.body ?? "", state: q.open ? "open" : "closed", draft: q.draft, merged_at: null, user: actorJson(h, q.author, base),
        head: { ref: q.headRef.replace("refs/heads/", ""), sha: q.headOid }, base: { ref: q.base.replace("refs/heads/", ""), sha: f.refs[q.base] ?? null },
        html_url: `${base}/${f.repo}/pull/${q.number}`, url: `${base}/api/v3/repos/${f.repo}/pulls/${q.number}` })));
    }
    if (x[3] === "branches") return json(200, Object.entries(f.refs).filter(([n]) => n.startsWith("refs/heads/")).map(([n, oid]) => ({ name: n.slice(11),
      commit: { sha: oid, url: `${base}/api/v3/repos/${f.repo}/commits/${oid}` }, protected: n === f.defaultBranch })));
    if (x[4] && !/^[0-9a-f]{40}$/.test(x[4])) {
      const oid = f.refs[`refs/heads/${x[4]}`];
      if (!oid) return json(422, { message: `No commit found for SHA: ${x[4]}`, documentation_url: DOC });
      return json(200, { sha: oid, url: `${base}/api/v3/repos/${f.repo}/commits/${oid}` });
    }
  }
  return null;
}
