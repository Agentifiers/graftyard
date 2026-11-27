// LM-3 row A: M0-WP14's registry stand-in (tests/m0/src/wp14/registry.ts, which stays as it ran) serving a Go proxy at
// the root too, where the product router sends it (`graftyard_git::outbound::go_module`); M0's router used `/go/`.
// M0-WP14 (L0-02): `graft-wp14-registry`, the stand-in Go module proxy for the `Registry` arm. It is permissive on
// purpose: it answers 200 for any module path, as a proxy that fetches upstream on demand would, so the router's
// "unknown Go module paths are refused" rule is the only thing that stops an unknown path. Each arrival is counted as
// `known` or `unknown` (`GET /m0/hits`).

import { Hits, hitsRoute, record, type HitsEnv } from "../../../m0/src/wp14/counter.ts";

export { Hits };

const KNOWN = "github.com/graftyard/known";

export default {
  async fetch(req: Request, env: HitsEnv): Promise<Response> {
    const h = await hitsRoute(req, env);
    if (h) return h;
    const m = /^(?:\/go)?\/(.+?)\/(@latest|@v\/.*)$/.exec(new URL(req.url).pathname);
    const mod = m ? decodeURIComponent(m[1]).toLowerCase() : "";
    await record(env, `${req.method} ${mod === KNOWN ? "known" : "unknown"}`);
    if (!m) return new Response("not a module path", { status: 404 });
    if (m[2] === "@latest") return Response.json({ Version: "v1.0.0", Time: "2026-01-01T00:00:00Z" });
    if (m[2] === "@v/list") return new Response("v1.0.0\n");
    return new Response("module bytes\n");
  },
};
