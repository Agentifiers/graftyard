// closetasks.mjs OWNER/REPO [--dry]: cancels every task not in a final state on the repository with the m3-ci
// installation token, so a rerun's agents are not refused at TASKS_OPEN_PER_REQUESTER (LM-1 round 24c), then creates
// one probe task and cancels it. The list pages as AGENT-LAYER's "API reference" says: `per_page` and the opaque
// `after` from the `Link` header's rel="next" URL. `page` is not a parameter of this API, so a loop over `page=N`
// read the first page every time. Prints counts only; tokens never leave lib.mjs's scan.
//   GRAFT_EDGE=... node tests/fanout/closetasks.mjs m3org/b034-mono
const { api, ghs, log } = await import("../am1/hosted/lib.mjs");
const [repo, flag] = process.argv.slice(2);
if (!repo) throw new Error("usage: closetasks.mjs OWNER/REPO [--dry]");
const V1 = `/api/graftyard/v1/repos/${repo}`;
const t = await ghs({});
if (t.status !== 201) throw new Error(`ghs ${t.status}`);
const tok = t.body.token;

/** Every task the list returns, following `Link: <...after=X>; rel="next"` until there is none. */
async function all(state) {
  const out = [], seen = new Set();
  let path = `${V1}/tasks?state=${state}&per_page=100`;
  for (;;) {
    const r = await api("GET", path, tok);
    if (r.status !== 200) throw new Error(`list ${state}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
    for (const x of r.body) {
      // A repeated id means the paging is wrong (the bug this script had with `page`): stop rather than loop.
      if (seen.has(x.id)) throw new Error(`task ${x.id} listed twice`);
      seen.add(x.id);
      out.push(x);
    }
    const next = /<([^>]+)>;\s*rel="next"/.exec(r.headers.link ?? "")?.[1];
    if (!next) return out;
    const u = new URL(next);
    path = `${V1}/tasks${u.search}${u.search.includes("state=") ? "" : `&state=${state}`}`;
  }
}

const OPEN = ["queued", "in_progress", "idle", "waiting_for_user"]; // the states that are not final
const openNow = async () => (await Promise.all(OPEN.map(all))).flat();
const open = await openNow();
log(`${open.length} open tasks on ${repo}`);
const codes = {};
if (flag !== "--dry") {
  for (const x of open) {
    const r = await api("POST", `${V1}/tasks/${x.id}/cancel`, tok, {});
    codes[r.status] = (codes[r.status] ?? 0) + 1;
  }
  log(`cancel: ${JSON.stringify(codes)}; open after: ${(await openNow()).length}`);
  const p = await api("POST", `${V1}/tasks`, tok, { kind: "external", session: `closetasks-probe-${Date.now()}`, summary: "probe" });
  log(`probe create: ${p.status}`);
  if (p.status === 201) log(`probe cancel: ${(await api("POST", `${V1}/tasks/${p.body.id}/cancel`, tok, {})).status}`);
}
