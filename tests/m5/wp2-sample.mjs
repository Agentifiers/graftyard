// M5-WP2's request sampler: one JSON row per request, for tests/m5/wp2-report.py. Runs until <stop file> exists.
//   node tests/m5/wp2-sample.mjs front <base url> <path> <concurrency> <out.jsonl> <stop file>
//     GET <base><path> through graft-front with `x-graftyard-report-version`: the edge version that ran, the pin the
//     request carried, whether it alarmed, and the front isolate's view.
//   node tests/m5/wp2-sample.mjs member <chain> <interval ms> <out.jsonl> <stop file>
//     A `/read` on every member of <chain> through the tools edge (tests/am1/hosted/lib.mjs), each row with the
//     member's answer kind: `stale` (with its until), `value`, or the error.
import { appendFileSync, existsSync } from "node:fs";

const [mode, ...a] = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const row = (out, v) => appendFileSync(out, JSON.stringify(v) + "\n");

if (mode === "front") {
  const [base, path, conc, out, stop] = a;
  const one = async () => {
    while (!existsSync(stop)) {
      const t0 = Date.now();
      try {
        const r = await fetch(base + path, { headers: { "x-graftyard-report-version": "1", "user-agent": "graftyard-m5-wp2" }, signal: AbortSignal.timeout(30_000) });
        await r.arrayBuffer();
        const h = (k) => r.headers.get(k);
        row(out, { t0, t1: Date.now(), s: r.status, ran: h("x-graftyard-edge-version"), pin: h("x-graftyard-front-pin"),
          alarm: h("x-graftyard-front-alarm"), view: h("x-graftyard-front-view") });
      } catch (e) {
        row(out, { t0, t1: Date.now(), s: 0, err: String(e).slice(0, 120) });
        await sleep(200);
      }
    }
  };
  await Promise.all([...Array(Number(conc))].map(one));
} else if (mode === "member") {
  const { member } = await import("../am1/hosted/lib.mjs");
  const [chain, every, out, stop] = a;
  while (!existsSync(stop)) {
    const t0 = Date.now();
    await Promise.all([0, 1, 2].map(async (site) => {
      try {
        const r = await member(chain, site, "/read", { q: { ranges: true } }, 5_000);
        const v = r.v ?? {};
        const kind = v.stale ? "stale" : v.value !== undefined ? "value" : "error";
        row(out, { t0, t1: Date.now(), site, kind, until: v.stale?.until ?? null,
          err: kind === "error" ? JSON.stringify(v.error ?? v).slice(0, 80) : undefined });
      } catch (e) {
        row(out, { t0, t1: Date.now(), site, kind: "error", err: String(e).slice(0, 80) });
      }
    }));
    await sleep(Math.max(0, Number(every) - (Date.now() - t0)));
  }
} else {
  throw new Error("mode: front or member");
}
