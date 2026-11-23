// refusalshape's requests (`run.sh`): one Agent-class REST write (the seed's PAT with `X-Graftyard-Agent-Session`, which
// the edge classes Agent, `graftyard_git::agent::admit_class`), the same write under the plain PAT (Human), and one
// upload-pack fetch, on edge script N. The bed's members shed the writes' row reads and its shard answers `warming`
// (`site.mjs`). Each answer is counted by a1.mjs's own tally (`tests/am1/rest/refusals.mjs`) and written as an a1 row,
// `{byKind, refusals}`, for `tests/fanout/refusalshape.py`.
//   node tests/refusalshape/probe.mjs <front url> <bed dir> <edge n> <out row>
import { readFileSync, writeFileSync } from "node:fs";
import { Refusals, isWrite } from "../am1/rest/refusals.mjs";

const [F, B, N, OUT] = process.argv.slice(2);
const seed = JSON.parse(readFileSync(`${B}/seed.json`, "utf8"));
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const fetchV2 = Buffer.concat([pkt("command=fetch\n"), Buffer.from("0001"), pkt(`want ${"1".repeat(40)}\n`), pkt("done\n"), Buffer.from("0000")]);
const rest = { authorization: `token ${seed.token}`, "user-agent": "gh/2.102.1", "content-type": "application/json" };
const label = `/api/v3/repos/${seed.login}/${seed.repo}/labels`;
const reqs = [
  ["Agent", "POST", label, { ...rest, "x-graftyard-agent-session": "refusalshape" }, JSON.stringify({ name: "rs-a", color: "ffffff" })],
  ["Human", "POST", label, rest, JSON.stringify({ name: "rs-h", color: "ffffff" })],
  ["Human", "POST", `/${seed.login}/${seed.repo}.git/git-upload-pack`, { authorization: "Basic " + Buffer.from(`x:${seed.token}`).toString("base64"),
    "git-protocol": "version=2", "user-agent": "git/2.43.0", "content-type": "application/x-git-upload-pack-request" }, fetchV2],
];
const tally = new Refusals();
for (const [cls, method, path, h, body] of reqs) {
  const r = await fetch(F + path, { method, headers: { ...h, "x-cw-edge": N }, body });
  const text = await r.text();
  let parsed = text;
  try { parsed = JSON.parse(text); } catch {}
  tally.note({ method, path, cls, write: isWrite(method, path), status: r.status, headers: r.headers, body: parsed });
  console.log(`${cls} ${method} ${path.split("/").pop()}: ${r.status} retry-after ${r.headers.get("retry-after") ?? "-"} | ${text.slice(0, 120).replace(/\n/g, " ")}`);
}
writeFileSync(OUT, JSON.stringify({ byKind: {}, refusals: tally.rows() }) + "\n");
// Each request must be refused (429 or 5xx), or the bed did not reach the answer under test and the line proves nothing.
const refused = tally.rows().filter((r) => r.status === 429 || r.status >= 500).reduce((n, r) => n + r.n, 0);
if (refused !== reqs.length) { console.log(`only ${refused} of ${reqs.length} requests were refused: the bed is wrong`); process.exit(1); }
