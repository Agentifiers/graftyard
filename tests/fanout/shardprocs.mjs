// shardprocs.mjs REPO_ID SECONDS [EVERY 15]: every EVERY seconds, each site's shard `/status?repo=` through the bed
// (`/_bed/shard_status`), one JSON line per site: running, the top git processes (command, nice, CPU seconds, RSS MB)
// with their summed RSS, and the network's packs (count, kept, bytes). Read only, and it starts no container: the
// shard object reads /procs only from a running one. Round 25's site-day probes:
//   (a) repack memory: summed git RSS per shard over time, against the container's peak memory from
//       `instcpu.py` (containersMetricsAdaptiveGroups) for the same minutes. A shard at its 12 GiB cap is a finding,
//       recorded with the process that held it, not a row failure to chase on the day.
//   (b) after the cold warm, `packs.kept` on every shard of repo:131296 equals the promotions loaded through their
//       stored .idx (the rehydrate report's `idx_used`, 61 on m36).
//   node tests/fanout/shardprocs.mjs 131296 1500 15 > tests/fanout/raw/shardprocs-TAG.jsonl
const { bedGet, sleep } = await import("../am1/hosted/lib.mjs");
const [id, secs, every = "15"] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
while (Date.now() < until) {
  const t = Date.now();
  await Promise.all([0, 1, 2].map(async (s) => {
    const r = await bedGet(`/_bed/shard_status?repo=${id}&shard=${s}`).catch((e) => ({ status: 0, body: String(e) }));
    let v = null;
    try { v = JSON.parse(r.body); } catch { /* an error body */ }
    const procs = v?.procs?.procs ?? [];
    console.log(JSON.stringify({
      at: new Date(t).toISOString(), site: s, status: r.status, running: v?.running ?? null,
      gitRssMb: procs.reduce((a, p) => a + (p.rss_mb ?? 0), 0), gitProcs: procs.length,
      // Stock upload-packs at once (a stock depth-1 miss is one `upload-pack` and its `pack-objects --shallow-file`,
      // about 0.4 GB private between them, round 27) and their summed RSS, against instcpu.py's container memory.
      uploads: procs.filter((p) => / upload-pack/.test(p.cmd)).length,
      uploadRssMb: procs.filter((p) => / upload-pack|--shallow-file/.test(p.cmd)).reduce((a, p) => a + (p.rss_mb ?? 0), 0),
      top: procs.slice(0, 4).map((p) => ({ cmd: p.cmd.slice(0, 80), nice: p.nice, cpu_s: p.cpu_s, rss_mb: p.rss_mb })),
      packs: v?.procs?.packs ?? null, ...(v ? {} : { error: r.body.slice(0, 120) }),
    }));
  }));
  await sleep(Math.max(0, Number(every) * 1000 - (Date.now() - t)));
}
