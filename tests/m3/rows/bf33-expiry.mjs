// BF-33's expiry and replay cases over HTTPS, as gh's client (`build/BUILD-PLAN.md`, M3-WP3): `runs` device codes are
// requested and approved as m3-member, `runs` more are requested, approved, exchanged once (a token) and once more
// (the replay), then the first set waits past DEVICE_CODE_TTL (900 s) and each is exchanged. Pass: every replay and
// every expired code gets no token. Device codes stay in this process's memory; nothing secret is printed.
//   node tests/m3/rows/bf33-expiry.mjs <state dir> <runs>
import { readFileSync } from "node:fs";
const [dir, runsArg] = process.argv.slice(2);
const runs = Number(runsArg ?? 100);
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const GH = "178c6fc778ccc68e1d6a";
const TTL_MS = 900_000;
const member = readFileSync(`${dir}/m3-tokens.txt`, "utf8").split("\n").find((l) => l.startsWith("member "))?.split(" ")[1];
const form = (o) => new URLSearchParams(o).toString();
const post = async (path, body, auth) => {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(EDGE + path, { method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded", ...(auth ? { authorization: `token ${auth}` } : {}) }, body: form(body), signal: AbortSignal.timeout(120_000) });
      const b = await r.json().catch(() => null);
      if (r.status >= 500 && i < 3) throw new Error(`${r.status}`);
      return { status: r.status, body: b };
    } catch (e) {
      if (i >= 3) return { status: 0, body: { error: String(e) } };
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
};
const exchange = (device_code) => post("/login/oauth/access_token", { client_id: GH, device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
const log = (o) => console.log(JSON.stringify({ at: new Date().toISOString(), ...o }));
const c = { runs, approved: 0, firstToken: 0, replayRefused: 0, replayOther: [], expiredRefused: 0, expiredOther: [] };
const aged = [];
for (let i = 0; i < runs; i++) {
  const d = (await post("/login/device/code", { client_id: GH, scope: "repo" })).body;
  const a = await post("/login/oauth/device/approve", { user_code: d.user_code }, member);
  if (a.status === 200) c.approved++;
  aged.push({ device: d.device_code, at: Date.now() });
}
log({ phase: "aging", codes: aged.length, approved: c.approved });
for (let i = 0; i < runs; i++) {
  const d = (await post("/login/device/code", { client_id: GH, scope: "repo" })).body;
  await post("/login/oauth/device/approve", { user_code: d.user_code }, member);
  const one = await exchange(d.device_code);
  if (/^gho_/.test(one.body?.access_token ?? "")) c.firstToken++;
  const two = await exchange(d.device_code);
  if (!two.body?.access_token && ["expired_token", "bad_verification_code"].includes(two.body?.error)) c.replayRefused++;
  else c.replayOther.push({ i, status: two.status, error: two.body?.error ?? null, token: Boolean(two.body?.access_token) });
}
log({ phase: "replayed", firstToken: c.firstToken, replayRefused: c.replayRefused });
const wait = aged[aged.length - 1].at + TTL_MS + 5_000 - Date.now();
if (wait > 0) await new Promise((r) => setTimeout(r, wait));
for (const [i, a] of aged.entries()) {
  const r = await exchange(a.device);
  const age = Date.now() - a.at;
  if (!r.body?.access_token && r.body?.error === "expired_token" && age > TTL_MS) c.expiredRefused++;
  else c.expiredOther.push({ i, age, status: r.status, error: r.body?.error ?? null, token: Boolean(r.body?.access_token) });
}
log({ phase: "done", ...c, replayOther: c.replayOther.length, expiredOther: c.expiredOther.length, rows: [...c.replayOther, ...c.expiredOther] });
