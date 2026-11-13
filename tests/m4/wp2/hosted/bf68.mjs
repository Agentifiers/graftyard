// BF-68 on graft-m4a: an EU-resident org's search documents stay in `eu`-jurisdiction shards; a killed shard gives
// `incomplete_results` until its generation-2 rebuild, then full results.
//
// Seed (operator, idempotent): org `wp2eu` (account 7400001) with `Names` rows and an `Eu2w` record on sites [1, 0, 2],
// home 1 (weur), an opened `Authz` with m3-admin as admin, and repositories `e0`..`e<R-1>`, each with ISSUES issues
// whose titles carry the run's word. m4a has no `eu` site, so the `Eu` domain's shards bind on the weur site under the
// `eu` jurisdiction (`SearchShard::site`'s region fallback, DECISIONS "M4-WP2's hosted half").
//
// Proof per shard (the parent's option (a)): every SearchShard holding a document of the org reports the `eu`
// jurisdiction and an EU colo, and no `default`-domain shard holds one; the same after the rebuild. Then one EU shard
// holding documents is lost (`/activity {lose}`), search reports `incomplete_results` with exactly the other shards'
// hits, `{rebuild}` builds generation 2, and search is complete again.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp2/hosted/bf68.mjs [R] [ISSUES]
import { OP, accountShard, api, log, loginShard, member, must, repoShard, row, sleep, state, tokens, tools } from "../../../am1/hosted/lib.mjs";

const [R = 4, ISSUES = 3] = process.argv.slice(2).map(Number);
const out = new URL("../raw/bf68.jsonl", import.meta.url).pathname;
const ORG = 7400001, LOGIN = "wp2eu";
const EU2W = { name: "Eu2w", sites: [1, 0, 2], home: 1, generation: 1, moving_from: null, at: 5 };
const WORD = `kumquat${Date.now().toString(36)}`;
const admin = tokens.admin;
const EU_COLOS = new Set("AMS ARN ATH BCN BEG BER BRU BTS BUD CDG CPH DUB DUS FCO FRA HAM HEL KBP LHR LIS MAD MAN MRS MUC MXP OSL OTP PRG SOF STR TLL VIE WAW ZRH ZAG VNO RIX LUX".split(" "));
const SHARDS = 16;

// ---- seed ----
const home = `names:${accountShard(ORG)}`;
await tools("/create", { chain: home });
await tools("/create", { chain: `names:${loginShard(LOGIN)}` });
await must(home, { cred: OP, op: { PutAccount: { id: ORG, suspended: false, sessions_not_before: 0 } } });
await must(`names:${loginShard(LOGIN)}`, { cred: OP, op: { ClaimLogin: { login: LOGIN, kind: "Org", account: ORG } } }, "Taken");
await must(home, { cred: OP, op: { SetAccountLogin: { account: ORG, login: LOGIN } } });
await must(home, { cred: OP, op: { InitPlacement: { owner: ORG, record: EU2W } } }, "RecordExists");
const placed = (await tools("/read", { chain: home, q: { placement: ORG } })).value?.placement;
log(`org ${LOGIN} = ${ORG}, record ${JSON.stringify(placed)}`);
await must(`authz:${ORG}`, { cred: OP, op: { Open: { kind: "Org", enterprise: null } } }, "AlreadyOpened");
await must(`authz:${ORG}`, { cred: OP, op: { SetMember: { user: state.admin, role: "Admin" } } });

const repos = [];
for (let i = 0; i < R; i++) {
  const name = `e${i}`;
  let r = await api("GET", `/api/v3/repos/${LOGIN}/${name}`, admin);
  if (r.status === 404) r = await api("POST", `/api/v3/orgs/${LOGIN}/repos`, admin, { name, private: false });
  if (![200, 201].includes(r.status)) throw new Error(`${name}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  repos.push({ name, id: r.body.id });
  for (let n = 0; n < ISSUES; n++) {
    const x = await api("POST", `/api/v3/repos/${LOGIN}/${name}/issues`, admin, { title: `${WORD} ${name} ${n}`, body: "BF-68" });
    if (x.status !== 201) throw new Error(`issue ${name}: ${x.status} ${JSON.stringify(x.body).slice(0, 200)}`);
  }
}
const want = R * ISSUES;
log(`repos ${JSON.stringify(repos)}; ${want} documents with ${WORD}`);

const search = async () => {
  const r = await api("GET", `/api/v3/search/issues?q=${WORD}&per_page=100`, admin);
  return { status: r.status, total: r.body?.total_count, incomplete: r.body?.incomplete_results, n: r.body?.items?.length, ms: r.ms };
};
const t0 = Date.now();
let s;
for (;;) {
  s = await search();
  if (s.total === want || Date.now() - t0 > 120_000) break;
  await sleep(1000);
}
log(`indexed: ${JSON.stringify(s)} after ${Date.now() - t0} ms`);
row(out, { step: "indexed", word: WORD, want, search: s, ms: Date.now() - t0 });

// ---- per shard ----
const anyRepo = `repo:${state.repo}`;
const act = async (b) => (await member(anyRepo, 0, "/activity", b, 60_000)).v;
// Each (domain, shard) at generations `gens`: the stats of the newest one that holds documents, else the first.
async function census(tag, gens) {
  const rows = [];
  for (const domain of ["eu", "default"]) {
    for (let shard = 0; shard < SHARDS; shard++) {
      let st = null;
      for (const g of gens) {
        const v = await act({ shardStats: `search#${domain}#${shard}#${g}`, owner: ORG });
        if (v?.unavailable || v?.error) { if (g === 1) st = { name: `search#${domain}#${shard}#${g}`, unavailable: v.unavailable ?? v.error }; continue; }
        if ((v?.docs ?? 0) > 0 || g === 1) st = v;
      }
      rows.push({ domain, shard, ...st });
    }
  }
  const holding = rows.filter((r) => (r.ownerDocs ?? 0) > 0);
  const outside = holding.filter((r) => r.domain !== "eu" || r.jurisdiction !== "eu" || !EU_COLOS.has(r.colo));
  const held = holding.reduce((a, r) => a + r.ownerDocs, 0);
  log(`${tag}: ${holding.length} shards hold ${held} docs of ${LOGIN}: ${holding.map((r) => `${r.name} ${r.jurisdiction} ${r.colo} ${r.ownerDocs}`).join("; ")}; outside EU: ${outside.length}`);
  row(out, { step: `census-${tag}`, rows, holding: holding.map((r) => r.name), held, outside });
  return { holding, outside, held };
}
const c1 = await census("gen1", [1]);

// ---- lose one, search, rebuild ----
const victim = c1.holding.sort((a, b) => b.ownerDocs - a.ownerDocs)[0];
log(`losing ${victim.name} (${victim.ownerDocs} docs)`);
const lost = await act({ lose: victim.name });
const s2 = await search();
log(`with ${victim.name} lost: ${JSON.stringify(s2)}; want total ${want - victim.ownerDocs}, incomplete true`);
row(out, { step: "lost", victim: victim.name, lost, search: s2 });
const [, d, sh, g] = victim.name.split("#");
const next = `search#${d}#${sh}#${Number(g) + 1}`;
const t1 = Date.now();
const rebuilt = await act({ rebuild: victim.name });
log(`rebuild ${Date.now() - t1} ms: ${JSON.stringify(rebuilt).slice(0, 300)}`);
let s3;
for (const t2 = Date.now(); ; ) {
  s3 = await search();
  if ((s3.total === want && s3.incomplete === false) || Date.now() - t2 > 60_000) break;
  await sleep(1000);
}
log(`after rebuild: ${JSON.stringify(s3)}`);
row(out, { step: "rebuilt", next, rebuilt, search: s3 });
const c2 = await census("gen2", [1, 2]);
const pass = c1.outside.length === 0 && c1.held === want
  && s2.incomplete === true && s2.total === want - victim.ownerDocs
  && s3.incomplete === false && s3.total === want
  && c2.outside.length === 0 && c2.held === want && c2.holding.some((r) => r.name === next);
row(out, { step: "verdict", pass });
log(pass ? "BF-68 PASS" : "BF-68 FAIL");
