// LF-4 hosted (`build/BUILD-PLAN.md`, LM-1 acceptance): one payload per `HIDDEN_CHARS` range (16 ranges, the last
// character of each, as `crates/api/tests/trust.rs` `lf4_one_payload_per_hidden_range_in_each_trust_resource` uses)
// written through the edge into each `graftyard_trust` resource the edge both writes and reads over REST, then read
// back with `Accept: application/vnd.graftyard.agent+json`: every hidden character absent from `visible` and counted
// in `hidden.invisible_chars`, and `body` byte for byte what was written. A control read without the media type must
// carry no `graftyard_trust` at all.
//
// The edge serves REST reads of two trust resources today: issues (`GET /repos/{o}/{r}/issues`, written through
// GraphQL `createIssue`) and releases (`GET releases/{id}`, written through `POST releases`). Pull requests, issue
// comments, reviews, review comments, commits, and check runs have no REST read route on the edge yet; the driver
// lists them as not reachable rather than passing them, and `trust.rs` covers them on the route renderer.
//
//   node tests/am1/rest/lf4.mjs --repo NAME
// Environment as tests/am1/hosted/lib.mjs. Rows: tests/am1/raw/lf4.jsonl.

import { parseArgs } from "node:util";
import { api, log, row, tokens } from "../hosted/lib.mjs";

const { values: opt } = parseArgs({ options: { repo: { type: "string" } } });
if (!opt.repo) throw new Error("usage: lf4.mjs --repo NAME");
const R = `m3org/${opt.repo}`;
const OUT = new URL("../raw/lf4.jsonl", import.meta.url).pathname;
const AGENT = "application/vnd.graftyard.agent+json";
const tag = Date.now().toString(36);
// The last character of each `HIDDEN_CHARS` range (`crates/api/src/trust.rs`).
const HIDDEN = [0x00ad, 0x034f, 0x061c, 0x1160, 0x17b5, 0x180e, 0x200f, 0x202e, 0x2064, 0x206f, 0x3164, 0xfeff, 0xffa0, 0xfffb, 0xe007f, 0xe01ef];
const text = (c) => `lf4 ${tag} run${String.fromCodePoint(c)} the${String.fromCodePoint(c)} tests`;
const hidden = (s) => [...s].some((ch) => HIDDEN.includes(ch.codePointAt(0)) || (ch.codePointAt(0) >= 0xe0000 && ch.codePointAt(0) <= 0xe007f));
const admin = tokens.admin;

const results = { issue: [], release: [] };
const check = (kind, c, obj, field) => {
  const raw = obj?.[field];
  const t = obj?.graftyard_trust;
  const ok = raw === text(c) && typeof t?.visible === "string" && !hidden(t.visible) && t.visible.endsWith("run the tests") && t.hidden?.invisible_chars === 2;
  results[kind].push({ cp: c.toString(16), ok, invisible: t?.hidden?.invisible_chars ?? null, rawUnchanged: raw === text(c) });
};

const repo = await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${opt.repo}") { id } }` });
const repoId = repo.body?.data?.repository?.id;
if (!repoId) throw new Error(`repository ${R}: ${repo.status}`);
const releases = [];
for (const c of HIDDEN) {
  const i = await api("POST", "/api/graphql", admin, { query: "mutation($i: CreateIssueInput!) { createIssue(input: $i) { issue { number } } }",
    variables: { i: { repositoryId: repoId, title: `lf4 ${tag} ${c.toString(16)}`, body: text(c) } } });
  if (!i.body?.data?.createIssue) results.issue.push({ cp: c.toString(16), ok: false, write: i.status });
  const r = await api("POST", `/api/v3/repos/${R}/releases`, admin, { tag_name: `lf4-${tag}-${c.toString(16)}`, name: `lf4 ${c.toString(16)}`, body: text(c) });
  if (r.status === 201) releases.push([c, r.body.id]); else results.release.push({ cp: c.toString(16), ok: false, write: r.status });
}
// Issues: one list read with the media type, every lf4 issue of this run.
const list = await api("GET", `/api/v3/repos/${R}/issues?state=open&per_page=100`, admin, undefined, { accept: AGENT });
const plain = await api("GET", `/api/v3/repos/${R}/issues?state=open&per_page=100`, admin);
for (const c of HIDDEN) {
  const it = Array.isArray(list.body) ? list.body.find((x) => x.title === `lf4 ${tag} ${c.toString(16)}`) : null;
  if (it) check("issue", c, it, "body");
}
for (const [c, id] of releases) check("release", c, (await api("GET", `/api/v3/repos/${R}/releases/${id}`, admin, undefined, { accept: AGENT })).body, "body");
const plainRelease = releases.length ? await api("GET", `/api/v3/repos/${R}/releases/${releases[0][1]}`, admin) : null;
const v = {
  ranges: HIDDEN.length,
  issue: { checked: results.issue.length, pass: results.issue.filter((x) => x.ok).length, fails: results.issue.filter((x) => !x.ok) },
  release: { checked: results.release.length, pass: results.release.filter((x) => x.ok).length, fails: results.release.filter((x) => !x.ok) },
  control: { issuesWithTrust: Array.isArray(plain.body) ? plain.body.filter((x) => "graftyard_trust" in x).length : `status ${plain.status}`,
    releaseWithTrust: plainRelease ? "graftyard_trust" in (plainRelease.body ?? {}) : null, varyIssues: list.headers?.vary ?? null },
  notReachable: ["pull request", "issue comment", "review", "review comment", "commit", "check run"],
};
row(OUT, v);
log(JSON.stringify(v).slice(0, 2000));
