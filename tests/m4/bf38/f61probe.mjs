// F61's hosted probe (decisions/OPEN-QUESTIONS.md, F61): on a repository seed.mjs made, pushes an annotated tag and
// labels the pull request, then reads the web app's eight chrome routes as m3-admin and saves each reply as a
// `crates/api/gh/validate.py` REST case.
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/bf38/f61probe.mjs <facts.json> <out dir>
import { mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { api, tokens, Git } from "../../am1/hosted/lib.mjs";

const [factsPath, out] = process.argv.slice(2);
const F = JSON.parse(readFileSync(factsPath, "utf8").split("\n").find((l) => l.startsWith("{")));
const R = `${F.owner}/${F.repo}`, admin = tokens.admin;
mkdirSync(out, { recursive: true });

// Data the routes would otherwise answer empty: an annotated tag (so `/tags` peels), a label on the PR. The assignee and
// reaction writes are not served (404 on graft-m4a, 2026-11-22), so those lists stay empty.
const g = new Git(R).as(admin);
g.clone();
console.log("tag", JSON.stringify(g.run(["-c", "user.name=m3-admin", "-c", "user.email=a@example.com", "tag", "-a", "v0.1.0", "-m", "f61", "origin/main"])).slice(0, 200));
const pushed = g.run(["push", "origin", "v0.1.0"]);
console.log("tag push", pushed.ok, (pushed.out ?? "").slice(0, 200));
const step = async (what, r) => console.log(what, r.status);
await step("label", await api("POST", `/api/v3/repos/${R}/labels`, admin, { name: "f61", color: "ededed" }));
await step("label pr", await api("POST", `/api/v3/repos/${R}/issues/${F.number}/labels`, admin, { labels: ["f61"] }));
await step("assign", await api("POST", `/api/v3/repos/${R}/issues/${F.number}/assignees`, admin, { assignees: ["m3-admin"] }));
await step("react pr", await api("POST", `/api/v3/repos/${R}/issues/${F.number}/reactions`, admin, { content: "+1" }));
const comments = await api("GET", `/api/v3/repos/${R}/issues/${F.number}/comments`, admin);
let cid = comments.body?.[0]?.id;
if (!cid) {
  const c = await api("POST", `/api/v3/repos/${R}/issues/${F.number}/comments`, admin, { body: "f61 probe" });
  cid = c.body.id;
}
await step("react comment", await api("POST", `/api/v3/repos/${R}/issues/comments/${cid}/reactions`, admin, { content: "heart" }));

const routes = [
  ["branches", `/repos/${R}/branches`, "/repos/{owner}/{repo}/branches"],
  ["tags", `/repos/${R}/tags`, "/repos/{owner}/{repo}/tags"],
  ["labels", `/repos/${R}/labels`, "/repos/{owner}/{repo}/labels"],
  ["assignees", `/repos/${R}/assignees`, "/repos/{owner}/{repo}/assignees"],
  ["teams", `/repos/${R}/teams`, "/repos/{owner}/{repo}/teams"],
  ["check-suites", `/repos/${R}/commits/${F.head}/check-suites`, "/repos/{owner}/{repo}/commits/{ref}/check-suites"],
  ["issue-reactions", `/repos/${R}/issues/${F.number}/reactions`, "/repos/{owner}/{repo}/issues/{issue_number}/reactions"],
  ["comment-reactions", `/repos/${R}/issues/comments/${cid}/reactions`, "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions"],
];
for (const [name, path, template] of routes) {
  const r = await api("GET", `/api/v3${path}`, admin);
  writeFileSync(`${out}/${name}.rest.json`, JSON.stringify({ method: "get", path: template, status: r.status, body: r.body }, null, 1));
  const n = Array.isArray(r.body) ? r.body.length : r.body?.total_count ?? "-";
  console.log(`${name} ${r.status} items=${n}`);
}
