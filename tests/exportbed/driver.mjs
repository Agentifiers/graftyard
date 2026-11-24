// exportbed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with a driver route in front.
// `/drive` joins one test-register group (`repo:<n>` with no owner, which `Kind::of` hosts as the register and
// `Source::Given` trusts) on the three sites' `Member` bindings, campaigns until a member leads, and commits one put.
// `/abort` resets a group's members, `/roles` and `/batches` read a group's members and the feed's objects, and `/hold`
// sets the test-faults feed hold. Every other request, the cron and the queue go
// to the site script unchanged. The members' leader report and the
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
    // Resets every member of a chain (`/abort`, BF-56's kill): each restores as a follower from storage, and with no
    // caller its group never elects again, so no leader of it reports.
    if (url.pathname === "/abort") {
      const chain = url.searchParams.get("chain");
      const out = [];
      for (const s of [0, 1, 2]) {
        try {
          const r = await env[`MEMBER_${s}`].getByName(`${chain}#1#f${s}`).fetch("https://member/abort", { method: "POST" });
          out.push(r.status);
        } catch (e) {
          out.push(String(e));
        }
      }
      return Response.json(out);
    }
    // A chain's members' roles and commit indexes, as `/status` answers them.
    if (url.pathname === "/roles") {
      const chain = url.searchParams.get("chain");
      const out = [];
      for (const s of [0, 1, 2]) {
        const r = await env[`MEMBER_${s}`].getByName(`${chain}#1#f${s}`).fetch("https://member/status");
        const v = await r.json();
        out.push({ site: s, role: v.role ?? null, commit: v.commit ?? null });
      }
      return Response.json(out);
    }
    // The chains each `export/` object's batch names: its first frame is the batch's JSON (`export::encode_object`).
    if (url.pathname === "/batches") {
      const l = await env.EXPORT.list({ prefix: "export/" });
      const out = [];
      for (const o of l.objects) {
        const b = new Uint8Array(await (await env.EXPORT.get(o.key)).arrayBuffer());
        const n = new DataView(b.buffer).getUint32(0);
        const head = JSON.parse(new TextDecoder().decode(b.subarray(4, 4 + n)));
        out.push({ key: o.key, chains: head.batch.chains, entries: head.entries });
      }
      return Response.json(out);
    }
    // The feed hold (test-faults `/export_hold` on the feed site's registry object `export`): `?hold=1|0`.
    if (url.pathname === "/hold") {
      const r = await env.REGISTRY_0.getByName("export").fetch(`https://registry/export_hold${url.search}`, { method: "POST" });
      return new Response(await r.text(), { status: r.status });
    }
    if (url.pathname === "/objects") {
      const l = await env.EXPORT.list({ prefix: url.searchParams.get("prefix") ?? "" });
      const out = [];
      for (const o of l.objects) out.push({ key: o.key, size: o.size });
      return Response.json(out);
    }
    return super.fetch(req);
  }
}
