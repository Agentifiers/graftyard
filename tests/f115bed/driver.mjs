// f115bed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with driver routes in front, for
// F115's storage-side question (`decisions/OPEN-QUESTIONS.md`): does a leader's per-entry SQLite cost grow with the
// stored log? One test-register group (`repo:<n>`, which `Kind::of` hosts as the register) on the three sites'
// `Member` bindings, its log grown through the real commit path, one put an entry. Every route answers counts only;
// `run.sh` times each from outside, because a Worker's clock advances only on I/O.
//   /setup?chain=          join the three members and elect site 0
//   /grow?chain=&from=&n=&bytes=&par=  n `/commit` puts of a `bytes`-character value, `par` at a time (1: each its own
//                          entry), to 64 keys, with request ids rid(from) .. rid(from + n - 1)
//   /entries?chain=&k=&lo=&hi=    k leader `/entries?from=i&to=i&local=1` reads, i spread over lo..=hi
//   /request?chain=&k=&lo=&hi=    k leader `/outcome_by_request` lookups of rid(i), i spread over lo..=hi: the
//                          `request` row read by its primary key
//   /status?chain=&k=      k leader `/status` calls: the per-call cost every route above shares
import site, * as classes from "./out/index.js";

export const { Member, Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
// A request id for put number i, the same on every route: a fixed high part, so it parses as a ULID.
function rid(i) {
  let v = (1n << 100n) | BigInt(i);
  let out = "";
  for (let k = 25; k >= 0; k--) out += A[Number((v >> BigInt(5 * k)) & 31n)];
  return out;
}

function member(env, chain) {
  return async (s, path, body) => {
    const r = await env[`MEMBER_${s}`].getByName(`${chain}#1#f${s}`).fetch(`https://member${path}`,
      body === undefined ? {} : { method: "POST", body: JSON.stringify(body) });
    const t = await r.text(); let v = t; try { v = JSON.parse(t); } catch {}
    return { status: r.status, body: v };
  };
}

// i for read j of k, spread evenly over lo..=hi, in a stride that does not walk the rows in order.
const spread = (j, k, lo, hi) => lo + ((j * 7919) % k) * Math.max(1, Math.floor((hi - lo + 1) / k));

export default class extends site {
  async fetch(req) {
    const url = new URL(req.url);
    const q = (k, d) => url.searchParams.get(k) ?? d;
    const chain = q("chain", "repo:900401");
    const call = member(this.env, chain);
    const int = (k, d) => Number(q(k, d));
    switch (url.pathname) {
      case "/setup": {
        const record = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
        for (const s of [0, 1, 2]) {
          const me = { site: s, kind: "full", slot: s, generation: 1, do_name: `${chain}#1#f${s}` };
          const j = await call(s, "/join", { chain, me, record });
          if (j.status !== 200) return Response.json({ join: s, status: j.status, body: j.body });
        }
        let role = null;
        for (let i = 0; i < 50 && role !== "leader"; i++) {
          await call(0, "/campaign", {});
          role = (await call(0, "/status")).body?.role ?? null;
          if (role !== "leader") await sleep(200);
        }
        return Response.json({ role });
      }
      case "/grow": {
        const [from, n, bytes, par] = [int("from", 0), int("n", 100), int("bytes", 4096), int("par", 1)];
        const value = "v".repeat(bytes);
        let ok = 0, bad = null, next = from;
        // `par` puts in flight at once, so the leader cuts entries back to back; at 1, one put an entry.
        const one = async () => {
          for (let i = next++; i < from + n; i = next++) {
            const r = await call(0, "/commit", { rid: rid(i), cmd: { kind: "put", key: `k${i % 64}`, value } });
            if (r.status === 200 && r.body?.done?.ok !== undefined) ok++;
            else bad ??= { i, status: r.status, body: JSON.stringify(r.body).slice(0, 300) };
          }
        };
        await Promise.all(Array.from({ length: par }, one));
        const st = (await call(0, "/status")).body;
        return Response.json({ ok, bad, role: st?.role ?? null, commit: st?.commit ?? null });
      }
      case "/entries": {
        const [k, lo, hi] = [int("k", 200), int("lo", 1), int("hi", 1)];
        let ok = 0, bytes = 0;
        for (let j = 0; j < k; j++) {
          const i = spread(j, k, lo, hi);
          const r = await call(0, `/entries?from=${i}&to=${i}&local=1`);
          const e = r.body?.entries;
          if (Array.isArray(e) && e.length === 1 && e[0].index === i) { ok++; bytes += JSON.stringify(e).length; }
        }
        return Response.json({ ok, bytes });
      }
      case "/request": {
        const [k, lo, hi] = [int("k", 200), int("lo", 0), int("hi", 0)];
        let ok = 0;
        for (let j = 0; j < k; j++) {
          const i = spread(j, k, lo, hi);
          const r = await call(0, "/outcome_by_request", { rid: rid(i), cmd: { kind: "put", key: "k", value: "" } });
          if (r.body?.done !== undefined) ok++;
        }
        return Response.json({ ok });
      }
      case "/status": {
        const k = int("k", 200);
        let ok = 0;
        for (let j = 0; j < k; j++) if ((await call(0, "/status")).status === 200) ok++;
        return Response.json({ ok });
      }
    }
    return super.fetch(req);
  }
}
