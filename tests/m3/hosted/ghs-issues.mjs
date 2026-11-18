// M3 close, item 1 over HTTPS with `@octokit/app`: a `ghs_` issue write is decided by the installation's
// permissions. `m3-triage` (issues: write) creates a label on m3org/j4, 201; `m3-ci` (contents only) is refused 403
// "Resource not accessible by integration", GHES's status for an installation token that reaches the repository
// without the permission. The edge serves no label delete, so each label name is new. Each case `n` times. Replies go to <out>/ghs-issues/<case>-<i>.json
// with tokens redacted; nothing secret is printed.
//
//   node tests/m3/hosted/ghs-issues.mjs <state dir> <out dir> [n]

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { App } from "@octokit/app";
import { Octokit } from "@octokit/core";

const [dir, out, n = "10"] = process.argv.slice(2);
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const s = JSON.parse(readFileSync(`${dir}/m3.json`, "utf8"));
const privateKey = readFileSync(`${dir}/app.pem`, "utf8");
const OctokitGhes = Octokit.defaults({ baseUrl: `${EDGE}/api/v3` });
mkdirSync(`${out}/ghs-issues`, { recursive: true });
const tally = {};
const record = (name, i, status, body, ok) => {
  writeFileSync(`${out}/ghs-issues/${name}-${i}.json`, JSON.stringify({ status, body }, null, 2));
  tally[name] ??= { pass: 0, fail: 0, statuses: {} };
  tally[name][ok ? "pass" : "fail"]++;
  tally[name].statuses[status] = (tally[name].statuses[status] ?? 0) + 1;
};
const tokenFor = async (appId, installation) => {
  const app = new App({ appId, privateKey, Octokit: OctokitGhes });
  const r = await app.octokit.request("POST /app/installations/{installation_id}/access_tokens", { installation_id: installation });
  return new OctokitGhes({ auth: r.data.token });
};
const call = async (o, route, params) => {
  try {
    const r = await o.request(route, params);
    return { status: r.status, data: r.data };
  } catch (e) {
    return { status: e.status, data: e.response?.data };
  }
};
for (let i = 0; i < Number(n); i++) {
  const triage = await tokenFor(s.triageApp, s.triageInstallation);
  const ci = await tokenFor(s.app, s.installation);
  const name = `ghs-close-${Date.now()}-${i}`;
  const made = await call(triage, "POST /repos/{owner}/{repo}/labels", { owner: "m3org", repo: "j4", name, color: "ededed" });
  record("issues-write-creates", i, made.status, made.data, made.status === 201 && made.data?.name === name);
  const refused = await call(ci, "POST /repos/{owner}/{repo}/labels", { owner: "m3org", repo: "j4", name: `${name}-ci`, color: "ededed" });
  record("contents-only-refused", i, refused.status, refused.data,
    refused.status === 403 && refused.data?.message === "Resource not accessible by integration");
}
writeFileSync(`${out}/ghs-issues/summary.json`, JSON.stringify(tally, null, 2));
console.log(JSON.stringify(tally));
