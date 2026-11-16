// On one follower: a whole-tier read (rebuilds the dump), a tier change, and at once a second whole-tier read and an
// access read. The second dump read comes inside DUMP_EVERY_MS of the first rebuild at a newer state.
import { api, tokens, member, statuses } from "/home/user/workspace/repo/tests/am1/hosted/lib.mjs";
const R = "/api/v3/repos/m3org/wp4-bf54/actions/variables";
console.log("create", (await api("POST", R, tokens.admin, { name: "F77_PROBE", value: "0" })).status);
const st = await statuses("repo:65565");
const names = { 0: "repo:65565#1#f0", 1: "repo:65565#2#f1", 2: "repo:65565#1#f2" };
const leader = st.find((s) => s.role === "leader")?.site;
const f = [0, 1, 2].find((s) => s !== leader);
const dump = { q: { rows: true, ranges: true, packed: true }, index: true };
const access = { q: { rows: { access: true }, ranges: true }, index: true };
const show = (v) => v?.overloaded ? `overloaded ${JSON.stringify(v.overloaded)}` : v?.error?.notLeader ? "notLeader" : v?.value !== undefined ? "value" : JSON.stringify(v).slice(0, 80);
const rd = async (b, tag) => { const t = Date.now(); const r = await member("repo:65565", f, "/read", b, 15_000, names[f]); return `${tag} ${Date.now() - t}ms ${show(r.v)}`; };
const tally = {};
for (let i = 1; i <= 6; i++) {
  const a = await rd(dump, "dump1");
  const t = Date.now();
  const p = (await api("PATCH", `${R}/F77_PROBE`, tokens.admin, { name: "F77_PROBE", value: String(i) })).status;
  const pm = Date.now() - t;
  const [b, c] = await Promise.all([rd(dump, "dump2"), rd(access, "access")]);
  console.log(`follower ${f} try ${i}: ${a} | patch ${p} ${pm}ms | ${b} | ${c}`);
}
console.log("delete", (await api("DELETE", `${R}/F77_PROBE`, tokens.admin)).status);
console.log(JSON.stringify((await member("repo:65565", f, "/stats?summary=1", undefined, 15_000, names[f])).v, (k, v) => k === "" || /shed|waited|follower|dump/i.test(k) ? v : undefined));
