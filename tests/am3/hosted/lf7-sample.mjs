// LF-7's webhook sampler (`build/BUILD-PLAN.md`, AM3-WP1, LF-7): signed `issues` deliveries POSTed through
// `graft-agents-front` with `x-graftyard-report-version`, one JSON row per delivery, until <stop file> exists.
//   node tests/am3/hosted/lf7-sample.mjs <front url> <secret file> <concurrency> <out.jsonl> <stop file>
// Each row is {t0, t1, s, ran, pin, alarm}: the status, the `graft-controller` version that answered
// (`x-graftyard-controller-version`), the `ControllerPointer` pin the front's view held for the request, and whether
// the front alarmed. The secret is `graftyard-agent`'s webhook secret as the controller reads it (its string's bytes),
// read from a 0600 file and never written anywhere.
import { createHmac, randomUUID } from "node:crypto";
import { appendFileSync, existsSync, readFileSync } from "node:fs";

const [url, secretFile, conc, out, stop] = process.argv.slice(2);
const secret = readFileSync(secretFile, "utf8").trim();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let n = 0;
const one = async () => {
  while (!existsSync(stop)) {
    const body = JSON.stringify({ action: "opened", issue: { number: ++n }, sampler: "lf7" });
    const sig = "sha256=" + createHmac("sha256", secret).update(body).digest("hex");
    const t0 = Date.now();
    try {
      const r = await fetch(url, {
        method: "POST",
        body,
        headers: { "content-type": "application/json", "x-github-event": "issues", "x-github-delivery": randomUUID(),
          "x-hub-signature-256": sig, "x-graftyard-report-version": "1", "user-agent": "graftyard-lf7" },
        signal: AbortSignal.timeout(30_000),
      });
      await r.arrayBuffer();
      const h = (k) => r.headers.get(k);
      appendFileSync(out, JSON.stringify({ t0, t1: Date.now(), s: r.status, ran: h("x-graftyard-controller-version"),
        pin: h("x-graftyard-agents-front-pin"), alarm: h("x-graftyard-agents-front-alarm") }) + "\n");
    } catch (e) {
      appendFileSync(out, JSON.stringify({ t0, t1: Date.now(), s: 0, err: String(e).slice(0, 120) }) + "\n");
      await sleep(200);
    }
  }
};
await Promise.all(Array.from({ length: Number(conc) }, one));
