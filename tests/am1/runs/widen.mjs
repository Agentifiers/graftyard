// B0-36's widened Graftyard-run installation: re-sends `graftyard-agent`'s App record and its m3org installation
// with every repository permission at write (`wide`), or with its shipped permissions again (`shipped`), as the
// operator, keeping the App's key and time (tests/am1/rest/apps.mjs `commands`).
//   node tests/am1/runs/widen.mjs <wide|shipped>
import { createPrivateKey, createPublicKey } from "node:crypto";
import { readFileSync } from "node:fs";
import { APPS, commands } from "../rest/apps.mjs";
import { log, must, state } from "../hosted/lib.mjs";
const mode = process.argv[2];
const slug = "graftyard-agent";
const a = APPS.find((x) => x.slug === slug);
const s = state.agentApps[slug];
const der = createPublicKey(createPrivateKey(readFileSync(`${process.env.M3_STATE}/agent-apps/${slug}.pem`))).export({ type: "spki", format: "der" });
const WIDE = { actions: "Write", administration: "Write", checks: "Write", contents: "Write", issues: "Write", metadata: "Read",
  pull_requests: "Write", statuses: "Write", workflows: "Write" };
const perms = mode === "wide" ? WIDE : a.permissions;
for (const c of commands({ ...a, permissions: perms }, s.app, der, state.org, s.installation, s.created)) {
  const op = Object.keys(c.cmd.op)[0];
  if (op === "PutApp" || op === "Install") await must(c.chain, c.cmd);
}
log(`${slug}: ${mode} (${Object.keys(perms).join(",")})`);
