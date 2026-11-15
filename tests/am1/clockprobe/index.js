// Throwaway probe: a busy TICK loop (100 ms timers, CPU work per tick) and a separate request's clock reads.
const spin = (ms) => { let x = 0; for (let i = 0; i < ms * 1e4; i++) x += Math.sqrt(i ^ (x & 7)); return x; };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export class P {
  constructor(state, env) { this.state = state; this.env = env; this.ticks = 0; }
  async peer() { return Number(await (await this.env.P.get(this.env.P.idFromName("peer")).fetch("https://p/peer")).text()); }
  async fetch(req) {
    const u = new URL(req.url);
    if (u.pathname === "/peer") return new Response(String(Date.now()));
    if (u.pathname === "/start") {
      const work = Number(u.searchParams.get("work") ?? 30), secs = Number(u.searchParams.get("secs") ?? 15);
      const end = Date.now() + secs * 1000;
      this.state.waitUntil?.(Promise.resolve());
      const mode = u.searchParams.get("mode") ?? "timer";
      const wait = mode === "timer" ? () => sleep(100) : () => this.peer();
      const deadline = Date.now() + secs * 1000; let n = 0;
      (async () => { this.ticks = 0; while (n++ < secs * 8) { await wait(); spin(work); this.ticks++; } })();
      return new Response("started");
    }
    // /probe: the isolate's clock and a floor raised by the last peer stamp, each against the next peer stamp.
    const r = { ticks: this.ticks, rows: [] };
    let floor = 0;
    for (let i = 0; i < 5; i++) {
      const sent = Date.now(), sentFloor = Math.max(sent, floor);
      const p = await this.peer();
      const got = Date.now();
      r.rows.push([p - sent, p - sentFloor, got - sent]);   // stamp minus clock at send: the clock's lag plus one way
      floor = Math.max(floor, p);
    }
    return Response.json(r);
  }
}
export default { async fetch(req, env) { return env.P.get(env.P.idFromName(new URL(req.url).searchParams.get("o") ?? "t4")).fetch(req); } };
