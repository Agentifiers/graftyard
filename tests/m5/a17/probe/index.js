// A17's probe (decisions/OPEN-QUESTIONS.md, A17; tests/m5/a17/row.sh): how long after a Secrets Store write does a
// running isolate that read the old value see the new one, with no new script version? Throwaway, in the runner
// account, bound to a throwaway secret PROBE. Nothing here returns or logs the value: only the first 16 hex digits of
// its SHA-256.
//
// - GET /h: this isolate's id (drawn once per isolate), the hash `PROBE.get()` answers now, and the time.
// - Watch objects, one per location hint (`/w/<hint>/start`, `/w/<hint>/log`): each reads PROBE every second on its
//   alarm, keeps its isolate id, and records every change of hash with the time it was read. An object stays in one
//   isolate while its alarm keeps it busy, so a change it records is a running isolate seeing a new value.
// Drawn at the first request: workerd disallows random values in global scope.
let isolateId = null;
const ISOLATE_ID = () => (isolateId ??= crypto.randomUUID().slice(0, 8));
async function hash(env) {
  const v = await env.PROBE.get();
  if (v == null) return "null";
  const d = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(v)));
  return [...d.slice(0, 8)].map((b) => b.toString(16).padStart(2, "0")).join("");
}
const TICK_MS = 1000;
const MAX_TICKS = 3600; // an hour at most, so a forgotten probe stops reading

export class Watch {
  constructor(state, env) { this.s = state.storage; this.env = env; }
  async fetch(req) {
    const u = new URL(req.url);
    if (u.pathname.endsWith("/start")) {
      await this.s.put("ticks", 0);
      await this.s.setAlarm(Date.now() + TICK_MS);
      return Response.json({ isolate: ISOLATE_ID(), hash: await hash(this.env), at: Date.now() });
    }
    if (u.pathname.endsWith("/stop")) { await this.s.deleteAlarm(); return Response.json({ stopped: true }); }
    const log = (await this.s.get("log")) ?? [];
    return Response.json({ isolate: ISOLATE_ID(), ticks: await this.s.get("ticks"), last: await this.s.get("last"), log });
  }
  async alarm() {
    const ticks = ((await this.s.get("ticks")) ?? 0) + 1;
    let h;
    try { h = await hash(this.env); } catch (e) { h = `error ${String(e).slice(0, 60)}`; }
    const at = Date.now();
    const last = await this.s.get("last");
    if (!last || last.hash !== h || last.isolate !== ISOLATE_ID()) {
      const log = (await this.s.get("log")) ?? [];
      log.push({ at, hash: h, isolate: ISOLATE_ID() });
      await this.s.put("log", log.slice(-200));
    }
    await this.s.put({ ticks, last: { at, hash: h, isolate: ISOLATE_ID() } });
    if (ticks < MAX_TICKS) await this.s.setAlarm(at + TICK_MS);
  }
}

export default {
  async fetch(req, env) {
    const u = new URL(req.url);
    if (req.headers.get("authorization") !== `Bearer ${env.PROBE_TOKEN}`) return new Response("unauthorized", { status: 401 });
    if (u.pathname === "/h") return Response.json({ isolate: ISOLATE_ID(), hash: await hash(env), at: Date.now(), colo: req.cf?.colo });
    const m = u.pathname.match(/^\/w\/(enam|weur|apac)\/(start|stop|log)$/);
    if (m) return env.WATCH.get(env.WATCH.idFromName(m[1]), { locationHint: m[1] }).fetch(req);
    return new Response("not found", { status: 404 });
  },
};
