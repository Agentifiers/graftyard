// M0-WP15, L0-07: the GHES 3.22 GitHub App surface a vendor's App uses (design/AGENT-LAYER.md, A2 steps 1 to 5;
// research/agents.md 2.1 to 2.5). A prototype of GHES's documented behavior, not GHES:
//   GET  /settings/apps/new?state=   (and /organizations/{org}/settings/apps/new) the manifest form, auto-posting
//   POST /settings/apps/new          the admin's "Create GitHub App": registers the App, redirects to redirect_url?code=
//   POST /api/v3/app-manifests/{code}/conversions   id, slug, client id and secret, webhook_secret, pem (one use)
//   POST /_m0/apps                   harness: the "Register new GitHub App" form (the Codex and Cursor manual paths)
//   POST /_m0/apps/{slug}/install    harness: the admin's install on selected repositories (sends `installation`)
//   GET  /api/v3/app, /app/installations, /repos/{o}/{r}/installation    App JWT (RS256, iss = App id or client id)
//   GET  /api/v3/repos/{o}/{r}/commits/{ref}/check-runs, /check-runs/{id}, /branches/{b}/protection
// App JWTs are verified with the App's RSA public key; installation tokens come from the shared `decideMint`.
// Webhooks: every committed event an App subscribes to is sent, signed X-Hub-Signature-256 with its webhook secret.
// `graftyard/overlap`: when `graftyard-insights` is installed (and the layer is on), a push under an agent prefix whose
// paths overlap another agent ref's PathSet gets a neutral check run on its head SHA (design/AGENT-LAYER.md, A5).

import type { Host } from "./host.ts";
import type { Credential, Event, Fold, Perm, PermSet, Principal } from "./model.ts";
import { actorKey } from "./model.ts";
import { derive, hmac, tokenOf, verify } from "./token.ts";
import { json } from "./ghes.ts";
import { APP } from "./fixture.ts";

const DOC = "https://docs.github.com/enterprise-server@3.22/rest";
const rand = (n = 20) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");
const b64u = (b: Uint8Array) => btoa(String.fromCharCode(...b)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const unb64u = (s: string) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
const pem = (label: string, der: ArrayBuffer) =>
  `-----BEGIN ${label}-----\n${btoa(String.fromCharCode(...new Uint8Array(der))).match(/.{1,64}/g)!.join("\n")}\n-----END ${label}-----\n`;
const iso = (ms: number) => new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");

/** One registered GitHub App, as GHES stores it after "Create GitHub App". */
export interface GhApp {
  id: number; slug: string; name: string; owner: string; clientId: string; clientSecret: string; webhookSecret: string;
  hookUrl: string | null; hookActive: boolean; events: string[]; perms: PermSet; publicJwk: JsonWebKey;
  callbackUrls: string[]; setupUrl: string | null; redirectUrl: string | null; createdAt: number;
}
export interface AppsState {
  apps: Record<string, GhApp>;
  /** Manifest codes: one use, an hour (GHES docs, "Creating a GitHub App from a manifest"). */
  codes: Record<string, { slug: string; pem: string; exp: number; used: boolean }>;
  nextApp: number; nextInst: number;
  /** Deliveries, newest last, for the harness. */
  sent: { id: string; app: string; event: string; action: string | null; status: number | null; at: number }[];
}
export const freshApps = (): AppsState => ({ apps: {}, codes: {}, nextApp: 20, nextInst: 200, sent: [] });

/** GitHub's manifest permission names to the prototype's `Perm`s (unknown names are kept out of the ceiling). */
const PERM: Record<string, Perm> = { contents: "contents", pull_requests: "pull_requests", issues: "issues", checks: "checks",
  statuses: "statuses", actions: "actions", administration: "administration", metadata: "metadata" };
function permsOf(m: Record<string, string> | undefined): PermSet {
  const out: PermSet = { metadata: "read" };
  for (const [k, v] of Object.entries(m ?? {})) if (PERM[k] && (v === "read" || v === "write")) out[PERM[k]] = v;
  return out;
}

async function register(h: Host, m: Record<string, any>, owner: string): Promise<{ app: GhApp; pem: string }> {
  const kp = await crypto.subtle.generateKey({ name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
                                            true, ["sign", "verify"]) as CryptoKeyPair;
  const priv = pem("PRIVATE KEY", await crypto.subtle.exportKey("pkcs8", kp.privateKey) as ArrayBuffer);
  const id = h.apps.nextApp++;
  const base = String(m.name ?? `app-${id}`).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  let slug = base; for (let i = 2; h.apps.apps[slug]; i++) slug = `${base}-${i}`;
  // Derived, so the user-auth client check in oauth.ts (derive(secret, secretLabel)) accepts it.
  const clientSecret = await derive(h.secret, `client:${slug}`);
  const app: GhApp = {
    id, slug, name: String(m.name ?? slug), owner, clientId: `Iv23${rand(8)}`, clientSecret,
    // The prototype derives the webhook secret from the harness secret so the aux sink can verify it; GHES generates one,
    // or takes the one the admin typed (`webhook_secret` on the manual form).
    webhookSecret: typeof m.webhook_secret === "string" ? m.webhook_secret : await derive(h.secret, `webhook:${slug}`),
    hookUrl: m.hook_attributes?.url ?? null, hookActive: m.hook_attributes?.active !== false && !!m.hook_attributes?.url,
    events: Array.isArray(m.default_events) ? m.default_events.map(String) : [], perms: permsOf(m.default_permissions),
    publicJwk: await crypto.subtle.exportKey("jwk", kp.publicKey) as JsonWebKey,
    callbackUrls: Array.isArray(m.callback_urls) ? m.callback_urls : [], setupUrl: m.setup_url ?? null, redirectUrl: m.redirect_url ?? null,
    createdAt: Date.now(),
  };
  h.apps.apps[slug] = app;
  h.authz.apps[id] = { slug };
  for (const cb of app.callbackUrls) h.oauth.clients[app.clientId] = { app: id, callback: cb, secretLabel: `client:${slug}` };
  h.saveKv("ghApps", h.apps); h.saveOAuth();
  return { app, pem: priv };
}

function appJson(a: GhApp, base: string): Record<string, unknown> {
  return { id: a.id, slug: a.slug, node_id: `A_${a.id}`, client_id: a.clientId, owner: { login: a.owner, type: "Organization" }, name: a.name,
           description: "", external_url: base, html_url: `${base}/github-apps/${a.slug}`, created_at: iso(a.createdAt), updated_at: iso(a.createdAt),
           permissions: a.perms, events: a.events };
}
function installJson(h: Host, id: number, a: GhApp, base: string): Record<string, unknown> {
  const inst = h.authz.installations[id];
  return { id, app_id: a.id, app_slug: a.slug, target_type: "Organization", account: { login: "acme", type: "Organization" },
           repository_selection: "selected", access_tokens_url: `${base}/api/v3/app/installations/${id}/access_tokens`,
           repositories_url: `${base}/api/v3/installation/repositories`, html_url: `${base}/organizations/acme/settings/installations/${id}`,
           permissions: inst.perms, events: a.events, suspended_at: inst.active ? null : iso(Date.now()) };
}
const bySlugOrId = (h: Host, k: string | number) => Object.values(h.apps.apps).find((a) => a.id === Number(k) || a.clientId === k || a.slug === k);

/** Verify an RS256 App JWT the way GHES does: `iss` names the App (id or client id), `exp` within 10 minutes, `iat` not in
 *  the future beyond clock skew, signature by the App's key. Returns the prototype's `AppJwt` credential. */
export async function verifyAppJwt(h: Host, token: string): Promise<{ cred: Extract<Credential, { t: "AppJwt" }> } | { error: string }> {
  const parts = token.split(".");
  if (parts.length !== 3) return { error: "A JSON web token could not be decoded" };
  let head: Record<string, unknown>, claims: Record<string, unknown>;
  try { head = JSON.parse(new TextDecoder().decode(unb64u(parts[0]))); claims = JSON.parse(new TextDecoder().decode(unb64u(parts[1]))); }
  catch { return { error: "A JSON web token could not be decoded" }; }
  if (head.alg !== "RS256") return { error: "A JSON web token could not be decoded" };
  const a = bySlugOrId(h, claims.iss as string | number);
  if (!a) return { error: "Integration not found" };
  const key = await crypto.subtle.importKey("jwk", a.publicJwk, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
  const ok = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, unb64u(parts[2]), new TextEncoder().encode(`${parts[0]}.${parts[1]}`));
  if (!ok) return { error: "A JSON web token could not be decoded" };
  const now = Math.floor(Date.now() / 1000), exp = Number(claims.exp), iat = Number(claims.iat);
  if (!(exp > now) || exp - iat > 600) return { error: "'Expiration time' claim ('exp') is too far in the future" };
  if (iat > now + 60) return { error: "'Issued at' claim ('iat') must be an Integer representing the time that the assertion was issued" };
  return { cred: { t: "AppJwt", app: a.id } };
}

/** The bearer is an App JWT (three dot-separated parts, not a prototype `gym0_` token). */
const isJwt = (t: string | null): t is string => !!t && !t.startsWith("gym0_") && t.split(".").length === 3;

// ---- the manifest form ----
function manifestForm(url: URL): Response {
  // The vendor's page posts `manifest` here (POST) or links with `?manifest=` would be unusual; GHES serves the form on
  // GET with `state` and renders the posted manifest for the admin. Framing is denied as GitHub does.
  const state = url.searchParams.get("state") ?? "";
  return new Response(`<!doctype html><title>Register new GitHub App</title><form method=post><input type=hidden name=state value="${state.replace(/"/g, "&quot;")}">` +
    `<textarea name=manifest></textarea><button>Create GitHub App</button></form>`,
    { headers: { "content-type": "text/html; charset=utf-8", "x-frame-options": "deny", "content-security-policy": "frame-ancestors 'none'" } });
}

export async function appRoutes(h: Host, req: Request, url: URL): Promise<Response | null> {
  const p = url.pathname, method = req.method, base = url.origin;
  let x: RegExpExecArray | null;

  // 1. Manifest flow (design A2 step 1). The admin's session is the `user_session` cookie (a harness PAT of an org admin).
  if ((x = /^\/(?:organizations\/([^/]+)\/)?settings\/apps\/new$/.exec(p))) {
    if (method === "GET") return manifestForm(url);
    if (method !== "POST") return null;
    const sess = /(?:^|;\s*)user_session=([^;]+)/.exec(req.headers.get("cookie") ?? "")?.[1];
    const who = sess ? await verify(h.secret, decodeURIComponent(sess)) : null;
    if (!who || who.t !== "Pat") return new Response(null, { status: 302, headers: { location: `/login?return_to=${encodeURIComponent(p)}` } });
    if (!h.org.admins.includes(who.user)) return new Response("You must be an organization owner to register an App for it.", { status: 403 });
    // Cross-site POST is how the flow works (the vendor's page posts the form), so there is no CSRF token by design.
    const form = await req.formData().catch(() => null);
    let m: Record<string, any>;
    try { m = JSON.parse(String(form?.get("manifest") ?? "")); } catch { return new Response("Invalid manifest", { status: 422 }); }
    if (typeof m.url !== "string" || !m.hook_attributes?.url && m.hook_attributes?.active !== false) return new Response("Invalid manifest: url and hook_attributes.url are required", { status: 422 });
    const { app, pem: key } = await register(h, m, x[1] ?? h.authz.users[who.user]?.login ?? "acme");
    const code = rand(16);
    h.apps.codes[code] = { slug: app.slug, pem: key, exp: Date.now() + 3_600_000, used: false };
    h.saveKv("ghApps", h.apps);
    const to = new URL(app.redirectUrl ?? m.url);
    to.searchParams.set("code", code);
    const state = form?.get("state"); if (state) to.searchParams.set("state", String(state));
    return new Response(null, { status: 302, headers: { location: to.toString() } });
  }
  if (method === "POST" && (x = /^\/api\/v3\/app-manifests\/([^/]+)\/conversions$/.exec(p))) {
    const c = h.apps.codes[x[1]];
    if (!c || c.used || c.exp < Date.now()) return json(404, { message: "Not Found", documentation_url: `${DOC}/apps/apps#create-a-github-app-from-a-manifest` });
    c.used = true; h.saveKv("ghApps", h.apps);
    const a = h.apps.apps[c.slug];
    return json(201, { ...appJson(a, base), client_secret: a.clientSecret, webhook_secret: a.webhookSecret, pem: c.pem });
  }

  // 2. Harness: the manual "Register new GitHub App" form and the admin's install (both browser actions on GHES).
  if (p.startsWith("/_m0/apps")) {
    if (req.headers.get("authorization") !== `Bearer ${h.secret}`) return new Response("unauthorized", { status: 401 });
    if (method === "POST" && p === "/_m0/apps") {
      const m = await req.json() as Record<string, any>;
      const { app, pem: key } = await register(h, m, "acme");
      return json(201, { ...appJson(app, base), client_secret: app.clientSecret, webhook_secret: app.webhookSecret, pem: key });
    }
    if (method === "POST" && (x = /^\/_m0\/apps\/([^/]+)\/install$/.exec(p))) {
      const a = h.apps.apps[x[1]]; if (!a) return json(404, { message: "Not Found" });
      const b = await req.json().catch(() => ({})) as { repositories?: string[]; agent_app?: { kind: "ThirdParty"; confine: string | null } };
      const id = h.apps.nextInst++;
      h.authz.installations[id] = { app: a.id, active: true, perms: { ...a.perms }, repos: (b.repositories ?? []).map((r) => r.includes("/") ? r : `acme/${r}`) };
      // design A2 step 2: site admin marks it an AgentApp, with its `confine` prefix (only where the layer is deployed).
      if (b.agent_app && h.layer) h.authz.agentApps = [...h.authz.agentApps.filter((r) => r.app !== a.id), { app: a.id, kind: b.agent_app.kind, confine: b.agent_app.confine }];
      h.saveKv("ghApps", h.apps); h.saveKv("appsAuthz", { installations: h.authz.installations, agentApps: h.authz.agentApps, apps: h.authz.apps });
      await sendHook(h, a, "installation", { action: "created", installation: installJson(h, id, a, base),
        repositories: h.authz.installations[id].repos!.map((r) => ({ full_name: r, name: r.split("/")[1], private: true })), sender: { login: "alice" } }, base);
      return json(201, installJson(h, id, a, base));
    }
    if (method === "DELETE" && (x = /^\/_m0\/apps\/([^/]+)\/installations\/(\d+)$/.exec(p))) {
      const a = h.apps.apps[x[1]]; const inst = h.authz.installations[x[2]];
      if (!a || !inst || inst.app !== a.id) return json(404, { message: "Not Found" });
      await sendHook(h, a, "installation", { action: "deleted", installation: installJson(h, Number(x[2]), a, base), sender: { login: "alice" } }, base);
      inst.active = false; h.saveKv("appsAuthz", { installations: h.authz.installations, agentApps: h.authz.agentApps, apps: h.authz.apps });
      return new Response(null, { status: 204 });
    }
    if (method === "GET" && p === "/_m0/apps/deliveries") return json(200, h.apps.sent);
    return null;
  }

  // 3. App-JWT endpoints, and the JWT path of POST /app/installations/{id}/access_tokens for these Apps.
  const bearer = tokenOf(req);
  if (/\/access_tokens$/.test(p)) return null; // host.ts restTop mints, with appJwtOf for these Apps
  if (p === "/api/v3/app" || p.startsWith("/api/v3/app/") || (/^\/api\/v3\/repos\/[^/]+\/[^/]+\/installation$/.test(p) && isJwt(bearer))) {
    if (!isJwt(bearer)) return null; // the fixture Apps' `gym0_` JWT stand-ins keep the WP13 path in host.ts
    const v = await verifyAppJwt(h, bearer);
    if ("error" in v) return json(401, { message: v.error, documentation_url: DOC });
    const a = bySlugOrId(h, v.cred.app)!;
    const mine = Object.entries(h.authz.installations).filter(([, i]) => i.app === a.id && i.active);
    if (method === "GET" && p === "/api/v3/app") return json(200, { ...appJson(a, base), installations_count: mine.length });
    if (method === "GET" && p === "/api/v3/app/installations") return json(200, mine.map(([id]) => installJson(h, Number(id), a, base)));
    if (method === "GET" && (x = /^\/api\/v3\/app\/installations\/(\d+)$/.exec(p))) {
      const i = mine.find(([id]) => id === x![1]); return i ? json(200, installJson(h, Number(i[0]), a, base)) : json(404, { message: "Not Found", documentation_url: DOC });
    }
    if (method === "GET" && (x = /^\/api\/v3\/repos\/([^/]+\/[^/]+)\/installation$/.exec(p))) {
      const i = mine.find(([, inst]) => inst.repos?.includes(x![1])); return i ? json(200, installJson(h, Number(i[0]), a, base)) : json(404, { message: "Not Found", documentation_url: DOC });
    }
    return json(404, { message: "Not Found", documentation_url: DOC });
  }
  return null;
}

/** App JWT for the access_tokens route (host.ts restTop): an RS256 JWT of a registered App, or null for `gym0_`. */
export async function appJwtOf(h: Host, req: Request): Promise<Extract<Credential, { t: "AppJwt" }> | { error: string } | null> {
  const bearer = tokenOf(req);
  if (!isJwt(bearer)) return null;
  const v = await verifyAppJwt(h, bearer);
  return "error" in v ? v : v.cred;
}

// ---- webhooks ----
async function sendHook(h: Host, a: GhApp, event: string, payload: Record<string, unknown>, base: string): Promise<void> {
  if (!a.hookActive || !a.hookUrl || !h.out) return;
  if (event !== "installation" && event !== "installation_repositories" && !a.events.includes(event)) return;
  const id = crypto.randomUUID();
  const body = JSON.stringify(payload);
  const sig = [...await hmac(a.webhookSecret, body)].map((b) => b.toString(16).padStart(2, "0")).join("");
  const row = { id, app: a.slug, event, action: (payload.action as string) ?? null, status: null as number | null, at: Date.now() };
  h.apps.sent.push(row); if (h.apps.sent.length > 2_000) h.apps.sent.splice(0, h.apps.sent.length - 2_000);
  const out = h.out;
  out.later((async () => {
    try {
      const r = await out.send(a.hookUrl!, { method: "POST", body, headers: { "content-type": "application/json", "x-github-event": event, "x-github-delivery": id,
        "x-github-hook-installation-target-type": "integration", "x-github-hook-installation-target-id": String(a.id), "x-github-enterprise-host": new URL(base).host,
        "x-github-enterprise-version": "3.22.0", "x-hub-signature-256": `sha256=${sig}`, "user-agent": "GitHub-Hookshot/graftyard" } });
      row.status = r.status; await r.arrayBuffer();
    } catch { row.status = 0; }
    h.saveKv("ghApps", h.apps);
  })());
}

/** Committed events to App webhooks (design A2 step 3), for every registered App installed on the repository. */
export function appEvents(h: Host, repo: string, events: Event[], p: Principal, before: Record<string, string>): void {
  const base = `https://${h.authz.host}`;
  const fold = h.folds[repo];
  const sender = p.id.t === "User" ? { login: h.authz.users[p.id.user]?.login ?? "user", type: "User" } : { login: `${p.name}[bot]`, type: "Bot" };
  const repository = { full_name: repo, name: repo.split("/")[1], owner: { login: repo.split("/")[0] }, private: true, default_branch: fold.defaultBranch.replace("refs/heads/", "") };
  const out: [string, Record<string, unknown>][] = [];
  for (const e of events) {
    if (e.t === "Refs") for (const u of e.updates)
      out.push(["push", { ref: u.name, before: before[u.name] ?? "0".repeat(40), after: u.new ?? "0".repeat(40), created: !before[u.name], deleted: u.new == null, forced: false }]);
    if (e.t === "PrOpened") out.push(["pull_request", { action: "opened", number: e.pr.number, pull_request: prPayload(fold, e.pr.number) }]);
    if (e.t === "PrSync") out.push(["pull_request", { action: "synchronize", number: e.pr, pull_request: prPayload(fold, e.pr) }]);
    if (e.t === "PrMerged") out.push(["pull_request", { action: "closed", number: e.pr, pull_request: prPayload(fold, e.pr) }]);
    if (e.t === "Review") out.push(["pull_request_review", { action: "submitted", review: { state: e.event.toLowerCase(), user: sender }, pull_request: prPayload(fold, e.pr) }]);
    if (e.t === "Commented") out.push(["issue_comment", { action: "created", issue: { number: e.comment.issue }, comment: { id: e.comment.id, body: e.comment.body, user: sender } }]);
    if (e.t === "IssueOpened") out.push(["issues", { action: "opened", issue: { number: e.issue.number, title: e.issue.revisions.at(-1)?.title } }]);
    if (e.t === "CheckRunStored") out.push(["check_run", { action: "created", check_run: checkJson(h, fold, e.run.id, base) }]);
    if (e.t === "Status" && !events.some((y) => y.t === "CheckRunStored")) out.push(["status", { sha: e.sha, context: e.context, state: "success" }]);
  }
  for (const a of Object.values(h.apps.apps)) {
    const inst = Object.entries(h.authz.installations).find(([, i]) => i.app === a.id && i.active && i.repos?.includes(repo));
    if (!inst) continue;
    for (const [event, payload] of out) void sendHook(h, a, event, { ...payload, repository, sender, installation: { id: Number(inst[0]) } }, base);
  }
}
function prPayload(fold: Fold, n: number): Record<string, unknown> {
  const pr = fold.prs[String(n)];
  return pr ? { number: n, state: pr.open ? "open" : "closed", draft: pr.draft, merged: pr.merged, title: pr.title ?? "", head: { ref: pr.headRef.replace("refs/heads/", ""), sha: pr.headOid },
                base: { ref: pr.base.replace("refs/heads/", "") } } : { number: n };
}

// ---- check runs (reads), branch protection ----
export function checkJson(h: Host, fold: Fold, id: number, base: string): Record<string, unknown> | null {
  const c = (fold.checkRuns ?? []).find((y) => y.id === id);
  if (!c) return null;
  const a = /^a(\d+)i/.exec(c.author);
  const slug = a ? h.authz.apps[a[1]]?.slug ?? `app-${a[1]}` : null;
  const neutral = c.name.startsWith("graftyard/");
  return { id: c.id, head_sha: c.sha, node_id: `CR_${c.id}`, external_id: "", url: `${base}/api/v3/repos/${fold.repo}/check-runs/${c.id}`,
           html_url: `${base}/${fold.repo}/runs/${c.id}`, status: "completed", conclusion: neutral ? "neutral" : "success",
           started_at: null, completed_at: null, output: { title: c.title, summary: c.summary, text: c.text, annotations_count: 0 }, name: c.name,
           app: a ? { id: Number(a[1]), slug } : null, pull_requests: Object.values(fold.prs).filter((p) => p.headOid === c.sha).map((p) => ({ number: p.number })) };
}

export function checkReads(h: Host, fold: Fold, rest: string, method: string, base: string, canRead: (perm: Perm) => boolean): Response | null {
  let x: RegExpExecArray | null;
  if (method === "GET" && (x = /^commits\/([^/]+)\/check-runs$/.exec(rest))) {
    if (!canRead("checks")) return json(403, { message: "Resource not accessible by integration", documentation_url: DOC });
    const sha = /^[0-9a-f]{40}$/.test(x[1]) ? x[1] : fold.refs[`refs/heads/${decodeURIComponent(x[1])}`] ?? x[1];
    const runs = (fold.checkRuns ?? []).filter((c) => c.sha === sha).map((c) => checkJson(h, fold, c.id, base)!);
    return json(200, { total_count: runs.length, check_runs: runs });
  }
  if (method === "GET" && (x = /^branches\/(.+)\/protection$/.exec(rest))) {
    if (!canRead("administration")) return json(403, { message: "Resource not accessible by integration", documentation_url: DOC });
    const ref = `refs/heads/${decodeURIComponent(x[1])}`;
    if (!fold.rulesets.some((r) => r.refs.includes(ref))) return json(404, { message: "Branch not protected", documentation_url: DOC });
    return json(200, { url: `${base}/api/v3/repos/${fold.repo}/branches/${x[1]}/protection`, required_status_checks: { strict: false, contexts: [], checks: [] },
                       enforce_admins: { enabled: false }, allow_force_pushes: { enabled: false }, allow_deletions: { enabled: false } });
  }
  return null;
}

// ---- graftyard/overlap from PathSets (insights installed, layer on) ----
/** Two agent refs overlap when their pushed path sets share a path. Returns the neutral check run to store, if any. */
export function overlapCheck(h: Host, fold: Fold, events: Event[], at: number): { sha: string; title: string; summary: string } | null {
  if (!h.layer) return null;
  if (!Object.values(h.authz.installations).some((i) => i.app === APP.insights && i.active && i.repos?.includes(fold.repo))) return null;
  const sets = (fold.pathSets ??= {});
  let hit: { sha: string; title: string; summary: string } | null = null;
  for (const e of events) {
    if (e.t !== "PathSet") continue;
    sets[e.refname] = { paths: e.paths, at };
    const sha = fold.refs[e.refname];
    const others = Object.entries(sets).filter(([r, s]) => r !== e.refname && fold.refs[r] && s.paths.some((q) => e.paths.includes(q)));
    if (sha) hit = { sha, title: others.length ? `Overlaps ${others.length} other agent branch${others.length > 1 ? "es" : ""}` : "No overlap",
                     summary: others.map(([r, s]) => `${r.replace("refs/heads/", "")}: ${s.paths.filter((q) => e.paths.includes(q)).join(", ")}`).join("\n") || "none" };
  }
  return hit;
}
export const insightsActor = (): string => actorKey({ t: "App", app: APP.insights, installation: 106 });
