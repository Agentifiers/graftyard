// Read-only probe of a push's pre-path (resolve + read gate, write class): POST git-receive-pack with git's bare
// flush "0000", which the edge answers without reaching the group. Records status, first body line, wall ms.
import { EDGE, tokens } from "../hosted/lib.mjs";
import { appendFileSync } from "node:fs";
const N = Number(process.env.N ?? 300), GAP = Number(process.env.GAP_MS ?? 2000);
const auth = { authorization: `Basic ${Buffer.from(`x:${tokens.admin}`).toString("base64")}` };
for (let i = 0; i < N; i++) {
  const t0 = Date.now();
  let st = 0, text = "";
  try {
    const r = await fetch(`${EDGE}/m3org/b0-30-r2.git/git-receive-pack`, { method: "POST", body: "0000",
      headers: { ...auth, "user-agent": "git/2.56.0", "content-type": "application/x-git-receive-pack-request" }, signal: AbortSignal.timeout(60_000) });
    st = r.status; text = (await r.text()).split("\n")[0].slice(0, 300);
  } catch (e) { text = String(e).slice(0, 120); }
  appendFileSync("tests/am1/raw/b030r2-probe.jsonl", JSON.stringify({ at: new Date(t0).toISOString(), status: st, ms: Date.now() - t0, text: st === 200 ? "" : text }) + "\n");
  await new Promise((r) => setTimeout(r, Math.max(0, GAP - (Date.now() - t0))));
}
