// Gives the m3-ci App and its m3org installation the permissions an agent App holds (contents, issues, pull requests
// and checks write; metadata read), as the operator, re-sending the App record setup.ts wrote with the same key.
//   node tests/am1/hosted/app-perms.mjs
import { createHash, createPrivateKey, createPublicKey } from "node:crypto";
import { readFileSync } from "node:fs";
import { OP, log, must, state } from "./lib.mjs";
const dir = process.env.M3_STATE ?? `${process.env.HOME}/.graft-m3h`;
const der = createPublicKey(createPrivateKey(readFileSync(`${dir}/app.pem`))).export({ type: "spki", format: "der" });
const secretHash = [...createHash("sha256").update(readFileSync(`${dir}/app.secret`, "utf8")).digest()];
const perms = { contents: "Write", metadata: "Read", issues: "Write", pull_requests: "Write", checks: "Write" };
const record = {
  id: state.app, slug: "m3-ci", name: "M3 CI", client: state.client, keys: [{ id: 1, public: [...der], created: 1794000000000 }],
  permissions: perms, events: [], webhook: null, webhook_secret: null, client_secrets: [secretHash], callbacks: [], public: true,
};
await must(`authz:${state.admin}`, { cred: OP, op: { PutApp: { record } } });
await must(`authz:${state.org}`, { cred: OP, op: { Install: { id: state.installation, row: { app: state.app, repos: "All", permissions: perms }, app_owner: state.admin } } });
log(`m3-ci ${state.app} and installation ${state.installation}: ${Object.keys(perms).join(",")}`);
