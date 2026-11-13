// Captures graft-m4a's hosted M4-WP2 replies for `crates/api/gh/validate.py`: each `<case>.rest.json` holds the
// method, the OpenAPI path, the status, and the body the product edge answered.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp2/hosted/replies.mjs /tmp/hosted-replies
//   /tmp/ghes/.venv/bin/python crates/api/gh/validate.py /tmp/ghes/ghes-3.22.json /tmp/ghes/schema.docs-enterprise.graphql /tmp/hosted-replies
import { mkdirSync, writeFileSync } from "node:fs";
import { api, log, tokens } from "../../../am1/hosted/lib.mjs";

const dir = process.argv[2];
mkdirSync(dir, { recursive: true });
const admin = tokens.admin, watcher = tokens.member;
const STAR = { accept: "application/vnd.github.star+json" };
const threads = (await api("GET", "/api/v3/notifications?all=true", watcher)).body;
const t = threads[0].id;
const cases = [
  ["stargazers-star", "get", "/repos/{owner}/{repo}/stargazers", "/api/v3/repos/m3org/wp2-stars/stargazers?per_page=5", admin, STAR],
  ["stargazers-user", "get", "/repos/{owner}/{repo}/stargazers", "/api/v3/repos/m3org/wp2-stars/stargazers?per_page=5", admin],
  ["subscribers", "get", "/repos/{owner}/{repo}/subscribers", "/api/v3/repos/m3org/wp2-bf5/subscribers", admin],
  ["subscription", "get", "/repos/{owner}/{repo}/subscription", "/api/v3/repos/m3org/wp2-bf5/subscription", watcher],
  ["starred-check", "get", "/user/starred/{owner}/{repo}", "/api/v3/user/starred/m3org/wp2-bf5", admin],
  ["notifications", "get", "/notifications", "/api/v3/notifications?all=true", watcher],
  ["thread", "get", "/notifications/threads/{thread_id}", `/api/v3/notifications/threads/${t}`, watcher],
  ["thread-subscription", "get", "/notifications/threads/{thread_id}/subscription", `/api/v3/notifications/threads/${t}/subscription`, watcher],
  ["search", "get", "/search/issues", "/api/v3/search/issues?q=kumquat", admin],
  ["search-422", "get", "/search/issues", "/api/v3/search/issues?q=kumquat+label:bug", admin],
  ["repo", "get", "/repos/{owner}/{repo}", "/api/v3/repos/m3org/wp2-stars", admin],
];
for (const [name, method, path, url, tok, extra] of cases) {
  const r = await api(method.toUpperCase(), url, tok, undefined, extra ?? {});
  writeFileSync(`${dir}/hosted-${name}.rest.json`, JSON.stringify({ method, path, status: r.status, body: r.body === "" ? null : r.body }));
  log(`${name}: ${r.status} ${r.ms} ms`);
}
