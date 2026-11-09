// Step 3 of am1-runs: m3-ci and apps.mjs's three Apps each mint an installation token (201) through graft-m2b-ops,
// and a `ghs_` from each pushes a new branch to m3org/<repo> (m3-ci's under `refs/heads/claude/` after it is
// flagged `third_party` with that `confine`). Prints statuses and token lengths only.
//   node tests/am1/runs/mintcheck.mjs <repo>
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, ghs, log, sleep, state, tokens } from "../hosted/lib.mjs";
import { agentToken } from "../rest/apps.mjs";
const repo = process.argv[2] ?? "lf13-r";
const flag = await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, tokens.admin, { kind: "third_party", confine: "refs/heads/claude/**" });
log(`flag m3-ci third_party confine claude/**: ${flag.status}`);
await sleep(2000);
const kinds = [["m3-ci", () => ghs({}), "claude/mint-ci"], ...["graftyard-workspace", "graftyard-agent", "graftyard-mcp"].map((s) => [s, () => agentToken(s), `mint-${s}`])];
const g = new Git(`m3org/${repo}`).as(tokens.admin);
const c = g.clone();
log(`admin clone: ${c.ok}`);
for (const [k, mint, branch] of kinds) {
  const r = await mint();
  const t = r.body?.token;
  const shape = t ? `${t.slice(0, 4) === "ghs_" ? "ghs_" : "other"} ${t.length} chars` : "-";
  let push = "-";
  if (t) {
    g.as(t);
    mkdirSync(join(g.wt, "mint"), { recursive: true });
    writeFileSync(join(g.wt, "mint", `${k}.txt`), `${k} ${Date.now()}\n`);
    g.run(["add", "-A"]); g.run(["commit", "-q", "-m", `mint ${k}`]);
    const p = g.run(["push", "-q", "origin", `HEAD:refs/heads/${branch}-${Date.now().toString(36)}`]);
    push = p.ok ? "accepted" : (p.out.match(/\[remote rejected\][^\n]*|error: [^\n]*/)?.[0] ?? "rejected").slice(0, 100);
  }
  log(`${k}: mint ${r.status} (${shape}, permissions ${JSON.stringify(r.body?.permissions ?? {})}); push ${push}`);
}
