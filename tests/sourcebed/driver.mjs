// sourcebed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with one driver route in
// front. `POST /call {site, name, path, body}` is one call to a member on that site's `Member` binding, answered as
// `{status, body}`; every other request goes to the site script unchanged, and the members' answers are the product's.
import site, * as classes from "./out/index.js";

export const { Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Local workerd's sites are under a millisecond apart, so `Placement::new` refuses every pair under MIN_PAIR_RTT. A
// `/ping`, which a member's round-trip measure times, waits PING_MS first, as a region's round trip would.
export class Member extends classes.Member {
  async fetch(req) {
    if (new URL(req.url).pathname === "/ping") await sleep(Number(this.env?.PING_MS ?? 40));
    return super.fetch(req);
  }
}

export default class extends site {
  async fetch(req) {
    if (new URL(req.url).pathname !== "/call") return super.fetch(req);
    const { site: s, name, path, body } = await req.json();
    const r = await this.env[`MEMBER_${s}`].getByName(name).fetch(`https://member${path}`,
      body === undefined || body === null ? {} : { method: "POST", body: JSON.stringify(body) });
    const t = await r.text();
    let v = t;
    try { v = JSON.parse(t); } catch {}
    return Response.json({ status: r.status, body: v });
  }
}
