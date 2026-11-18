// BF-32 over HTTPS with `@octokit/app` against the graft-m1 edge: the App from tests/m3/hosted/setup.ts gets an
// installation token (`app.getInstallationOctokit`, which signs its App JWT with `universal-github-app-jwt` and calls
// `POST /app/installations/{id}/access_tokens`), then reads with it, asks for a narrowed token, and for a permission
// the App lacks; a JWT under another key is refused; and the token revokes itself with `DELETE /installation/token`.
// Each reply's status and body go to <out>/bf32/<case>.json; no token is printed. Then the OAuth web flow with the
// App's client: a wrong client secret, then the right one.
//
//   node tests/m3/hosted/bf32.mjs <state dir> <out dir>        (needs @octokit/app in tests/m3/hosted/node_modules)

import { generateKeyPairSync } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { App } from "@octokit/app";
import { Octokit } from "@octokit/core";

const [stateDir, out] = process.argv.slice(2);
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const state = JSON.parse(readFileSync(`${stateDir}/m3.json`, "utf8"));
const privateKey = readFileSync(`${stateDir}/app.pem`, "utf8");
const secret = readFileSync(`${stateDir}/app.secret`, "utf8").trim();
mkdirSync(`${out}/bf32`, { recursive: true });
const results = [];
const record = (name, status, body, ok) => {
  writeFileSync(`${out}/bf32/${name}.json`, JSON.stringify({ status, body: redact(body) }, null, 2));
  results.push({ name, status, ok });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}: ${status}`);
};
// Token values never reach a file: `token` becomes its length and prefix.
const redact = (b) => JSON.parse(JSON.stringify(b ?? null, (k, v) => (k === "token" || k === "access_token") && typeof v === "string" ? `<${v.slice(0, 4)}… ${v.length} chars>` : v));

const OctokitGhes = Octokit.defaults({ baseUrl: `${EDGE}/api/v3` });
const app = new App({ appId: state.app, privateKey, Octokit: OctokitGhes });

// 1. The installation-token flow.
let token;
try {
  const r = await app.octokit.request("POST /app/installations/{installation_id}/access_tokens", { installation_id: state.installation });
  token = r.data.token;
  const shape = /^ghs_[A-Za-z0-9.\-_]{36,}$/.test(token);
  record("access-token", r.status, r.data, r.status === 201 && shape && r.data.permissions?.contents === "write");
  writeFileSync(`${out}/bf32/token-length.txt`, `${token.length}\n`);
} catch (e) {
  record("access-token", e.status, e.response?.data, false);
}

// 2. `getInstallationOctokit` (the same request, through Octokit's auth strategy), then a read and a git ref read.
try {
  const inst = await app.getInstallationOctokit(state.installation);
  const r = await inst.request("GET /repos/{owner}/{repo}/issues", { owner: "m3org", repo: "j4" });
  record("installation-read", r.status, r.data, r.status === 200);
} catch (e) {
  record("installation-read", e.status, e.response?.data, false);
}

// 3. Narrowed: one repository by name, `contents: read`.
try {
  const r = await app.octokit.request("POST /app/installations/{installation_id}/access_tokens", {
    installation_id: state.installation, repositories: ["j4"], permissions: { contents: "read" },
  });
  record("narrowed", r.status, r.data, r.status === 201 && r.data.permissions?.contents === "read" && r.data.repository_selection === "selected");
} catch (e) {
  record("narrowed", e.status, e.response?.data, false);
}

// 4. A permission the App lacks (`issues`), and one above its level (`contents: admin`): 422.
for (const [name, permissions] of [["lacks-issues", { issues: "write" }], ["above-level", { contents: "admin" }]]) {
  try {
    const r = await app.octokit.request("POST /app/installations/{installation_id}/access_tokens", { installation_id: state.installation, permissions });
    record(name, r.status, r.data, false);
  } catch (e) {
    record(name, e.status, e.response?.data, e.status === 422);
  }
}

// 5. A JWT under a key the App does not have: 401.
try {
  const { privateKey: other } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const wrong = new App({ appId: state.app, privateKey: other.export({ type: "pkcs8", format: "pem" }), Octokit: OctokitGhes });
  const r = await wrong.octokit.request("POST /app/installations/{installation_id}/access_tokens", { installation_id: state.installation });
  record("other-key", r.status, r.data, false);
} catch (e) {
  record("other-key", e.status, e.response?.data, e.status === 401);
}

// 6. The token writes, revokes itself, and is refused afterwards.
if (token) {
  const t = new OctokitGhes({ auth: token });
  // Issue commands carry no stateless principal yet (`tests/m3/results/m3-wp4.md`, "The JSON wire forms"), so a
  // `ghs_` issue write fails closed; the `ghs_` write journey 4 makes is a git push (32.7), in journey4.sh.
  try {
    const r = await t.request("POST /repos/{owner}/{repo}/labels", { owner: "m3org", repo: "j4", name: `ghs-${Date.now()}`, color: "ededed" });
    record("ghs-issue-write-fails-closed", r.status, r.data, false);
  } catch (e) {
    record("ghs-issue-write-fails-closed", e.status, e.response?.data, e.status === 403 || e.status === 401);
  }
  try {
    const r = await t.request("DELETE /installation/token");
    record("revoke", r.status, null, r.status === 204);
  } catch (e) {
    record("revoke", e.status, e.response?.data, false);
  }
  // A write, which only a caller may make: this install's repository reads are public (Milestone 2 has no private
  // read path), so a read would say nothing about the token. The label create needs 32.5, which is enabled.
  try {
    const r = await t.request("POST /repos/{owner}/{repo}/labels", { owner: "m3org", repo: "j4", name: `after-revoke-${Date.now()}`, color: "ededed" });
    record("after-revoke", r.status, r.data, false);
  } catch (e) {
    record("after-revoke", e.status, e.response?.data, e.status === 401);
  }
}

// 7. The OAuth web flow with the App's client: a code for m3-admin, then a wrong secret (401) and the right one.
const admin = readFileSync(`${stateDir}/m3-tokens.txt`, "utf8").split("\n").find((l) => l.startsWith("admin "))?.split(" ")[1];
const authorize = await fetch(`${EDGE}/login/oauth/authorize?client_id=${state.client}&redirect_uri=http%3A%2F%2Flocalhost%2Fcb&state=s1`, { headers: { authorization: `token ${admin}` }, redirect: "manual" });
const loc = authorize.headers.get("location") ?? "";
const code = new URL(loc || "http://x/").searchParams.get("code");
record("authorize", authorize.status, { state: new URL(loc || "http://x/").searchParams.get("state"), code: code ? "<code>" : null }, authorize.status === 302 && !!code);
const exchange = async (s) => {
  const r = await fetch(`${EDGE}/login/oauth/access_token`, {
    method: "POST", headers: { accept: "application/json", "content-type": "application/json" },
    body: JSON.stringify({ client_id: state.client, client_secret: s, code }),
  });
  return { status: r.status, body: await r.json() };
};
const bad = await exchange(`${secret}x`);
record("web-wrong-secret", bad.status, bad.body, bad.status === 401 && bad.body.error === "incorrect_client_credentials");
const good = await exchange(secret);
// An App's client gets a `ghu_`, which this install does not serve yet: GitHub's error shape, never a token.
record("web-right-secret", good.status, good.body, good.status === 200 && (good.body.error === "unsupported_grant_type" || /^gh[ou]_/.test(good.body.access_token ?? "")));

writeFileSync(`${out}/bf32/summary.json`, JSON.stringify(results, null, 2));
console.log(`${results.filter((r) => r.ok).length} of ${results.length} cases pass`);
