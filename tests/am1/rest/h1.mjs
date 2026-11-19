// B0-32's burst client for a1.mjs: one HTTP/1.1 POST that ends at a wall-clock deadline, and the edge's own record
// of whether a request that got no answer ever ran there.
//
// On m3fe2 two burst pushes got no answer and stayed open 403 s against a 200 s `timeout`. That option is the
// socket's idle timer, which any byte on the socket restarts. The edge had no invocation for either
// (`tests/am1/results/lm1-rows-m3f.md`, "Rows m3fe2"; `decisions/OPEN-QUESTIONS.md`, A32). So the deadline here runs
// on a timer from the send, whatever the socket does, and a push that gets no answer is looked up in Workers Logs by
// its `x-graft-client-push` id.
import { request as httpsRequest } from "node:https";

/** POSTs `body` to `url` on `agent` and resolves {status, buf, headers}. It rejects with code DEADLINE once `deadlineMs`
 * have passed since the send, on a wall clock. `request` is the test's seam (node:http's `request`). */
export function postH1(url, body, headers, agent, deadlineMs, request = httpsRequest) {
  return new Promise((resolve, reject) => {
    let timer = null, done = false;
    // The first outcome wins: once the deadline rejects, the socket's own error from `destroy` is dropped.
    const settle = (f) => (v) => { if (done) return; done = true; clearTimeout(timer); f(v); };
    const req = request(url, { method: "POST", headers: { ...headers, "content-length": body.length }, agent }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", settle(() => resolve({ status: res.statusCode, buf: Buffer.concat(chunks),
        headers: new Headers(Object.entries(res.headers).map(([k, v]) => [k, Array.isArray(v) ? v.join(", ") : String(v)])) })));
      res.on("error", settle(reject));
    });
    timer = setTimeout(() => {
      const e = Object.assign(new Error("deadline"), { code: "DEADLINE" });
      settle(reject)(e);
      req.destroy(e);
    }, deadlineMs);
    req.on("error", settle(reject));
    req.end(body);
  });
}

/** How many invocations of the Worker `script` carried `x-graft-client-push: client` from `from` to `to` (ms, on
 * Cloudflare's clock), from the Workers Logs calculations view. This is a driver tool, not a request path. It
 * throws when the API does not answer. A count of 0 means "never ran" only while the script logs every invocation
 * (no head sampling). graft-m3f's edge logged 1,126 of 1,126 on m3fe2. */
export async function edgeInvocations({ account, token, script, client, from, to, fetchFn = fetch }) {
  const filters = [{ key: "$metadata.service", operation: "eq", type: "string", value: script },
    { key: "$workers.event.request.headers.x-graft-client-push", operation: "eq", type: "string", value: client }];
  const r = await fetchFn(`https://api.cloudflare.com/client/v4/accounts/${account}/workers/observability/telemetry/query`, {
    method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify({ queryId: "q", timeframe: { from, to }, view: "calculations", limit: 10,
      parameters: { filters, calculations: [{ operator: "count" }] } }),
    signal: AbortSignal.timeout(30_000) });
  if (!r.ok) throw new Error(`telemetry ${r.status}`);
  const d = await r.json();
  const aggs = d?.result?.calculations?.[0]?.aggregates;
  if (!Array.isArray(aggs)) throw new Error("telemetry: no calculations");
  return aggs.reduce((n, a) => n + Number(a.value ?? 0), 0);
}

export const NO_EDGE = "no edge invocation";
export const EDGE_RAN = "edge ran it";
export const LOOKUP_FAILED = "edge lookup failed";

/** Each unanswered push `{client, start, end}` (Cloudflare's clock, ms) tagged NO_EDGE, EDGE_RAN, or LOOKUP_FAILED
 * when `count` threw `tries` times. A failed lookup is never NO_EDGE. `count(client, from, to)` gives a count. */
export async function tagLost(lost, count, tries = 3, slackMs = 5_000) {
  const out = [];
  for (const p of lost) {
    let tag = LOOKUP_FAILED, why = null;
    for (let i = 0; i < tries && tag === LOOKUP_FAILED; i++) {
      try {
        tag = (await count(p.client, p.start - slackMs, p.end + slackMs)) === 0 ? NO_EDGE : EDGE_RAN;
      } catch (e) { why = String(e?.message ?? e).slice(0, 80); }
    }
    out.push({ ...p, edge: tag, ...(tag === LOOKUP_FAILED ? { why } : {}) });
  }
  return out;
}
