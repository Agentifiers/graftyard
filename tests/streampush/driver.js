// stream-push's driver Worker (`graft-m3f-sp-driver`): sends a receive-pack body of hundreds of MB or GB to the edge
// over its service binding (`EDGE`), so the push never crosses the orb's uplink or workers.dev's 100 MB body limit.
// The body is the commands, then `packgen.mjs`'s pack streamed from its manifest. POST /push with the manifest as
// the body and `authorization: Bearer <TOKEN>`; it answers {status, ms, sent, body} once the edge answered.
import { head, pack } from "./packgen.mjs";

export default {
  async fetch(req, env) {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("no", { status: 401 });
    const m = await req.json();
    const it = pack(m);
    let first = true, sent = 0;
    const body = new ReadableStream({
      pull(c) {
        if (first) { first = false; const h = head(m); sent += h.length; c.enqueue(h); return; }
        const n = it.next();
        if (n.done) return c.close();
        sent += n.value.length;
        c.enqueue(n.value);
      },
    }, { highWaterMark: 0 });
    const t0 = Date.now();
    const r = await env.EDGE.fetch(`https://${env.HOST}/m3org/stream-push.git/git-receive-pack`, {
      method: "POST",
      body,
      headers: { "content-type": "application/x-git-receive-pack-request", "user-agent": "git/2.47.0",
        authorization: req.headers.get("x-push-auth") },
    });
    const text = await r.text();
    return Response.json({ status: r.status, ms: Date.now() - t0, sent, body: text.slice(0, 1500) });
  },
};
