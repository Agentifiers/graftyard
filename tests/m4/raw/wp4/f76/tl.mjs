// tl.mjs FROM TO : per minute and site, count and wall p50/p99/max of broker/poll and member/runsvc, member/raft counts.
const [from, to] = process.argv.slice(2);
const calc = (operator, key, alias) => ({ operator, key, keyType: "number", alias });
async function one(script, sub, a, b) {
  const body = { queryId: `f76-${Date.now()}-${Math.random()}`, timeframe: { from: a, to: b }, view: "calculations",
    parameters: { datasets: ["cloudflare-workers"], filters: [{ key: "$metadata.service", operation: "eq", type: "string", value: script },
      { key: "$workers.event.request.url", operation: "includes", type: "string", value: sub }],
      calculations: [{ operator: "count", alias: "n" }, calc("median", "$workers.wallTimeMs", "p50"), calc("p99", "$workers.wallTimeMs", "p99"), calc("max", "$workers.wallTimeMs", "max")] } };
  for (let i = 0; i < 4; i++) {
    const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, {
      method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" }, body: JSON.stringify(body) });
    const j = await r.json().catch(() => null);
    if (j?.success) { const o = {}; for (const c of j.result.calculations) o[c.alias] = Math.round(c.aggregates?.[0]?.value ?? 0); return o; }
    await new Promise((s) => setTimeout(s, 2000));
  }
  return null;
}
for (let t = Date.parse(from); t < Date.parse(to); t += 60_000) {
  const row = [new Date(t).toISOString().slice(11, 16)];
  for (const s of ["a", "b", "c"]) {
    const [p, r] = await Promise.all([one(`graft-m4a-site-${s}`, "broker/poll", t, t + 60_000), one(`graft-m4a-site-${s}`, "member/runsvc", t, t + 60_000)]);
    row.push(`${s}: poll ${p?.n}/${p?.p50}/${p?.p99} runsvc ${r?.n}/${r?.p50}/${r?.p99}/${r?.max}`);
  }
  console.log(row.join(" | "));
}
