// acquires.mjs FROM TO: BF-54 run 3's acquire path from the edge's Workers Logs: every `run/acquirejob` by status,
// the edge's "secrets:" errors (job_config), and the wall times; then the site's `acquire` answers.
const [from, to] = process.argv.slice(2);
const q = async (filters, view = "events", extra = {}) => {
  const body = { queryId: `acq-${Math.random()}`, timeframe: { from: Date.parse(from), to: Date.parse(to) }, view, limit: 2000,
    parameters: { datasets: ["cloudflare-workers"], filters, ...extra } };
  const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, { method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" }, body: JSON.stringify(body) });
  return (await r.json()).result;
};
const edge = { key: "$metadata.service", operation: "eq", type: "string", value: "graft-m4a-edge" };
const acq = { key: "$workers.event.request.url", operation: "includes", type: "string", value: "run/acquirejob" };
const by = await q([edge, acq], "calculations", { groupBys: [{ type: "number", value: "$workers.event.response.status" }], calculations: [{ operator: "count", alias: "n" }] });
console.log("acquirejob by status:", JSON.stringify((by?.calculations?.[0]?.aggregates ?? []).map((a) => [a.groups?.[0]?.value, a.value])));
const secrets = await q([edge, { key: "$metadata.message", operation: "includes", type: "string", value: "secrets:" }]);
console.log("job_config failures (edge 'secrets:' lines):", secrets?.events?.events?.length ?? 0);
for (const e of (secrets?.events?.events ?? []).slice(0, 5)) console.log("  ", new Date(e.timestamp).toISOString(), String(e.$metadata.message).slice(0, 200));
const ev = (await q([edge, acq]))?.events?.events ?? [];
const walls = ev.map((e) => e.$workers?.wallTimeMs).filter((w) => w !== undefined).sort((a, b) => a - b);
const p = (x) => walls[Math.min(walls.length - 1, Math.floor(walls.length * x))];
console.log(`acquirejob wall ms: n ${walls.length} p50 ${p(0.5)} p99 ${p(0.99)} max ${walls.at(-1)}`);
const c409 = ev.filter((e) => e.$workers?.event?.response?.status === 409);
for (const e of c409) console.log("  409", new Date(e.timestamp).toISOString(), e.$workers?.wallTimeMs, "ms");
