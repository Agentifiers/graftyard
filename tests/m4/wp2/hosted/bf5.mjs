// BF-5 on graft-m4a: m3-member (the watcher) watches m3org/wp2-bf5; m3-admin stars it and comments on an issue in it.
// Measured: when `stargazers_count` shows the star and when the watcher's `/notifications` shows the comment's thread
// (pass within 10 s, fail over 60 s). Then the watcher marks the thread read, its `Inbox` is snapshotted, deleted, and
// rebuilt through its `Identity` leader's `/inbox`, and `/notifications?all=true` must show the thread still read.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp2/hosted/bf5.mjs
import { api, leaderOf, log, member, row, sleep, state, tokens } from "../../../am1/hosted/lib.mjs";

const out = new URL("../raw/bf5.jsonl", import.meta.url).pathname;
const REPO = "m3org/wp2-bf5";
const watcher = tokens.member, actor = tokens.admin;
const must = (r, ...ok) => { if (!ok.includes(r.status)) throw new Error(`${r.status} ${JSON.stringify(r.body).slice(0, 300)}`); return r; };
// Issues and comments through GraphQL (`createIssue`, `addComment`), as gh sends them; the install serves no REST create.
async function gql(token, query, variables) {
  const r = await api("POST", "/api/graphql", token, { query, variables });
  if (r.status !== 200 || r.body?.errors) throw new Error(`graphql ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
  return r.body.data;
}

/** Polls `f` every 250 ms until it answers truthy; the ms it took, or null after 60 s. */
async function until(f, t0) {
  for (;;) {
    if (await f()) return Date.now() - t0;
    if (Date.now() - t0 > 60_000) return null;
    await sleep(250);
  }
}

must(await api("PUT", `/api/v3/repos/${REPO}/subscription`, watcher, { subscribed: true }), 200);
must(await api("DELETE", `/api/v3/user/starred/${REPO}`, actor), 204);
// The unstar reaches the count through `Counts` too, so wait until the count stops moving before taking `before`.
let before = -1;
for (let same = 0; same < 4; await sleep(1000)) {
  const n = (await api("GET", `/api/v3/repos/${REPO}`, actor)).body.stargazers_count;
  same = n === before ? same + 1 : 0;
  before = n;
}
const repoNode = must(await api("GET", `/api/v3/repos/${REPO}`, actor), 200).body.node_id;
const issue = (await gql(actor, "mutation($r: ID!, $t: String!) { createIssue(input: {repositoryId: $r, title: $t}) { issue { id number } } }",
  { r: repoNode, t: `BF-5 ${new Date().toISOString()}` })).createIssue.issue;
const thread = (await api("GET", "/api/v3/notifications?all=true", watcher)).body;
log(`before: ${before} stars; issue #${issue.number}; watcher threads ${Array.isArray(thread) ? thread.length : thread}`);
await sleep(15_000); // let the issue's own notice land first, so the comment is what moves `updated_at`

const t0 = Date.now();
must(await api("PUT", `/api/v3/user/starred/${REPO}`, actor), 204);
const comment = (await gql(actor, "mutation($s: ID!) { addComment(input: {subjectId: $s, body: \"BF-5: a comment on a watched issue\"}) { commentEdge { node { createdAt } } } }",
  { s: issue.id })).addComment.commentEdge.node;
comment.created_at = comment.createdAt;
const tWrite = Date.now() - t0;
const [star, notice] = await Promise.all([
  until(async () => (await api("GET", `/api/v3/repos/${REPO}`, actor)).body?.stargazers_count === before + 1, t0),
  until(async () => {
    const r = await api("GET", "/api/v3/notifications", watcher);
    const n = Array.isArray(r.body) ? r.body.find((x) => x.subject?.url?.endsWith(`/issues/${issue.number}`)) : null;
    return n && n.unread && Date.parse(n.updated_at) >= Date.parse(comment.created_at) - 1000;
  }, t0),
]);
log(`writes ${tWrite} ms; star visible ${star} ms; notice visible ${notice} ms`);
row(out, { step: "visible", writes_ms: tWrite, star_ms: star, notice_ms: notice, issue: issue.number });

// Read state, then the rebuild.
const listed = (await api("GET", "/api/v3/notifications", watcher)).body.find((x) => x.subject?.url?.endsWith(`/issues/${issue.number}`));
must(await api("PATCH", `/api/v3/notifications/threads/${listed.id}`, watcher), 205);
const view = async () => (await api("GET", "/api/v3/notifications?all=true", watcher)).body;
const shape = (l) => l.map((x) => ({ id: x.id, unread: x.unread, reason: x.reason, last_read_at: x.last_read_at, updated_at: x.updated_at })).sort((a, b) => a.id.localeCompare(b.id));
const live = shape(await view());
const chain = `identity:${state.member}`;
const lead = await leaderOf(chain);
const inbox = async (op) => (await member(chain, lead, "/inbox", { op }, 60_000)).v;
const snap = await inbox("snapshot");
const del = await inbox("delete");
const empty = shape(await view());
const t1 = Date.now();
const rebuilt = await inbox("rebuild");
const rebuildMs = Date.now() - t1;
const after = shape(await view());
const same = JSON.stringify(after) === JSON.stringify(live);
const read = after.find((x) => x.id === listed.id);
log(`snapshot ${JSON.stringify(snap).slice(0, 160)}; delete ${JSON.stringify(del).slice(0, 80)}; empty after delete ${empty.length}`);
log(`rebuild ${rebuildMs} ms ${JSON.stringify(rebuilt).slice(0, 300)}`);
log(`live ${JSON.stringify(live)}`);
log(`after ${JSON.stringify(after)}`);
const pass = star !== null && notice !== null && star <= 10_000 && notice <= 10_000 && same && read && !read.unread;
row(out, { step: "rebuild", snapshot: snap, deleted: del, empty: empty.length, rebuild_ms: rebuildMs, rebuilt, live, after, same, pass });
log(pass ? "BF-5 PASS" : "BF-5 FAIL");
