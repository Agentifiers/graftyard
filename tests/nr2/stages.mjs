// stages.mjs SCRIPT FROM TO : the edge's sampled `apiTiming` log lines (1 API request in 100, `workers/edge/src/api.rs`,
// `timing`) in [FROM, TO] from the Workers Observability telemetry query API, as p50 and p99 per stage and kind, plus
// the count. Prints one JSON line. Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN (read-only Observability).
//   node tests/nr2/stages.mjs graft-m3c-edge 2026-11-12T23:00:00Z 2026-11-12T23:05:00Z
const [script, from, to] = process.argv.slice(2);
if (!script || !from || !to) throw new Error("usage: stages.mjs SCRIPT FROM TO");
const account = process.env.CLOUDFLARE_ACCOUNT_ID;
const rows = [];
{
  const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${account}/workers/observability/telemetry/query`, {
    method: "POST",
    headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" },
    body: JSON.stringify({
      queryId: `nr2-stages-${Date.now()}`,
      timeframe: { from: Date.parse(from), to: Date.parse(to) },
      view: "events",
      limit: 2000,
      dry: true,
      parameters: {
        datasets: ["cloudflare-workers"],
        needle: { value: "apiTiming", isRegex: false, matchCase: true },
        filters: [{ key: "$metadata.service", operation: "eq", type: "string", value: script }],
      },
    }),
  });
  const j = await r.json();
  if (!j.success) throw new Error(JSON.stringify(j.errors ?? j).slice(0, 500));
  const evs = j.result?.events?.events ?? [];
  for (const e of evs) if (e?.source?.apiTiming) rows.push(e.source);
}
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const out = { script, from, to, n: rows.length };
for (const kind of ["rest-repo", "graphql"]) {
  const ks = rows.filter((r) => r.kind === kind && r.status === 200);
  const stages = {};
  for (const r of ks) for (const [k, v] of Object.entries(r.apiTiming ?? {})) if (typeof v === "number") (stages[k] ??= []).push(v);
  out[kind] = { n: ks.length, ...Object.fromEntries(Object.entries(stages).map(([k, xs]) => [k, [pct(xs, 0.5), pct(xs, 0.99)]])) };
}
console.log(JSON.stringify(out));
