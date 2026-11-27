// LM-3's refusal probes ("a bad controller at 1% swaps back", `build/BUILD-PLAN.md`, AM3-WP1): a throwaway Worker in
// the agents account, `graft-agents-m4a-probe`, bound to the same `graftyard-agent` key as `graft-agents-front`
// (`AGENT_APP_KEY`, a Secrets Store binding), so the key never leaves the store. `POST /probe` with
// `x-probe-token: <PROBE_TOKEN>` and `{since, flushes: [{name, counts}]}` reads `GET release/errors?since=`, posts each
// flush to `POST release/errors` under a fresh App JWT, and reads again after each, answering the statuses, the
// edge's bodies, and the summed counts. `POST /hook` sets graftyard-agent's webhook (LM-3's kill row), and `POST /mint`
// mints either App's installation token (LM-3's in-repo row; WORKSPACE_APP_KEY bound too). It answers no
// JWT and logs nothing. Deployed and deleted by
// `tests/am3/hosted/lf7-day.sh` (ROW=lm3, steps `probe-deploy` and `teardown`).
const PATH = "/api/graftyard/v1/release/errors";
const b64url = (bytes) => btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const text64 = (s) => b64url(new TextEncoder().encode(s));

async function jwt(env, which = "agent") {
  const pem = await (which === "workspace" ? env.WORKSPACE_APP_KEY : env.AGENT_APP_KEY).get();
  const der = Uint8Array.from(atob(pem.replace(/-----[^-]+-----/g, "").replace(/\s+/g, "")), (c) => c.charCodeAt(0));
  const key = await crypto.subtle.importKey("pkcs8", der, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["sign"]);
  const now = Math.floor(Date.now() / 1000);
  const iss = Number(which === "workspace" ? env.WORKSPACE_APP_ID : env.AGENT_APP_ID);
  const signed = `${text64('{"alg":"RS256","typ":"JWT"}')}.${text64(JSON.stringify({ iat: now - 60, exp: now + 540, iss }))}`;
  const sig = await crypto.subtle.sign("RSASSA-PKCS1-v1_5", key, new TextEncoder().encode(signed));
  return `${signed}.${b64url(sig)}`;
}

async function call(env, auth, method, url, body) {
  const r = await fetch(url, {
    method,
    headers: { authorization: `Bearer ${auth}`, "content-type": "application/json", "user-agent": "graft-agents-lm3-probe" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const t = await r.text();
  let v = t;
  try { v = JSON.parse(t); } catch {}
  return { status: r.status, body: v };
}

export default {
  async fetch(req, env) {
    if (req.method !== "POST" || !["/probe", "/hook", "/mint"].includes(new URL(req.url).pathname) || req.headers.get("x-probe-token") !== env.PROBE_TOKEN) {
      return new Response("not found", { status: 404 });
    }
    if (new URL(req.url).pathname === "/mint") {
      // LM-3's in-repo row: an installation token for graftyard-agent or graftyard-workspace, `{app, installation,
      // body}` with the body (`repositories`, `permissions`, `graftyard_task`) passed through; the answer is the
      // edge's, token included, to the row's driver only (behind PROBE_TOKEN). The JWT is never answered.
      const { app, installation, body } = await req.json();
      const auth = await jwt(env, app === "workspace" ? "workspace" : "agent");
      return Response.json(await call(env, auth, "POST", `https://${env.GRAFTYARD_HOST}/api/v3/app/installations/${Number(installation)}/access_tokens`, body ?? {}));
    }
    const auth = await jwt(env);
    if (new URL(req.url).pathname === "/hook") {
      // LM-3's kill row: graftyard-agent's webhook config through GHES's `PATCH /app/hook/config` under its App JWT,
      // the body (url and secret) passed through and never logged.
      return Response.json(await call(env, auth, "PATCH", `https://${env.GRAFTYARD_HOST}/api/v3/app/hook/config`, await req.json()));
    }
    const { since, flushes } = await req.json();
    const base = `https://${env.GRAFTYARD_HOST}${PATH}`;
    const read = () => call(env, auth, "GET", `${base}?since=${since}`);
    const out = { before: await read(), flushes: [] };
    for (const f of flushes) {
      const post = await call(env, auth, "POST", base, { counts: f.counts });
      out.flushes.push({ name: f.name, post, after: await read() });
    }
    return Response.json(out);
  },
};
