// Prints a fresh `ghs_` for the setup's installation, from `@octokit/app` (stdout goes to a 0600 file).
//   node tests/m3/hosted/ghs.mjs <state dir>
import { readFileSync } from "node:fs";
import { App } from "@octokit/app";
import { Octokit } from "@octokit/core";
const [dir] = process.argv.slice(2);
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const s = JSON.parse(readFileSync(`${dir}/m3.json`, "utf8"));
const app = new App({ appId: s.app, privateKey: readFileSync(`${dir}/app.pem`, "utf8"), Octokit: Octokit.defaults({ baseUrl: `${EDGE}/api/v3` }) });
const r = await app.octokit.request("POST /app/installations/{installation_id}/access_tokens", { installation_id: s.installation });
process.stdout.write(r.data.token);
