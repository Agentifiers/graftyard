// BF-56's kills on a product install: one call to a member's operator-only route (`/abort`, or an Actions member's
// `/owner_fault`) through a binding to the site scripts' `Member` classes. The edge forwards neither route.
//   POST /?site=S&name=<do_name>&path=/abort   with header x-t: <TOKEN>, body passed through
export default {
  async fetch(req, env) {
    if (req.headers.get("x-t") !== env.TOKEN) return new Response("no", { status: 403 });
    const u = new URL(req.url);
    const ns = env["MEMBER_" + u.searchParams.get("site")];
    const stub = ns.get(ns.idFromName(u.searchParams.get("name")));
    const at = Date.now();
    const r = await stub.fetch("https://member" + u.searchParams.get("path"), { method: "POST", body: await req.text() });
    return Response.json({ at, status: r.status, body: await r.text() });
  },
};
