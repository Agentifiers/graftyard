// merge-budget-3 item 4: each site's shard `/status` for a repository, with its stored test-merge counts, so a read
// after the shard object reset shows the counts outlived it. Prints one JSON line per site.
//   node tests/am1/mb3/shardstatus.mjs <repo id>
const { bedGet } = await import("../hosted/lib.mjs");
const id = process.argv[2];
for (const s of [0, 1, 2]) {
  const r = await bedGet(`/_bed/shard_status?repo=${id}&shard=${s}`).catch((e) => ({ status: 0, body: String(e) }));
  console.log(JSON.stringify({ at: new Date().toISOString(), site: s, status: r.status, body: r.body.slice(0, 900) }));
}
