// telemetry.mjs SCRIPT FROM TO [PATH_SUBSTRING] : median and p99 of `$workers.cpuTimeMs` and `$workers.wallTimeMs`,
// and the count, for one Worker script's invocations in [FROM, TO] (ISO times), from the Workers Observability
// telemetry query API, optionally only those whose request URL includes PATH_SUBSTRING. Prints one JSON line.
// Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN (read-only Workers Observability).
//   node tests/nr/telemetry.mjs graft-m3c-edge 2026-11-12T15:00:00Z 2026-11-12T15:05:00Z /api/graphql
const [script, from, to, path] = process.argv.slice(2);
if (!script || !from || !to) throw new Error("usage: telemetry.mjs SCRIPT FROM TO [PATH_SUBSTRING]");
const account = process.env.CLOUDFLARE_ACCOUNT_ID;
const filters = [{ key: "$metadata.service", operation: "eq", type: "string", value: script }];
if (path) filters.push({ key: "$workers.event.request.url", operation: "includes", type: "string", value: path });
const calc = (operator, key, alias) => ({ operator, key, keyType: "number", alias });
const body = {
  queryId: `narrow-reads-${Date.now()}`,
  timeframe: { from: Date.parse(from), to: Date.parse(to) },
  view: "calculations",
  dry: true,
  parameters: {
    datasets: ["cloudflare-workers"],
    filters,
    calculations: [
      { operator: "count", alias: "n" },
      calc("median", "$workers.cpuTimeMs", "cpu_p50"),
      calc("p99", "$workers.cpuTimeMs", "cpu_p99"),
      calc("median", "$workers.wallTimeMs", "wall_p50"),
      calc("p99", "$workers.wallTimeMs", "wall_p99"),
    ],
  },
};
const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${account}/workers/observability/telemetry/query`, {
  method: "POST",
  headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" },
  body: JSON.stringify(body),
});
const j = await r.json();
if (!j.success) throw new Error(JSON.stringify(j.errors ?? j).slice(0, 500));
const out = { script, from, to, path: path ?? null };
for (const c of j.result?.calculations ?? []) out[c.alias ?? c.calculation] = c.aggregates?.[0]?.value ?? null;
console.log(JSON.stringify(out));
