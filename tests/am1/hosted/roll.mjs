// Finishes a site day's `--roll-shards` (workers/site/deploy.mjs) when one catchup times out: every running git cache
// instance on graft-m1's sites whose image is not IMAGE is killed and caught up through the test edge's bed routes,
// continuing past failures. Tokens reach curl on stdin, never argv.
//   node tests/am1/hosted/roll.mjs <image> [--list]
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
const [image, flag] = process.argv.slice(2);
const EDGE = "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const bed = readFileSync(process.env.BED_TOKEN_FILE ?? `${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
const cf = (path) => JSON.parse(execFileSync("curl", ["-sf", "-H", "@-",
  `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/containers/${path}`],
  { input: `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}\n` })).result;
const call = (route, q) => { try { return execFileSync("curl", ["-s", "-m", "420", "-X", "POST", "-H", "@-", "-o", "/dev/null",
  "-w", "%{http_code}", `${EDGE}/_bed/${route}?${q}`], { input: `x-bed-token: ${bed}\n` }).toString(); } catch (e) { return `fail(${e.status})`; } };
const apps = cf("applications").filter((a) => /^graft-m1-site-[a-d]-gitshard$/.test(a.name));
let old = 0, ok = 0;
for (const app of apps) {
  for (const i of cf(`applications/${app.id}/instances?per_page=1000`).instances) {
    const m = i.name.match(/^git#(\d+)#net:(\d+)$/);
    if (!m || i.status.state !== "running") continue;
    if (i.image === image) { ok++; continue; }
    old++;
    if (flag === "--list") { console.log(`${app.name} ${i.name} ${i.image.slice(-12)}`); continue; }
    const q = `repo=${m[2]}&shard=${m[1]}`;
    console.log(`roll ${app.name} ${i.name} from ${i.image.slice(-12)}: kill ${call("kill", q)}, catchup ${call("catchup", q)}`);
  }
}
console.log(`running on new image ${ok}, on another ${old}`);
