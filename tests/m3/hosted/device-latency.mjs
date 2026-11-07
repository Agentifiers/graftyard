// R20's measurement: `runs` device flows as gh's client, each timed at its first request (`POST /login/device/code`)
// and at its token exchange after approval as m3-member. One JSON line per run, then the medians. No token or code
// is printed: only status, timing, and whether a `gho_` came back.
//   node tests/m3/hosted/device-latency.mjs <state dir> <runs>
import { readFileSync } from "node:fs";
const [dir, runsArg] = process.argv.slice(2);
const runs = Number(runsArg ?? 12);
const EDGE = process.env.GRAFT_EDGE ?? "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const GH = "178c6fc778ccc68e1d6a";
const member = readFileSync(`${dir}/m3-tokens.txt`, "utf8").split("\n").find((l) => l.startsWith("member "))?.split(" ")[1];
const post = async (path, body, auth) => {
  const t0 = performance.now();
  const r = await fetch(EDGE + path, {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded", ...(auth ? { authorization: `token ${auth}` } : {}) },
    body: new URLSearchParams(body).toString(),
  });
  const json = await r.json().catch(() => null);
  return { status: r.status, ms: Math.round(performance.now() - t0), body: json };
};
const median = (xs) => { const s = [...xs].sort((a, b) => a - b); return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2; };
const code = [], exch = [];
for (let i = 0; i < runs; i++) {
  const d = await post("/login/device/code", { client_id: GH, scope: "repo" });
  await post("/login/oauth/device/approve", { user_code: d.body?.user_code }, member);
  const x = await post("/login/oauth/access_token", { client_id: GH, device_code: d.body?.device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
  code.push(d.ms); exch.push(x.ms);
  console.log(JSON.stringify({ run: i, code: { status: d.status, ms: d.ms }, exchange: { status: x.status, ms: x.ms, token: /^gho_/.test(x.body?.access_token ?? "") } }));
}
console.log(JSON.stringify({ runs, codeMedianMs: median(code), codeMaxMs: Math.max(...code), exchangeMedianMs: median(exch), exchangeMaxMs: Math.max(...exch) }));
