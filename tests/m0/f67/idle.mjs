// F67's idle shape on graft-m3c: a fresh repository group left with three followers and no leader must elect on the
// first ordinary edge request, within the 30 s bound (`tests/m0/results/f67.md`). No `/campaign`, no `leaderOf`.
//   1. A fresh m3org repository (`mkrepos.mjs` NAME first); one task write creates and elects its group.
//   2. IDLE_S with no calls, so the platform may evict the members; then each member's `/status`. If a member still
//      leads, it is reset (`/reset`, as an eviction drops its in-memory state) and the shape is checked again after
//      15 s. The trial runs only once all three answer `follower`.
//   3. One request, KIND `read` (`GET /api/v3/repos/m3org/NAME`) or `write` (a task write), repeated on any non-2xx
//      until a 2xx or 60 s; the time from the first send to the first 2xx. Then each member's role and term.
// Rows to tests/m0/f67/raw/idle.jsonl.
//   node tests/m0/f67/idle.mjs NAME REPO_ID KIND [idle_s=150]
import { mkdirSync } from "node:fs";
import { api, ghs, log, member, row, sleep, statuses } from "../../am1/hosted/lib.mjs";

const [NAME, RID, KIND] = process.argv.slice(2, 5);
const IDLE_S = Number(process.argv[5] ?? 150);
if (!NAME || !RID || !["read", "write"].includes(KIND)) throw new Error("usage: node tests/m0/f67/idle.mjs NAME REPO_ID read|write [idle_s]");
const CHAIN = `repo:${RID}`;
const V1 = `/api/graftyard/v1/repos/m3org/${NAME}`;
mkdirSync(new URL("./raw/", import.meta.url), { recursive: true });
const OUT = new URL("./raw/idle.jsonl", import.meta.url).pathname;
const step = (name, v) => { row(OUT, { repo: NAME, kind: KIND, step: name, at: Date.now(), ...v }); log(`${NAME} ${name} ${JSON.stringify(v).slice(0, 400)}`); };
const brief = (st) => st.map((s) => [s.site, s.role ?? s.error, s.term, s.ledTerm ?? null]);

let app = (await ghs({})).body.token;
const w0 = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `idle-${NAME}-w0`, summary: "F67 idle: first write" });
step("first write", { status: w0.status, ms: w0.ms });
if (w0.status < 200 || w0.status >= 300) throw new Error(`first write answered ${w0.status}`);

await sleep(IDLE_S * 1_000);
let st = await statuses(CHAIN);
let shape = "idle";
const lead = st.find((s) => s.role === "leader");
if (lead) {
  step("still led", { members: brief(st) });
  await member(CHAIN, lead.site, "/reset", {}, 15_000).catch(() => null);
  await sleep(15_000);
  st = await statuses(CHAIN);
  shape = `reset site ${lead.site}`;
}
step("shape", { how: shape, members: brief(st) });
if (!st.every((s) => s.role === "follower")) throw new Error("not three followers; no trial");

app = (await ghs({})).body.token;
const t0 = Date.now();
const tries = [];
let ok = null;
while (Date.now() - t0 < 60_000) {
  const r = KIND === "read"
    ? await api("GET", `/api/v3/repos/m3org/${NAME}`, app)
    : await api("POST", `${V1}/tasks`, app, { kind: "external", session: `idle-${NAME}-w${tries.length + 1}`, summary: "F67 idle: one request" });
  tries.push([r.status, r.ms]);
  if (r.status >= 200 && r.status < 300) { ok = Date.now() - t0; break; }
  await sleep(500);
}
step("result", { how: shape, firstOkMs: ok, tries, campaign: "none sent", members: brief(await statuses(CHAIN)) });
