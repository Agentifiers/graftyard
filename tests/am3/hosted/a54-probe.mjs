// OPEN-QUESTIONS A54, folded into LF-7 r2's setup: does a fresh token with Workers Scripts Write alone upload a version
// of a script that has a Secrets Store binding? In one account per call, everything named <prefix>a54-:
//   1. a store secret <prefix>a54-probe (random value) and a script <prefix>a54-probe bound to it as PROBE, made with
//      the account's own token;
//   2. a token <prefix>a54-sw with Workers Scripts Write alone, and a version upload of the script with it, the
//      deployed bindings sent as read (as `graft-ops` sends them);
//   3. the same upload with the account's own token, the control;
//   4. the token revoked, the script and the secret deleted.
//
//   node tests/am3/hosted/a54-probe.mjs <account id> <store id> <prefix>     (CF_TOKEN: the account's own token)
//
// Prints one JSON line per step, with error codes and messages, never a token value.
const [acct, store, prefix] = process.argv.slice(2);
const own = process.env.CF_TOKEN;
if (!acct || !store || !prefix || !own) throw new Error("usage: CF_TOKEN=... a54-probe.mjs <account> <store> <prefix>");
const B = `https://api.cloudflare.com/client/v4/accounts/${acct}`;
const SW = "e086da7e2179491d91ee5f35b3ca210a"; // Workers Scripts Write
const name = `${prefix}a54-probe`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function call(token, method, path, body, json = true) {
  const headers = { authorization: `Bearer ${token}` };
  if (json && body) headers["content-type"] = "application/json";
  const r = await fetch(`${B}${path}`, { method, headers, body: json && body ? JSON.stringify(body) : body });
  const v = await r.json().catch(() => ({}));
  return { status: r.status, ok: v.success === true, result: v.result, errors: (v.errors ?? []).map((e) => [e.code, e.message]) };
}
const out = (o) => console.log(JSON.stringify(o));
const code = "export default { fetch() { return new Response('a54'); } };";
function form(meta) {
  const f = new FormData();
  f.append("metadata", JSON.stringify(meta));
  f.append("index.js", new Blob([code], { type: "application/javascript+module" }), "index.js");
  return f;
}
const binding = { type: "secrets_store_secret", name: "PROBE", store_id: store, secret_name: name };
let token;
try {
  const value = [...crypto.getRandomValues(new Uint8Array(24))].map((b) => b.toString(16).padStart(2, "0")).join("");
  const s = await call(own, "POST", `/secrets_store/stores/${store}/secrets`, [{ name, value, scopes: ["workers"], comment: "A54 probe" }]);
  out({ step: "secret", ok: s.ok, errors: s.errors });
  const meta = { main_module: "index.js", bindings: [binding], compatibility_date: "2026-11-01" };
  const d = await call(own, "PUT", `/workers/scripts/${name}`, form(meta), false);
  out({ step: "script", ok: d.ok, errors: d.errors });
  const t = await call(own, "POST", "/tokens", {
    name: `${prefix}a54-sw`,
    policies: [{ effect: "allow", resources: { [`com.cloudflare.api.account.${acct}`]: "*" }, permission_groups: [{ id: SW }] }],
  });
  token = { id: t.result?.id, value: t.result?.value };
  out({ step: "token", scope: "Workers Scripts Write", ok: t.ok, errors: t.errors });
  await sleep(10_000);
  const up = { ...meta, annotations: { "workers/tag": "a54-probe" } };
  let r;
  for (let i = 0; i < 3; i++) {
    r = await call(token.value, "POST", `/workers/scripts/${name}/versions`, form(up), false);
    if (r.ok || !r.errors.some(([c]) => c === 10000 || c === 9109)) break;
    await sleep(10_000);
  }
  out({ step: "upload", token: "Workers Scripts Write alone", status: r.status, ok: r.ok, version: r.result?.id?.slice(0, 8), errors: r.errors });
  const c = await call(own, "POST", `/workers/scripts/${name}/versions`, form(up), false);
  out({ step: "upload", token: "the account's own", status: c.status, ok: c.ok, version: c.result?.id?.slice(0, 8), errors: c.errors });
} finally {
  if (token?.id) out({ step: "revoke", ok: (await call(own, "DELETE", `/tokens/${token.id}`)).ok });
  out({ step: "delete script", ok: (await call(own, "DELETE", `/workers/scripts/${name}?force=true`)).ok });
  const list = await call(own, "GET", `/secrets_store/stores/${store}/secrets?per_page=100`);
  const id = (list.result ?? []).find((x) => x.name === name)?.id;
  if (id) out({ step: "delete secret", ok: (await call(own, "DELETE", `/secrets_store/stores/${store}/secrets/${id}`)).ok });
}
