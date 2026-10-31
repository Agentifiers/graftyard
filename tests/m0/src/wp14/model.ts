// M0-WP14 (L0-02): `graft-wp14-model`, the stub model inference endpoint the `ModelInference` arm allows. No real model
// is called. It needs the model key, which only the router attaches, and is otherwise permissive on purpose: it answers
// 200 to `mcp_servers`, server-side tools, and file and batch endpoints alike, so the router is the only thing that
// refuses them. Each arrival is counted by kind (`GET /m0/hits`), the server-side check that no forbidden request came.
// A clean answer carries a `usage` block, so the harness can sum model tokens.

import { Hits, hitsRoute, record, type HitsEnv } from "./counter.ts";
import { modelBodyRefusal } from "./router.ts";

export { Hits };

interface Env extends HitsEnv { MODEL_KEY: string }

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const h = await hitsRoute(req, env);
    if (h) return h;
    const url = new URL(req.url);
    const keyed = req.headers.get("authorization") === `Bearer ${env.MODEL_KEY}`;
    let kind = "clean";
    if (url.pathname !== "/v1/chat/completions") kind = "other-path";
    else {
      try { if (modelBodyRefusal(await req.json())) kind = "forbidden-body"; } catch { kind = "bad-json"; }
    }
    await record(env, `${keyed ? "keyed" : "unkeyed"} ${kind}`);
    if (!keyed) return Response.json({ error: "missing model key" }, { status: 401 });
    return Response.json({
      id: "wp14-stub", object: "chat.completion", model: "wp14-stub",
      choices: [{ index: 0, message: { role: "assistant", content: "ok" }, finish_reason: "stop" }],
      usage: { prompt_tokens: 12, completion_tokens: 3, total_tokens: 15 },
    });
  },
};
