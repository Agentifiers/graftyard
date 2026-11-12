// LF-4's seven new REST GETs as the deployed edge answers them, each written as a `<case>.rest.json` for
// `crates/api/gh/validate.py` (method, OpenAPI path, status, body), once without the agent media type and once with
// it. Reads the objects tests/am1/rest/lf4.mjs wrote on the repository: the newest pull request, its reviews and
// review comments, the newest issue's comments, and the newest check run's commit and its list.
//   node tests/am1/lf4/capture.mjs --repo NAME --out DIR --tip SHA   # SHA: the tip of lf4/<tag>/commits
import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { api, ghs, log, tokens } from "../hosted/lib.mjs";

const { values: opt } = parseArgs({ options: { repo: { type: "string" }, out: { type: "string" }, tip: { type: "string" } } });
if (!opt.repo || !opt.out || !opt.tip) throw new Error("usage: capture.mjs --repo NAME --out DIR --tip SHA");
mkdirSync(opt.out, { recursive: true });
const R = `/api/v3/repos/m3org/${opt.repo}`;
const admin = tokens.admin;
const AGENT = "application/vnd.graftyard.agent+json";
const get = (p, agent) => api("GET", `${R}${p}`, admin, undefined, agent ? { accept: AGENT } : {});

const pulls = (await get("/issues?state=open&per_page=100")).body.filter((x) => x.pull_request);
const host = pulls.find((x) => x.title.endsWith(" host"));
const issue = (await get("/issues?state=open&per_page=100")).body.filter((x) => !x.pull_request && x.comments > 0)[0];
// The commits branch's tip holds the check runs (head_sha); its sha is given, as lf4.jsonl does not keep it.
const commitsTip = opt.tip;
const runs = (await get(`/commits/${commitsTip}/check-runs`)).body;
const cases = [
  ["pull", `/pulls/${host.number}`, "/repos/{owner}/{repo}/pulls/{pull_number}"],
  ["issue-comments", `/issues/${issue.number}/comments`, "/repos/{owner}/{repo}/issues/{issue_number}/comments"],
  ["reviews", `/pulls/${host.number}/reviews`, "/repos/{owner}/{repo}/pulls/{pull_number}/reviews"],
  ["review-comments", `/pulls/${host.number}/comments`, "/repos/{owner}/{repo}/pulls/{pull_number}/comments"],
  ["commit", `/commits/${commitsTip}`, "/repos/{owner}/{repo}/commits/{ref}"],
  ["check-runs", `/commits/${commitsTip}/check-runs`, "/repos/{owner}/{repo}/commits/{ref}/check-runs"],
  ["check-run", `/check-runs/${runs.check_runs[0].id}`, "/repos/{owner}/{repo}/check-runs/{check_run_id}"],
  ["commit-missing", `/commits/${"0".repeat(40)}`, "/repos/{owner}/{repo}/commits/{ref}"],
];
const out = [];
// The two writes (lf4-rest-2): a review comment under the admin's PAT and a check run under m3-ci's token, each 201
// written as its POST case, plus GHES's 403 for a check run under a user token and 404 for a review comment on an
// issue.
const ci = (await ghs({})).body.token;
const writes = [
  ["review-comment-created", admin, `/pulls/${host.number}/comments`, "/repos/{owner}/{repo}/pulls/{pull_number}/comments",
    { body: "capture", commit_id: (await get(`/pulls/${host.number}`)).body.head.sha, path: "capture.txt", line: 1 }],
  ["check-run-created", ci, "/check-runs", "/repos/{owner}/{repo}/check-runs",
    { name: "capture", head_sha: commitsTip, status: "in_progress", output: { title: "t", summary: "s" } }],
  ["check-run-user", admin, "/check-runs", "/repos/{owner}/{repo}/check-runs", { name: "capture", head_sha: commitsTip }],
  ["review-comment-on-issue", admin, `/pulls/${issue.number}/comments`, "/repos/{owner}/{repo}/pulls/{pull_number}/comments",
    { body: "x", commit_id: commitsTip, path: "a", line: 1 }],
];
for (const [name, tok, path, oapi, body] of writes) {
  const r = await api("POST", `${R}${path}`, tok, body);
  // GHES 3.22's description lists only 201 for `POST check-runs` and no 404 for review comments, so the two refusals
  // are kept beside the replies (`.refusal.json`), not checked by validate.py (F56's kind of known mismatch).
  const ext = r.status === 201 ? "rest.json" : "refusal.json";
  writeFileSync(`${opt.out}/hosted-${name}.${ext}`, JSON.stringify({ method: "POST", path: oapi, status: r.status, body: r.body }));
  out.push(`${name} ${r.status}`);
}
for (const [name, path, oapi] of cases) {
  for (const agent of [false, true]) {
    const r = await get(path, agent);
    const file = `hosted-${name}${agent ? ".agent" : ""}.rest.json`;
    writeFileSync(`${opt.out}/${file}`, JSON.stringify({ method: "GET", path: oapi, status: r.status, body: r.body }));
    out.push(`${file} ${r.status}`);
  }
}
log(`pull ${host.number} issue ${issue.number} tip ${commitsTip.slice(0, 7)}: ${out.join(", ")}`);
