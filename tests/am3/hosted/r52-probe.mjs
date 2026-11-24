// OPEN-QUESTIONS R52: when does a new script's 1-minute Cron Trigger first fire, and does a reset speed it up? In the
// runner account (CLOUDFLARE_RUNNER_ACCOUNT_ID, CLOUDFLARE_RUNNER_API_TOKEN), three throwaway scripts <prefix>{a,b,c},
// each a no-op `scheduled` handler with Workers Logs on:
//   a: uploaded, then one PUT of `* * * * *` (what `wrangler deploy` does), never touched again;
//   b: as a, then at +2 min and +5 min the reset the LF-7 driver uses (a yearly cron, then `* * * * *`);
//   c: uploaded with no cron, the cron PUT 3 minutes later (a schedule change on a script already live).
//
//   node tests/am3/hosted/r52-probe.mjs start <prefix>    uploads and sets the crons, printing each step's time
//   node tests/am3/hosted/r52-probe.mjs reset <prefix>    b's reset (run at +2 and +5 min)
//   node tests/am3/hosted/r52-probe.mjs cron-c <prefix>   c's cron PUT (run at +3 min)
//   node tests/am3/hosted/r52-probe.mjs read <prefix> <from ms>   each script's scheduled runs from Workers Logs
//   node tests/am3/hosted/r52-probe.mjs delete <prefix>   deletes all three
const [step, prefix, from] = process.argv.slice(2);
const acct = process.env.CLOUDFLARE_RUNNER_ACCOUNT_ID;
const tok = process.env.CLOUDFLARE_RUNNER_API_TOKEN;
if (!step || !prefix || !acct || !tok) throw new Error("usage: r52-probe.mjs start|reset|cron-c|read|delete <prefix>");
const B = `https://api.cloudflare.com/client/v4/accounts/${acct}`;
const H = { authorization: `Bearer ${tok}` };
const now = () => new Date().toISOString();
async function call(method, path, body, json = true) {
  const headers = { ...H };
  if (json && body !== undefined) headers["content-type"] = "application/json";
  const r = await fetch(`${B}${path}`, { method, headers, body: json && body !== undefined ? JSON.stringify(body) : body });
  const v = await r.json().catch(() => ({}));
  return { ok: v.success === true, result: v.result, errors: (v.errors ?? []).map((e) => [e.code, e.message]) };
}
const cron = (n, crons) => call("PUT", `/workers/scripts/${n}/schedules`, crons.map((c) => ({ cron: c })));
async function upload(n) {
  const f = new FormData();
  f.append("metadata", JSON.stringify({ main_module: "index.js", compatibility_date: "2026-11-01",
    observability: { enabled: true, logs: { enabled: true, invocation_logs: true } } }));
  const code = "export default { async scheduled(e) { console.log('r52 ' + e.cron); }, fetch() { return new Response('r52'); } };";
  f.append("index.js", new Blob([code], { type: "application/javascript+module" }), "index.js");
  return call("PUT", `/workers/scripts/${n}`, f, false);
}
const out = (o) => console.log(JSON.stringify({ at: now(), ...o }));
const names = ["a", "b", "c"].map((x) => `${prefix}${x}`);
if (step === "start") {
  for (const n of names) out({ upload: n, ...(await upload(n)), result: undefined });
  for (const n of names.slice(0, 2)) out({ cron: n, ok: (await cron(n, ["* * * * *"])).ok });
} else if (step === "reset") {
  out({ reset: names[1], yearly: (await cron(names[1], ["0 0 1 1 *"])).ok });
  await new Promise((r) => setTimeout(r, 3000));
  out({ reset: names[1], minute: (await cron(names[1], ["* * * * *"])).ok });
} else if (step === "cron-c") {
  out({ cron: names[2], ok: (await cron(names[2], ["* * * * *"])).ok });
} else if (step === "read") {
  for (const n of names) {
    const body = { queryId: "q", timeframe: { from: Number(from), to: Date.now() }, view: "events", limit: 200,
      parameters: { filters: [{ key: "$metadata.service", operation: "eq", type: "string", value: n },
        { key: "$workers.eventType", operation: "eq", type: "string", value: "scheduled" }] } };
    const r = await fetch(`${B}/workers/observability/telemetry/query`,
      { method: "POST", headers: { ...H, "content-type": "application/json" }, body: JSON.stringify(body) });
    const ev = (await r.json()).result?.events?.events ?? [];
    const runs = [...new Set(ev.map((e) => Math.round((e.$workers?.event?.scheduledTime ?? e.timestamp / 1000))))].sort();
    console.log(JSON.stringify({ script: n, runs: runs.map((t) => new Date(t * 1000).toISOString().slice(11, 19)) }));
  }
} else if (step === "delete") {
  for (const n of names) out({ delete: n, ok: (await call("DELETE", `/workers/scripts/${n}?force=true`)).ok });
}
