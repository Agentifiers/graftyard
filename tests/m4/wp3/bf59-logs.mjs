// BF-59's log check: counts Workers Logs events of the install's scripts in [FROM, TO] whose text includes any of the
// BF-59 values (from <out>/values.json), their base64, or their hex, through the observability query API. Prints one
// JSON line with a count per needle kind and script, never a value. Needs CLOUDFLARE_ACCOUNT_ID and an API token.
//   node tests/m4/wp3/bf59-logs.mjs <out dir> FROM TO
import { readFileSync } from "node:fs";
import { join } from "node:path";
const [out, from, to] = process.argv.slice(2);
const v = JSON.parse(readFileSync(join(out, "values.json"), "utf8"));
const scripts = ["graft-m4a-edge", "graft-m4a-ops", "graft-m4a-site-a", "graft-m4a-site-b", "graft-m4a-site-c"];
const needles = [];
for (const k of ["repo", "env", "org"]) {
  needles.push([`${k}:plain`, v[k]], [`${k}:base64`, Buffer.from(v[k]).toString("base64").replace(/=+$/, "")], [`${k}:hex`, Buffer.from(v[k]).toString("hex")]);
}
const result = { from, to, hits: 0, events: {} };
for (const script of scripts) {
  // All events of the script in the window, as a count, so a zero is not a query that matched nothing at all.
  const q = async (extra) => {
    const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, {
      method: "POST",
      headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ queryId: `bf59-${Date.now()}`, timeframe: { from: Date.parse(from), to: Date.parse(to) }, view: "calculations", dry: true,
        parameters: { datasets: ["cloudflare-workers"], filters: [{ key: "$metadata.service", operation: "eq", type: "string", value: script }, ...extra],
          calculations: [{ operator: "count", alias: "n" }], needle: undefined } }),
    });
    const j = await r.json();
    if (!j.success) throw new Error(JSON.stringify(j.errors ?? j).slice(0, 300));
    return j.result?.calculations?.[0]?.aggregates?.[0]?.value ?? 0;
  };
  result.events[script] = await q([]);
  for (const [kind, n] of needles) {
    const c = await q([{ key: "$metadata.message", operation: "includes", type: "string", value: n }]);
    const c2 = await q([{ key: "$workers.event.request.url", operation: "includes", type: "string", value: n }]);
    if (c + c2) { result.hits += c + c2; result[`${script} ${kind}`] = c + c2; }
  }
}
console.log(JSON.stringify(result));
