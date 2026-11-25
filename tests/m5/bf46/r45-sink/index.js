// BF-46's SecretsWrapKey row (tests/m5/bf46/r45-row.sh): a hook receiver. POST /h/<n> checks X-Hub-Signature-256 against
// HOOK_SECRET (the secret the row gave each hook, wrapped by graft-ops and unwrapped by the HookSequencer to sign) and
// keeps the verdict; GET /seen/<n> with `Bearer HOOK_SECRET` answers it. No body is kept.
export class Seen {
  constructor(state) { this.s = state.storage; }
  async fetch(req) {
    const u = new URL(req.url);
    if (req.method === "PUT") { await this.s.put(u.pathname, await req.json()); return new Response("ok"); }
    const v = await this.s.get(u.pathname);
    return v ? Response.json(v) : new Response("not yet", { status: 404 });
  }
}
async function hmac(key, body) {
  const k = await crypto.subtle.importKey("raw", new TextEncoder().encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return [...new Uint8Array(await crypto.subtle.sign("HMAC", k, body))].map((b) => b.toString(16).padStart(2, "0")).join("");
}
export default {
  async fetch(req, env) {
    const u = new URL(req.url);
    const log = env.SEEN.get(env.SEEN.idFromName("seen"));
    let m = u.pathname.match(/^\/h\/(\d+)$/);
    if (m && req.method === "POST") {
      const body = new Uint8Array(await req.arrayBuffer());
      // BF57_J4: deployed as graft-m4-hooksink for j4's existing hooks, whose secrets BF-57 gave as
      // `bf57-j4-<i>-secret` for `/h/<100 + i>` (tests/m4/wp1/bf57.mjs).
      const key = env.BF57_J4 ? `bf57-j4-${Number(m[1]) - 100}-secret` : env.HOOK_SECRET;
      const sig_ok = req.headers.get("x-hub-signature-256") === `sha256=${await hmac(key, body)}`;
      await log.fetch(`https://s/${m[1]}`, { method: "PUT", body: JSON.stringify({ at: Date.now(), sig_ok, event: req.headers.get("x-github-event") }) });
      return new Response(sig_ok ? "ok" : "bad signature", { status: sig_ok ? 200 : 401 });
    }
    m = u.pathname.match(/^\/seen\/(\d+)$/);
    if (m && req.method === "GET") {
      if (req.headers.get("authorization") !== `Bearer ${env.HOOK_SECRET}`) return new Response("unauthorized", { status: 401 });
      return log.fetch(`https://s/${m[1]}`);
    }
    return new Response("not found", { status: 404 });
  },
};
