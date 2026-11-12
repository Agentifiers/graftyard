// m0-carried-4's bare-object control for B0-02's leader stall (`tests/m0/results/carried-4.md`). Two Durable Objects
// with no Raft code: `Leader` (hinted enam) and `Peer` (hinted weur). Each round the leader writes a row to SQLite in
// one transaction and sends a body to the peer, in one of three modes interleaved round by round: `gate` writes then
// sends, so the output gate holds the send for the write; `nogate` sends, then writes one macrotask later, the
// member's order (`workers/site/src/host.rs`, `step`); `nowrite` only sends; `late` and `latekv` send `lateMs` after
// an earlier write (SQL, or KV with `allowUnconfirmed`); `resp` writes `respMs` after sending, just before the answer. It records, from the round's start, the
// write, its `sync()`, the peer's receipt (the peer's clock), its body arrival, the answer, and the leader's
// event-loop lag. A round starts every `everyMs`, or when the last answer arrives if that is later.

interface Env {
  LEADER: DurableObjectNamespace;
  PEER: DurableObjectNamespace;
  TOKEN: string;
}

type Round = { at: number; mode: string; bytes: number; syncMs: number; sendMs: number; peerRecvMs: number; callMs: number; lagMs: number };

export class Leader {
  constructor(private ctx: DurableObjectState, private env: Env) {
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS log (i INTEGER PRIMARY KEY, b BLOB NOT NULL)");
  }

  async fetch(req: Request): Promise<Response> {
    const u = new URL(req.url);
    // Modes, interleaved round by round on one leader and one peer, so placement and the hour are shared:
    //   gate: write, then send. nogate: send, then write one macrotask later (the member's order). nowrite: send only.
    const modes = (u.searchParams.get("modes") ?? "gate,nogate,nowrite").split(",");
    const seconds = Number(u.searchParams.get("seconds") ?? 60);
    const bytes = Number(u.searchParams.get("bytes") ?? 50_000);
    const writeBytes = Number(u.searchParams.get("writeBytes") ?? bytes);
    const everyMs = Number(u.searchParams.get("everyMs") ?? 150);
    const peer = this.env.PEER.get(this.env.PEER.idFromName(u.searchParams.get("peer") ?? "p0"), { locationHint: (u.searchParams.get("peerHint") ?? "weur") as DurableObjectLocationHint });
    // `fill=x` sends the member's B0-02 bytes: runs of "x" (the bench's 1 KB values), not random ones.
    const fill = u.searchParams.get("fill");
    const body = new Uint8Array(bytes);
    const row = new Uint8Array(writeBytes);
    if (fill) {
      body.fill(fill.charCodeAt(0));
      row.fill(fill.charCodeAt(0));
    } else {
      crypto.getRandomValues(body.subarray(0, Math.min(bytes, 65_536)));
      crypto.getRandomValues(row.subarray(0, Math.min(writeBytes, 65_536)));
    }
    let lagMax = 0, ticking = true;
    const tick = async () => {
      while (ticking) {
        const due = Date.now() + 20;
        await new Promise((r) => setTimeout(r, 20));
        lagMax = Math.max(lagMax, Date.now() - due);
      }
    };
    void tick();
    const rows: Record<string, number | string>[] = [];
    let i = Number(this.ctx.storage.sql.exec("SELECT coalesce(max(i), 0) AS m FROM log").one().m) + 1;
    const end = Date.now() + seconds * 1000;
    let k = 0;
    while (Date.now() < end) {
      const mode = modes[k++ % modes.length];
      const t0 = Date.now();
      lagMax = 0;
      // `stmts=N` splits the row over N inserts, as a member's transaction carries 80 to 400 statements; `keep=N` keeps
      // N rows (default 20), so `keep` large lets the database grow as a member's log and fold do.
      const stmts = Number(u.searchParams.get("stmts") ?? 1), keep = Number(u.searchParams.get("keep") ?? 20);
      const part = row.subarray(0, Math.ceil(row.length / stmts));
      const write = () => {
        this.ctx.storage.transactionSync(() => {
          if (stmts <= 1) this.ctx.storage.sql.exec("INSERT INTO log (i, b) VALUES (?, ?)", i, row);
          else for (let s = 0; s < stmts; s++) this.ctx.storage.sql.exec("INSERT INTO log (i, b) VALUES (?, ?)", i * 1000 + s, part);
          this.ctx.storage.sql.exec("DELETE FROM log WHERE i < ?", stmts <= 1 ? i - keep : (i - keep) * 1000);
        });
        i++;
      };
      let send!: Promise<Response>;
      let wroteAt = 0;
      const lateMs = Number(u.searchParams.get("lateMs") ?? 5);
      if (mode === "resp") {
        // A response arriving while a write is unconfirmed: send, then write `respMs` later, just before the answer
        // is due, and see whether the answer's delivery waits for the write's confirm.
        send = peer.fetch("https://peer/r", { method: "POST", body });
        await new Promise((r) => setTimeout(r, Number(u.searchParams.get("respMs") ?? 80)));
        write();
        wroteAt = Date.now();
      } else if (mode === "late" || mode === "latekv") {
        // A send issued `lateMs` after an earlier write, in a later task, while that write may be unconfirmed: the
        // member's next round leaving during its last round's confirm. `latekv` writes through the KV API with
        // `allowUnconfirmed`, which the docs exempt from the gate.
        if (mode === "late") write();
        else this.ctx.storage.put(`k${i++ % 20}`, row, { allowUnconfirmed: true });
        wroteAt = Date.now();
        await new Promise((r) => setTimeout(r, lateMs));
        send = peer.fetch("https://peer/r", { method: "POST", body });
      } else if (mode === "gate") {
        write();
        wroteAt = Date.now();
        send = peer.fetch("https://peer/r", { method: "POST", body });
      } else if (mode === "nogatekv") {
        // As `nogate`, but the write goes through KV with `allowUnconfirmed`, which does not lock the output gate.
        send = peer.fetch("https://peer/r", { method: "POST", body });
        await new Promise((r) => setTimeout(r, 0));
        this.ctx.storage.put(`k${i++ % 20}`, row, { allowUnconfirmed: true });
        wroteAt = Date.now();
      } else if (mode === "stream" || mode === "streamnowait") {
        // The body goes through a fixed-length stream. `stream` writes once the stream's writer closed (the runtime
        // took every byte); `streamnowait` writes one macrotask after the send, as `nogate` does.
        const fl = new FixedLengthStream(bytes);
        send = peer.fetch("https://peer/r", { method: "POST", body: fl.readable });
        const w = fl.writable.getWriter();
        const closed = w.write(body).then(() => w.close());
        if (mode === "stream") await closed;
        else await new Promise((r) => setTimeout(r, 0));
        write();
        wroteAt = Date.now();
        await closed;
      } else {
        send = peer.fetch("https://peer/r", { method: "POST", body });
        if (mode === "nogate") {
          // `gapMs`: how long after the send the write starts (the member's `defer_flush` uses 0).
          await new Promise((r) => setTimeout(r, Number(u.searchParams.get("gapMs") ?? 0)));
          write();
          wroteAt = Date.now();
        }
      }
      let syncEnd = 0;
      const sp = mode === "nowrite" ? Promise.resolve() : this.ctx.storage.sync().then(() => { syncEnd = Date.now(); });
      const res = await send;
      const [arrive, bodyMs] = (await res.text()).split(",").map(Number);
      const answered = Date.now();
      await sp;
      rows.push({
        mode, at: t0, sentMs: mode.startsWith("late") ? lateMs : 0, wroteMs: wroteAt ? wroteAt - t0 : -1, syncMs: syncEnd ? syncEnd - t0 : -1, arriveMs: arrive - t0,
        bodyMs, callMs: answered - t0, lagMs: lagMax,
      });
      const wait = everyMs - (Date.now() - t0);
      if (wait > 0) await new Promise((r) => setTimeout(r, wait));
    }
    ticking = false;
    return Response.json(rows);
  }
}

export class Peer {
  constructor(private ctx: DurableObjectState, private env: Env) {}
  async fetch(req: Request): Promise<Response> {
    // When the request reached this handler (on this machine's clock), and how long its body took to arrive.
    const t = Date.now();
    await req.arrayBuffer();
    return new Response(`${t},${Date.now() - t}`);
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const u = new URL(req.url);
    const name = u.searchParams.get("leader") ?? "l0";
    return env.LEADER.get(env.LEADER.idFromName(name), { locationHint: "enam" }).fetch(req);
  },
};
