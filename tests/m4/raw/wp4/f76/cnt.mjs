// cnt.mjs SCRIPT FROM TO SUBSTR [GROUPKEY] [FILTERKEY OP VALUE]: count-only per minute (exact), optional group.
const [script, from, to, sub, group, fk, fop, fv] = process.argv.slice(2);
const filters = [{ key: "$metadata.service", operation: "eq", type: "string", value: script }];
if (sub && sub !== "-") filters.push({ key: "$workers.event.request.url", operation: "includes", type: "string", value: sub });
if (fk) filters.push({ key: fk, operation: fop, type: isNaN(Number(fv)) ? "string" : "number", value: isNaN(Number(fv)) ? fv : Number(fv) });
for (let t = Date.parse(from); t < Date.parse(to); t += 60_000) {
  const body = { queryId: `f76n-${t}-${Math.random()}`, timeframe: { from: t, to: t + 60_000 }, view: "calculations", limit: 10,
    parameters: { datasets: ["cloudflare-workers"], filters, ...(group && group !== "-" ? { groupBys: [{ type: "string", value: group }] } : {}), calculations: [{ operator: "count", alias: "n" }] } };
  let j;
  for (let i = 0; i < 4; i++) {
    const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, {
      method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" }, body: JSON.stringify(body) });
    j = await r.json().catch(() => null); if (j?.success) break; await new Promise((s) => setTimeout(s, 2000));
  }
  const ag = j?.result?.calculations?.[0]?.aggregates ?? [];
  console.log(new Date(t).toISOString().slice(11, 16), ag.map((a) => `${a.groupKey ?? "all"}=${a.value}`).join(" "));
}
