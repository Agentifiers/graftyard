// One depth-1 v2 clone of REPO's main from the orb, timed to its first byte (B0-34's cold depth-1 hang, mt9; the
// failing case of decisions/DECISIONS.md, `chain-hold`): an ls-refs for the tip, then the fetch, read to its first chunk
// or 130 s. One JSON line: status, ttfbMs, the clone-cache header, the first chunk's bytes and server-timing.
//   node tests/fanout/orbclone.mjs <owner/name> <admin|member|hostile>
import { tokens, EDGE } from "../am1/hosted/lib.mjs";
const [R, who] = process.argv.slice(2);
const pkt = (s) => Buffer.from(`${(s.length + 4).toString(16).padStart(4, "0")}${s}`);
const H = { authorization: `Basic ${Buffer.from(`x:${tokens[who]}`).toString("base64")}`, "user-agent": "git/2.56.0",
  "git-protocol": "version=2", "content-type": "application/x-git-upload-pack-request" };
const ls = await fetch(`${EDGE}/${R}.git/git-upload-pack`, { method: "POST", headers: H,
  body: Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("ref-prefix refs/heads/main\n"), Buffer.from("0000")]) });
const want = Buffer.from(await ls.arrayBuffer()).toString("latin1").match(/([0-9a-f]{40}) refs\/heads\/main/)?.[1];
const t = Date.now();
try {
  const r = await fetch(`${EDGE}/${R}.git/git-upload-pack`, { method: "POST", headers: H, signal: AbortSignal.timeout(130_000),
    body: Buffer.concat([pkt("command=fetch\n"), Buffer.from("0001"), pkt("no-progress\n"), pkt("ofs-delta\n"), pkt(`want ${want}\n`),
      pkt("deepen 1\n"), pkt("done\n"), Buffer.from("0000")]) });
  const rd = r.body.getReader(); const { value } = await rd.read(); rd.cancel();
  console.log(JSON.stringify({ R, want, status: r.status, ttfbMs: Date.now() - t, cache: r.headers.get("x-graft-clone-cache"),
    retryAfter: r.headers.get("retry-after"), bytes: value?.byteLength, timing: r.headers.get("server-timing") }));
} catch (e) { console.log(JSON.stringify({ R, want, error: String(e).slice(0, 100), ms: Date.now() - t })); }
