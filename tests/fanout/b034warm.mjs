// LM-1 round 18: B0-34's warm step, before its window. Drives `/catchup` on every site's shard of one network through
// the test edge's bed route (`/_bed/catchup?repo=N&shard=S&wait=W`, each at most W s, `warming` past it) and polls
// ls-refs of the repository through the edge until it lists refs/heads/main, for at most `limit` minutes. One JSON
// line per catch-up call and per ls-refs try, then a summary: each site's first `/catchup` 200 and the first ls-refs
// with main, in seconds from the start. Exit 0 when main is listed, 2 past the limit. Tokens never reach a log.
//   node tests/fanout/b034warm.mjs <repo id> <owner/name> [limit minutes 45] [wait seconds 60]
import { bedGet, tokens } from "../am1/hosted/lib.mjs";
const [id, repo, limitMin = "45", waitS = "60"] = process.argv.slice(2);
const t0 = Date.now();
const until = t0 + Number(limitMin) * 60_000;
const s = () => +((Date.now() - t0) / 1000).toFixed(1);
const out = (v) => console.log(JSON.stringify({ at: new Date().toISOString(), s: s(), ...v }));
const warm = {}; // site -> seconds to the first 200 `/catchup`
let main = null;

const pkt = (x) => { const b = Buffer.from(x); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const body = Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("ref-prefix refs/heads/main\n"), Buffer.from("0000")]);
async function lsRefs() {
  const t = Date.now();
  const r = await fetch(`${process.env.GRAFT_EDGE}/${repo}.git/git-upload-pack`, { method: "POST", body,
    headers: { authorization: `token ${tokens.admin}`, "user-agent": "git/2.56.0", "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" },
    signal: AbortSignal.timeout(90_000) }).catch((e) => ({ status: 0, text: async () => String(e).slice(0, 80) }));
  const text = await r.text();
  const ok = r.status === 200 && text.includes("refs/heads/main");
  out({ kind: "ls-refs", status: r.status, ms: Date.now() - t, ok, text: ok ? "main" : text.slice(0, 120).replace(/[0-9a-f]{40}/g, "<oid>") });
  return ok;
}

// One loop per site: catch-up calls back to back until one answers 200 (the shard's report), then idle.
const sites = [0, 1, 2].map(async (site) => {
  // Past main's listing a site gets 5 more minutes, so each shard's own time is still logged.
  while (Date.now() < (main === null ? until : Math.min(until, t0 + main * 1000 + 300_000)) && warm[site] === undefined) {
    const t = Date.now();
    const r = await bedGet(`/_bed/catchup?repo=${id}&shard=${site}&wait=${waitS}`).catch((e) => ({ status: 0, body: String(e).slice(0, 80) }));
    let v = r.body.slice(0, 160);
    try { const j = JSON.parse(r.body); v = { entries: j.entries, packs: j.packs, held_max: j.held_max, ms: j.ms, rehydrated: j.rehydrated, warming: j.warming }; } catch {}
    out({ kind: "catchup", site, status: r.status, ms: Date.now() - t, v });
    if (r.status === 200) warm[site] = s();
    else await new Promise((d) => setTimeout(d, 5_000));
  }
});
const refs = (async () => {
  while (Date.now() < until && !main) {
    if (await lsRefs()) { main = s(); break; }
    await new Promise((d) => setTimeout(d, 15_000));
  }
})();
await Promise.all([refs, ...sites]);
out({ kind: "summary", mainS: main, catchupS: warm, limitMin: Number(limitMin) });
process.exit(main === null ? 2 : 0);
