// M0-WP15, L0-11: the request recorder. A TLS listener at https://ghes.wp15.test:8443 (the GHES hostname the
// unmodified clients are pointed at; /etc/hosts maps it to 127.0.0.1, cert in ~/.cache/wp15/tls). Each request is sent
// to graft-wp15-host (agent layer on) and then to graft-wp15-base (agent layer off), in that order, and the client gets
// the "on" response. Both responses go to results/raw/wp15-l011.jsonl with the request. `/m0/` paths are refused.
// Both deployments hold the same seeded folds; requests are serialized, so a write lands on both before the next read.
//
//   node run/wp15-tee.ts [--port 8443] [--out results/raw/wp15-l011.jsonl]

import { createServer } from "node:https";
import { appendFileSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { parseArgs } from "node:util";
import { ROOT, URLS } from "./wp15.ts";

const { values: o } = parseArgs({ options: { port: { type: "string", default: "8443" }, out: { type: "string", default: `${ROOT}results/raw/wp15-l011.jsonl` } } });
const HOST = "ghes.wp15.test";
let seq = 0;
let chain: Promise<unknown> = Promise.resolve();
const HOP = new Set(["host", "connection", "keep-alive", "transfer-encoding", "content-length", "accept-encoding"]);

async function forward(target: string, method: string, path: string, headers: Record<string, string>, body: Buffer | null) {
  const t0 = Date.now();
  const r = await fetch(`${target}${path}`, { method, headers, body: body && body.length ? body : undefined, redirect: "manual" });
  const buf = Buffer.from(await r.arrayBuffer());
  const h: Record<string, string> = {};
  r.headers.forEach((v, k) => { h[k] = v; });
  return { status: r.status, headers: h, body: buf, ms: Date.now() - t0 };
}
/** Origins in bodies and headers are rewritten to the GHES hostname, so the two deployments compare as one host. */
const unhost = (s: string) => s.split(URLS.host).join(`https://${HOST}`).split(URLS.base).join(`https://${HOST}`)
  .split(new URL(URLS.host).host).join(HOST).split(new URL(URLS.base).host).join(HOST);

createServer({ key: readFileSync(`${homedir()}/.cache/wp15/tls/key.pem`), cert: readFileSync(`${homedir()}/.cache/wp15/tls/cert.pem`) }, (req, res) => {
  const parts: Buffer[] = [];
  req.on("data", (c) => parts.push(c));
  req.on("end", () => {
    chain = chain.then(async () => {
      const body = Buffer.concat(parts);
      const path = req.url ?? "/";
      if (path.startsWith("/m0/")) { res.writeHead(404).end(); return; }
      const headers: Record<string, string> = {};
      for (const [k, v] of Object.entries(req.headers)) if (typeof v === "string" && !HOP.has(k)) headers[k] = v;
      const client = String(req.headers["x-wp15-client"] ?? req.headers["user-agent"] ?? "").slice(0, 80);
      try {
        const on = await forward(URLS.host, req.method!, path, headers, body);
        const off = await forward(URLS.base, req.method!, path, headers, body);
        const n = ++seq;
        const enc = (b: Buffer) => { const s = b.toString("utf8"); return Buffer.from(s, "utf8").equals(b) ? { text: unhost(s) } : { b64: b.toString("base64") }; };
        appendFileSync(o.out!, JSON.stringify({ seq: n, at: new Date().toISOString(), client, method: req.method, path, reqHeaders: headers, reqBody: body.toString("utf8").slice(0, 4000),
          on: { status: on.status, headers: Object.fromEntries(Object.entries(on.headers).map(([k, v]) => [k, unhost(v)])), ...enc(on.body), ms: on.ms },
          off: { status: off.status, headers: Object.fromEntries(Object.entries(off.headers).map(([k, v]) => [k, unhost(v)])), ...enc(off.body), ms: off.ms } }) + "\n");
        const out = Buffer.from(unhost(on.body.toString("utf8")), "utf8");
        const hh: Record<string, string> = {};
        for (const [k, v] of Object.entries(on.headers)) if (!HOP.has(k) && k !== "content-encoding") hh[k] = unhost(v);
        res.writeHead(on.status, hh).end(Buffer.from(on.body.toString("utf8"), "utf8").equals(on.body) ? out : on.body);
      } catch (e) { res.writeHead(502).end(String(e)); }
    });
  });
}).listen(Number(o.port), "127.0.0.1", () => console.log(`tee on https://${HOST}:${o.port} -> on ${URLS.host}, off ${URLS.base}; out ${o.out}`));
