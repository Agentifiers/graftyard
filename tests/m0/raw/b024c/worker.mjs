export default {
  async connect(socket, env, ctx) {
    const info = await socket.opened;
    const w = socket.writable.getWriter();
    await w.write(new TextEncoder().encode("graft-probe hello " + JSON.stringify(info) + "\n"));
    w.releaseLock();
    const r = socket.readable.getReader();
    const w2 = socket.writable.getWriter();
    for (;;) {
      const { value, done } = await r.read();
      if (done) break;
      await w2.write(value);
    }
    await w2.close();
  },
  async fetch() { return new Response("tcp probe\n"); },
};
