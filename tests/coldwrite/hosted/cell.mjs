// `cold-write`'s hosted cell on graft-m1 (tests/am1/results/b034-cw1.md): receive-pack pushes as git sends them, each
// request on a connection of its own, so the edge's isolates are reached as new clients reach them. Rounds every
// arm every ROUND_S (default 12 s, past `READ_CACHE`, so no arm's gate entry serves the next arm), four arms in turn:
//   git     m3-admin: advertisement, then its POST (git's order; `cold-write` C's entry is the advertisement's put),
//           then a second POST on the POST's own connection (the warm POST)
//   fetch   m3-member: a v2 ls-refs fetch, then a POST with no advertisement (a fetch's Allow must never serve it)
//   other   m3-admin's advertisement, then m3-member's POST (another token's Allow must never serve it)
//   ro      the hostile owner, who holds a Read grant: advertisement, then its POST (no `may_push` bypass: 403)
// Each request's status, client ms, and `server-timing` stages go to OUT as JSON lines, with the push id the edge logs
// (`x-graft-client-push`), so tests/coldwrite/hosted/judge.py joins each POST to its isolate in Workers Logs.
//   [ARMS=git,git,fetch,other,ro] CELL_REPO_JSON=<setup.mjs output> node tests/coldwrite/hosted/cell.mjs <owner/name> <out.jsonl> [rounds 40]
import { readFileSync, appendFileSync } from "node:fs";
import { createRequire } from "node:module";
import { onePack, receiveBody, pushResult } from "../../am1/rest/wire.mjs";
import { EDGE, scan, tokens } from "../../am1/hosted/lib.mjs";
const { EnvHttpProxyAgent, fetch: ufetch } = createRequire(import.meta.url)("../../m0/node_modules/undici");

const [repo, out, roundsArg = "40"] = process.argv.slice(2);
const ROUNDS = Number(roundsArg), ROUND_S = Number(process.env.ROUND_S ?? 12);
const run = `cw${Date.now().toString(36)}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, Math.max(0, ms)));
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const LS = Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("ref-prefix refs/heads/main\n"), Buffer.from("0000")]);
const auth = (who) => ({ authorization: "Basic " + Buffer.from(`x:${tokens[who]}`).toString("base64"), "user-agent": "git/2.56.0" });
const stages = (st) => Object.fromEntries([...(st ?? "").matchAll(/([a-z_]+);dur=([\d.]+)/g)].map((m) => [m[1], Number(m[2])]));
const main = JSON.parse(readFileSync(process.env.CELL_REPO_JSON, "utf8")).main;

/** A connection of its own: a new dispatcher, closed after `fn`. */
async function fresh(fn) {
  const d = new EnvHttpProxyAgent({ keepAliveTimeout: 30_000 });
  try { return await fn(d); } finally { await d.close().catch(() => {}); }
}
async function req(d, method, path, who, body, headers = {}) {
  const t0 = Date.now();
  try {
    const r = await ufetch(`${EDGE}/${repo}.git/${path}`, { method, body, dispatcher: d, headers: { ...auth(who), ...headers },
      signal: AbortSignal.timeout(120_000) });
    const text = Buffer.from(await r.arrayBuffer()).toString("latin1");
    return { status: r.status, ms: Date.now() - t0, st: stages(r.headers.get("server-timing")), text,
      colo: (r.headers.get("cf-ray") ?? "").split("-")[1] ?? null, spread: r.headers.get("x-graft-spread") };
  } catch (e) { return { status: 0, ms: Date.now() - t0, st: {}, text: String(e).slice(0, 80) }; }
}
let seq = 0;
function post(d, who, arm, round) {
  const client = `${run}-${arm}-${round}-${++seq}`;
  const { pack, commit } = onePack(`cw/${client}.txt`, Buffer.from(client), main, client, Math.floor(Date.now() / 1000));
  const ref = `refs/heads/cw/${client}`;
  return req(d, "POST", "git-receive-pack", who, receiveBody(ref, null, commit, pack),
    { "content-type": "application/x-git-receive-pack-request", "x-graft-client-push": client }).then((r) => ({ ...r, client }));
}
const advert = (d, who) => req(d, "GET", "info/refs?service=git-receive-pack", who);
const fetchRefs = (d, who) => req(d, "POST", "git-upload-pack", who, LS,
  { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" });
function note(round, arm, step, who, r) {
  const result = step.startsWith("post") ? (r.status === 200 ? pushResult(r.text) : `http ${r.status} ${r.text.slice(0, 60).trim()}`) : null;
  appendFileSync(out, scan(JSON.stringify({ run, round, arm, step, who, at: new Date().toISOString(), status: r.status,
    ms: r.ms, result, client: r.client ?? null, colo: r.colo ?? null, st: r.st, spread: r.spread ?? null })) + "\n");
}

const arms = {
  git: async (round) => {
    note(round, "git", "advert", "admin", await fresh((d) => advert(d, "admin")));
    await fresh(async (d) => {
      note(round, "git", "post", "admin", await post(d, "admin", "git", round));
      note(round, "git", "post-warm", "admin", await post(d, "admin", "git", round));
    });
  },
  fetch: async (round) => {
    note(round, "fetch", "fetch", "member", await fresh((d) => fetchRefs(d, "member")));
    note(round, "fetch", "post", "member", await fresh((d) => post(d, "member", "fetch", round)));
  },
  other: async (round) => {
    note(round, "other", "advert", "admin", await fresh((d) => advert(d, "admin")));
    note(round, "other", "post", "member", await fresh((d) => post(d, "member", "other", round)));
  },
  ro: async (round) => {
    note(round, "ro", "advert", "hostile", await fresh((d) => advert(d, "hostile")));
    note(round, "ro", "post", "hostile", await fresh((d) => post(d, "hostile", "ro", round)));
  },
};
// Each arm has its own quarter of the round, past `READ_CACHE` from the last arm that put an entry for its token, and
// the order turns each round.
const order = (process.env.ARMS ?? "git,fetch,other,ro").split(",");
const slot = ROUND_S * 1000;
const t0 = Date.now();
console.log(`run ${run}: ${ROUNDS} rounds of ${ROUND_S} s on ${repo}`);
for (let round = 0; round < ROUNDS; round++) {
  for (let k = 0; k < order.length; k++) {
    await sleep(t0 + (round * order.length + k) * slot - Date.now());
    await arms[order[(k + round) % order.length]](round);
  }
}
console.log(`run ${run} done in ${Math.round((Date.now() - t0) / 1000)} s`);
