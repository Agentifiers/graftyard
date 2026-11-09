// After recreate.mjs: on each recreated group, the lease path end to end through graft-m1-edge with the m3-ci App's
// installation token: an external task, an Exclusive lease under it (a `LeaseAcquire` entry, tag 35), the lease
// listed, released (`LeaseRelease`, tag 37), and the group's members restored from their own rows afterward
// (each member's `/status` with no restore error).
//   node tests/m3/wirefix/lease-check.mjs [name:id ...]   (default j4:1048576 lf1-2:1048579 lf1-3:1048580)
import { api, ghs, log, logFrom, statuses } from "../../am1/hosted/lib.mjs";

const groups = (process.argv.slice(2).length ? process.argv.slice(2) : ["j4:1048576", "lf1-2:1048579", "lf1-3:1048580"])
  .map((s) => s.split(":")).map(([name, id]) => ({ name, chain: `repo:${id}` }));
const app = (await ghs({})).body.token;
const tag = Date.now().toString(36);
for (const g of groups) {
  const V1 = `/api/graftyard/v1/repos/m3org/${g.name}`;
  const t = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `wirefix-${tag}`, summary: "wire-fix lease check" });
  const l = await api("POST", `${V1}/leases`, app, { paths: [`wirefix/${tag}/**`], mode: "exclusive", ttl_seconds: 600, intent: "wire-fix" },
    { "x-graftyard-agent-session": t.body?.id ?? "" });
  const id = l.body?.lease?.id;
  const list = await api("GET", `${V1}/leases?per_page=100&paths=wirefix/${tag}/**`, app);
  const rel = id ? await api("DELETE", `${V1}/leases/${id}`, app, undefined, { "x-graftyard-agent-session": t.body?.id ?? "" }) : { status: null };
  const kinds = (await logFrom(g.chain, 1, 50)).map((e) => (e.kinds ?? []).flat().join("+") || e.kind).filter(Boolean);
  const st = (await statuses(g.chain)).map((s) => `${s.site}:${s.role ?? "?"}/t${s.term}/c${s.commit}${s.restoreError ? "/" + s.restoreError : ""}`);
  log(`${g.name} task ${t.status} lease ${l.status} listed ${list.status}:${(list.body?.leases ?? list.body ?? []).length} release ${rel.status}; log ${kinds.join(",")}; ${st.join(" ")}`);
}
