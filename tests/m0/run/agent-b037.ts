// M0-WP10, B0-37: provenance and hidden refs. 10,000 pushes with malformed trailers and notes, forged
// `Graftyard-Session` trailers, and writer pushes to `refs/graftyard/`, then v0, v1, v2, and `--mirror` fetches, all
// with real git against graft-m0-agent (or a local Host with --local). Raw rows: results/raw/wp10-b037-<run>*.
//
//   node run/agent-b037.ts [--local] [--url U] [--total 10000] [--k 8] [--run X]

import { writeFileSync, mkdirSync, appendFileSync, rmSync } from "node:fs";
import { parseArgs } from "node:util";
import { ROOT, Work, api, harness, pool, push, sh, tok, authArgs, baseUrl, type PushResult } from "./agent-lib.ts";
import { startLocal } from "./agent.ts";
import { APP, BASE, HOST, T1, T2, T3, T4, X1, X2 } from "../src/agent/fixture.ts";
import { SESSION_REFS } from "../src/agent/model.ts";
import { concat, pkt, FLUSH, DELIM } from "../src/agent/smart.ts";

const { values: opt } = parseArgs({ options: { local: { type: "boolean", default: false }, url: { type: "string" }, total: { type: "string", default: "10000" },
  k: { type: "string", default: "8" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const TOTAL = Number(opt.total), K = Number(opt.k);
const local = opt.local ? await startLocal() : null;
const U = local ? local.url : baseUrl(opt);
const hostName = new URL(U).host;
const RAW = `${ROOT}results/raw`;
mkdirSync(RAW, { recursive: true });
const rawPath = `${RAW}/wp10-b037-${opt.run}.jsonl`;
writeFileSync(rawPath, "");
const W = new Work(`b037-${opt.run}`);
await W.init();
const git = `${U}/${BASE}.git`;

// Shares of the 10,000.
const N = { malformed: Math.round(TOTAL * 0.5), notes: Math.round(TOTAL * 0.15), forged: Math.round(TOTAL * 0.25) };
const nHost = TOTAL - N.malformed - N.notes - N.forged;
const CONTROLS = Math.max(4, Math.round(TOTAL / 100));

console.log(`B0-37 against ${U}, run ${opt.run}: ${N.malformed} malformed trailers, ${N.notes} malformed notes, ${N.forged} forged sessions, ${nHost} refs/graftyard/ writes, ${CONTROLS * 3} controls`);
await harness(U, "/m0/setup", {});
const T = { alice: await tok("alice"), bob: await tok("bob"), carol: await tok("carol"), grunWs: await tok("grun-ws"), grunAgent: await tok("grun-agent"),
  grunT2: await tok("grun-ws-t2"), grunDone: await tok("grun-ws-done"), vendor: await tok("vendor"), flagged: await tok("flagged"), mcp: await tok("mcp-bob") };
const main = await W.commit(null, { "README.md": "mono\n", "src/a.txt": "a\n" }, "init\n");
const seeded = await push(W, git, T.alice, [`${main}:refs/heads/main`]);
if (seeded.code) throw new Error(seeded.err);
const mainTree = await W.git(["rev-parse", `${main}^{tree}`]);

interface Row { cat: string; sub: string; i: number; pusher: string; refs: string[]; accepted: boolean; summary: string; unpackOk: boolean;
                rule?: string; parseErrors?: number; parseFailed?: boolean; sources?: { commit: string; t: string; key?: string; session?: string }[];
                expect: "accepted" | "refused" | "session"; ms: number }
const rows: Row[] = [];
const P1 = `refs/heads/graftyard/${T1}/`;

// ---- Generators ----
function rng(seed: number) { let s = seed >>> 0 || 1; return () => (s = (s * 1664525 + 1013904223) >>> 0) / 2 ** 32; }
const enc = (s: string) => Buffer.from(s, "utf8");
const bytes = (n: number, r: () => number, lo = 1) => Buffer.from(Array.from({ length: n }, () => lo + Math.floor(r() * (256 - lo))));
const KEYS = ["Graftyard-Session", "Claude-Session", "Amp-Thread-ID", "Co-authored-by", "Assisted-by"];
/** One malformed message (and extra header lines) per kind; none contains NUL, so git's fsck accepts every one. */
const MALFORMED: ((i: number, r: () => number) => { header: string; msg: Buffer })[] = [
  (i, r) => ({ header: "", msg: Buffer.concat([enc(`m ${i}\n\nGraftyard-Session: `), bytes(64, r, 128), enc("\nClaude-Session: \xff\n")]) }),
  (i, r) => ({ header: "encoding bogus-charset-\u00e9\n", msg: Buffer.concat([enc(`m ${i}\n\n`), bytes(40, r, 128), enc("\nAmp-Thread-ID: "), bytes(30, r, 128), enc("\n")]) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session: https://${HOST}/${"a".repeat(200_000)}\n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\n` + Array.from({ length: 2000 }, (_, j) => `${KEYS[j % 5]}: v${j}`).join("\n") + "\n") }),
  (i) => ({ header: "", msg: enc(`m ${i}\r\n\r\nbody\r\n\r\nGraftyard-Session: x\r\nCo-authored-by: A <a@b>\r\n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\r\rGraftyard-Session: x\rClaude-Session: y\r`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraft\u200byard-Session: x\nGraftyard-Session: \u202ehttps://evil\u202c/${i}\n\ufeffClaude-Session: \u2066z\n`) }),
  (i) => ({ header: "", msg: enc(`Graftyard-Session: https://${HOST}/${BASE}/_graftyard/sessions/${T1}\nClaude-Session: ${i}`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session: a\n\tb\n   c\n \nAssisted-by:\n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session:\nClaude-Session:   \nAmp-Thread-ID: \n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session : x\nClaude-Session\t: y\n-Amp-Thread-ID: z\n: nokey\n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\n` + Array.from({ length: 40 }, (_, j) => `Graftyard-Session: https://${HOST}/${BASE}/_graftyard/sessions/${j}`).join("\n") + "\n") }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session: x\n---\ndiff --git a/x b/x\n+Claude-Session: y\n`) }),
  (i, r) => ({ header: "", msg: Buffer.concat([enc(`m ${i}\n\n`), Buffer.from(Array.from({ length: 1_500_000 }, () => 32 + Math.floor(r() * 95))), enc("\n\nGraftyard-Session: tail\n")]) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session: https://\nClaude-Session: https://[::1\nAmp-Thread-ID: https://${HOST}/%ZZ/_graftyard/sessions/%E0%A4%A\nAssisted-by: javascript:alert(1)\nGraftyard-Session: https://xn--80ak6aa92e.com/${BASE}/_graftyard/sessions/${T1}\nGraftyard-Session: https://${HOST}/${BASE}/_graftyard/sessions/../../${T1}\n`) }),
  (i, r) => ({ header: "", msg: Buffer.concat([enc(`${i}`), bytes(4096, r, 1)]) }),
  () => ({ header: "", msg: Buffer.alloc(0) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session: no-newline-at-end`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGraftyard-Session: in the middle\n\nplain closing paragraph ${i}\n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nGRAFTYARD-SESSION: a\nco-AUTHORED-by: b\nclaude-session: c\nAMP-THREAD-ID: d\n`) }),
  (i, r) => ({ header: `x-garbage ${Buffer.from(bytes(20, r, 128)).toString("latin1")}\ngpgsig -----BEGIN PGP SIGNATURE-----\n garbage ${i}\n -----END PGP SIGNATURE-----\n`, msg: enc(`m ${i}\n\nGraftyard-Session: signed-garbage\n`) }),
  (i) => ({ header: "", msg: enc(`m ${i}\n\nnot: a trailer block because\nthis line is prose and so is this one\nand this one too\nClaude-Session: z\n`) }),
];

async function malformedCommit(i: number, parent: string, n = 1): Promise<string> {
  const r = rng(i * 7919 + 13);
  let tip = parent;
  for (let j = 0; j < n; j++) {
    const g = MALFORMED[(i + j) % MALFORMED.length](i * 100 + j, r);
    tip = await W.rawCommit(mainTree, tip, g.header, g.msg);
  }
  return tip;
}

async function record(rs: Row[]): Promise<void> {
  for (let s = 0; s < rs.length; s += 500) {
    const chunk = rs.slice(s, s + 500);
    const audit = await harness(U, "/m0/audit", chunk.map((r) => `${opt.run}-${r.cat}-${r.i}`)) as ({ ok: boolean; rule?: string; parseErrors?: number; parseFailed?: boolean; sources?: Row["sources"] } | null)[];
    audit.forEach((a, j) => { if (a) Object.assign(chunk[j], { rule: a.ok ? "ok" : a.rule, parseErrors: a.parseErrors, parseFailed: a.parseFailed, sources: a.sources }); });
  }
  for (const r of rs) appendFileSync(rawPath, JSON.stringify({ ...r, sources: r.sources?.slice(0, 8) }) + "\n");
  rows.push(...rs);
}

function rowOf(cat: string, sub: string, i: number, pusher: string, refs: string[], r: PushResult, expect: Row["expect"]): Row {
  const accepted = r.code === 0 && r.refs.length > 0 && r.refs.every((x) => x.flag !== "!");
  return { cat, sub, i, pusher, refs, accepted, summary: (r.refs.find((x) => x.flag === "!")?.summary ?? r.refs[0]?.summary ?? r.err).slice(0, 200),
           unpackOk: !/unpack/.test(r.err + r.refs.map((x) => x.summary).join("")), expect, ms: r.ms };
}

// ---- 1. Malformed trailers ----
const pushers1 = [
  { name: "bob", token: T.bob, ref: (i: number) => `refs/heads/bob/m-${i}`, headers: {} },
  { name: "graftyard-workspace", token: T.grunWs, ref: (i: number) => `${P1}m-${i}`, headers: {} },
  { name: "vendor", token: T.vendor, ref: (i: number) => `refs/heads/claude/m-${i}`, headers: { "X-Graftyard-Agent-Session": X1 } },
  { name: "flagged", token: T.flagged, ref: (i: number) => `refs/heads/cursor/m-${i}`, headers: {} },
];
let t0 = Date.now();
const mal = await pool(N.malformed, K, async (i) => {
  const p = pushers1[i % pushers1.length];
  const c = await malformedCommit(i, main, i % 50 === 0 ? 20 : 1);
  const ref = p.ref(i);
  const r = await push(W, git, p.token, [`${c}:${ref}`], { headers: p.headers, probe: `${opt.run}-malformed-${i}` });
  return rowOf("malformed", `kind${i % MALFORMED.length}`, i, p.name, [ref], r, "accepted");
});
await record(mal);
console.log(`malformed trailers: ${mal.filter((r) => r.accepted).length}/${mal.length} accepted, ${((Date.now() - t0) / 1000).toFixed(0)} s`);

// ---- 2. Malformed notes, sequential on refs/notes/ai (one branch commit and one notes update per push) ----
t0 = Date.now();
let notesTip: string | null = null;
const notesRows: Row[] = [];
for (let i = 0; i < N.notes; i++) {
  const r = rng(i * 104729 + 7);
  const c = await W.commit(main, { [`notes/n${i}.txt`]: `${i}\n` }, `noted ${i}\n\nCo-authored-by: Claude <noreply@anthropic.com>\n`);
  const garbage = await W.git(["hash-object", "-w", "--stdin"], bytes(512 + Math.floor(r() * 4096), r, 0));
  const kind = i % 9;
  const entries: { mode: string; oid: string; path: string }[] = [];
  const fan = (o: string) => `${o.slice(0, 2)}/${o.slice(2)}`;
  if (kind === 0) entries.push({ mode: "100644", oid: garbage, path: c });
  if (kind === 1) entries.push({ mode: "100644", oid: garbage, path: `zz/not-a-hash-${i}` });
  if (kind === 2) entries.push({ mode: "100644", oid: garbage, path: fan(c) });
  if (kind === 3) entries.push({ mode: "100644", oid: garbage, path: `${c}/inner/deeper` });
  if (kind === 4) entries.push({ mode: "100644", oid: garbage, path: "f".repeat(40) });
  if (kind === 5) entries.push({ mode: "100644", oid: await W.git(["hash-object", "-w", "--stdin"], bytes(200_000, r, 0)), path: c });
  if (kind === 6) entries.push({ mode: "160000", oid: "e".repeat(40), path: c }, { mode: "100755", oid: garbage, path: `${c.slice(0, 39)}x` });
  if (kind === 7) for (let j = 0; j < 300; j++) entries.push({ mode: "100644", oid: garbage, path: `${(j * 2654435761 >>> 0).toString(16).padStart(8, "0")}${"0".repeat(32)}` });
  let target: string;
  if (kind === 8) target = garbage; // refs/notes/ai pointing at a blob
  else {
    const tree = await W.treeEntries(entries);
    target = await W.git(["commit-tree", tree, ...(notesTip && i % 9 !== 0 ? ["-p", notesTip] : [])], `Notes added by 'git notes add' ${i}\n`);
  }
  const res = await push(W, git, T.bob, [`${c}:refs/heads/bob/noted-${i}`, `+${target}:refs/notes/ai`], { probe: `${opt.run}-notes-${i}` });
  if (res.code === 0) notesTip = kind === 8 ? null : target;
  notesRows.push(rowOf("notes", `kind${kind}`, i, "bob", [`refs/heads/bob/noted-${i}`, "refs/notes/ai"], res, "accepted"));
}
await record(notesRows);
console.log(`malformed notes: ${notesRows.filter((r) => r.accepted).length}/${notesRows.length} accepted, ${((Date.now() - t0) / 1000).toFixed(0)} s`);

// ---- 3. Forged Graftyard-Session trailers ----
const url = (s: string, host = hostName, repo = BASE, scheme = "https") => `${scheme}://${host}/${repo}/_graftyard/sessions/${s}`;
const forgers: { name: string; token: string; ref: (i: number) => string; value: string; headers?: Record<string, string> }[] = [
  { name: "bob names T1", token: T.bob, ref: (i) => `refs/heads/bob/f-${i}`, value: url(T1) },
  { name: "alice (T1's requester) names T1", token: T.alice, ref: (i) => `refs/heads/alice/f-${i}`, value: url(T1) },
  { name: "user + session header names T1", token: T.bob, ref: (i) => `refs/heads/bob/fh-${i}`, value: url(T1), headers: { "X-Graftyard-Agent-Session": T1 } },
  { name: "flagged App names T1", token: T.flagged, ref: (i) => `refs/heads/cursor/f-${i}`, value: url(T1) },
  { name: "MCP names T1", token: T.mcp, ref: (i) => `refs/heads/mcp/f-${i}`, value: url(T1) },
  { name: "vendor names T1", token: T.vendor, ref: (i) => `refs/heads/claude/f-${i}`, value: url(T1), headers: { "X-Graftyard-Agent-Session": X1 } },
  { name: "vendor names another App's External name", token: T.vendor, ref: (i) => `refs/heads/claude/fo-${i}`, value: url(`${APP.cursor}.fix-1`), headers: { "X-Graftyard-Agent-Session": X1 } },
  { name: "workspace(T1) names T2", token: T.grunWs, ref: (i) => `${P1}f2-${i}`, value: url(T2) },
  { name: "workspace(T1) names T3", token: T.grunWs, ref: (i) => `${P1}f3-${i}`, value: url(T3) },
  { name: "workspace(T1) names T4", token: T.grunWs, ref: (i) => `${P1}f4-${i}`, value: url(T4) },
  { name: "workspace(T2) names T1", token: T.grunT2, ref: (i) => `refs/heads/graftyard/${T2}/f1-${i}`, value: url(T1) },
  { name: "workspace(T1) names T1 on another host", token: T.grunWs, ref: (i) => `${P1}fh-${i}`, value: url(T1, "evil.example") },
  { name: "workspace(T1) names T1 over http", token: T.grunWs, ref: (i) => `${P1}fs-${i}`, value: url(T1, hostName, BASE, "http") },
  { name: "workspace(T1) names T1 in another repo", token: T.grunWs, ref: (i) => `${P1}fr-${i}`, value: url(T1, hostName, "acme/other") },
  { name: "workspace(T1) names T1 with userinfo", token: T.grunWs, ref: (i) => `${P1}fu-${i}`, value: url(T1).replace("https://", "https://x@") },
  { name: "workspace(T1) names T1 with a query", token: T.grunWs, ref: (i) => `${P1}fq-${i}`, value: url(T1) + "?a=1" },
];
t0 = Date.now();
const forged = await pool(N.forged, K, async (i) => {
  const f = forgers[i % forgers.length];
  const c = await W.rawCommit(mainTree, main, "", `forged ${i}\n\nGraftyard-Session: ${f.value}\n`);
  const r = await push(W, git, f.token, [`${c}:${f.ref(i)}`], { headers: f.headers, probe: `${opt.run}-forged-${i}` });
  return rowOf("forged", f.name, i, f.name, [f.ref(i)], r, "accepted");
});
await record(forged);
console.log(`forged trailers: ${forged.filter((r) => r.accepted).length}/${forged.length} accepted, ${((Date.now() - t0) / 1000).toFixed(0)} s`);

// Positive controls (outside the 10,000): the matching pusher links.
// The third variant names the vendor's own cancelled External task: not a forgery (the pusher's App created it), and
// the design does not say whether a final task still links; this prototype links it (see "Proposed design edits").
const ctl = await pool(CONTROLS * 3, K, async (i) => {
  const v = i % 3;
  const value = v === 0 ? url(T1) : v === 1 ? url(X1) : url(X2);
  const c = await W.rawCommit(mainTree, main, "", `control ${i}\n\nGraftyard-Session: ${value}\n`);
  const ref = v === 0 ? `${P1}ok-${i}` : `refs/heads/claude/ok-${i}`;
  const r = await push(W, git, v === 0 ? T.grunWs : T.vendor, [`${c}:${ref}`], { headers: v === 0 ? {} : { "X-Graftyard-Agent-Session": X1 }, probe: `${opt.run}-control-${i}` });
  return rowOf("control", ["workspace(T1) names T1", "vendor names its live External task", "vendor names its own cancelled External task"][v], i,
    v === 0 ? "graftyard-workspace" : "vendor", [ref], r, "session");
});
await record(ctl);
console.log(`controls (expect Session): ${ctl.filter((r) => r.sources?.some((s) => s.t === "Session")).length}/${ctl.length}`);

// ---- 4. Writes under refs/graftyard/ ----
const S1 = `${SESSION_REFS}${T1}`;
// Control: graftyard-workspace creates its task's session log (one push, as journey A3 step 8 has it), then tries to
// append. Refs under refs/graftyard/ are hidden from the receive-pack advertisement too, so git sends old = zero for
// the append, and the host answers `fetch first`. Recorded, not counted in the 10,000 (see "Proposed design edits").
let sessTip = await W.commit(main, { "log/0.json": "{\"step\":0}\n" }, "session log 0\n");
const appendCtl: Row[] = [];
{
  const r = await push(W, git, T.grunWs, [`${sessTip}:${S1}`], { probe: `${opt.run}-append-0` });
  appendCtl.push(rowOf("append", "graftyard-workspace creates its task's session log", 0, "graftyard-workspace", [S1], r, "accepted"));
  for (let i = 1; i < CONTROLS; i++) {
    const c = await W.commit(sessTip, { [`log/${i}.json`]: `{"step":${i}}\n` }, `session log ${i}\n`);
    const a = await push(W, git, T.grunWs, [`${c}:${S1}`], { probe: `${opt.run}-append-${i}` });
    appendCtl.push(rowOf("append", "graftyard-workspace appends with plain git push (ref hidden from receive-pack)", i, "graftyard-workspace", [S1], a, "accepted"));
  }
}
await record(appendCtl);
console.log(`session log: create ${appendCtl[0].accepted ? "accepted" : "refused"}; appends by plain git push accepted ${appendCtl.slice(1).filter((r) => r.accepted).length}/${appendCtl.length - 1} (${appendCtl[1]?.summary})`);
const writers: { name: string; ws: boolean; do: (i: number, probe: string) => Promise<PushResult | { status: number; body: any }>; }[] = [
  { name: "alice (admin) creates a session ref", ws: false, do: async (i, probe) => push(W, git, T.alice, [`${main}:${SESSION_REFS}a-${i}`], { probe }) },
  { name: "bob (writer) creates a session ref", ws: false, do: async (i, probe) => push(W, git, T.bob, [`${main}:${SESSION_REFS}b-${i}`], { probe }) },
  { name: "bob overwrites T1's session ref", ws: false, do: async (i, probe) => push(W, git, T.bob, [`+${await W.commit(main, { x: `${i}` }, "x\n")}:${S1}`], { probe }) },
  { name: "bob deletes T1's session ref", ws: false, do: async (_i, probe) => push(W, git, T.bob, [`:${S1}`], { probe }) },
  { name: "bob writes refs/graftyard/other", ws: false, do: async (i, probe) => push(W, git, T.bob, [`${main}:refs/graftyard/other-${i}`], { probe }) },
  { name: "vendor creates a session ref", ws: false, do: async (i, probe) => push(W, git, T.vendor, [`${main}:${SESSION_REFS}${X1}-${i}`], { probe, headers: { "X-Graftyard-Agent-Session": X1 } }) },
  { name: "flagged App creates a session ref", ws: false, do: async (i, probe) => push(W, git, T.flagged, [`${main}:${SESSION_REFS}c-${i}`], { probe }) },
  { name: "MCP creates a session ref", ws: false, do: async (i, probe) => push(W, git, T.mcp, [`${main}:${SESSION_REFS}m-${i}`], { probe }) },
  { name: "carol REST-creates a session ref", ws: false, do: async (i, probe) => api(U, "POST", `/api/v3/repos/${BASE}/git/refs`, T.carol, { ref: `${SESSION_REFS}r-${i}`, sha: main }, { "x-m0-probe": probe }) },
  { name: "bob GraphQL-creates a session ref", ws: false, do: async (i, probe) => api(U, "POST", "/api/graphql", T.bob, { query: "mutation($input: CreateRefInput!) { createRef(input: $input) { clientMutationId } }",
      variables: { input: { repositoryId: `R_${BASE}`, name: `${SESSION_REFS}g-${i}`, oid: main } } }, { "x-m0-probe": probe }) },
  { name: "graftyard-workspace(T1) writes T2's session ref", ws: true, do: async (_i, probe) => push(W, git, T.grunWs, [`${main}:${SESSION_REFS}${T2}`], { probe }) },
  { name: "graftyard-workspace(T1) writes refs/graftyard/other", ws: true, do: async (i, probe) => push(W, git, T.grunWs, [`${main}:refs/graftyard/x-${i}`], { probe }) },
  { name: "graftyard-workspace(T1) deletes its session ref", ws: true, do: async (_i, probe) => push(W, git, T.grunWs, [`:${S1}`], { probe }) },
  { name: "graftyard-workspace(T1) rewrites its session ref", ws: true, do: async (i, probe) => push(W, git, T.grunWs, [`+${await W.commit(main, { y: `${i}` }, "y\n")}:${S1}`], { probe }) },
  { name: "graftyard-workspace(completed T4) appends its session ref", ws: true, do: async (_i, probe) => push(W, git, T.grunDone, [`${main}:${SESSION_REFS}${T4}`], { probe }) },
];
t0 = Date.now();
const hostRows = await pool(nHost, K, async (i) => {
  const w = writers[i % writers.length];
  const probe = `${opt.run}-hostref-${i}`;
  const r = await w.do(i, probe);
  if ("status" in r) {
    const refused = r.status >= 400 || !!r.body?.errors;
    return { cat: "hostref", sub: w.name, i, pusher: w.name, refs: [], accepted: !refused, summary: JSON.stringify(r.body?.errors?.[0]?.message ?? r.body?.message ?? r.body).slice(0, 200),
             unpackOk: true, expect: "refused", ms: 0 } as Row;
  }
  return rowOf("hostref", w.name, i, w.name, [], r, "refused");
});
await record(hostRows);
console.log(`refs/graftyard/ writes: ${hostRows.filter((r) => !r.accepted).length}/${hostRows.length} refused, ${((Date.now() - t0) / 1000).toFixed(0)} s`);

// ---- 5. Fetches ----
const fetchDir = `${ROOT}.gen/wp10-fetch-${opt.run}`;
rmSync(fetchDir, { recursive: true, force: true });
mkdirSync(fetchDir, { recursive: true });
const A = authArgs(T.bob);
const refsIn = async (dir: string) => (await sh("git", ["--git-dir", dir, "for-each-ref", "--format=%(refname)"])).out.split("\n").filter(Boolean);
const fetches: { mode: string; proto: number; graftyard: string[]; refs: number; prefixes: string[]; code: number; err: string; expectVisible: boolean }[] = [];
const prefixesOf = (trace: string) => [...new Set([...trace.matchAll(/ref-prefix ([^\s]+)/g)].map((m) => m[1]))];
async function fetchCase(mode: string, proto: number, args: string[], dir: string | null, expectVisible: boolean): Promise<void> {
  const r = await sh("git", [...A, "-c", `protocol.version=${proto}`, ...args], { env: { GIT_TRACE_PACKET: "1" } });
  const listed = dir ? await refsIn(dir) : r.out.split("\n").map((l) => l.split("\t")[1]).filter(Boolean);
  fetches.push({ mode, proto, graftyard: listed.filter((x) => x.includes("graftyard/sessions") || x.includes("refs/graftyard/") || x.startsWith("refs/s/") || x.startsWith("refs/one")),
                 refs: listed.length, prefixes: prefixesOf(r.err).slice(0, 6), code: r.code, err: r.code ? r.err.split("\n").filter((l) => !l.includes("packet:")).join(" ").slice(-200) : "", expectVisible });
}
for (const v of [0, 1, 2]) {
  await fetchCase("ls-remote", v, ["ls-remote", git], null, false);
  await fetchCase("ls-remote with pattern refs/graftyard/sessions/*", v, ["ls-remote", git, "refs/graftyard/sessions/*"], null, false);
  await fetchCase("clone --mirror", v, ["clone", "-q", "--mirror", git, `${fetchDir}/mirror-v${v}.git`], `${fetchDir}/mirror-v${v}.git`, false);
  await fetchCase("clone", v, ["clone", "-q", "--bare", git, `${fetchDir}/clone-v${v}.git`], `${fetchDir}/clone-v${v}.git`, false);
  for (const [name, spec, vis] of [
    ["fetch refs/*", "+refs/*:refs/all/*", false],
    ["fetch refs/graftyard/*", "+refs/graftyard/*:refs/g/*", false],
    ["fetch refs/graftyard/sessions/*", "+refs/graftyard/sessions/*:refs/s/*", v === 2],
    ["fetch refs/graftyard/sessions/<T1>", `+${S1}:refs/one`, v === 2],
  ] as const) {
    const dir = `${fetchDir}/f-${v}-${name.replace(/\W+/g, "_")}.git`;
    await sh("git", ["init", "-q", "--bare", dir]);
    await fetchCase(name, v, ["--git-dir", dir, "fetch", "-q", git, spec], dir, vis);
  }
}
// Raw protocol: v0/v1 advertisements and v2 ls-refs with each prefix set.
const raw: { probe: string; graftyardLines: string[]; lines: number; expectVisible: boolean; status: number; body?: string }[] = [];
const auth = { authorization: `Basic ${Buffer.from(`x:${T.bob}`).toString("base64")}` };
for (const [svc, v] of [["git-upload-pack", 0], ["git-upload-pack", 1], ["git-receive-pack", 0], ["git-receive-pack", 1]] as const) {
  const r = await fetch(`${git}/info/refs?service=${svc}`, { headers: { ...auth, ...(v ? { "git-protocol": `version=${v}` } : {}) } });
  const text = await r.text();
  raw.push({ probe: `${svc} v${v} advertisement`, graftyardLines: text.split("\n").filter((l) => l.includes("refs/graftyard/")), lines: text.split("\n").length, expectVisible: false, status: r.status });
}
for (const prefixes of [[], ["refs/"], ["HEAD", "refs/heads/", "refs/tags/"], ["refs/graftyard/"], ["refs/graftyard/sessions"], ["refs/graftyard/sessions/"],
                        [`refs/graftyard/sessions/${T1.slice(0, 4)}`], ["refs/graftyard/sessions/zz"], [S1], ["refs/heads/", "refs/graftyard/sessions/"], ["refs/graftyard/x-"]]) {
  const body = concat([pkt("command=ls-refs\n"), pkt("object-format=sha1\n"), DELIM, pkt("peel\n"), pkt("symrefs\n"), ...prefixes.map((p) => pkt(`ref-prefix ${p}\n`)), FLUSH]);
  const r = await fetch(`${git}/git-upload-pack`, { method: "POST", body, headers: { ...auth, "git-protocol": "version=2", "content-type": "application/x-git-upload-pack-request" } });
  const text = await r.text();
  raw.push({ probe: `v2 ls-refs ref-prefix ${JSON.stringify(prefixes)}`, graftyardLines: text.split("\n").filter((l) => l.includes("refs/graftyard/")).map((l) => l.slice(45)),
             lines: text.split("\n").length, expectVisible: prefixes.some((p) => p.startsWith(SESSION_REFS) && S1.startsWith(p)), status: r.status });
}
// A v0 want of the session tip by object id (git checks "our ref" against its hidden refs).
{
  const body = concat([pkt(`want ${sessTip} multi_ack_detailed side-band-64k ofs-delta\n`), FLUSH, pkt("done\n")]);
  const r = await fetch(`${git}/git-upload-pack`, { method: "POST", body, headers: { ...auth, "content-type": "application/x-git-upload-pack-request" } });
  const text = Buffer.from(await r.arrayBuffer()).toString("latin1");
  raw.push({ probe: "v0 want <session tip oid>", graftyardLines: [], lines: 0, expectVisible: false, status: r.status, body: text.slice(0, 120).replace(/[^\x20-\x7e]/g, ".") });
}
{
  const body = concat([pkt("command=fetch\n"), pkt("object-format=sha1\n"), DELIM, pkt(`want ${sessTip}\n`), pkt("done\n"), FLUSH]);
  const r = await fetch(`${git}/git-upload-pack`, { method: "POST", body, headers: { ...auth, "git-protocol": "version=2", "content-type": "application/x-git-upload-pack-request" } });
  const text = Buffer.from(await r.arrayBuffer()).toString("latin1");
  raw.push({ probe: "v2 fetch want <session tip oid>", graftyardLines: [], lines: 0, expectVisible: false, status: r.status, body: text.slice(0, 120).replace(/[^\x20-\x7e]/g, ".") });
}
const hostRefs = await harness(U, `/m0/host-refs?repo=${encodeURIComponent(BASE)}`);
rmSync(fetchDir, { recursive: true, force: true });

// ---- Verdict inputs ----
const pushes = [...mal, ...notesRows, ...forged, ...hostRows];
const parseFail = [...mal, ...notesRows, ...forged].filter((r) => !r.accepted || r.parseFailed);
const forgedLinked = forged.filter((r) => r.sources?.some((s) => s.t === "Session"));
const forgedClaimed = forged.filter((r) => r.sources?.some((s) => s.t === "Claimed" && s.key === "GraftyardSession")).length;
const hostAccepted = hostRows.filter((r) => r.accepted);
const fetchBad = fetches.filter((f) => (f.graftyard.length > 0) !== f.expectVisible);
const rawBad = raw.filter((r) => r.probe.includes("ls-refs") || r.probe.includes("advertisement") ? (r.graftyardLines.length > 0) !== r.expectVisible : false);
const bySub = (rs: Row[]) => Object.fromEntries([...new Set(rs.map((r) => r.sub))].map((s) => { const x = rs.filter((r) => r.sub === s); return [s, { n: x.length, accepted: x.filter((r) => r.accepted).length,
  rules: [...new Set(x.map((r) => r.rule))], session: x.filter((r) => r.sources?.some((y) => y.t === "Session")).length, sample: x.find((r) => !r.accepted)?.summary ?? "" }]; }));
const summary = { run: opt.run, url: local ? "local" : U, pushes: pushes.length, controls: ctl.length + appendCtl.length,
  malformed: { n: mal.length, accepted: mal.filter((r) => r.accepted).length, parseFailed: mal.filter((r) => r.parseFailed).length,
               withSources: mal.filter((r) => r.sources?.length).length, kinds: bySub(mal) },
  notes: { n: notesRows.length, accepted: notesRows.filter((r) => r.accepted).length, parseErrors: notesRows.reduce((a, r) => a + (r.parseErrors ?? 0), 0),
           aiNotes: notesRows.filter((r) => r.sources?.some((s) => s.t === "AiNote")).length, kinds: bySub(notesRows) },
  forged: { n: forged.length, accepted: forged.filter((r) => r.accepted).length, linked: forgedLinked.length, claimed: forgedClaimed, kinds: bySub(forged) },
  controls: { session: bySub(ctl), append: bySub(appendCtl) },
  hostref: { n: hostRows.length, refused: hostRows.length - hostAccepted.length, kinds: bySub(hostRows) },
  parseFail: parseFail.length, parseFailSample: parseFail.slice(0, 5).map((r) => ({ cat: r.cat, sub: r.sub, summary: r.summary })),
  fetches, raw, hostRefs, fetchBad: fetchBad.length, rawBad: rawBad.length };
writeFileSync(`${RAW}/wp10-b037-${opt.run}-summary.json`, JSON.stringify(summary, null, 1));
console.log(`\npushes ${pushes.length}; failed on parsing ${parseFail.length}; forged linked ${forgedLinked.length} of ${forged.length}; refs/graftyard/ writes accepted ${hostAccepted.length} of ${hostRows.length}`);
for (const f of fetches) console.log(`  v${f.proto} ${f.mode.padEnd(40)} refs=${String(f.refs).padStart(5)} graftyard=${f.graftyard.length} expect=${f.expectVisible ? "visible" : "hidden"} prefixes=${f.prefixes.join(",")}${f.code ? ` exit ${f.code} ${f.err}` : ""}`);
for (const r of raw) console.log(`  ${r.probe.padEnd(70)} graftyard=${r.graftyardLines.length} expect=${r.expectVisible ? "visible" : "hidden"}${r.body ? ` ${r.body}` : ""}`);
console.log(`fetch cases off expectation: ${fetchBad.length}, raw probes off: ${rawBad.length}`);
// The fold's refs against git's refs on the container's disk (a restart mid-run would show here).
const check = { ...(await harness(U, "/m0/check")), boots: (await harness(U, "/m0/state")).boots ?? 0 };
console.log(`fold vs git refs: ${JSON.stringify(check)}`);
writeFileSync(`${RAW}/${"wp10-b037"}-${opt.run}-check.json`, JSON.stringify(check, null, 1));
local?.stop();
