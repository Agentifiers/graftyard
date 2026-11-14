// narrow-reads-3: every member's read-index read stages (`/stats` `readStages`: grant, rebuild wait, and total ms as
// n/p50/p99/max, and how many waited) and narrow answer builds (`narrowBuilds`, `narrowBuildMs`), for one repository.
//   node tests/nr3/lstats.mjs repo:ID
const { member, statuses } = await import("../am1/hosted/lib.mjs");
const chain = process.argv[2];
const st = await statuses(chain);
const cfg = st.find((s) => s.config)?.config?.config?.normal;
const targets = [...st.map((s) => [s.site, undefined, `f${s.site}`]), ...(cfg?.learners ?? []).map((l, i) => [l.site, l.do_name, `l${i}`])];
for (const [site, name, tag] of targets) {
  try {
    const v = (await member(chain, site, "/stats", undefined, 15000, name)).v ?? {};
    console.log(tag, JSON.stringify({ stages: v.readStages, builds: v.narrowBuilds, buildMs: v.narrowBuildMs, waited: v.waitedReads, late: v.followerLate, shed: v.shedReads }));
  } catch (e) { console.log(tag, String(e).slice(0, 80)); }
}
