// M0-WP6 B0-20/B0-21 and M0-WP7 prerequisites on a real plan zone added to the Milestone 0 account.
//
//   node run/zone.ts check  --zone graftyard-m0.dev     zone id, plan, status, and whether this token can edit it
//   node run/zone.ts route  --zone graftyard-m0.dev     proxied AAAA m0.<zone> and a Worker route to graft-m0-rehydrate
//   node run/zone.ts push   --zone graftyard-m0.dev     B0-21 and B0-20 at the plan's limit: sized and chunked bodies
//                                                       around the limit, then git pushes over the edge cap and the limit
//
// The route reuses graft-m0-rehydrate's `/put` (streams the body into R2) and `/push/<cap>/r.git` (the edge's own cap
// with a report-status naming SSH), so nothing new is deployed and no graft-raft-* script is touched.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { request } from "node:https";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: { zone: { type: "string" } } });
const ROOT = new URL("../", import.meta.url).pathname;
const { CLOUDFLARE_API_TOKEN: CF, CLOUDFLARE_ACCOUNT_ID: ACCOUNT } = process.env;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const zoneName = opt.zone!;
const host = `m0.${zoneName}`;
const SCRIPT = "graft-m0-rehydrate";

async function api<T>(path: string, init: RequestInit = {}): Promise<{ success: boolean; result: T; errors: unknown[] }> {
  const r = await fetch(`https://api.cloudflare.com/client/v4${path}`, { ...init, headers: { authorization: `Bearer ${CF}`, "content-type": "application/json" } });
  return r.json() as never;
}
async function zone(): Promise<{ id: string; name: string; status: string; plan: { name: string; legacy_id: string } }> {
  const z = await api<{ id: string; name: string; status: string; plan: { name: string; legacy_id: string } }[]>(`/zones?name=${zoneName}&account.id=${ACCOUNT}`);
  if (!z.success || !z.result.length) throw new Error(`zone ${zoneName} not visible to this token: ${JSON.stringify(z.errors)}`);
  return z.result[0];
}

/** One request with a body of `mb` MiB, sized or chunked; reports status, bytes the client had sent at the response,
 *  and the first bytes of the body (side-band for the edge's report-status, HTML for Cloudflare's 413). */
function send(path: string, mb: number, chunked: boolean, headers: Record<string, string>): Promise<object> {
  const total = mb * 1024 * 1024, buf = Buffer.alloc(1 << 20, 7);
  const t0 = Date.now();
  let sent = 0, stopped = false;
  return new Promise((resolve) => {
    const req = request(`https://${host}${path}`, { method: path.startsWith("/put") ? "PUT" : "POST",
      headers: { ...headers, ...(chunked ? { "transfer-encoding": "chunked" } : { "content-length": String(total) }) } }, (res) => {
      stopped = true;
      let b = Buffer.alloc(0);
      res.on("data", (d) => (b = Buffer.concat([b, d])));
      res.on("end", () => resolve({ path: path.split("?")[0], mb, chunked, status: res.statusCode, sentMB: Math.round(sent / 1048576),
        body: b.toString("latin1").replace(/[\x00-\x1f]/g, ".").slice(0, 140), ms: Date.now() - t0 }));
    });
    req.on("error", (e) => resolve({ path, mb, chunked, error: String(e), sentMB: Math.round(sent / 1048576), ms: Date.now() - t0 }));
    const pump = () => { while (sent < total && !stopped) { sent += buf.length; if (!req.write(buf)) return void req.once("drain", pump); } req.end(); };
    pump();
  });
}

function gitPush(cap: number, mb: number): string {
  const dir = `/tmp/zone-push-${mb}`;
  if (!existsSync(dir)) {
    execFileSync("git", ["init", "-q", dir]);
    execFileSync("sh", ["-c", `head -c ${mb * 1024 * 1024} /dev/urandom > ${dir}/f.bin && git -C ${dir} add f.bin && git -C ${dir} -c user.name=a -c user.email=a@b commit -qm f`]);
  }
  try {
    return execFileSync("git", ["-C", dir, "push", `https://m0:${TOKEN}@${host}/push/${cap}/r.git/`, "HEAD:refs/heads/main"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } });
  } catch (e) {
    return String((e as { stderr?: string }).stderr ?? e).replaceAll(TOKEN, "TOKEN").split("\n").filter((l) => /remote:|error|rejected|413/.test(l)).join(" | ");
  }
}

const cmd = positionals[0];
if (cmd === "check") {
  const z = await zone();
  const dns = await api<unknown[]>(`/zones/${z.id}/dns_records?per_page=1`);
  const routes = await api<unknown[]>(`/zones/${z.id}/workers/routes`);
  const spectrum = await api<unknown[]>(`/zones/${z.id}/spectrum/apps`);
  console.log(JSON.stringify({ zone: z.name, id: z.id, status: z.status, plan: z.plan.name, dnsRead: dns.success, routesRead: routes.success,
    spectrumRead: spectrum.success, spectrumErrors: spectrum.errors }, null, 1));
} else if (cmd === "route") {
  const z = await zone();
  const rec = await api(`/zones/${z.id}/dns_records`, { method: "POST", body: JSON.stringify({ type: "AAAA", name: host, content: "100::", proxied: true, comment: "M0 route to graft-m0-rehydrate" }) });
  const route = await api(`/zones/${z.id}/workers/routes`, { method: "POST", body: JSON.stringify({ pattern: `${host}/*`, script: SCRIPT }) });
  console.log(JSON.stringify({ dns: rec.success || rec.errors, route: route.success || route.errors }));
} else if (cmd === "push") {
  const z = await zone();
  const limit = { "Free Website": 100, "Pro Website": 100, "Business Website": 200 }[z.plan.name] ?? 500;
  const bearer = { authorization: `Bearer ${TOKEN}` };
  const basic = { authorization: `Basic ${Buffer.from(`m0:${TOKEN}`).toString("base64")}`, "content-type": "application/x-git-receive-pack-request" };
  const rows: object[] = [];
  // B0-21: the plan's limit for sized and chunked bodies, one MiB either side.
  for (const mb of [limit - 1, limit, limit + 1]) for (const chunked of [false, true]) rows.push(await send(`/put?key=zone-b0-21/${mb}-${chunked}`, mb, chunked, bearer));
  // B0-20: the edge's cap at limit - 20 MiB; a body between the cap and the limit, then one past the limit.
  const cap = limit - 20;
  for (const mb of [cap + 10, limit + 20]) rows.push(await send(`/push/${cap}/r.git/git-receive-pack`, mb, true, basic));
  const git = { underLimit: gitPush(cap, cap + 10), overLimit: gitPush(cap, limit + 20) };
  const out = { at: new Date().toISOString(), zone: z.name, plan: z.plan.name, limitMB: limit, edgeCapMB: cap, rows, git };
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(`${ROOT}results/raw/zone-push-${z.plan.name.split(" ")[0].toLowerCase()}.json`, JSON.stringify(out, null, 1));
  for (const d of ["/tmp/zone-push-" + (cap + 10), "/tmp/zone-push-" + (limit + 20)]) rmSync(d, { recursive: true, force: true });
  console.log(JSON.stringify(out, null, 1));
} else {
  console.log("usage: check | route | push  --zone NAME");
}
