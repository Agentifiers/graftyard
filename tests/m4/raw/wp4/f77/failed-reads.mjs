// evf.mjs FROM TO: for every failed acquire, the repo:65565 members' /read events in its trace with wall ms.
const [from, to] = process.argv.slice(2);
const q = async (filters, f, t, lim = 100) => {
  const body = { queryId: `evf-${Math.random()}`, timeframe: { from: f, to: t }, view: "events", limit: lim, parameters: { datasets: ["cloudflare-workers"], filters } };
  const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, { method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" }, body: JSON.stringify(body) });
  return (await r.json()).result?.events?.events ?? [];
};
const V = { "559961": "a#f0", "d3afc8": "b#f1", "9c0ab1": "c#f2", "50c89d": "b#f1old" };
const errs = await q([{ key: "$metadata.service", operation: "eq", type: "string", value: "graft-m4a-edge" }, { key: "$metadata.message", operation: "includes", type: "string", value: "rows did not answer" }], Date.parse(from), Date.parse(to), 200);
const hist = {}; const walls = [];
for (const e of errs) {
  const rel = await q([{ key: "$metadata.traceId", operation: "eq", type: "string", value: e.$metadata.traceId }], e.timestamp - 15000, e.timestamp + 5000);
  const reads = rel.filter(x => V[(x.$workers?.durableObjectId ?? "").slice(0, 6)] && x.$workers?.event?.request?.url === "https://member/read").sort((a, b) => a.timestamp - b.timestamp);
  const edge = rel.find(x => x.$metadata.service === "graft-m4a-edge" && x.$workers?.wallTimeMs);
  const s = reads.map(x => `${V[x.$workers.durableObjectId.slice(0, 6)]}:${x.$workers.wallTimeMs}/${x.$workers.cpuTimeMs}`).join(" ");
  const key = reads.map(x => V[x.$workers.durableObjectId.slice(0, 6)]).sort().join(",");
  hist[key] = (hist[key] ?? 0) + 1;
  for (const x of reads) walls.push(x.$workers.wallTimeMs);
  console.log(new Date(e.timestamp).toISOString(), "edge", edge?.$workers?.wallTimeMs, "|", s);
}
walls.sort((a, b) => a - b);
console.log("members asked per failure:", JSON.stringify(hist));
console.log("member read wall ms n", walls.length, "min", walls[0], "p50", walls[walls.length >> 1], "p90", walls[Math.floor(walls.length * 0.9)], "max", walls.at(-1), ">=1000:", walls.filter(w => w >= 1000).length, "<20:", walls.filter(w => w < 20).length);
