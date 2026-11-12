// LF-4 hosted (`build/BUILD-PLAN.md`, LM-1 acceptance): one payload per `HIDDEN_CHARS` range (16 ranges, the last
// character of each, as `crates/api/tests/trust.rs` `lf4_one_payload_per_hidden_range_in_each_trust_resource` uses)
// in each of the eight `graftyard_trust` resources, read back over the edge's REST routes with
// `Accept: application/vnd.graftyard.agent+json`: every hidden character absent from `visible` and counted in
// `hidden.invisible_chars`, and the raw text (`body`, `commit.message`, `output.summary`) byte for byte what was
// written. A control read of each route without the media type must carry no `graftyard_trust`.
//
// Writers: an untrusted account (a fresh user with no org membership and Read on the public repository, under its
// own PAT) writes the issues, pull requests, issue comments, reviews, and review comments, so their views must also
// read `trusted: false`; the admin writes the releases (they need Write), pushes the commits, and records the check
// runs (as App m3-ci). Issues, pull requests, comments, and reviews go through the edge's GraphQL, releases through
// `POST releases`, commits through git. The edge serves no REST or GraphQL write of a review comment or a check run,
// so those two are committed on the repository's group through the tools edge with the writer's own credential
// (`IssueOp::Comment` on a review line, `IssueOp::RecordCheck`), which `decide` checks as it checks any write.
//
//   node tests/am1/rest/lf4.mjs --repo NAME --chain repo:ID
// Environment as tests/am1/hosted/lib.mjs. Rows: tests/am1/raw/lf4.jsonl.

import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { Git, OP, accountShard, api, commit, log, loginShard, must, pat, rid, row, state, tokens } from "../hosted/lib.mjs";

const { values: opt } = parseArgs({ options: { repo: { type: "string" }, chain: { type: "string" } } });
if (!opt.repo || !opt.chain) throw new Error("usage: lf4.mjs --repo NAME --chain repo:ID");
const R = `m3org/${opt.repo}`;
const RAW = new URL("../raw/", import.meta.url).pathname;
mkdirSync(RAW, { recursive: true });
const OUT = `${RAW}lf4.jsonl`;
const AGENT = "application/vnd.graftyard.agent+json";
const tag = Date.now().toString(36);
// The last character of each `HIDDEN_CHARS` range (`crates/api/src/trust.rs`).
const HIDDEN = [0x00ad, 0x034f, 0x061c, 0x1160, 0x17b5, 0x180e, 0x200f, 0x202e, 0x2064, 0x206f, 0x3164, 0xfeff, 0xffa0, 0xfffb, 0xe007f, 0xe01ef];
const text = (c) => `lf4 ${tag} run${String.fromCodePoint(c)} the${String.fromCodePoint(c)} tests`;
const RANGES = [[0xad, 0xad], [0x34f, 0x34f], [0x61c, 0x61c], [0x115f, 0x1160], [0x17b4, 0x17b5], [0x180e, 0x180e], [0x200b, 0x200f], [0x202a, 0x202e],
  [0x2060, 0x2064], [0x2066, 0x206f], [0x3164, 0x3164], [0xfeff, 0xfeff], [0xffa0, 0xffa0], [0xfff9, 0xfffb], [0xe0000, 0xe007f], [0xe0100, 0xe01ef]];
const hidden = (s) => [...s].some((ch) => RANGES.some(([a, b]) => ch.codePointAt(0) >= a && ch.codePointAt(0) <= b));
const admin = tokens.admin;
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });

// ---- the untrusted writer ----
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
const uid = 7100500 + (Date.now() % 400);
const ulogin = `lf4-u${tag}`;
await must(`names:${accountShard(uid)}`, { cred: OP, op: { PutAccount: { id: uid, suspended: false, sessions_not_before: 0 } } });
await must(`names:${loginShard(ulogin)}`, { cred: OP, op: { ClaimLogin: { login: ulogin, kind: "User", account: uid } } }, "Taken");
await must(`names:${accountShard(uid)}`, { cred: OP, op: { SetAccountLogin: { account: uid, login: ulogin } } });
await must(`names:${accountShard(uid)}`, { cred: OP, op: { InitPlacement: { owner: uid, record: TRI } } }, "RecordExists");
await must(`authz:${uid}`, { cred: OP, op: { Open: { kind: "User", enterprise: null } } }, "AlreadyOpened");
await must(`identity:${uid}`, { cred: OP, op: { Create: { login: ulogin, kind: "User", created: Date.now() } } }, "AlreadyCreated");
const untrusted = await pat(uid, `lf4 ${tag}`);
// The PAT's stored form, for the two member commits: SHA-256 of "graftyard/token/" and its 30-character secret.
const patHash = [...createHash("sha256").update("graftyard/token/").update(untrusted.slice(4, 34)).digest()];
const adminAccount = state.admin;
const adminHash = [...createHash("sha256").update("graftyard/token/").update(admin.slice(4, 34)).digest()];
row(OUT, { step: "untrusted user", id: uid });

const issueCall = (account, token, op) => ({ issue: { format: [32, 5], op, account, scopes: 1, token, sso: [], request: rid(), render: null } });
const memberCommit = async (account, token, op) => {
  const o = await commit(opt.chain, issueCall(account, token, op));
  return o?.ok?.out ? "ok" : JSON.stringify(o).slice(0, 200);
};

// ---- writes ----
const repo = await gql(admin, `query { repository(owner: "m3org", name: "${opt.repo}") { id } }`);
const repoId = repo.body?.data?.repository?.id;
if (!repoId) throw new Error(`repository ${R}: ${repo.status}`);
const g = new Git(R).as(admin);
if (!g.clone().ok) throw new Error("clone");
const main = g.run(["rev-parse", "HEAD"]).out.trim();
const commitOn = (base, path, message) => {
  g.run(["checkout", "-q", "--detach", base]);
  mkdirSync(join(g.wt, "lf4", tag), { recursive: true });
  writeFileSync(join(g.wt, "lf4", tag, path), `${path} ${Math.random()}\n`);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", message]);
  return g.run(["rev-parse", "HEAD"]).out.trim();
};
const writes = {};
const wrote = (kind, ok) => { writes[kind] ??= { ok: 0, fail: [] }; ok === true ? writes[kind].ok++ : writes[kind].fail.push(String(ok).slice(0, 160)); };

// Commits: one per payload, chained on `lf4/<tag>/commits`, each message the payload.
let tip = main;
const shas = [];
for (const c of HIDDEN) {
  tip = commitOn(tip, `c${c.toString(16)}.txt`, text(c));
  shas.push([c, tip]);
}
const pushC = g.run(["push", "-q", "origin", `${tip}:refs/heads/lf4/${tag}/commits`]);
for (const _ of HIDDEN) wrote("commit", pushC.ok || pushC.out);

// Pull requests: one per payload, each from its own branch the admin pushed, opened by the untrusted user.
const CREATE_PR = "mutation($input: CreatePullRequestInput!) { createPullRequest(input: $input) { pullRequest { id number } } }";
const prs = [];
for (const c of HIDDEN) {
  const b = `lf4/${tag}/pr-${c.toString(16)}`;
  const sha = commitOn(main, `p${c.toString(16)}.txt`, `lf4 pr ${c.toString(16)}`);
  g.run(["push", "-q", "origin", `${sha}:refs/heads/${b}`]);
  const r = await gql(untrusted, CREATE_PR, { input: { repositoryId: repoId, baseRefName: "main", headRefName: b, title: `lf4 ${tag} ${c.toString(16)}`, body: text(c) } });
  const pr = r.body?.data?.createPullRequest?.pullRequest;
  if (pr) prs.push([c, pr.number, pr.id, sha]);
  wrote("pull request", pr ? true : JSON.stringify(r.body).slice(0, 160));
}
// The admin's pull request that carries the reviews and review comments.
const hostB = `lf4/${tag}/host`;
const hostSha = commitOn(main, "host.txt", "lf4 host");
g.run(["push", "-q", "origin", `${hostSha}:refs/heads/${hostB}`]);
const hostPr = (await gql(admin, CREATE_PR, { input: { repositoryId: repoId, baseRefName: "main", headRefName: hostB, title: `lf4 ${tag} host`, body: "host" } })).body?.data?.createPullRequest?.pullRequest;
if (!hostPr) throw new Error("host pull request");

// Issues by the untrusted user, then one issue carrying 16 comments by the untrusted user.
const issues = [];
for (const c of HIDDEN) {
  const r = await gql(untrusted, "mutation($i: CreateIssueInput!) { createIssue(input: $i) { issue { id number } } }",
    { i: { repositoryId: repoId, title: `lf4 ${tag} ${c.toString(16)}`, body: text(c) } });
  const it = r.body?.data?.createIssue?.issue;
  if (it) issues.push([c, it.number, it.id]);
  wrote("issue", it ? true : JSON.stringify(r.body).slice(0, 160));
}
const host = issues[0];
for (const c of HIDDEN) {
  const r = await gql(untrusted, "mutation($i: AddCommentInput!) { addComment(input: $i) { clientMutationId } }", { i: { subjectId: host[2], body: text(c) } });
  wrote("issue comment", r.body?.data?.addComment ? true : JSON.stringify(r.body).slice(0, 160));
}
// Reviews (COMMENT) by the untrusted user on the admin's pull request.
for (const c of HIDDEN) {
  const r = await gql(untrusted, "mutation($i: AddPullRequestReviewInput!) { addPullRequestReview(input: $i) { pullRequestReview { id } } }",
    { i: { pullRequestId: hostPr.id, event: "COMMENT", body: text(c) } });
  wrote("review", r.body?.data?.addPullRequestReview ? true : JSON.stringify(r.body).slice(0, 160));
}
// Review comments by the untrusted user on a line of the admin's pull request (member commit, its own PAT).
const hostHead = [...Buffer.from(hostSha, "hex")];
for (const [i, c] of HIDDEN.entries()) {
  const at = { commit: hostHead, path: "lf4/host.txt", line: i + 1, start_line: null, side: "Right" };
  wrote("review comment", (await memberCommit(uid, patHash, { Comment: { on: { Review: { n: hostPr.number, review: null, at } }, body: text(c), reply_to: null } })) === "ok" || "refused");
}
// Check runs on the commits tip, by App m3-ci, recorded under the admin's PAT (member commit; `RecordCheck` needs Write).
const runBase = 900000 + (Date.now() % 90000) * 10;
const runIds = [];
for (const [i, c] of HIDDEN.entries()) {
  const id = runBase + i;
  const op = { RecordCheck: { run: id, suite: runBase, app: state.app, head_sha: [...Buffer.from(tip, "hex")], name: `lf4-${c.toString(16)}`, version: 1,
    status: "Completed", conclusion: "Success", started: Date.now() - 1000, completed: Date.now(), external_id: null, details_url: null,
    output: { title: `lf4 ${c.toString(16)}`, summary: text(c), text: null, annotations: [] } } };
  const r = await memberCommit(adminAccount, adminHash, op);
  if (r === "ok") runIds.push([c, id]);
  wrote("check run", r === "ok" || r);
}
// Releases by the admin.
const releases = [];
for (const c of HIDDEN) {
  const r = await api("POST", `/api/v3/repos/${R}/releases`, admin, { tag_name: `lf4-${tag}-${c.toString(16)}`, name: `lf4 ${c.toString(16)}`, body: text(c) });
  if (r.status === 201) releases.push([c, r.body.id]);
  wrote("release", r.status === 201 || r.status);
}
row(OUT, { step: "writes", writes });
log(`writes ${JSON.stringify(writes)}`);

// ---- reads ----
const results = {};
const check = (kind, c, obj, raw, untrustedAuthor) => {
  const t = obj?.graftyard_trust;
  const ok = raw === text(c) && typeof t?.visible === "string" && !hidden(t.visible) && t.visible.endsWith("run the tests")
    && t.hidden?.invisible_chars === 2 && (!untrustedAuthor || t.author?.trusted === false);
  (results[kind] ??= []).push({ cp: c.toString(16), ok, invisible: t?.hidden?.invisible_chars ?? null, rawUnchanged: raw === text(c), trusted: t?.author?.trusted ?? null });
};
const controls = {};
const control = async (kind, path) => {
  const plain = await api("GET", path, admin);
  const objs = Array.isArray(plain.body) ? plain.body : plain.body?.check_runs ?? [plain.body];
  controls[kind] = { status: plain.status, withTrust: objs.filter((o) => o && typeof o === "object" && "graftyard_trust" in o).length };
};
const get = async (path) => (await api("GET", path, admin, undefined, { accept: AGENT })).body;
const missing = (kind, c) => (results[kind] ??= []).push({ cp: c.toString(16), ok: false, missing: true });

// Issues: the list.
const list = await get(`/api/v3/repos/${R}/issues?state=open&per_page=100`);
for (const [c] of issues) {
  const it = Array.isArray(list) ? list.find((x) => x.title === `lf4 ${tag} ${c.toString(16)}`) : null;
  it ? check("issue", c, it, it.body, true) : missing("issue", c);
}
await control("issue", `/api/v3/repos/${R}/issues?state=open&per_page=100`);
// Pull requests: one GET each.
for (const [c, n] of prs) { const p = await get(`/api/v3/repos/${R}/pulls/${n}`); check("pull request", c, p, p?.body, true); }
await control("pull request", `/api/v3/repos/${R}/pulls/${prs[0]?.[1]}`);
// Issue comments: the host issue's list.
const comments = await get(`/api/v3/repos/${R}/issues/${host[1]}/comments?per_page=100`);
for (const c of HIDDEN) {
  const o = Array.isArray(comments) ? comments.find((x) => x.body === text(c)) ?? comments.find((x) => typeof x.body === "string" && x.body.startsWith(`lf4 ${tag} run${String.fromCodePoint(c)}`)) : null;
  o ? check("issue comment", c, o, o.body, true) : missing("issue comment", c);
}
await control("issue comment", `/api/v3/repos/${R}/issues/${host[1]}/comments?per_page=100`);
// Reviews and review comments: the host pull request's lists.
const reviews = await get(`/api/v3/repos/${R}/pulls/${hostPr.number}/reviews?per_page=100`);
for (const c of HIDDEN) { const o = Array.isArray(reviews) ? reviews.find((x) => x.body === text(c)) : null; o ? check("review", c, o, o.body, true) : missing("review", c); }
await control("review", `/api/v3/repos/${R}/pulls/${hostPr.number}/reviews?per_page=100`);
const rcs = await get(`/api/v3/repos/${R}/pulls/${hostPr.number}/comments?per_page=100`);
for (const c of HIDDEN) { const o = Array.isArray(rcs) ? rcs.find((x) => x.body === text(c)) : null; o ? check("review comment", c, o, o.body, true) : missing("review comment", c); }
await control("review comment", `/api/v3/repos/${R}/pulls/${hostPr.number}/comments?per_page=100`);
// Commits: one GET each, by sha.
for (const [c, sha] of shas) { const o = await get(`/api/v3/repos/${R}/commits/${sha}`); check("commit", c, o, o?.commit?.message, false); }
await control("commit", `/api/v3/repos/${R}/commits/${shas[0][1]}`);
// Check runs: one GET each by id, and the tip's list.
for (const [c, id] of runIds) { const o = await get(`/api/v3/repos/${R}/check-runs/${id}`); check("check run", c, o, o?.output?.summary, false); }
const runList = await get(`/api/v3/repos/${R}/commits/${tip}/check-runs?per_page=100`);
const listed = (runList?.check_runs ?? []).filter((x) => x.graftyard_trust && !hidden(x.graftyard_trust.visible) && x.graftyard_trust.hidden?.invisible_chars === 2).length;
await control("check run", `/api/v3/repos/${R}/check-runs/${runIds[0]?.[1]}`);
// Releases: one GET each.
for (const [c, id] of releases) { const o = await get(`/api/v3/repos/${R}/releases/${id}`); check("release", c, o, o?.body, false); }
await control("release", `/api/v3/repos/${R}/releases/${releases[0]?.[1]}`);

const v = { tag, ranges: HIDDEN.length, writes, controls, checkRunList: { total: runList?.total_count ?? null, viewsOk: listed } };
for (const [k, xs] of Object.entries(results)) v[k] = { checked: xs.length, pass: xs.filter((x) => x.ok).length, fails: xs.filter((x) => !x.ok) };
row(OUT, v);
log(JSON.stringify(v).slice(0, 4000));
