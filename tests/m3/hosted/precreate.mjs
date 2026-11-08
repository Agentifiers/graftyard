// R20 on a test install with no `Release` group: creates every `Tokens` shard's group through the edge's bed-token
// `/_bed/precreate?from=F&count=16`, AT_ONCE ranges at a time, the same ranges `graftyard install` posts to
// `/api/graftyard/v1/release/tokens` on a product install. Idempotent: rerun to retry failed ranges.
//   node tests/m3/hosted/precreate.mjs <out.jsonl> [AT_ONCE]
// The bed token is read from ~/.graft-wp6/bed.token (or BED_TOKEN_FILE) and sent as a header only. With
// R20_GENESIS=<genesis.signed>, an install that holds a `Release` group is driven through the product route itself,
// `POST /api/graftyard/v1/release/tokens`, with the signed genesis as the body, as `graftyard install` does.
import { appendFileSync, readFileSync } from "node:fs";

const EDGE = process.env.GRAFT_EDGE ?? "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const GENESIS = process.env.R20_GENESIS ? readFileSync(process.env.R20_GENESIS) : null;
const TOKEN = GENESIS ? "" : readFileSync(process.env.BED_TOKEN_FILE ?? `${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
const [out, atOnce = "8"] = process.argv.slice(2);
if (!out) throw new Error("usage: precreate.mjs <out.jsonl> [AT_ONCE]");
const SHARDS = 3072, RANGE = 16;

let next = 0;
const failed = [];
async function worker() {
  for (;;) {
    const from = next;
    next += RANGE;
    if (from >= SHARDS) return;
    const t0 = Date.now();
    let row;
    try {
      const r = GENESIS
        ? await fetch(`${EDGE}/api/graftyard/v1/release/tokens?from=${from}&count=${RANGE}`, {
          method: "POST", headers: { "content-type": "application/octet-stream" }, body: GENESIS, signal: AbortSignal.timeout(900_000),
        })
        : await fetch(`${EDGE}/_bed/precreate?from=${from}&count=${RANGE}`, {
          method: "POST", headers: { "x-bed-token": TOKEN }, signal: AbortSignal.timeout(900_000),
        });
      const text = await r.text();
      let body;
      try { body = JSON.parse(text); } catch { body = text.slice(0, 300); }
      row = { from, status: r.status, ms: Date.now() - t0, body };
    } catch (e) {
      row = { from, status: 0, ms: Date.now() - t0, error: String(e) };
    }
    if (row.status !== 200) failed.push(from);
    appendFileSync(out, JSON.stringify({ at: new Date().toISOString(), ...row }) + "\n");
  }
}
const t0 = Date.now();
await Promise.all(Array.from({ length: Number(atOnce) }, worker));
appendFileSync(out, JSON.stringify({ done: true, ms: Date.now() - t0, failed }) + "\n");
