// AM2-WP1's hosted half: commits the request files `graftyard apps init` writes, in file order, on a test install
// (environment as `tests/am1/hosted/lib.mjs`: GRAFT_EDGE, GRAFT_TOOLS_EDGE, BED_TOKEN_FILE, M3_STATE). No operator
// route exists yet (R14), so owner chains go through the edge's bed route and `Names` shards through the tools edge,
// as `tests/am1/rest/apps.mjs` sends them.
//
//   node tests/am2/hosted/apps.mjs claims <out>     each ClaimSlug in apps-claims.request.json; prints each slug's App
//                                                   id, from the answer or, when the slug is Taken, from its route
//   node tests/am2/hosted/apps.mjs register <out>   every command in apps-register.request.json, stopping at the first
//                                                   refusal; then reads back each App's route and record
// A third argument names the files' prefix: `agents` for the files `graftyard agents init` writes (AM3-WP1's LF-7).
//
// `register` over an App that `tests/am1/rest/apps.mjs` registered (graft-m3f's `graftyard-mcp`, App 327680) marks it
// `moved` in the state file before it sends anything, so `agentToken` and L0-12's `Graftyard MCP` kind stop using the
// old key and nobody reruns apps.mjs over the new record (apps.mjs refuses too, from the record's callback).
import { readFileSync, writeFileSync } from "node:fs";
import { bed, commit, log, refusal, state, tools } from "../../am1/hosted/lib.mjs";

const [step, out, prefix = "apps"] = process.argv.slice(2);
if (!["claims", "register"].includes(step) || !out) throw new Error("usage: apps.mjs claims|register <out> [prefix]");
const requests = (f) => JSON.parse(readFileSync(`${out}/${f}`, "utf8"));
const statePath = `${process.env.M3_STATE}/m3.json`;

async function send(r) {
  const o = await commit(r.chain, r.cmd);
  log(`${r.op} on ${r.chain}: ${refusal(o)}`);
  return o;
}

if (step === "claims") {
  for (const r of requests(`${prefix}-claims.request.json`)) {
    const slug = r.cmd.op.ClaimSlug.slug;
    const o = await send(r);
    let app = o?.ok?.out?.App;
    if (!app && refusal(o) === "Taken") app = (await tools("/read", { chain: r.chain, q: { slug } })).value?.slug?.app;
    if (!app) throw new Error(`${slug}: no App id (${refusal(o)})`);
    console.log(`${slug} ${app}`);
  }
} else {
  const reqs = requests(`${prefix}-register.request.json`);
  const put = reqs.find((r) => r.op === "PutApp (graftyard-mcp)")?.cmd.op.PutApp.record.id;
  const old = state.agentApps?.["graftyard-mcp"];
  if (put && old?.app === put && !old.moved) {
    old.moved = `taken over by \`graftyard apps init\` (AM2-WP1 hosted, ${new Date().toISOString()}): its key is in `
      + "Secrets Store, so agent-apps/graftyard-mcp.pem no longer verifies";
    writeFileSync(statePath, JSON.stringify(state, null, 2));
    log(`graftyard-mcp: App ${put} marked moved in ${statePath}`);
  }
  for (const r of reqs) {
    const o = await send(r);
    if (!o?.ok) throw new Error(`${r.op}: ${refusal(o)}; stopped, the rest not sent`);
  }
  // Read back: each App's record on the install account's chain (the `app` read ships with this release).
  for (const r of reqs.filter((x) => x.op.startsWith("PutApp"))) {
    const rec = r.cmd.op.PutApp.record;
    const v = (await bed({ chain: r.chain, read: { app: rec.id } })).value?.app;
    console.log(JSON.stringify({ app: rec.id, slug: v?.slug, callbacks: v?.callbacks, client_secrets: v?.client_secrets }));
  }
}
