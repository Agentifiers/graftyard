// refusalshape's site entry (`run.sh`): coldwrite's site entry (`tests/coldwrite/site.mjs`, copied in as cw-site.mjs)
// with two stand-ins for B0-33 erDA under load. Its members shed a write's row read, whole tiers (`{"rows": true}`,
// the edge's `tier_rows`) or a command's rows (`{"rows": {"command": ..}}`, `command_tier_rows`), with a member's own
// `overloaded` answer (`graftyard_lookup::overloaded_reply`), as erDA's learners shed them; every other member call,
// the read gate's access rows among them, is the site's own. Its git
// cache shard answers every call 503 `{"warming": true}`, as erDA's home shard did while it caught up.
import site, * as cw from "./cw-site.mjs";

export const { Registry, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = cw;

export class Member extends cw.Member {
  async fetch(req) {
    if (new URL(req.url).pathname === "/read") {
      const rows = JSON.parse((await req.clone().text()) || "{}").q?.rows;
      if (rows === true || rows?.command) return Response.json({ busy: true, overloaded: { retryAfterMs: 250 } });
    }
    return super.fetch(req);
  }
}

export class GitShard extends cw.GitShard {
  async fetch() {
    return Response.json({ warming: true }, { status: 503 });
  }
}

export default site;
