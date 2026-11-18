// BF-33's replay and expiry cases over HTTPS, `runs` times each: a device flow as gh's client, approved as m3-member,
// exchanged once (a token), then exchanged again (must be refused); and a device code nobody approved (pending),
// and a made-up device code (refused). Prints counts; no token is printed.
//   node tests/m3/hosted/device-replay.mjs <state dir> <runs>
import { readFileSync } from "node:fs";
const [dir, runsArg] = process.argv.slice(2);
const runs = Number(runsArg ?? 10);
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const GH = "178c6fc778ccc68e1d6a";
const member = readFileSync(`${dir}/m3-tokens.txt`, "utf8").split("\n").find((l) => l.startsWith("member "))?.split(" ")[1];
const form = (o) => new URLSearchParams(o).toString();
const post = async (path, body, auth) => {
  const r = await fetch(EDGE + path, { method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded", ...(auth ? { authorization: `token ${auth}` } : {}) }, body: form(body) });
  return { status: r.status, body: await r.json().catch(() => null) };
};
const exchange = (device_code) => post("/login/oauth/access_token", { client_id: GH, device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
const c = { first: 0, replayRefused: 0, pending: 0, unknownRefused: 0 };
for (let i = 0; i < runs; i++) {
  const d = (await post("/login/device/code", { client_id: GH, scope: "repo" })).body;
  const before = await exchange(d.device_code);
  if (before.body?.error === "authorization_pending") c.pending++;
  await post("/login/oauth/device/approve", { user_code: d.user_code }, member);
  const one = await exchange(d.device_code);
  if (/^gho_/.test(one.body?.access_token ?? "")) c.first++;
  const two = await exchange(d.device_code);
  if (!two.body?.access_token && ["expired_token", "bad_verification_code"].includes(two.body?.error)) c.replayRefused++;
  const bogus = await exchange("X".repeat(30));
  if (!bogus.body?.access_token && bogus.body?.error) c.unknownRefused++;
}
console.log(JSON.stringify({ runs, ...c }));
