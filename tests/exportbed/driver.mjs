// exportbed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with a driver route in front.
// `/drive` joins one test-register group (`repo:<n>` with no owner, which `Kind::of` hosts as the register and
// `Source::Given` trusts) on the three sites' `Member` bindings, campaigns until a member leads, and commits one put.
// Every other request, the cron and the queue go to the site script unchanged. The members' leader report and the
// feed's round are the product's: `Host::report_commit`, the registry object `export`, `export_feed::kick`.
import site, * as classes from "./out/index.js";

export const { Member, Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function rid() {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

async function drive(env, chain) {
  const record = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
  const stub = (s) => env[`MEMBER_${s}`].getByName(`${chain}#1#f${s}`);
  const call = async (s, path, body) => {
    const r = await stub(s).fetch(`https://member${path}`, body === undefined ? {} : { method: "POST", body: JSON.stringify(body) });
    const t = await r.text(); let v = t; try { v = JSON.parse(t); } catch {} return { status: r.status, body: v };
  };
  const joins = [];
  for (const s of record.sites) {
    const me = { site: s, kind: "full", slot: s, generation: 1, do_name: `${chain}#1#f${s}` };
    { const j = await call(s, "/join", { chain, me, record }); joins.push(j.status === 200 ? 200 : { status: j.status, body: j.body }); }
  }
  let role = null;
  for (let i = 0; i < 50 && role !== "leader"; i++) {
    await call(0, "/campaign", {});
    role = (await call(0, "/status")).body?.role ?? null;
    if (role !== "leader") await sleep(200);
  }
  const put = await call(0, "/commit", { rid: rid(), cmd: { kind: "put", key: "k", value: "v" } });
  const status = (await call(0, "/status")).body;
  return { joins, role, put: put.status, commit: status?.commit ?? null };
}

// The site's default export is a `WorkerEntrypoint` class; this one adds the driver's routes and leaves its `scheduled`
// and `queue` as they are.
export default class extends site {
  async fetch(req) {
    const env = this.env;
    const url = new URL(req.url);
    if (url.pathname === "/drive") return Response.json(await drive(env, url.searchParams.get("chain") ?? "repo:900001"));
    if (url.pathname === "/objects") {
      const l = await env.EXPORT.list({ prefix: url.searchParams.get("prefix") ?? "" });
      const out = [];
      for (const o of l.objects) out.push({ key: o.key, size: o.size });
      return Response.json(out);
    }
    return super.fetch(req);
  }
}
