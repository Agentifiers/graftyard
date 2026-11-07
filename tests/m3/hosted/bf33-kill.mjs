// BF-33's leader-kill case over HTTPS (`build/BUILD-PLAN.md`, M3-WP3): a device flow as gh's client, where the
// leader of the code's `Tokens` shard is killed while it decides the exchange's `UseCode`, then the exchange runs as
// gh would. Each run: `POST /login/device/code`, approve as m3-member, then `UseCode` sent through the edge's bed
// route with `kill` on its first send (`after_quorum` or `before_commit_sent`, alternating; every 5th run holds the
// killed object down 12 s), then gh's `POST /login/oauth/access_token`. The edge's own exchange spends the code
// with `UseCode` too; the one the kill rode on committed or not, so the exchange either gets a token (the killed
// command did not commit) or `expired_token` (it did, and the code is spent with no token: the login restarts with
// a new code). Pass: every run ends in one of the two, and a replay of the code never yields a token.
//   node tests/m3/hosted/bf33-kill.mjs <state dir> <runs>
import { createHash, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
const [dir, runsArg] = process.argv.slice(2);
const runs = Number(runsArg ?? 20);
const EDGE = process.env.GRAFT_EDGE ?? "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const BED = readFileSync(`${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
const GH = "178c6fc778ccc68e1d6a";
const member = readFileSync(`${dir}/m3-tokens.txt`, "utf8").split("\n").find((l) => l.startsWith("member "))?.split(" ")[1];
const form = (o) => new URLSearchParams(o).toString();
const post = async (path, body, auth) => {
  const r = await fetch(EDGE + path, { method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded", ...(auth ? { authorization: `token ${auth}` } : {}) }, body: form(body), signal: AbortSignal.timeout(180_000) });
  return { status: r.status, body: await r.json().catch(() => null) };
};
const exchange = (device_code) => post("/login/oauth/access_token", { client_id: GH, device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
const codeHash = (code) => [...createHash("sha256").update("graftyard/code/").update(code).digest()];
const shard = (h) => Buffer.from(h.slice(0, 4)).readUInt32BE(0) % 3072;
const rid = () => { const C = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"; let v = BigInt(Date.now()) & 0xffff_ffff_ffffn; for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b); let o = ""; for (let i = 25; i >= 0; i--) o += C[Number((v >> BigInt(5 * i)) & 31n)]; return o; };
const c = { runs, fired: 0, token: 0, restarted: 0, killedCommitted: 0, killedNot: 0, replayRefused: 0, other: [] };
for (let i = 1; i <= runs; i++) {
  const d = (await post("/login/device/code", { client_id: GH, scope: "repo" })).body;
  await post("/login/oauth/device/approve", { user_code: d.user_code }, member);
  const h = codeHash(d.device_code);
  const kill = i % 2 ? "after_quorum" : "before_commit_sent";
  const down_ms = i % 5 === 0 ? 12_000 : 0;
  const t0 = Date.now();
  const k = await fetch(`${EDGE}/_bed/identity`, { method: "POST", headers: { "x-bed-token": BED }, signal: AbortSignal.timeout(180_000),
    body: JSON.stringify({ chain: `tokens:${shard(h)}`, cmd: { cred: { account: null, key: null }, op: { UseCode: { hash: h, client: GH } } }, kill, down_ms }) }).then((r) => r.json()).catch((e) => ({ error: String(e) }));
  const killed = k.outcome?.ok ? "committed" : k.outcome?.refused ? `refused ${k.outcome.refused.err}` : `error ${String(k.error).slice(0, 80)}`;
  if (k.outcome?.ok) c.killedCommitted++; else c.killedNot++;
  const one = await exchange(d.device_code);
  const two = await exchange(d.device_code);
  const got = /^gho_/.test(one.body?.access_token ?? "");
  if (got) c.token++;
  else if (one.body?.error === "expired_token") c.restarted++;
  else c.other.push({ i, status: one.status, error: one.body?.error });
  if (!two.body?.access_token) c.replayRefused++;
  const sends = (k.trace ?? []).filter((t) => t.send !== undefined).length;
  const missed = (k.trace ?? []).filter((t) => t.missed !== undefined || t.notLeader !== undefined).length;
  if (sends > 1 && missed > 0) c.fired++;
  console.log(JSON.stringify({ i, kill, down_ms, killMs: Date.now() - t0, killed, sends, missed, exchange: got ? "token" : one.body?.error ?? one.status, replay: two.body?.access_token ? "TOKEN" : two.body?.error ?? two.status }));
}
console.log(JSON.stringify({ ...c, other: c.other.length, otherRows: c.other }));
