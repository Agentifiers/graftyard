// The shard's stored-pack path (crates/git/src/host.rs, put_stream): an R2 body handed as a stream to a fetch, so the
// isolate holds no bytes of it. /seed writes N packs of SIZE bytes; /fetch?k=K streams the first K to the sink at once.
export default {
  async fetch(req, env) {
    const u = new URL(req.url);
    const size = Number(u.searchParams.get("size") || 207000000);
    if (u.pathname === "/seed") {
      const n = Number(u.searchParams.get("n") || 4);
      for (let i = 0; i < n; i++) {
        const chunk = new Uint8Array(1 << 20).fill(i + 1);
        let left = size;
        const body = new ReadableStream({
          pull(c) {
            if (left <= 0) return c.close();
            const m = Math.min(left, chunk.length);
            c.enqueue(chunk.subarray(0, m));
            left -= m;
          },
        });
        await env.B.put(`pack-${i}`, body.pipeThrough(new FixedLengthStream(size)));
      }
      return new Response("seeded\n");
    }
    if (u.pathname === "/fetch") {
      const k = Number(u.searchParams.get("k") || 1);
      const t0 = Date.now();
      const out = await Promise.all(
        [...Array(k).keys()].map(async (i) => {
          const o = await env.B.get(`pack-${i}`);
          // mode=buffer is the control: the whole pack read into the isolate first, as a path that buffered would.
          const body = u.searchParams.get("mode") === "buffer" ? await o.arrayBuffer() : o.body;
          const r = await fetch(`${env.SINK}/spool?i=${i}`, { method: "POST", body });
          return r.text();
        }),
      );
      return Response.json({ k, ms: Date.now() - t0, out });
    }
    return new Response("not found", { status: 404 });
  },
};
