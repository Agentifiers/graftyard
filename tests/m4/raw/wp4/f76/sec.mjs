// sec.mjs SCRIPT FROM TO SUBSTR OUTCOME: per-second counts (series) of SUBSTR events with outcome OUTCOME ("-" for any).
const [script, from, to, sub, outcome] = process.argv.slice(2);
const filters = [{ key: "$metadata.service", operation: "eq", type: "string", value: script }, { key: "$workers.event.request.url", operation: "includes", type: "string", value: sub }];
if (outcome !== "-") filters.push({ key: "$workers.outcome", operation: "eq", type: "string", value: outcome });
const body = { queryId: `f76s-${Math.random()}`, timeframe: { from: Date.parse(from), to: Date.parse(to) }, view: "calculations",
  parameters: { datasets: ["cloudflare-workers"], filters, calculations: [{ operator: "count", alias: "n" }] } };
const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, {
  method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" }, body: JSON.stringify(body) });
const j = await r.json();
console.log((j.result.calculations[0].series ?? []).filter((s) => s.data[0]?.value).map((s) => `${s.time.slice(14)}:${s.data[0].value}`).join(" "));
