// BM-2's home-site clause needs a client in the home region (enam). This test-only Worker holds one Durable Object,
// created with `locationHint: "enam"`, that sends gh's own `PullRequestMerge` mutation to graft-m1-edge and times it
// from inside that region. The driver (`tests/m2/bm2home/home.py`) does the rest of each sample from the orb.
//
// POST /merge  body {"token", "node"}      -> {ms, status, body, timing, trace, shardTiming, edgeColo, clientColo}
// GET  /where                              -> {clientColo, edgeColo, ms}: the DO's own colo (cdn-cgi/trace) and the
//                                             colo graft-m1-edge ran in for a request from the DO.
// Every request needs `authorization: Bearer <TOKEN secret>`. The edge token rides in the body, never a URL.

interface Env { CLIENT: DurableObjectNamespace; TOKEN: string; EDGE: string }

const MUTATION =
  "mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input: $input){clientMutationId}}";

async function colo(url: string): Promise<string> {
  const t = await (await fetch(url)).text();
  return /^colo=(.*)$/m.exec(t)?.[1] ?? "";
}

export class Client {
  constructor(private state: DurableObjectState, private env: Env) {}

  async fetch(req: Request): Promise<Response> {
    const path = new URL(req.url).pathname;
    const clientColo = await colo("https://www.cloudflare.com/cdn-cgi/trace");
    if (path === "/where") {
      const t0 = Date.now();
      const r = await fetch(`${this.env.EDGE}/api/graphql`, {
        method: "POST",
        headers: { "content-type": "application/json", "user-agent": "graftyard-bm2home" },
        body: JSON.stringify({ query: "{__typename}" }),
      });
      await r.arrayBuffer();
      return Response.json({ clientColo, edgeColo: r.headers.get("x-graft-colo"), status: r.status, ms: Date.now() - t0 });
    }
    if (path === "/merge") {
      const { token, node } = (await req.json()) as { token: string; node: string };
      const t0 = Date.now();
      const r = await fetch(`${this.env.EDGE}/api/graphql`, {
        method: "POST",
        headers: { "content-type": "application/json", "user-agent": "graftyard-bm2home", authorization: `bearer ${token}` },
        body: JSON.stringify({ query: MUTATION, variables: { input: { pullRequestId: node, mergeMethod: "MERGE" } } }),
      });
      const text = await r.text();
      const ms = Date.now() - t0;
      let body: unknown = text;
      try { body = JSON.parse(text); } catch { /* keep the text */ }
      return Response.json({
        ms, status: r.status, body, clientColo,
        timing: r.headers.get("server-timing"), trace: r.headers.get("x-graft-trace"),
        shardTiming: r.headers.get("x-graft-shard-timing"), edgeColo: r.headers.get("x-graft-colo"),
      });
    }
    return new Response("not found", { status: 404 });
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const stub = env.CLIENT.get(env.CLIENT.idFromName("enam"), { locationHint: "enam" });
    return stub.fetch(req);
  },
};
