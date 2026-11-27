// plaindiff's comparison (`run.sh`): the same requests from a client that does not opt in to the agent layer (a plain
// PAT, git's and gh's user agents, no agent header) sent to edge 0 (the base build) and edge 1 (the head build), and
// their status, headers and body compared. Masked: the headers `run.sh` lists as per request (`date`, `server-timing`,
// request ids, lengths), GitHub request ids in bodies, `id` and `node_id` values, and the label name, which differs per
// edge so both label writes create. `CHANGED` names the requests the head means to answer otherwise. ALLOW names the request whose only allowed difference is a `retry-after` header the
// base lacks, with its value (`fetch=2`: edge-shed-429's upload-pack shard 503). Exits 1 on any other difference.
//   node tests/plaindiff/diff.mjs <front url> <bed dir> [ALLOW]
import { readFileSync } from "node:fs";

const [F, B, ALLOW = ""] = process.argv.slice(2);
const [allowName, allowValue] = ALLOW.split("=");
// Requests the head is meant to answer differently: the base's and the head's status, and the head's body.
// A body given as a function is a check of the head's body.
const CHANGED = { "rest-pulls": [404, 200, "[]"], "rest-issue-comments": [404, 200, "[]"], "rest-issue-events": [404, 200, "[]"],
  // label-routes: one label by name, which main answered 404, is the label; a write-access login can be assigned.
  "rest-label-get": [404, 200, (b) => JSON.parse(b).name === "plain"],
  "rest-repo-assignee": [404, 204, ""],
  // A reader's label delete, which main answered 404, is refused 403 with the route's own documentation.
  "rest-label-delete-reader": [404, 403, (b) => JSON.parse(b).documentation_url.endsWith("/rest/issues/labels#delete-a-label")] };
// Requests whose only allowed difference is the error body's `documentation_url`, with the head's page: a refusal a
// route renders through `What::Deleted` named the releases page on main.
const DOC_MOVED = { "rest-issue-comment-delete-reader": "/rest/issues/comments#delete-an-issue-comment" };
const seed = JSON.parse(readFileSync(`${B}/seed.json`, "utf8"));
const seed2 = JSON.parse(readFileSync(`${B}/seed2.json`, "utf8"));
const reader = { authorization: `token ${seed2.token}`, "user-agent": "gh/2.102.1" };
const auth = { authorization: "Basic " + Buffer.from(`x:${seed.token}`).toString("base64") };
const tok = { authorization: `token ${seed.token}`, "user-agent": "gh/2.102.1" };
const p = `/${seed.login}/${seed.repo}.git`;
const VOL = new Set(["date", "cf-ray", "x-github-request-id", "server-timing", "report-to", "nel", "alt-svc",
  "x-ratelimit-reset", "cf-cache-status", "content-length", "x-graft-timing", "x-graftyard-edge-version"]);
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const lsRefs = Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("peel\n"), pkt("ref-prefix refs/heads/\n"), Buffer.from("0000")]);
// A fetch of an object no shard holds: no git cache shard runs here, so the shard read answers non-200 and the edge
// answers 503 from that branch.
const fetchV2 = Buffer.concat([pkt("command=fetch\n"), Buffer.from("0001"), pkt(`want ${"1".repeat(40)}\n`), pkt("done\n"), Buffer.from("0000")]);
const v2 = { "git-protocol": "version=2", "user-agent": "git/2.43.0" };
const up = { ...auth, ...v2, "content-type": "application/x-git-upload-pack-request" };
const reqs = [
  ["info-refs-upload", "GET", `${p}/info/refs?service=git-upload-pack`, { ...auth, ...v2 }],
  ["ls-refs", "POST", `${p}/git-upload-pack`, up, lsRefs],
  ["fetch", "POST", `${p}/git-upload-pack`, up, fetchV2],
  ["info-refs-receive", "GET", `${p}/info/refs?service=git-receive-pack`, { ...auth, "user-agent": "git/2.43.0" }],
  ["rest-repo", "GET", `/api/v3/repos/${seed.login}/${seed.repo}`, tok],
  // F136: the head serves the list of pull requests main answered 404; `CHANGED` names that difference.
  ["rest-pulls", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/pulls?state=all&per_page=5`, tok],
  ["rest-pull-1", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/pulls/1`, tok],
  ["rest-issues", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues?state=all`, tok],
  // F137: the head serves the single-issue read, and the comment create only once 32.32 is enabled, which this bed
  // does not enable; the seed holds no issue, so both stay main's 404.
  ["rest-issue-1", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1`, tok],
  ["rest-issue-comment", "POST", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/comments`, { ...tok, "content-type": "application/json" }, JSON.stringify({ body: "plain" })],
  // F137's reads: the repository-wide comment and event lists, which main answered 404 as `issues/{n}`, are the
  // seed's empty lists on the head (`CHANGED`); an item's labels, events and timeline, and a comment or event by id,
  // stay 404 with no issue.
  ["rest-issue-comments", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/comments?sort=updated`, tok],
  ["rest-issue-events", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/events`, tok],
  ["rest-issue-1-labels", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/labels`, tok],
  ["rest-issue-1-events", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/events`, tok],
  ["rest-issue-1-timeline", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/timeline`, tok],
  ["rest-issue-comment-1", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/comments/1`, tok],
  ["rest-issue-event-1", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/events/1`, tok],
  // F141: the single-field issue PATCH is served only once 32.33 is enabled, which this bed does not enable, and a
  // two-field PATCH is never this route; both stay main's answer.
  ["rest-issue-1-patch-title", "PATCH", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1`, { ...tok, "content-type": "application/json" }, JSON.stringify({ title: "plain" })],
  ["rest-issue-1-patch-two", "PATCH", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1`, { ...tok, "content-type": "application/json" }, JSON.stringify({ title: "plain", body: "b" })],
  // F137's writes: a comment edit and a lock change are served only once 32.35 is enabled, which this bed does not
  // enable; a comment delete and the assignee check are served on the head, and with no issue all stay main's 404.
  ["rest-issue-comment-1-edit", "PATCH", `/api/v3/repos/${seed.login}/${seed.repo}/issues/comments/1`, { ...tok, "content-type": "application/json" }, JSON.stringify({ body: "plain" })],
  ["rest-issue-comment-1-delete", "DELETE", `/api/v3/repos/${seed.login}/${seed.repo}/issues/comments/999999`, tok],
  ["rest-issue-1-lock", "PUT", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/lock`, { ...tok, "content-type": "application/json" }, "{}"],
  ["rest-issue-1-unlock", "DELETE", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/lock`, tok],
  ["rest-issue-1-assignee", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1/assignees/${seed.login}`, tok],
  // label-routes (`seed2.mjs`: the reader `cw2`, issue 1 with the owner's comment, the label `plain`). The update is
  // served only once 32.36 is enabled, which this bed does not enable; the reader's deletes are refused in `decide`.
  ["rest-label-get", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/labels/plain`, tok],
  ["rest-label-missing", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/labels/none`, tok],
  ["rest-label-update", "PATCH", `/api/v3/repos/${seed.login}/${seed.repo}/labels/plain`, { ...tok, "content-type": "application/json" }, JSON.stringify({ color: "000000" })],
  ["rest-label-delete-missing", "DELETE", `/api/v3/repos/${seed.login}/${seed.repo}/labels/none`, tok],
  ["rest-label-delete-reader", "DELETE", `/api/v3/repos/${seed.login}/${seed.repo}/labels/plain`, reader],
  ["rest-issue-comment-delete-reader", "DELETE", `/api/v3/repos/${seed.login}/${seed.repo}/issues/comments/${seed2.comment}`, reader],
  ["rest-repo-assignee", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/assignees/${seed.login}`, tok],
  ["rest-repo-assignee-reader", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/assignees/cw2`, tok],
  // Milestones are served only once 32.37 is enabled, which this bed does not enable: every route, and an issue
  // body's `milestone`, stay main's answer.
  ["rest-milestones", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/milestones`, tok],
  ["rest-milestone-1", "GET", `/api/v3/repos/${seed.login}/${seed.repo}/milestones/1`, tok],
  ["rest-milestone-create", "POST", `/api/v3/repos/${seed.login}/${seed.repo}/milestones`, { ...tok, "content-type": "application/json" }, JSON.stringify({ title: "v1" })],
  ["rest-milestone-delete", "DELETE", `/api/v3/repos/${seed.login}/${seed.repo}/milestones/1`, tok],
  ["rest-issue-1-patch-milestone", "PATCH", `/api/v3/repos/${seed.login}/${seed.repo}/issues/1`, { ...tok, "content-type": "application/json" }, JSON.stringify({ milestone: 1 })],
  ["rest-label-write", "POST", `/api/v3/repos/${seed.login}/${seed.repo}/labels`, { ...tok, "content-type": "application/json" }, "LABEL"],
  ["graphql", "POST", "/api/graphql", { ...tok, "content-type": "application/json" }, JSON.stringify({ query: "{ repository(owner: \"cw\", name: \"r\") { name } }" })],
  // Milestones head 2: with 32.37 off, gh's milestone list and `milestone(number:)` are main's `undefinedField`, and an
  // issue's or pull request's `milestone`, which no row names, main's `null`.
  ["graphql-milestones", "POST", "/api/graphql", { ...tok, "content-type": "application/json" }, JSON.stringify({ query: "query RepositoryMilestoneList($endCursor:String$name:String!$owner:String!$states:[MilestoneState!]!){repository(owner: $owner, name: $name){milestones(states: $states, first: 100, after: $endCursor){nodes{id,title},pageInfo{hasNextPage,endCursor}}}}", variables: { endCursor: null, name: seed.repo, owner: seed.login, states: ["OPEN"] } })],
  ["graphql-milestone-1", "POST", "/api/graphql", { ...tok, "content-type": "application/json" }, JSON.stringify({ query: `{ repository(owner: "${seed.login}", name: "${seed.repo}") { milestone(number: 1) { title } } }` })],
  ["graphql-item-milestones", "POST", "/api/graphql", { ...tok, "content-type": "application/json" }, JSON.stringify({ query: `{ repository(owner: "${seed.login}", name: "${seed.repo}") { issues(first: 5) { nodes { number milestone { title } } } pullRequests(first: 5) { nodes { number milestone { title } } } } }` })],
];
const mask = (s) => s.replace(/bx\dr\d+/g, "LABELNAME").replace(/"(node_id|id)":(\d+|"[^"]*")/g, '"$1":"*"')
  .replace(/[0-9A-F]{4,8}:[0-9A-F]{4,8}:[0-9A-F]{4,10}:[0-9A-F]{4,10}:[0-9A-F]{6,10}/g, "RID");

async function one(edge, [, m, path, h, body0]) {
  const body = body0 === "LABEL" ? JSON.stringify({ name: `bx${edge}r${Date.now() % 100000}`, color: "ffffff" }) : body0;
  let r = await fetch(F + path, { method: m, headers: { ...h, "x-cw-edge": String(edge) }, body });
  // A read the members shed under the bed's load (`read_failed`: 503, `retry-after: 1`) is asked again, as a client
  // that honors Retry-After does: main's tier-row read for `GET assignees/{login}` was shed on two fresh beds.
  for (let i = 0; i < 3 && m === "GET" && r.status === 503 && r.headers.get("retry-after") === "1"; i++) {
    await r.arrayBuffer();
    await new Promise((s) => setTimeout(s, 1000));
    r = await fetch(F + path, { method: m, headers: { ...h, "x-cw-edge": String(edge) }, body });
  }
  const headers = [...r.headers].filter(([k]) => !VOL.has(k)).sort();
  return { status: r.status, headers, body: mask(Buffer.from(await r.arrayBuffer()).toString("latin1")) };
}

let bad = 0;
for (const q of reqs) {
  const [a, b] = [await one(0, q), await one(1, q)];
  const ra = a.headers.find(([k]) => k === "retry-after")?.[1];
  const rb = b.headers.find(([k]) => k === "retry-after")?.[1];
  const same = JSON.stringify(a) === JSON.stringify(b);
  const allowed = !same && q[0] === allowName && a.status === b.status && a.body === b.body && ra === undefined
    && rb === allowValue && JSON.stringify(a.headers) === JSON.stringify(b.headers.filter(([k]) => k !== "retry-after"));
  const ch = CHANGED[q[0]];
  const changed = !same && ch && a.status === ch[0] && b.status === ch[1]
    && (typeof ch[2] === "function" ? ch[2](b.body) : b.body === ch[2]);
  const doc = (x) => { try { return JSON.parse(x.body).documentation_url; } catch { return undefined; } };
  const moved = !same && DOC_MOVED[q[0]] && a.status === b.status && JSON.stringify(a.headers) === JSON.stringify(b.headers)
    && doc(a) !== doc(b) && doc(b)?.endsWith(DOC_MOVED[q[0]]) && a.body.replace(doc(a), doc(b)) === b.body;
  const verdict = same ? "identical" : allowed ? `differs only by retry-after: ${rb} (allowed)`
    : changed ? `changed as intended: ${a.status} -> ${b.status}`
    : moved ? `differs only by documentation_url (intended): ${doc(a)} -> ${doc(b)}` : "DIFFERS";
  console.log(`${q[0]}: base ${a.status} retry-after ${ra ?? "-"} | head ${b.status} retry-after ${rb ?? "-"} | ${verdict} | ${a.body.slice(0, 80).replace(/\n/g, " ")}`);
  if (!same && !allowed && !changed && !moved) {
    bad++;
    console.log(`  base ${JSON.stringify(a).slice(0, 400)}\n  head ${JSON.stringify(b).slice(0, 400)}`);
  }
}
console.log(`${reqs.length} request pairs, ${bad} unexpected differences`);
process.exit(bad ? 1 : 0);
