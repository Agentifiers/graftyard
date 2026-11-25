// LF-8's forger (`build/BUILD-PLAN.md`, AM3-WP1, LF-8): a Worker that answers `GET /api/graftyard/v1/release/entries`
// in the edge's page shape (`workers/edge/src/release.rs`, `read`: a JSON array of {index, entry, at}) from a synthetic
// history and one forged page that `POST /_lf8/select` chooses. It runs in the install account as `graft-lf8-feed`,
// because a Worker's fetch of another workers.dev Worker in its own account answers Cloudflare 1042
// (`tests/am3/raw/lf8/reach.log`), and `graft-agents-front` runs unmodified in the runner account.
//
// The state is one Durable Object (`Feed`, name "lf8"), so the driver's selection and the front's reads meet whatever
// isolate each lands on. Under the `x-lf8-token` header:
//   POST /_lf8/load    {history: [...], pages: {name: [...]}}: stores them, selection none
//   POST /_lf8/select  {name, alone?}: `name` null serves the history alone; `alone` serves the page alone (the cold
//                      view's); the selection's read count starts at 0
//   GET  /_lf8/state   {loaded, selected, alone, reads}
// A GET of the entries answers the history after `after`, then every entry of the selected page whatever `after` is,
// at most per_page: the page carries its own indexes, which may repeat or skip the history's. That is the forgery.
// Authorization is not checked: the front's App JWT is signed by a throwaway key.
import { DurableObject } from "cloudflare:workers";

const json = (v, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

export class Feed extends DurableObject {
  async fetch(req) {
    const url = new URL(req.url);
    const s = this.ctx.storage;
    if (url.pathname === "/_lf8/load") {
      const b = await req.json();
      await s.deleteAll();
      await s.put("history", b.history);
      for (const [name, page] of Object.entries(b.pages)) await s.put(`page:${name}`, page);
      await s.put("sel", { name: null, alone: false, reads: 0 });
      return json({ loaded: true, history: b.history.length, pages: Object.keys(b.pages).length });
    }
    if (url.pathname === "/_lf8/select") {
      const b = await req.json();
      if (b.name !== null && !(await s.get(`page:${b.name}`))) return json({ error: `no page ${b.name}` }, 400);
      await s.put("sel", { name: b.name, alone: b.alone === true, reads: 0 });
      return json({ selected: b.name, alone: b.alone === true });
    }
    if (url.pathname === "/_lf8/state") {
      const sel = (await s.get("sel")) ?? {};
      return json({ loaded: !!(await s.get("history")), selected: sel.name ?? null, alone: !!sel.alone, reads: sel.reads ?? 0 });
    }
    const history = await s.get("history");
    if (!history) return json({ message: "lf8 feed not loaded" }, 503);
    const sel = await s.get("sel");
    const after = Number(url.searchParams.get("after") ?? 0);
    const per = Math.min(Math.max(Number(url.searchParams.get("per_page") ?? 30), 1), 100);
    const forged = sel.name === null ? [] : await s.get(`page:${sel.name}`);
    await s.put("sel", { ...sel, reads: sel.reads + 1 });
    const rows = (sel.alone ? [] : history.filter((r) => r.index > after)).concat(forged);
    return json(rows.slice(0, per).map(({ index, entry, at }) => ({ index, entry, at })));
  }
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    if (url.pathname.startsWith("/_lf8/")) {
      if (req.headers.get("x-lf8-token") !== env.LF8_TOKEN) return new Response("Not Found", { status: 404 });
    } else if (url.pathname !== "/api/graftyard/v1/release/entries" || req.method !== "GET") {
      return new Response("Not Found", { status: 404 });
    }
    return env.FEED.get(env.FEED.idFromName("lf8")).fetch(req);
  },
};
