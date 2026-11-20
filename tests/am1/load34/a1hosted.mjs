// a1.mjs --hosted N: B0-34's load from N Load34 clients (load34.mjs) in place of this process. `run` starts them for
// the window, reads their progress every 15 s (passing on the members a feed poll may go to, and each clone-cache
// miss for a1's R2 fill check), stops them early if a1's window was cut (memguard's SIGTERM), and `collect` turns their
// chunks in R2 into what a1's in-process load leaves: samples, pushes, feed events and the per-client summary.
import { collect as collectClient, forget, packsPrefix, progress, start, stop } from "./driver.mjs";
import { feedLatency } from "./plan.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, Math.max(0, ms)));
// A client's tail (load34.mjs TAIL_MS) and its last flush, past the window.
const AFTER_MS = 240_000;

/** The members a feed poll may go to, as a1's `readMembers`: learners first, then the voters that do not lead. */
async function members(statuses, chain) {
  const st = await statuses(chain);
  const cfg = st.find((s) => s.config)?.config?.config?.normal;
  return [...(cfg?.learners ?? []).map((l) => ({ site: l.site, name: l.do_name, learner: true })),
    ...(cfg?.voters ?? []).filter((v) => !st.some((s) => s.site === v.site && s.role === "leader")).map((v) => ({ site: v.site, name: v.do_name, learner: false }))];
}

export function run(o) {
  const { n, RUN, b034, statuses, log, writeRow } = o;
  // Only the tokens the clients use: the admin's PAT, the App's installation token, and each hot network's owner's.
  const who = [...new Set([b034.second?.who, ...(b034.hostile ?? []).map((h) => h.who)].filter(Boolean))];
  const spec = {
    run: RUN, n, t0: o.t0Cf, seconds: o.seconds, edge: o.edge, repo: o.repo, chain: o.chain, repoId: o.repoId, target: o.target,
    main: o.main, tasks: o.tasks, plan: o.plan, tokens: { admin: o.tokens.admin, app: o.tokens.app, ...Object.fromEntries(who.map((w) => [w, o.tokens[w]])) },
    b034: { future: b034.future, packs: packsPrefix(b034.future), second: b034.second, hostile: b034.hostile,
      hotPushPerS: b034.hotPushPerS, hotClonePerS: b034.hotClonePerS, hotReadMBps: b034.hotReadMBps ?? 8 },
    marks: { killAt: o.killAt, secondAt: b034.secondAt, hostileAt: b034.hostileAt },
  };
  const states = {};
  // Each clone-cache miss once: from a progress read while the run goes, or from the last chunk after it.
  const missSeen = new Set();
  const miss = (m) => { const k = `${m.key} ${m.at}`; if (!missSeen.has(k)) { missSeen.add(k); o.onMiss(m); } };
  const done = (async () => {
    spec.members = await members(statuses, o.chain);
    const r = await start(spec, o.hints);
    // The clients' alarms are set for 20 s before t0; a start later than that shortens their window, and is written.
    const leadS = Math.round((o.t0 - Date.now()) / 1000);
    writeRow({ step: "hosted start", clients: r.started, hints: o.hints, t0: new Date(o.t0Cf).toISOString(), members: spec.members.length, leadS });
    if (leadS < 20) log(`hosted: started ${leadS} s before t0, under the alarm's 20 s`);
    log(`hosted: ${r.started} Load34 clients for ${new Date(o.t0Cf).toISOString()}, hints ${o.hints.join(",")}`);
    const planned = o.t0 + o.seconds * 1000;
    let list = spec.members, listAt = Date.now(), stopped = false;
    for (;;) {
      await sleep(15_000);
      if (!stopped && o.end() < planned && Date.now() >= o.end()) { await stop(RUN, n).catch(() => undefined); stopped = true; log("hosted: clients stopped with the window"); }
      if (Date.now() - listAt > 60_000) { list = await members(statuses, o.chain).catch(() => list); listAt = Date.now(); }
      const ps = await progress(RUN, n, list).catch((e) => { log(`hosted: progress ${String(e).slice(0, 100)}`); return []; });
      for (const p of ps) {
        states[p.i ?? "?"] = p.state;
        for (const m of p.misses ?? []) miss(m);
      }
      const by = Object.values(states).reduce((m, s) => ((m[s] = (m[s] ?? 0) + 1), m), {});
      if (Math.round((Date.now() - o.t0) / 1000) % 60 < 15) log(`hosted: ${JSON.stringify(by)}`);
      const settled = ps.length === n && ps.every((p) => p.state === "done" || p.state === "lost" || String(p.state).startsWith("failed"));
      if (settled || Date.now() > o.end() + AFTER_MS + 120_000) break;
    }
  })();
  return {
    done,
    async collect() {
      const samples = [], pushes = [], acks = [], seen = [], clients = [], lsRefsBy = {};
      for (let i = 0; i < n; i++) {
        const chunks = await collectClient(RUN, i).catch((e) => { log(`hosted: collect ${i} ${String(e).slice(0, 100)}`); return []; });
        for (const c of chunks) {
          samples.push(...c.samples); pushes.push(...c.pushes); acks.push(...c.acks); seen.push(...c.seen);
          if (c.summary) {
            clients.push(c.summary);
            for (const [k, v] of Object.entries(c.summary.lsRefsBy ?? {})) lsRefsBy[k] = (lsRefsBy[k] ?? 0) + v;
          }
          // A miss after the last progress read: its fill check starts now.
          for (const m of c.misses ?? []) miss(m);
        }
        if (!chunks.some((c) => c.done)) clients.push({ i, state: states[i] ?? "no output", chunks: chunks.length });
      }
      samples.sort((a, b) => a[0] - b[0]);
      // The specs held the row's tokens; the objects keep nothing once their output is here.
      await forget(RUN, n).catch((e) => log(`hosted: forget ${String(e).slice(0, 100)}`));
      // a1's feed rows: [poller, head, ms after its push's ack, seen at, site, misses].
      const at = new Map(acks);
      const feed = seen.filter(([head]) => at.has(head)).map(([head, seenAt, k, site, missed]) => [k, head, seenAt - at.get(head), seenAt, site, missed]);
      const requests = clients.reduce((m, c) => { for (const [k, v] of Object.entries(c.requests ?? {})) m[k] = (m[k] ?? 0) + v; return m; }, {});
      return { samples, pushes, seen: feed, lsRefsBy,
        summary: { clients: clients.sort((a, b) => a.i - b.i), n, hints: o.hints, requests, feedEvents: feedLatency(acks, seen).length,
          states: Object.values(states).reduce((m, s) => ((m[s] = (m[s] ?? 0) + 1), m), {}) } };
    },
  };
}
