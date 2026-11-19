// h1.mjs against a local HTTP server and a fake Workers Logs API: node --test tests/am1/rest/h1.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer, request, Agent } from "node:http";
import { EDGE_RAN, LOOKUP_FAILED, NO_EDGE, edgeInvocations, postH1, tagLost } from "./h1.mjs";

/** A server whose handler is `h`; resolves {url, close}. */
async function serve(h) {
  const s = createServer(h);
  await new Promise((r) => s.listen(0, "127.0.0.1", r));
  return { url: `http://127.0.0.1:${s.address().port}/x`, close: () => { s.closeAllConnections(); s.close(); } };
}

test("a request that keeps its socket busy but never answers ends at the deadline on a wall clock", async () => {
  // m3fe2's shape: an idle timer is restarted by every byte, so a socket that trickles never times out. The server
  // sends a byte every 50 ms and never ends the response; a 300 ms deadline must still end it.
  const srv = await serve((req, res) => {
    req.resume();
    res.writeHead(200);
    const t = setInterval(() => res.write("."), 50);
    res.on("close", () => clearInterval(t));
  });
  const agent = new Agent({ keepAlive: true });
  const t0 = Date.now();
  await assert.rejects(postH1(srv.url, Buffer.from("x"), {}, agent, 300, request), (e) => e.code === "DEADLINE");
  const took = Date.now() - t0;
  assert.ok(took >= 290 && took < 1_500, `ended after ${took} ms`);
  agent.destroy();
  srv.close();
});

test("an answer before the deadline resolves with its status, body and headers, and the deadline never fires", async () => {
  const srv = await serve((req, res) => { req.resume(); res.writeHead(201, { "server-timing": "total;dur=7" }); res.end("ok"); });
  const agent = new Agent({ keepAlive: true });
  const r = await postH1(srv.url, Buffer.from("x"), {}, agent, 200, request);
  assert.deepEqual([r.status, r.buf.toString(), r.headers.get("server-timing")], [201, "ok", "total;dur=7"]);
  // Past the deadline: a stray rejection would be an unhandled one and fail the run.
  await new Promise((s) => setTimeout(s, 300));
  agent.destroy();
  srv.close();
});

test("edgeInvocations counts the script's invocations with that client id, and throws when the API fails", async () => {
  let sent = null;
  const fetchFn = async (url, init) => {
    sent = { url, body: JSON.parse(init.body), auth: init.headers.authorization };
    return { ok: true, json: async () => ({ result: { calculations: [{ aggregates: [{ value: 1 }, { value: 2 }] }] } }) };
  };
  const n = await edgeInvocations({ account: "acct", token: "tok", script: "graft-m3f-edge", client: "run-486", from: 1, to: 2, fetchFn });
  assert.equal(n, 3);
  assert.match(sent.url, /accounts\/acct\/workers\/observability\/telemetry\/query$/);
  assert.equal(sent.auth, "Bearer tok");
  assert.deepEqual(sent.body.timeframe, { from: 1, to: 2 });
  assert.deepEqual(sent.body.parameters.filters.map((f) => f.value), ["graft-m3f-edge", "run-486"]);
  // No aggregates means no invocation, not an error.
  const none = async () => ({ ok: true, json: async () => ({ result: { calculations: [{ aggregates: [] }] } }) });
  assert.equal(await edgeInvocations({ account: "a", token: "t", script: "s", client: "c", from: 1, to: 2, fetchFn: none }), 0);
  await assert.rejects(edgeInvocations({ account: "a", token: "t", script: "s", client: "c", from: 1, to: 2,
    fetchFn: async () => ({ ok: false, status: 500 }) }), /telemetry 500/);
  await assert.rejects(edgeInvocations({ account: "a", token: "t", script: "s", client: "c", from: 1, to: 2,
    fetchFn: async () => ({ ok: true, json: async () => ({ errors: ["x"] }) }) }), /no calculations/);
});

test("tagLost: 0 invocations is lost before the edge, any is the edge's, and a lookup that keeps failing is neither", async () => {
  const lost = [{ client: "a", start: 10_000, end: 20_000 }, { client: "b", start: 10_000, end: 20_000 }, { client: "c", start: 0, end: 1 }];
  const asked = [];
  let flaky = 0;
  const count = async (client, from, to) => {
    asked.push([client, from, to]);
    if (client === "a") return 0;
    if (client === "b") { if (flaky++ === 0) throw new Error("502"); return 1; }
    throw new Error("telemetry 500");
  };
  const out = await tagLost(lost, count);
  assert.deepEqual(out.map((x) => x.edge), [NO_EDGE, EDGE_RAN, LOOKUP_FAILED]);
  assert.equal(out[2].why, "telemetry 500");
  // The window covers the push with slack on both sides, and a failed lookup is retried, at most 3 times.
  assert.deepEqual(asked[0], ["a", 5_000, 25_000]);
  assert.equal(asked.filter(([c]) => c === "b").length, 2);
  assert.equal(asked.filter(([c]) => c === "c").length, 3);
});
