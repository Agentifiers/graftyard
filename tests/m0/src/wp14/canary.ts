// M0-WP14 (L0-02): `graft-wp14-canary`, the unlisted host. It is on no `Allowed` arm, so the router must refuse every
// request to it. It answers 200 with a marker body to any request that does arrive, and counts each arrival, so a
// router leak shows up both in the sandbox (the marker) and here (the counter). It is ours, in our account.

import { Hits, hitsRoute, record, type HitsEnv } from "./counter.ts";

export { Hits };

export const MARKER = "WP14-CANARY-REACHED";

export default {
  async fetch(req: Request, env: HitsEnv): Promise<Response> {
    const h = await hitsRoute(req, env);
    if (h) return h;
    await record(env, `${req.method} ${new URL(req.url).pathname}`);
    return new Response(MARKER, { status: 200 });
  },
};
