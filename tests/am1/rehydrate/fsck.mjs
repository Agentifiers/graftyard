// `git fsck --strict` on repo $HOT_ID's first shard on each site (`/_bed/fsck`), after a catch-up: per shard whether it
// passed, its ref count, a digest of its sorted refs (the same on every shard when they agree), and its object counts.
//   node tests/am1/rehydrate/fsck.mjs
import { bedGet, log } from "../hosted/lib.mjs";
const ID = process.env.HOT_ID;
for (const s of (process.env.SITES ?? "0,1,2").split(",").map(Number)) {
  await bedGet(`/_bed/catchup?repo=${ID}&shard=${s}&wait=110`).catch(() => null);
  const r = await bedGet(`/_bed/fsck?repo=${ID}&shard=${s}`).catch((e) => ({ status: 0, body: String(e) }));
  let v = r.body.slice(0, 300);
  try {
    const j = JSON.parse(r.body);
    const refs = Object.entries(j.refs ?? {}).sort();
    const digest = (await import("node:crypto")).createHash("sha256").update(JSON.stringify(refs)).digest("hex").slice(0, 16);
    v = JSON.stringify({ ok: j.ok, err: j.err, out: (j.out ?? "").slice(0, 300), refs: refs.length, refsDigest: digest, count: j.count });
  } catch {}
  log(`fsck shard ${s}: ${r.status} ${v}`);
}
