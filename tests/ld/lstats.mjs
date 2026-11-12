// Every member's /stats read counters for a repository chain: voters by site, learners by doName.
const { member, statuses } = await import("../am1/hosted/lib.mjs");
const chain = process.argv[2];
const st = await statuses(chain);
const cfg = st.find((s) => s.config)?.config?.config?.normal;
const targets = [...st.map((s) => [s.site, undefined, `f${s.site}`]), ...(cfg?.learners ?? []).map((l, i) => [l.site, l.do_name, `l${i}`])];
for (const [site, name, tag] of targets) {
  try {
    const v = (await member(chain, site, "/stats", undefined, 15000, name)).v ?? {};
    console.log(tag, JSON.stringify({ role: v.role, shed: v.shedReads, shared: v.sharedReads, fr: v.followerReads, late: v.followerLate, refused: v.followerRefused, waited: v.waitedReads, builds: v.dumpBuilds, heavy: v.heavyReads, inflight: v.readsInFlight, read: v.inbound?.["/read"] }));
  } catch (e) { console.log(tag, String(e).slice(0, 80)); }
}
