// LM-3's in-repo batched row on graft-m4a (`tests/am3/results/lm3-inrepo.md`): the lines, through graft-m4a-edge. The
// agents half (Apps, the controller with its `TaskBox`, `graft-agents-front`, the probe with `/mint`) is up first
// (`tests/am3/hosted/inrepo-row.sh`). The harness pushes twice under its task's prefix, then holds while
// `refs/heads/hold/<task>` exists on the base, so a line can keep a Graftyard-run task live.
//
//   node tests/am3/hosted/inrepo.mjs <line> <out dir> <repo name> [ctrl token file] [front url]
//
// Lines: repo | reader | l012 | b036 | lf9 | b037 | pause | a3 | l014 | l004 | teardown. Each appends JSON rows to
// <out>/<line>.jsonl and prints one `VERDICT <line> PASS|FAIL|INCOMPLETE <why>` line. No token is printed (`lib.mjs`'s
// `scan`). Tokens of graftyard-agent and graftyard-workspace come from the probe Worker's `/mint` (PROBE_URL, with
// ~/.graft-m4a/kill/probe.token), which signs the App JWT inside the Worker.
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, OP, accountShard, api, ghs, log, loginShard, must, pat, secret, sleep, state, tokens } from "../../am1/hosted/lib.mjs";

const [line, out, name] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const R = `m3org/${name}`, V3 = `/api/v3/repos/${R}`, V1 = `/api/graftyard/v1/repos/${R}`;
const admin = tokens.admin, member = tokens.member;
const AGENT_APP = 3801088, WS_APP = 3866624, INST = { agent: 7190003, workspace: 7190004 };
const BOT_LOGIN = "graftyard-agent[bot]";
const N = Number(process.env.INREPO_N ?? 100);
const K = `${process.env.HOME}/.graft-m4a/kill`;
const READER = { id: 7100011, login: "m3-reader" };
const READER_FILE = `${K}/reader.token`;
const rows = join(out, `${line}.jsonl`);
const ev = (v) => { appendFileSync(rows, JSON.stringify({ t: Date.now(), ...v }) + "\n"); log(JSON.stringify(v)); };
const fails = [], incomplete = [];
const check = (ok, what) => { ev({ check: what, ok }); if (!ok) fails.push(what); };
const gap = (what) => { ev({ incomplete: what }); incomplete.push(what); };
const verdict = () => {
  const v = fails.length ? "FAIL" : incomplete.length ? "INCOMPLETE" : "PASS";
  console.log(`VERDICT ${line} ${v} ${[...fails, ...incomplete.map((x) => `incomplete: ${x}`)].join("; ")}`);
  process.exit(fails.length ? 1 : 0);
};
const count = (xs) => xs.reduce((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {});
const all = (c, k) => Object.keys(c).length === 1 && Object.keys(c)[0] === k;
const short = (s) => String(s ?? "").replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, "<task>").replace(/[0-9A-Z]{26}/g, "<id>").replace(/\d{13}/g, "<ms>").slice(0, 90);
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });
async function until(what, f, ms, every = 5000) {
  const t = Date.now();
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() - t > ms) { ev({ timeout: what, ms }); return null; }
    await sleep(every);
  }
}

// ---- tokens through the probe ----
const PROBE = process.env.PROBE_URL;
async function mint(app, body = {}) {
  const r = await fetch(`${PROBE}/mint`, { method: "POST", signal: AbortSignal.timeout(60_000),
    headers: { "x-probe-token": readFileSync(`${K}/probe.token`, "utf8").trim(), "content-type": "application/json" },
    body: JSON.stringify({ app, installation: INST[app], body }) });
  const v = await r.json().catch(() => ({ status: r.status, body: null }));
  if (v.status === 201) secret(v.body.token);
  return v;
}
const tok = async (app, body) => { const m = await mint(app, body); if (m.status !== 201) throw new Error(`mint ${app}: ${m.status} ${short(m.body?.message)}`); return m.body.token; };

// ---- tasks ----
const task = async (id, who = admin) => {
  for (let i = 0; ; i++) {
    const r = await api("GET", `${V1}/tasks/${encodeURIComponent(id)}`, who);
    if (r.status === 200 || i >= 4) return r.status === 200 ? r.body : null;
    await sleep(2000);
  }
};
const clone = async (who = admin, repo = R) => {
  const g = new Git(repo).as(who);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
  return g;
};
const hold = async (g, id, on) => {
  g.as(admin);
  const r = on ? g.run(["push", "-q", "origin", `HEAD:refs/heads/hold/${id}`]) : g.run(["push", "-q", "origin", `:refs/heads/hold/${id}`]);
  ev({ hold: id, on, ok: r.ok });
  return r.ok;
};
// A Graftyard-run task created by m3-admin over the task API and held running: its box running and two pushes acked.
async function liveTask(g, summary) {
  const r = await api("POST", `${V1}/tasks`, admin, { prompt: `LM-3 in-repo ${summary}`, summary });
  ev({ create: summary, status: r.status, id: r.body?.id, state: r.body?.state });
  if (r.status !== 201) return null;
  const id = r.body.id;
  g.run(["fetch", "-q", "origin"]);
  g.run(["checkout", "-q", "--detach", "origin/main"]);
  await hold(g, id, true);
  const run = await until(`${summary} in_progress`, async () => (await task(id))?.state === "in_progress", 8 * 60_000);
  ev({ live: summary, id, in_progress: !!run });
  return run ? id : null;
}
const endTask = async (g, id) => { if (id) { await hold(g, id, false); ev({ cancel: id, status: (await api("POST", `${V1}/tasks/${encodeURIComponent(id)}/cancel`, admin)).status }); } };

// A commit on main's tip touching `path`, pushed by the admin under a scratch ref so a REST ref write can name it.
const commitAt = (g, path, k, scratch) => {
  g.as(admin);
  g.run(["checkout", "-q", "--detach", "origin/main"]);
  mkdirSync(join(g.wt, path, ".."), { recursive: true });
  writeFileSync(join(g.wt, path), `${k} ${Math.random()}\n`);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", `inrepo ${k}`]);
  // r1's l012 lost one scratch push (its ref write then named a commit the base never got): retry, and record a loss.
  for (let i = 0; scratch && i < 3; i++) {
    const p = g.run(["push", "-q", "origin", `HEAD:refs/heads/${scratch}`]);
    if (p.ok) break;
    ev({ scratch_push: scratch, try: i, error: short(p.out) });
  }
  return g.run(["rev-parse", "HEAD"]).out.trim();
};
const pushResult = (r) => (r.ok ? "accepted" : short(r.out.match(/refused: [^)\n]*/)?.[0] ?? r.out.match(/\(([^)]*)\)/)?.[1] ?? "rejected"));
const restResult = (r) => (r.status >= 200 && r.status < 300 ? "accepted" : `${r.status} ${short(r.body?.message)}`);

if (line === "repo") {
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
  ev({ create: R, status: made.status });
  if (made.status !== 201) process.exit(1);
  const g = await clone();
  ev({ main: g.run(["rev-parse", "HEAD"]).out.trim() });
} else if (line === "reader") {
  // m3-reader: a user who is not an org member, invited with read on the row's repository (it accepts), with a PAT.
  const { id, login } = READER;
  await must(`names:${accountShard(id)}`, { cred: OP, op: { PutAccount: { id, suspended: false, sessions_not_before: 0 } } });
  await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind: "User", account: id } } }, "Taken");
  await must(`names:${accountShard(id)}`, { cred: OP, op: { SetAccountLogin: { account: id, login } } });
  await must(`names:${accountShard(id)}`, { cred: OP, op: { InitPlacement: { owner: id, record: { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 } } } }, "RecordExists");
  await must(`authz:${id}`, { cred: OP, op: { Open: { kind: "User", enterprise: null } } }, "AlreadyOpened");
  await must(`identity:${id}`, { cred: OP, op: { Create: { login, kind: "User", created: Date.now() } } }, "AlreadyCreated");
  let t = existsSync(READER_FILE) ? secret(readFileSync(READER_FILE, "utf8").trim()) : null;
  if (!t) { t = await pat(id, "LM-3 in-repo reader"); writeFileSync(READER_FILE, t, { mode: 0o600 }); }
  const inv = await api("PUT", `${V3}/collaborators/${login}`, admin, { permission: "pull" });
  ev({ invite: inv.status });
  const list = await api("GET", `/api/v3/user/repository_invitations?repo=${R}`, t);
  const iid = (Array.isArray(list.body) ? list.body : []).find((x) => x.repository?.full_name === R)?.id;
  const acc = iid ? await api("PATCH", `/api/v3/user/repository_invitations/${iid}?repo=${R}`, t) : { status: "no invitation" };
  ev({ accept: acc.status });
  const perm = await api("GET", `${V3}/collaborators/${login}/permission`, admin);
  ev({ reader_permission: perm.body?.permission, role: perm.body?.role_name, status: perm.status });
  check(perm.body?.role_name === "read", "m3-reader reads the repository with read only");
} else if (line === "l012") {
  // L0-12, the Graftyard-run kind: m3-member holds an Exclusive lease on `l012/<tag>/**` through an external task of
  // m3-ci's (as the earlier kinds), under AgentsExclusive; a live Graftyard-run task's graftyard-workspace token then
  // sends N pushes and N REST ref writes under its own prefix whose changed paths fall in the scope.
  const tag = Date.now().toString(36);
  const g = await clone();
  const pol = await api("PUT", `${V1}/agents/policy`, admin, { leases: "agents_exclusive" });
  ev({ policy: pol.status, leases: pol.body?.leases });
  const ci = (await ghs({})).body?.token;
  await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, admin, { kind: "third_party", confine: "refs/heads/claude/**" });
  const ht = await api("POST", `${V1}/tasks`, ci, { kind: "external", session: `l012h-${tag}`, summary: "inrepo l012 holder", requester: "m3-member" });
  const held = await api("POST", `${V1}/leases`, member, { paths: [`l012/${tag}/**`], mode: "exclusive", ttl_seconds: 3600, intent: "L0-12 holder" },
    ht.body?.id ? { "x-graftyard-agent-session": ht.body.id } : {});
  ev({ holder_task: ht.status, holder_lease: held.status, lease: held.body?.lease?.id });
  check(pol.status === 200 && held.status === 201, "the policy and the holder's Exclusive lease");
  const id = await liveTask(g, "l012 pusher");
  check(!!id, "a live Graftyard-run task");
  if (id && held.status === 201) {
    const ws = await tok("workspace", { repositories: [name], graftyard_task: id });
    const pre = `refs/heads/graftyard/${id}`;
    const pushes = [], writes = [];
    for (let i = 0; i < N; i++) {
      const sha = commitAt(g, `l012/${tag}/w${i}.txt`, `w${i}`, `l012s-${tag}-${i}`);
      writes.push(restResult(await api("POST", `${V3}/git/refs`, ws, { ref: `${pre}/rest-${i}`, sha })));
      commitAt(g, `l012/${tag}/p${i}.txt`, `p${i}`);
      g.as(ws);
      pushes.push(pushResult(g.run(["push", "-q", "origin", `HEAD:${pre}/push-${i}`])));
    }
    const cp = count(pushes), cw = count(writes);
    ev({ kind: "Graftyard-run App (graftyard-workspace)", pushes: cp, writes: cw });
    const named = (c) => Object.keys(c).every((k) => k.includes("is leased Exclusive"));
    check(!cp.accepted && Object.values(cp).reduce((a, b) => a + b, 0) === N && named(cp), `${N} of ${N} pushes refused naming the lease, none NoLiveTask`);
    check(!cw.accepted && Object.values(cw).reduce((a, b) => a + b, 0) === N && named(cw), `${N} of ${N} REST ref writes refused naming the lease`);
  }
  await endTask(g, id);
  if (ht.body?.id) await api("POST", `${V1}/tasks/${encodeURIComponent(ht.body.id)}/cancel`, ci);
  ev({ policy_back: (await api("PUT", `${V1}/agents/policy`, admin, { leases: "advisory" })).status });
} else if (line === "b036") {
  // B0-36, Graftyard-run kind: the workspace token's REST ref write into another task's Exclusive scope (N), and a
  // push and a REST ref write after its task is cancelled (N each), with the shipped installation and widened.
  const tag = Date.now().toString(36);
  const g = await clone();
  await api("PUT", `${V1}/agents/policy`, admin, { leases: "agents_exclusive" });
  const ci = (await ghs({})).body?.token;
  const ht = await api("POST", `${V1}/tasks`, ci, { kind: "external", session: `b036h-${tag}`, summary: "inrepo b036 holder", requester: "m3-member" });
  const held = await api("POST", `${V1}/leases`, member, { paths: [`b036/${tag}/**`], mode: "exclusive", ttl_seconds: 3600 }, ht.body?.id ? { "x-graftyard-agent-session": ht.body.id } : {});
  ev({ holder: held.status });
  const reg = JSON.parse(readFileSync(`${K}/agents/agents-register.request.json`, "utf8"));
  const shipped = reg.find((x) => x.op.startsWith("PutApp") && x.cmd.op.PutApp.record.id === WS_APP).cmd.op.PutApp.record.permissions;
  const WIDE = { actions: "Write", administration: "Write", checks: "Write", contents: "Write", issues: "Write", metadata: "Read",
    pull_requests: "Write", statuses: "Write", workflows: "Write" };
  // As `tests/am1/runs/widen.mjs`: the App's record is widened first (an installation beyond its record is BadRecord),
  // then its installation; both go back to the shipped permissions after.
  const wsApp = reg.find((x) => x.op.startsWith("PutApp") && x.cmd.op.PutApp.record.id === WS_APP);
  const install = async (perms) => {
    await must(wsApp.chain, { ...wsApp.cmd, op: { PutApp: { ...wsApp.cmd.op.PutApp, record: { ...wsApp.cmd.op.PutApp.record, permissions: perms } } } });
    return must(`authz:${state.org}`, { cred: OP, op: { Install: { id: INST.workspace, row: { app: WS_APP, repos: "All", permissions: perms }, app_owner: 1 } } });
  };
  for (const [mode, perms] of [["shipped", null], ["widened", WIDE]]) {
    if (perms) await install(perms);
    const id = await liveTask(g, `b036 ${mode}`);
    if (!id) { check(false, `a live task (${mode})`); continue; }
    const ws = await tok("workspace", { repositories: [name], graftyard_task: id });
    const pre = `refs/heads/graftyard/${id}`;
    const lease = [];
    for (let i = 0; i < N; i++) {
      const sha = commitAt(g, `b036/${tag}/${mode}${i}.txt`, `${mode}${i}`, `b036s-${tag}-${mode}-${i}`);
      lease.push(restResult(await api("POST", `${V3}/git/refs`, ws, { ref: `${pre}/l-${i}`, sha })));
    }
    await endTask(g, id);
    await sleep(3000);
    const push = [], rest = [];
    for (let i = 0; i < N; i++) {
      const sha = commitAt(g, `b036/${tag}/c${mode}${i}.txt`, `c${mode}${i}`, `b036c-${tag}-${mode}-${i}`);
      rest.push(restResult(await api("POST", `${V3}/git/refs`, ws, { ref: `${pre}/c-${i}`, sha })));
      g.as(ws);
      push.push(pushResult(g.run(["push", "-q", "origin", `HEAD:${pre}/cp-${i}`])));
    }
    const [cl, cp, cr] = [count(lease), count(push), count(rest)];
    ev({ mode, lease_ref_write: cl, push_after_cancel: cp, rest_after_cancel: cr });
    check(!cl.accepted && Object.keys(cl).every((k) => k.includes("is leased Exclusive")), `${mode}: ${N} of ${N} ref writes into the scope refused naming the lease`);
    check(!cp.accepted && Object.values(cp).reduce((a, b) => a + b, 0) === N, `${mode}: ${N} of ${N} pushes after cancel refused`);
    check(!cr.accepted && Object.values(cr).reduce((a, b) => a + b, 0) === N, `${mode}: ${N} of ${N} REST ref writes after cancel refused`);
  }
  await install(shipped);
  ev({ installation_back: "shipped" });
  if (ht.body?.id) await api("POST", `${V1}/tasks/${encodeURIComponent(ht.body.id)}/cancel`, ci);
  await api("PUT", `${V1}/agents/policy`, admin, { leases: "advisory" });
  gap("the fork push after cancel waits for AM3-WP2a (lead's ruling)");
} else if (line === "lf9") {
  // LF-9 in-repo: a `graftyard_task` request from m3-ci naming the task (N), graftyard-workspace naming a repository
  // that is not the task's (N), and a workspace token pushing to the base outside its task's prefix (N).
  const g = await clone();
  const other = `${name}-other`;
  const mo = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: other, private: true, auto_init: true });
  ev({ other_repo: mo.status });
  const id = await liveTask(g, "lf9");
  check(!!id, "a live task");
  if (id) {
    const a = [], b = [], c = [];
    for (let i = 0; i < N; i++) a.push(String((await ghs({ repositories: [name], graftyard_task: id })).status));
    for (let i = 0; i < N; i++) b.push(String((await mint("workspace", { repositories: [other], graftyard_task: id })).status));
    const ws = await tok("workspace", { repositories: [name], graftyard_task: id });
    for (let i = 0; i < N; i++) { commitAt(g, `lf9/${i}.txt`, i); g.as(ws); c.push(pushResult(g.run(["push", "-q", "origin", `HEAD:refs/heads/lf9-${i}`]))); }
    const [ca, cb, cc] = [count(a), count(b), count(c)];
    ev({ third_app: ca, other_repo: cb, outside_prefix: cc });
    check(!ca["201"] && Object.values(ca).reduce((x, y) => x + y, 0) === N, `${N} of ${N} third-App task mints refused`);
    check(!cb["201"] && Object.values(cb).reduce((x, y) => x + y, 0) === N, `${N} of ${N} other-repository mints refused`);
    check(!cc.accepted && Object.values(cc).reduce((x, y) => x + y, 0) === N, `${N} of ${N} pushes outside the prefix refused`);
  }
  await endTask(g, id);
  ev({ delete_other: (await api("DELETE", `/api/v3/repos/m3org/${other}`, admin)).status });
} else if (line === "b037") {
  // B0-37 fetch half: the workspace token pushes refs/graftyard/sessions/<task> under its live task; then listings.
  const g = await clone();
  const id = await liveTask(g, "b037");
  check(!!id, "a live task");
  if (id) {
    const ref = `refs/graftyard/sessions/${id}`;
    const ws = await tok("workspace", { repositories: [name], graftyard_task: id });
    g.as(ws);
    const p = g.run(["push", "-q", "origin", `HEAD:${ref}`]);
    ev({ session_push: p.ok, out: p.ok ? undefined : short(p.out) });
    check(p.ok, "graftyard-workspace pushes its session ref under its live task");
    g.as(admin);
    const has = (o) => o.includes(ref);
    const lr = (proto, extra = []) => g.run(["-c", `protocol.version=${proto}`, "ls-remote", g.url, ...extra], g.dir);
    const seen = {
      v0: has(lr(0).out), v1: has(lr(1).out), v2: has(lr(2).out),
      v2_other_prefix: has(lr(2, ["refs/heads/"]).out),
      mirror: (() => { const d = join(g.dir, "m.git"); g.run(["clone", "-q", "--mirror", g.url, d], g.dir); return has(g.run(["for-each-ref"], d).out); })(),
      v2_matching_fetch: (() => { const r = g.run(["-c", "protocol.version=2", "fetch", "-q", "origin", `+${ref}:refs/remotes/s`]); return r.ok && g.run(["rev-parse", "refs/remotes/s"]).ok; })(),
    };
    ev({ seen });
    check(!seen.v0 && !seen.v1 && !seen.v2 && !seen.v2_other_prefix && !seen.mirror, "the session ref is hidden from v0, v1, v2 without a matching ref-prefix, and --mirror");
    check(seen.v2_matching_fetch, "a v2 fetch whose ref-prefix starts with the session ref gets it");
  }
  await endTask(g, id);
} else if (line === "pause") {
  // Pause and resume: an issue opened by m3-reader (read), assigned to the bot by m3-admin, its task running. The
  // controls first: a writer's edit pauses nothing, a reader's reassignment resumes nothing.
  const reader = secret(readFileSync(READER_FILE, "utf8").trim());
  const g = await clone();
  const iss = await api("POST", `${V3}/issues`, reader, { title: "LM-3 pause line", body: "please fix" });
  ev({ issue: iss.status, n: iss.body?.number, author: iss.body?.user?.login });
  const n = iss.body?.number;
  if (!n) { check(false, "m3-reader opens the issue"); verdict(); }
  const as = await api("POST", `${V3}/issues/${n}/assignees`, admin, { assignees: [BOT_LOGIN] });
  const tAssign = Date.now();
  ev({ assign: as.status });
  const t = await until("the controller's task", async () => {
    const r = await api("GET", `${V1}/tasks?issue_number=${n}&per_page=10`, admin);
    return (Array.isArray(r.body) ? r.body : []).find((x) => x.kind === "graftyard_run");
  }, 8 * 60_000);
  ev({ controller_create_ms: t ? Date.now() - tAssign : null, task: t?.id, created_at: t?.created_at });
  if (!t) { check(false, "the controller creates the task"); verdict(); }
  await hold(g, t.id, true);
  const running = await until("in_progress", async () => (await task(t.id))?.state === "in_progress", 8 * 60_000);
  check(!!running, "the task runs");
  const snap0 = (await api("GET", `${V1}/tasks/${encodeURIComponent(t.id)}`, admin)).body;
  // Control 1: a writer's edit.
  const we = await api("PATCH", `${V3}/issues/${n}`, admin, { body: "please fix (writer)" });
  await sleep(10_000);
  ev({ writer_edit: we.status, state_after: (await task(t.id))?.state });
  check(we.status === 200 && (await task(t.id))?.state === "in_progress", "a writer's edit pauses nothing");
  // The reader's edit.
  const re = await api("PATCH", `${V3}/issues/${n}`, reader, { body: "please fix, more detail" });
  const after = await task(t.id);
  ev({ reader_edit: re.status, state_in_reply_window: after?.state });
  check(re.status === 200 && after?.state === "waiting_for_user", "the base reads waiting_for_user right after the reader's edit");
  const ctrl = process.argv[5];
  const box = async () => {
    const r = await fetch(`${process.argv[6]}/_controller/box/${t.id}/status`,
      { headers: { "x-controller-token": secret(readFileSync(ctrl, "utf8").trim()) }, signal: AbortSignal.timeout(60_000) });
    return r.json().catch(() => null);
  };
  const t0 = Date.now();
  const st = await until("box stopped", async () => { const b = await box(); return b && b.running === false && b; }, 60_000, 3000);
  ev({ box_stopped_ms: st ? Date.now() - t0 : null, box_state: st?.box?.task?.state });
  check(!!st && Date.now() - t0 <= 30_000 + 3000, "the box stops its Sandbox within 30 s");
  const agent = await tok("agent", { repositories: [name] });
  const pa = await api("PATCH", `${V1}/tasks/${encodeURIComponent(t.id)}`, agent, { state: "in_progress" });
  ev({ app_patch: pa.status, message: short(pa.body?.message) });
  check(pa.status === 409, "the App's PATCH in_progress gets 409");
  const mt = await mint("workspace", { repositories: [name], graftyard_task: t.id });
  ev({ task_mint: mt.status, message: short(mt.body?.message) });
  check(mt.status === 422, "a graftyard_task mint gets 422");
  // Control 2: a reader's reassignment resumes nothing (GHES drops a reader's assignment silently).
  await api("DELETE", `${V3}/issues/${n}/assignees`, admin, { assignees: [BOT_LOGIN] });
  const rr = await api("POST", `${V3}/issues/${n}/assignees`, reader, { assignees: [BOT_LOGIN] });
  await sleep(10_000);
  ev({ reader_reassign: rr.status, state: (await task(t.id))?.state });
  check((await task(t.id))?.state === "waiting_for_user", "a reader's reassignment resumes nothing");
  // The writer's reassignment.
  const wr = await api("POST", `${V3}/issues/${n}/assignees`, admin, { assignees: [BOT_LOGIN] });
  const q = await task(t.id);
  ev({ writer_reassign: wr.status, state: q?.state });
  const resumed = await until("in_progress again", async () => (await task(t.id))?.state === "in_progress", 8 * 60_000);
  const b2 = await box();
  const list = await api("GET", `${V1}/tasks?issue_number=${n}&per_page=10`, admin);
  const runs = (Array.isArray(list.body) ? list.body : []).filter((x) => x.kind === "graftyard_run");
  ev({ resumed: !!resumed, sandbox: b2?.box?.task?.state?.Running?.sandbox, generation: b2?.box?.generation, tasks_for_issue: runs.map((x) => x.id) });
  check(["queued", "in_progress"].includes(q?.state), "after the writer's reassignment the same task reads queued (or already in_progress)");
  check(!!resumed && (b2?.box?.generation ?? 0) >= 2, "the same task runs again on a new Sandbox");
  check(runs.length === 1 && runs[0].id === t.id, "no second task exists");
  ev({ snapshot_note: "the base's task JSON carries no snapshot; the row reads it from the box generation and the TaskResumed entry (see results)", before: snap0?.updated_at });
  await endTask(g, t.id);
} else if (line === "a3") {
  // A3 end to end in-repo, without its draft step (F140, lead's ruling): an issue assigned to the bot by m3-admin, the
  // controller's task queued, then in_progress, two pushes, completed. Then a prompt-only task by m3-admin.
  const iss = await api("POST", `${V3}/issues`, admin, { title: "LM-3 A3", body: "do it" });
  const n = iss.body?.number;
  const seen = [];
  const watch = async (id, ms) => until(`${id} completed`, async () => {
    const s = (await task(id))?.state;
    if (s && seen.at(-1)?.[1] !== s) seen.push([id, s, Date.now()]);
    return s === "completed" || s === "failed" || s === "cancelled" ? s : null;
  }, ms, 3000);
  await api("POST", `${V3}/issues/${n}/assignees`, admin, { assignees: [BOT_LOGIN] });
  const t = await until("the controller's task", async () => {
    const r = await api("GET", `${V1}/tasks?issue_number=${n}&per_page=10`, admin);
    return (Array.isArray(r.body) ? r.body : []).find((x) => x.kind === "graftyard_run");
  }, 8 * 60_000, 2000);
  if (!t) { check(false, "the controller creates the task"); verdict(); }
  const end = await watch(t.id, 12 * 60_000);
  const states = seen.filter((x) => x[0] === t.id).map((x) => x[1]);
  const pushed = (await clone()).run(["ls-remote", "origin", `refs/heads/graftyard/${t.id}/work`]).out.trim();
  ev({ issue_task: t.id, states, end, work_ref: !!pushed, pr_number: (await task(t.id))?.pr_number });
  check(end === "completed" && states.join() .match(/^(queued,)?in_progress,completed$/) !== null, `the issue task reads queued, in_progress, completed in order (${states.join(",")})`);
  check(!!pushed, "the harness's pushes are under the task's prefix");
  gap("pr_number on a draft whose AgentMeta names the task: no product path opens it (F140, pending (b))");
  const p = await api("POST", `${V1}/tasks`, admin, { prompt: "LM-3 A3 prompt-only", summary: "a3 prompt" });
  ev({ prompt_create: p.status, id: p.body?.id });
  seen.length = 0;
  const end2 = p.body?.id ? await watch(p.body.id, 12 * 60_000) : null;
  const s2 = seen.map((x) => x[1]);
  ev({ prompt_task: p.body?.id, states: s2, end: end2 });
  check(end2 === "completed" && s2.join().match(/^(queued,)?in_progress,completed$/) !== null, `the prompt-only task completes the same way (${s2.join(",")})`);
} else if (line === "l004") {
  // L0-04, the read-only half (lead's ruling: "jobs that reference secrets wait" is F142, not built). graft-m4a's sites
  // set GRAFTYARD_PR_TRIGGER and not GRAFTYARD_PUSH_TRIGGER, so the agent push's run is the `pull_request` run of the
  // task's own branch (`refs/heads/graftyard/<task>/work`, which `agent_run` reads as an agent's). The workflow is on
  // main first, pushed by m3-admin: one job on graft-runners' hosted runner printing a repository secret's hash and
  // length, and what a ref write with its GITHUB_TOKEN answers.
  const { spawnSync } = await import("node:child_process");
  const { HOST, EDGE } = await import("../../am1/hosted/lib.mjs");
  const { createHash, randomBytes } = await import("node:crypto");
  const g = await clone();
  const value = secret(`l004-${randomBytes(18).toString("base64url")}`);
  mkdirSync(join(out, "ghcfg"), { recursive: true });
  const ghr = spawnSync("gh", ["secret", "set", "L004_SECRET", "-R", `${HOST}/${R}`], { input: value, encoding: "utf8",
    env: { ...process.env, GH_HOST: HOST, GH_ENTERPRISE_TOKEN: admin, GH_CONFIG_DIR: join(out, "ghcfg"), GH_PROMPT_DISABLED: "1", NO_COLOR: "1" } });
  const sl = await api("GET", `${V3}/actions/secrets`, admin);
  ev({ secret_set: ghr.status, gh: short(ghr.stderr), secrets: (sl.body?.secrets ?? []).map((x) => x.name) });
  check((sl.body?.secrets ?? []).some((x) => x.name === "L004_SECRET"), "the repository secret is set");
  g.as(admin);
  g.run(["fetch", "-q", "origin"]); g.run(["checkout", "-q", "--detach", "origin/main"]);
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, ".github/workflows/l004.yml"), [
    "on: pull_request", "jobs:", "  read:", "    runs-on: ubuntu-latest", "    steps:",
    "      - run: printf 'secret %s %s\\n' \"$(printf %s \"$S\" | sha256sum | cut -c1-16)\" \"${#S}\"",
    "        env:", "          S: ${{ secrets.L004_SECRET }}",
    "      - run: curl -s -o /dev/null -w 'token write %{http_code}\\n' -X POST -H \"authorization: token $T\" \"$GITHUB_API_URL/repos/$GITHUB_REPOSITORY/git/refs\" -d \"{\\\"ref\\\":\\\"refs/heads/l004-probe-$GITHUB_RUN_ID\\\",\\\"sha\\\":\\\"$GITHUB_SHA\\\"}\"",
    "        env:", "          T: ${{ github.token }}", ""].join("\n"));
  g.run(["add", "-A"]); g.run(["commit", "-q", "-m", "l004 workflow"]);
  ev({ workflow_main: g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok });
  const pol = await api("PUT", `${V1}/agents/policy`, admin, { ci: "auto_read_only" });
  ev({ policy: pol.status, ci: pol.body?.ci });
  const id = await liveTask(g, "l004");
  check(!!id, "a live task that pushes");
  const head = `graftyard/${id}/work`;
  const pushed = id ? await until("the task's branch", async () => (await api("GET", `${V3}/git/ref/heads/${head}`, admin)).status === 200, 5 * 60_000, 3000) : null;
  const tOpen = Date.now();
  const pr = pushed ? (await gql(admin, "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    { i: { repositoryId: (await gql(admin, `query { repository(owner: "m3org", name: "${name}") { id } }`)).body?.data?.repository?.id, baseRefName: "main", headRefName: head, title: "l004", body: "l004" } })).body?.data?.createPullRequest?.pullRequest : null;
  ev({ branch: head, pushed: !!pushed, pr: pr?.number });
  const run = pr ? await until("the run", async () => ((await api("GET", `${V1}/actions/runs`, admin)).body?.runs ?? []).find((x) => x.head_ref === `refs/pull/${pr.number}/merge`), 3 * 60_000, 2000) : null;
  const tRun = Date.now();
  const jobs = async () => (await api("GET", `${V3}/actions/runs/${run.id}/jobs`, admin)).body?.jobs?.[0];
  const job = run ? await until("job started", async () => { const j = await jobs(); return j?.started_at && j.status !== "queued" ? j : null; }, 10 * 60_000, 2000) : null;
  const tJob = Date.now();
  const row = run ? (await api("GET", `${V1}/actions/runs/${run.id}`, admin)).body : null;
  ev({ run: run?.id, run_created: run?.created_at, fork: row?.value?.run?.run?.fork, job: job?.id, started_at: job?.started_at,
    open_to_run_ms: run ? tRun - tOpen : null, open_to_job_seen_ms: job ? tJob - tOpen : null,
    run_to_job_ms: job ? Date.parse(job.started_at) - Date.parse(run.created_at) : null });
  const done = job ? await until("job completed", async () => { const j = await jobs(); return j?.status === "completed" ? j : null; }, 10 * 60_000, 5000) : null;
  const lr = done ? await fetch(`${EDGE}${V3}/actions/jobs/${done.id}/logs`, { headers: { authorization: `token ${admin}` }, signal: AbortSignal.timeout(60_000) }) : null;
  const text = lr ? await lr.text() : "";
  writeFileSync(join(out, "l004-job.log"), text.replaceAll(value, "<secret>"));
  const emptyHash = createHash("sha256").update("").digest("hex").slice(0, 16);
  const realHash = createHash("sha256").update(value).digest("hex").slice(0, 16);
  const m = text.match(/secret ([0-9a-f]{16}) (\d+)/), w = text.match(/token write (\d{3})/);
  ev({ log: lr?.status, secret_line: m?.[0], secret_is_empty: m?.[1] === emptyHash, token_write: w?.[1], conclusion: done?.conclusion });
  check(row?.value?.run?.run?.fork === true, "the agent branch's run is marked fork");
  check(!!job && Date.parse(job.started_at) - Date.parse(run.created_at) <= 30_000, "the job starts within 30 s of its run");
  check(!!m && m[1] === emptyHash && m[2] === "0" && !text.includes(value) && !text.includes(realHash), "the job sees no secret (the empty value's hash, length 0)");
  check(!!w && w[1] !== "201", "the job's token cannot write (read-only)");
  gap("jobs that reference secrets wait: not built (F142, LM-1 builds it; lead's ruling)");
  if (run) ev({ cancel_run: (await api("POST", `${V3}/actions/runs/${run.id}/cancel`, admin)).status });
  await endTask(g, id);
  ev({ policy_back: (await api("PUT", `${V1}/agents/policy`, admin, { ci: "approve" })).status });
} else if (line === "l014") {
  // L0-14 as `tests/m0/run/agent-l014.ts` writes it, on graft-m4a: every bad case N times with `agents_enabled` off,
  // then on. Accepted Graftyard-run creates each start a Sandbox, so the accepted controls are a sample (PREFIX_N,
  // each cancelled at once) and every create and cancel the row makes is timed at the controller through `wrangler
  // tail`. A kind this install cannot mint is a gap: the line is then incomplete, not passed (lead's ruling).
  const { spawn } = await import("node:child_process");
  const { EDGE, appJwt } = await import("../../am1/hosted/lib.mjs");
  const PREFIX_N = Number(process.env.L014_PREFIX_N ?? 20);
  const tail = spawn("npx", ["--prefix", "tests/m0", "wrangler", "tail", "graft-agents-m4a-controller", "--format", "json"],
    { env: { ...process.env, CLOUDFLARE_ACCOUNT_ID: process.env.CLOUDFLARE_RUNNER_ACCOUNT_ID, CLOUDFLARE_API_TOKEN: process.env.CLOUDFLARE_RUNNER_API_TOKEN }, stdio: ["ignore", "pipe", "pipe"] });
  let tailBuf = "";
  tail.stdout.on("data", (d) => { tailBuf += d; });
  tail.stderr.on("data", () => {});
  await sleep(15_000);
  const reader = secret(readFileSync(READER_FILE, "utf8").trim());
  const ci = (await ghs({})).body?.token;
  const agentTok = await tok("agent", { repositories: [name] });
  await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, admin, { kind: "third_party", confine: "refs/heads/claude/**" });
  const ghu = await (async () => {
    const SECRET = secret(readFileSync(`${process.env.M3_STATE}/app.secret`, "utf8").trim());
    const q = new URLSearchParams({ client_id: state.client, redirect_uri: "http://localhost/cb", state: "s" });
    const r = await fetch(`${EDGE}/login/oauth/authorize?${q}`, { redirect: "manual", headers: { authorization: `token ${member}` }, signal: AbortSignal.timeout(60_000) });
    const code = new URL(r.headers.get("location") ?? "http://x/").searchParams.get("code");
    if (!code) return ev({ ghu: `authorize ${r.status}` });
    const t = await fetch(`${EDGE}/login/oauth/access_token`, { method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ client_id: state.client, client_secret: SECRET, code }).toString(), signal: AbortSignal.timeout(60_000) });
    const v = await t.json().catch(() => ({}));
    return v.access_token ? secret(v.access_token) : ev({ ghu: `access_token ${t.status} ${short(v.error)}` });
  })();
  ev({ agentapp_user_token: !!ghu });
  if (!ghu) gap("an AgentApp's user token could not be minted on graft-m4a");
  gap("an MCP token: graft-m4a runs no graft-mcp App and its edge has neither 17.4 nor 20.4, so none can be minted");
  gap("graftyard-insights' installation token: skipped (lead's ruling)");
  // Issues: #ia assigned to graftyard-agent's bot (while off, so the controller's own create from it is refused),
  // #ib assigned to m3-ci's bot when the edge accepts that assignment.
  const ia = (await api("POST", `${V3}/issues`, admin, { title: "l014 assigned to graftyard-agent" })).body?.number;
  const ib = (await api("POST", `${V3}/issues`, admin, { title: "l014 assigned to another bot" })).body?.number;
  const slug = (await api("GET", "/api/v3/app", null, undefined, { authorization: `Bearer ${appJwt()}` })).body?.slug;
  const off = await api("PATCH", "/api/graftyard/v1/orgs/m3org/agents", admin, { agents_enabled: false });
  ev({ agents_off: off.status, enabled: off.body?.agents_enabled });
  const aa = await api("POST", `${V3}/issues/${ia}/assignees`, admin, { assignees: [BOT_LOGIN] });
  const ab = await api("POST", `${V3}/issues/${ib}/assignees`, admin, { assignees: [`${slug}[bot]`] });
  const other = (ab.body?.assignees ?? []).map((x) => x.login);
  ev({ ia, ib, assign_ours: aa.status, assign_other: ab.status, other_bot: `${slug}[bot]`, ib_assignees: other });
  if (!other.includes(`${slug}[bot]`)) gap(`another bot's assignment: the edge did not assign ${slug}[bot] (${ab.status}); #ib has no bot assignment`);
  const demote = async () => {
    let n = 0;
    await Promise.all(Array.from({ length: 16 }, async () => { while (n < 64) { n++; await api("POST", `${V3}/issues/${ia}/comments`, member, { body: `ping ${n}` }); } }));
  };
  const SESS = "graftyard_run.00000000-0000-4000-8000-000000000001";
  const creators = [
    ["installation token (m3-ci, an AgentApp)", () => ci, () => ({ prompt: "p" })],
    ...(ghu ? [["AgentApp's user token (m3-ci for m3-member)", () => ghu, () => ({ prompt: "p" })]] : []),
    ["demoted user (m3-member past DEMOTE_PER_MIN)", () => member, () => ({ prompt: "p" })],
    ["read-only user (m3-reader)", () => reader, () => ({ prompt: "p" })],
    ["user with X-Graftyard-Agent-Session (m3-admin)", () => admin, () => ({ prompt: "p" }), { "x-graftyard-agent-session": SESS }],
    ["controller from an issue assigned to another bot", () => agentTok, () => ({ issue_number: ib })],
    ["controller with a prompt and no issue", () => agentTok, () => ({ prompt: "p" })],
  ];
  const cells = {};
  const cell = (k, st, ok) => { cells[k] ??= { statuses: {}, off: 0 }; cells[k].statuses[st] = (cells[k].statuses[st] ?? 0) + 1; if (!ok) cells[k].off++; };
  const runCase = async (phase, [k, t, body, headers], want) => {
    if (k.startsWith("demoted")) await demote();
    for (let i = 0; i < N; i++) {
      if (k.startsWith("demoted") && i % 25 === 24) await demote();
      const r = await api("POST", `${V1}/tasks`, t(), body(i), headers ?? {});
      cell(`${phase}/${k}`, `${r.status} ${short(r.body?.message)}`, want(r.status));
    }
    const c = cells[`${phase}/${k}`];
    ev({ cell: `${phase}/${k}`, ...c });
    check(c.off === 0, `${phase}: ${k}, ${N} of ${N} refused`);
  };
  for (const c of [...creators, ["human writer (m3-admin), the valid creator", () => admin, () => ({ prompt: "p" })],
    ["controller from an issue assigned to its bot", () => agentTok, () => ({ issue_number: ia })]]) await runCase("off", c, (s) => s === 422);
  const on = await api("PATCH", "/api/graftyard/v1/orgs/m3org/agents", admin, { agents_enabled: true, workspace: "in_repo" });
  ev({ agents_on: on.status, enabled: on.body?.agents_enabled });
  for (const c of creators) await runCase("on", c, (s) => s === 403 || s === 422);
  // A body that sets `prefix` (accepted, the prefix derived from the id), cancelled at once; and one that sets
  // `session` for a Graftyard-run task (refused).
  const timed = [];
  const prefixes = [];
  for (let i = 0; i < PREFIX_N; i++) {
    const r = await api("POST", `${V1}/tasks`, admin, { prompt: `l014 prefix ${i}`, prefix: "refs/heads/main/" });
    prefixes.push(r.status === 201 && r.body.prefix === `refs/heads/graftyard/${r.body.id}/`);
    if (r.status === 201) {
      timed.push({ id: r.body.id, action: "created", t: Date.now() });
      const c = await api("POST", `${V1}/tasks/${encodeURIComponent(r.body.id)}/cancel`, admin);
      if (c.status === 200) timed.push({ id: r.body.id, action: "cancelled", t: Date.now() });
    }
  }
  ev({ prefix_sample: PREFIX_N, derived: prefixes.filter(Boolean).length });
  check(prefixes.every(Boolean), `a body that sets prefix: ${PREFIX_N} of ${PREFIX_N} accepted with the prefix derived from the id`);
  await runCase("on", ["body sets session for a Graftyard-run task", () => admin, (i) => ({ prompt: "p", session: `mine-${i}` })], (s) => s === 422);
  // 101 open tasks for one requester. The cap counts every kind per requester (`task::open_counts`), so m3-ci's
  // External tasks for m3-member fill it without Sandboxes; the 101st is tried as External and as Graftyard-run.
  await sleep(65_000);
  const tag = Date.now().toString(36);
  const fill = [];
  for (let i = 0; i < 100; i++) fill.push((await api("POST", `${V1}/tasks`, ci, { kind: "external", session: `cap-${tag}-${i}`, summary: "cap", requester: "m3-member" })).status);
  ev({ cap_fill: count(fill.map(String)) });
  check(fill.every((s) => s === 201), "m3-member's first 100 open tasks (control)");
  await runCase("on", ["101st open task for one requester (External, m3-ci for m3-member)", () => ci, (i) => ({ kind: "external", session: `cap-${tag}-x${i}`, requester: "m3-member" })], (s) => s === 422);
  await runCase("on", ["101st open task for one requester (Graftyard-run, m3-member)", () => member, () => ({ prompt: "the 101st" })], (s) => s === 422);
  // PATCH of a cancelled task to in_progress by its App: a Graftyard-run task (graftyard-agent) and an External one
  // (m3-ci), each cancelled by m3-admin.
  const g = await clone();
  const live = await liveTask(g, "l014 live");
  const gr = await api("POST", `${V1}/tasks`, admin, { prompt: "l014 to cancel" });
  if (gr.status === 201) timed.push({ id: gr.body.id, action: "created", t: Date.now() });
  const grc = await api("POST", `${V1}/tasks/${encodeURIComponent(gr.body?.id)}/cancel`, admin);
  if (grc.status === 200) timed.push({ id: gr.body.id, action: "cancelled", t: Date.now() });
  const xc = await api("POST", `${V1}/tasks/${encodeURIComponent(`${state.app}.cap-${tag}-0`)}/cancel`, admin);
  ev({ cancelled_run: grc.status, cancelled_external: xc.status, live });
  const patchCase = async (k, id, t, body) => {
    for (let i = 0; i < N; i++) { const r = await api("PATCH", `${V1}/tasks/${encodeURIComponent(id)}`, t, body); cell(`on/${k}`, `${r.status} ${short(r.body?.message)}`, r.status >= 400); }
    ev({ cell: `on/${k}`, ...cells[`on/${k}`] });
    check(cells[`on/${k}`].off === 0, `on: ${k}, ${N} of ${N} refused`);
  };
  await patchCase("PATCH of a cancelled Graftyard-run task to in_progress (graftyard-agent)", gr.body?.id, agentTok, { state: "in_progress" });
  await patchCase("PATCH of a cancelled External task to in_progress (m3-ci)", `${state.app}.cap-${tag}-0`, ci, { state: "in_progress" });
  if (live) await patchCase("PATCH state cancelled (host-only) on a live task", live, agentTok, { state: "cancelled" });
  if (live) await patchCase("PATCH of graftyard-agent's task by another App (m3-ci)", live, ci, { state: "in_progress" });
  // Token requests. Controls: graftyard-agent and graftyard-workspace naming the live task with the task's repository.
  const mintCase = async (k, f, want) => {
    for (let i = 0; i < N; i++) { const r = await f(); cell(`on/${k}`, `${r.status} ${short(r.body?.message)}`, want(r.status)); }
    ev({ cell: `on/${k}`, ...cells[`on/${k}`] });
    check(cells[`on/${k}`].off === 0, `on: ${k}, ${N} of ${N} as expected`);
  };
  if (live) {
    await mintCase("mint naming another App's task (m3-ci's JWT)", () => ghs({ repositories: [name], graftyard_task: live }), (s) => s >= 400);
    await mintCase("mint with a repository outside the task's", () => mint("agent", { repositories: [name, "b035-r1"], graftyard_task: live }), (s) => s >= 400);
    await mintCase("mint with every repository of the installation", () => mint("agent", { graftyard_task: live }), (s) => s >= 400);
    await mintCase("mint naming an External task", () => ghs({ repositories: [name], graftyard_task: `${state.app}.cap-${tag}-1` }), (s) => s >= 400);
    await mintCase("mint naming a cancelled task", () => mint("workspace", { repositories: [name], graftyard_task: gr.body?.id }), (s) => s >= 400);
    await mintCase("mint: graftyard-workspace on the live task (control)", () => mint("workspace", { repositories: [name], graftyard_task: live }), (s) => s === 201);
    await mintCase("mint: graftyard-agent on the live task (control)", () => mint("agent", { repositories: [name], graftyard_task: live }), (s) => s === 201);
  }
  const lc = live ? await api("POST", `${V1}/tasks/${encodeURIComponent(live)}/cancel`, admin) : null;
  if (lc?.status === 200) timed.push({ id: live, action: "cancelled", t: Date.now() });
  if (live) await hold(g, live, false);
  for (let i = 1; i < 100; i++) await api("POST", `${V1}/tasks/${encodeURIComponent(`${state.app}.cap-${tag}-${i}`)}/cancel`, admin);
  // Deliveries at the controller: each create's `started` and each cancel's `cancelled` line, against the driver's
  // clock at the API's answer (both clocks NTP; the gap is an upper bound on the delivery's latency plus skew).
  await sleep(20_000);
  tail.kill("SIGTERM");
  const events = tailBuf.split(/\n(?=\{)/).map((x) => { try { return JSON.parse(x); } catch { return null; } }).filter(Boolean);
  writeFileSync(join(out, "l014-tail.jsonl"), events.map((e) => JSON.stringify({ t: e.eventTimestamp, logs: (e.logs ?? []).map((l) => [l.timestamp, String(l.message?.[0] ?? "").slice(0, 160)]) })).join("\n") + "\n");
  const lines = events.flatMap((e) => (e.logs ?? []).map((l) => ({ at: l.timestamp, m: String(l.message?.[0] ?? "") })));
  const lat = [];
  let missing = 0;
  for (const e of timed) {
    const re = e.action === "created" ? new RegExp(`task ${e.id} (started|base)`) : new RegExp(`task ${e.id} cancelled`);
    const hit = lines.filter((l) => re.test(l.m)).sort((a, b) => a.at - b.at)[0];
    if (!hit) { missing++; continue; }
    lat.push(hit.at - e.t);
  }
  lat.sort((a, b) => a - b);
  const hookOther = await api("GET", "/api/v3/app/hook/config", null, undefined, { authorization: `Bearer ${appJwt()}` });
  ev({ deliveries: { expected: timed.length, at_controller: lat.length, missing, late: lat.filter((x) => x > 5000).length,
    p50: lat[Math.floor(lat.length / 2)], max: lat.at(-1), tail_events: events.length }, m3ci_hook: hookOther.status, m3ci_hook_url: hookOther.body?.url ? "set" : "none" });
  check(timed.length > 0 && missing === 0 && lat.every((x) => x <= 5000), "every create and cancel at the controller within 5 s");
  check(!hookOther.body?.url, "no other App's webhook can receive a task delivery (m3-ci has none)");
  writeFileSync(join(out, "l014-cells.json"), JSON.stringify(cells, null, 1));
} else if (line === "teardown") {
  const d = await api("DELETE", V3, admin);
  ev({ delete: R, status: d.status });
  ev({ policy: (await api("PUT", `${V1}/agents/policy`, admin, { leases: "advisory", ci: "approve" })).status });
  if (existsSync(READER_FILE)) {
    const t = secret(readFileSync(READER_FILE, "utf8").trim());
    const h = (await import("node:crypto")).createHash("sha256").update("graftyard/token/").update(t.slice(4, 34)).digest();
    await must(`tokens:${h.readUInt32BE(0) % 3072}`, { cred: OP, op: { Revoke: { hash: [...h] } } }, "NoToken", "AlreadyRevoked");
    ev({ reader_pat: "revoked", after: (await api("GET", "/api/v3/user", t)).status });
    writeFileSync(READER_FILE, "", { mode: 0o600 });
  }
  await must(`names:${accountShard(READER.id)}`, { cred: OP, op: { PutAccount: { id: READER.id, suspended: true, sessions_not_before: Date.now() } } });
  ev({ reader: "suspended" });
} else {
  console.error("line: repo reader l012 b036 lf9 b037 pause a3 l014 l004 teardown");
  process.exit(2);
}
if (!["repo", "teardown"].includes(line)) verdict();
