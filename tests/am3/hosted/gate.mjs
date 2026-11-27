// LM-3's row A on graft-m4a (`tests/am3/results/lm3-gate.md`, "Row A"; the lead's GO, 2026-11-26): L0-03's token
// scopes, after the L0-02 phase (`gate-row.sh l002`, which runs the adapted harness in a Sandbox). Run by
// `tests/am3/hosted/gate-row.sh`, which owns the agents half, the probe and the teardown.
//
//   node tests/am3/hosted/gate.mjs repos <tag>                 m3org/gate-<tag> and m3org/gate-other-<tag>, each with a
//        first commit, and m3-admin's pull request (`gate-pr-<tag>` into main) that the merge cells target; `pr <tag>`
//        makes only the pull request, on repositories that exist
//   node tests/am3/hosted/gate.mjs task <name> <label>         one prompt-only Graftyard-run task by m3-admin; prints its
//        id and prefix as JSON
//   node tests/am3/hosted/gate.mjs hold <name> <id>|release <name> <id>    `refs/heads/hold/<id>` on the base, which
//        the hold-mode harness waits on (AM2's `harness.py`), created or deleted by m3-admin
//   node tests/am3/hosted/gate.mjs complete <name> <id>       waits until the task is final (its harness reports
//        Finished once its hold is gone); exit 1 unless it ended `completed`
//   node tests/am3/hosted/gate.mjs cancel <name> <id>         the task cancelled by m3-admin, its requester
//   node tests/am3/hosted/gate.mjs results <name> <id> <out>  the L0-02 harness's class files from the task's
//        `results` branch, into <out>/l002-classes.json; exit 1 until the branch exists
//   node tests/am3/hosted/gate.mjs l003 <out> <tag> <pass> <probe url> <probe token file> <tasks.json>
//        the L0-03 cells, 100 of each over git and REST, the merge cells over every surface, the unserved surfaces
//        10 times each to record their answer, and the 10 controls; one JSON row per attempt in <out>/l003-<pass>.jsonl,
//        the verdict's inputs in <out>/l003-<pass>.json. <pass> is `shipped` or `widened`.
//   node tests/am3/hosted/gate.mjs widen|restore              both Apps' records and installations widened as B0-36's
//        widened pass widens them (`WIDE`), or back to their shipped permissions
//   node tests/am3/hosted/gate.mjs delete <tag>               both repositories deleted
//
// Tokens come only from the probe's `POST /mint` (AM2's, `{app, installation, body}` under `x-probe-token`), are held in
// memory, are registered with `lib.mjs`'s `secret` so `scan` throws if one reaches a log line, and are never written.
import { readFileSync, writeFileSync, appendFileSync, mkdirSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { EDGE, HOST, OP, api, log, must, sleep, state, tokens, secret, scan } from "../../am1/hosted/lib.mjs";

const [step, ...args] = process.argv.slice(2);
const admin = tokens.admin;
const N = Number(process.env.GATE_N ?? 100);
const UNSERVED_N = 10;
// The two installations kill.mjs installs (`INST`): graftyard-agent and graftyard-workspace on m3org.
const INST = { agent: 7190003, workspace: 7190004 };
// B0-36's widened set (`tests/am3/hosted/inrepo.mjs`, `b036`).
const WIDE = { actions: "Write", administration: "Write", checks: "Write", contents: "Write", issues: "Write", metadata: "Read",
  pull_requests: "Write", statuses: "Write", workflows: "Write" };
// Sets both Apps' records and their installations to `perms`, or back to the records' shipped permissions when null,
// as B0-36's widened pass does (an installation beyond its App's record is BadRecord, so the record goes first).
async function installAs(perms) {
  const reg = JSON.parse(readFileSync(`${process.env.HOME}/.graft-m4a/gate/agents/agents-register.request.json`, "utf8"));
  for (const [app, inst] of [["graftyard-agent", INST.agent], ["graftyard-workspace", INST.workspace]]) {
    const put = reg.find((x) => x.op.startsWith("PutApp") && x.cmd.op.PutApp.record.slug === app)
      ?? reg.find((x) => x.op.startsWith("PutApp") && x.op.includes(app));
    const rec = put.cmd.op.PutApp.record;
    const p = perms ?? rec.permissions;
    await must(put.chain, { ...put.cmd, op: { PutApp: { ...put.cmd.op.PutApp, record: { ...rec, permissions: p } } } });
    await must(`authz:${state.org}`, { cred: OP, op: { Install: { id: inst, row: { app: rec.id, repos: "All", permissions: p }, app_owner: 1 } } });
    log(`${app}: record and installation ${perms ? "widened" : "shipped"}`);
  }
}

const repo = (tag) => `gate-${tag}`;
// A fresh git directory acting as m3-admin on m3org/<name>; `g.url` carries the token as a Basic password, which git
// never prints. Throws on a failed command, with the token masked.
const adminGit = (name) => {
  const dir = `${process.env.TMPDIR ?? "/tmp"}/gate-admin-${name}-${process.pid}`;
  rmSync(dir, { recursive: true, force: true }); mkdirSync(dir, { recursive: true });
  const g = (argv) => {
    try {
      return execFileSync("git", argv, { cwd: dir, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" }, stdio: ["ignore", "pipe", "pipe"], timeout: 120_000 }).toString();
    } catch (e) {
      throw new Error(`git ${argv[0]}: ${`${e.stderr ?? ""}`.replace(/https:\/\/[^@\s]*@/g, "https://<token>@").slice(-300)}`);
    }
  };
  g.url = `https://x-access-token:${admin}@${HOST}/m3org/${name}.git`;
  return g;
};
const other = (tag) => `gate-other-${tag}`;

if (step === "repos" || step === "pr") {
  const [tag] = args;
  for (const name of step === "pr" ? [] : [repo(tag), other(tag)]) {
    const r = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
    log(`create m3org/${name}: ${r.status}`);
    if (r.status !== 201) process.exit(1);
  }
  // m3-admin's pull request: a branch with one commit, opened into main, which A's agent token tries to merge. The edge
  // serves no REST ref read, so the branch is made over git (as LM-3's in-repo row makes its refs).
  const g = adminGit(repo(tag));
  g(["clone", "-q", g.url, "."]);
  g(["-c", "user.email=row-a@graftyard.invalid", "-c", "user.name=row-a", "commit", "-q", "--allow-empty", "-m", "row A merge target"]);
  const sha = g(["rev-parse", "HEAD"]).trim();
  g(["push", "-q", "--force", "origin", `HEAD:refs/heads/gate-pr-${tag}`]);
  log(`branch gate-pr-${tag} at ${sha.slice(0, 7)}`);
  // The edge serves the create over GraphQL only (as LM-3's batched row opens its pull requests).
  const id = (await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${repo(tag)}") { id } }` })).body?.data?.repository?.id;
  const pr = await api("POST", "/api/graphql", admin, {
    query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    variables: { i: { repositoryId: id, baseRefName: "main", headRefName: `gate-pr-${tag}`, title: "row A merge target", body: "row A" } },
  });
  const number = pr.body?.data?.createPullRequest?.pullRequest?.number ?? null;
  log(`pull request: ${pr.status} #${number ?? "-"} ${JSON.stringify(pr.body?.errors?.[0]?.message ?? "")}`);
  writeFileSync(`tests/am3/raw/gate-${tag}/pr.json`, JSON.stringify({ number, head: `gate-pr-${tag}`, sha }));
  if (number === null) process.exit(1);
} else if (step === "task") {
  const [name, label] = args;
  const r = await api("POST", `/api/graftyard/v1/repos/m3org/${name}/tasks`, admin, { prompt: `LM-3 row A ${label}`, summary: `row A ${label}` });
  log(`task ${label}: ${r.status} ${r.body?.id ?? JSON.stringify(r.body)}`);
  if (r.status !== 201) process.exit(1);
  console.log(JSON.stringify({ label, id: r.body.id, prefix: r.body.prefix ?? `refs/heads/graftyard/${r.body.id}/` }));
} else if (step === "hold" || step === "release") {
  const [name, id] = args;
  const g = adminGit(name);
  g(["init", "-q", "."]);
  if (step === "hold") {
    g(["fetch", "-q", g.url, "refs/heads/main"]);
    g(["push", "-q", g.url, `FETCH_HEAD:refs/heads/hold/${id}`]);
  } else {
    g(["push", "-q", g.url, `:refs/heads/hold/${id}`]);
  }
  log(`${step} hold/${id}: ok`);
} else if (step === "complete" || step === "cancel") {
  // `completed` is the task's App's to report (`IssueOp::UpdateTask`): the harness reports `Finished` once its hold is
  // released, so `complete` waits for a final state; `cancel` is m3-admin's (the requester), for cleanup.
  const [name, id] = args;
  if (step === "cancel") {
    const r = await api("POST", `/api/graftyard/v1/repos/m3org/${name}/tasks/${id}/cancel`, admin);
    log(`cancel ${id}: ${r.status} ${r.body?.state ?? JSON.stringify(r.body?.message ?? null)}`);
  } else {
    let state = null;
    for (let i = 0; i < 60; i++) {
      state = (await api("GET", `/api/graftyard/v1/repos/m3org/${name}/tasks/${id}`, admin)).body?.state;
      if (["completed", "failed", "cancelled", "timed_out"].includes(state)) break;
      await sleep(10_000);
    }
    log(`complete ${id}: ${state}`);
    if (state !== "completed") process.exit(1);
  }
} else if (step === "results") {
  // The L0-02 harness's class files, from `refs/heads/graftyard/<id>/results` on the base (attempts.py pushes them
  // through the host arm); exit 1 while the ref does not exist yet.
  // The edge serves no REST `contents` read, so a git fetch of the ref, as m3-admin.
  const [name, id, out] = args;
  const ref = `refs/heads/graftyard/${id}/results`;
  const g = adminGit(name);
  try {
    g(["init", "-q", "."]);
    g(["fetch", "-q", g.url, `${ref}:refs/r`]);
  } catch { log(`results ${ref}: not there yet`); process.exit(1); }
  const classes = {};
  for (const f of g(["ls-tree", "-r", "--name-only", "refs/r", "out/"]).split("\n").filter((p) => p.endsWith(".json"))) {
    classes[f.replace(/^out\//, "").replace(/\.json$/, "")] = JSON.parse(g(["show", `refs/r:${f}`]));
  }
  writeFileSync(`${out}/l002-classes.json`, JSON.stringify(classes, null, 1));
  log(`results ${ref}: ${Object.keys(classes).length} files`);
} else if (step === "delete") {
  const [tag] = args;
  for (const name of [repo(tag), other(tag)]) {
    const d = await api("DELETE", `/api/v3/repos/m3org/${name}`, admin);
    log(`delete m3org/${name}: ${d.status}`);
  }
} else if (step === "l003") {
  const [out, tag, pass, probeUrl, probeTokenFile, tasksFile] = args;
  const ptoken = secret(readFileSync(probeTokenFile, "utf8").trim());
  const tasks = JSON.parse(readFileSync(tasksFile, "utf8"));
  const pr = JSON.parse(readFileSync(`${out}/pr.json`, "utf8"));
  const base = repo(tag);
  const rows = `${out}/l003-${pass}.jsonl`;
  const ev = (v) => appendFileSync(rows, scan(JSON.stringify({ t: Date.now(), pass, ...v })) + "\n");
  // `widened` runs after `widen` (both Apps' records and installations as B0-36's widened pass sets them), so each mint
  // gets the installation's whole widened set. The router never mints these: the probe does, as an attacker holding
  // the key would.
  const perms = undefined;
  const mint = async (app, task) => {
    const body = { repositories: [base], ...(task ? { graftyard_task: task } : {}), ...(perms ? { permissions: perms } : {}) };
    const r = await fetch(`${probeUrl}/mint`, {
      method: "POST", headers: { "x-probe-token": ptoken, "content-type": "application/json" },
      body: JSON.stringify({ app, installation: INST[app], body }), signal: AbortSignal.timeout(60_000),
    });
    const v = await r.json().catch(() => null);
    const tok = v?.body?.token;
    if (r.status !== 200 || (v?.status !== 201 && v?.status !== 200) || !tok) {
      log(`mint ${app} ${task ?? "(no task)"}: ${r.status} ${v?.status} ${JSON.stringify(v?.body?.message ?? null)}`);
      return null;
    }
    return secret(tok);
  };
  const [A, B, C] = ["A", "B", "C"].map((l) => tasks.find((t) => t.label === l));
  const held = {
    wsA: await mint("workspace", A.id),
    agentA: await mint("agent", A.id),
    wsNone: await mint("workspace", null),
    wsC: null,
  };
  if (!held.wsA || !held.agentA || !held.wsNone) { log("STOP: a token did not mint"); process.exit(3); }
  // C's token: minted by `gate-row.sh c-mint` while C was live, kept in a 0600 file, read here after C completed, and
  // shredded by the teardown.
  const cFile = `${process.env.HOME}/.graft-m4a/gate/c.token`;
  try { held.wsC = secret(readFileSync(cFile, "utf8").trim()); } catch { held.wsC = null; }
  if (!held.wsC) { log("STOP: no token held for completed task C"); process.exit(3); }

  // A scratch clone of the base, with one new commit per attempt so every push is a new ref update.
  const work = `${process.env.TMPDIR ?? "/tmp"}/gate-${tag}-${pass}`;
  rmSync(work, { recursive: true, force: true });
  mkdirSync(work, { recursive: true });
  const git = (argv, env = {}) => {
    try {
      const o = execFileSync("git", argv, { cwd: work, env: { ...process.env, GIT_TERMINAL_PROMPT: "0", ...env }, stdio: ["ignore", "pipe", "pipe"], timeout: 120_000 });
      return { ok: true, out: o.toString() };
    } catch (e) {
      return { ok: false, out: `${e.stdout ?? ""}${e.stderr ?? ""}` };
    }
  };
  git(["init", "-q", "-b", "main", "."]);
  git(["-c", "user.email=row-a@graftyard.invalid", "-c", "user.name=row-a", "commit", "-q", "--allow-empty", "-m", "seed"]);
  git(["tag", "seed"]);
  // Every cell and each control set starts from this seed, so a push carries only its own file: in r1's first full
  // shipped pass the after-controls carried the workflow cell's file in their history and were refused as a protected
  // path, correctly, for the driver's own reason.
  const fresh = () => { git(["checkout", "-q", "--force", "--detach", "seed"]); git(["clean", "-qfdx"]); };
  // A push's URL carries its token as a Basic password; git never prints it, and `scan` checks every line we write.
  const url = (tok, r = base) => `https://x-access-token:${tok}@${HOST}/m3org/${r}.git`;
  const commit = (file, n) => {
    mkdirSync(`${work}/${file.split("/").slice(0, -1).join("/") || "."}`, { recursive: true });
    writeFileSync(`${work}/${file}`, `${pass} ${n} ${Date.now()}\n`);
    git(["add", "-A"]);
    git(["-c", "user.email=row-a@graftyard.invalid", "-c", "user.name=row-a", "commit", "-q", "-m", `row A ${pass} ${n}`]);
    return git(["rev-parse", "HEAD"]).out.trim();
  };
  const push = (tok, ref, file, n, r = base) => {
    commit(file, n);
    const p = git(["push", "-q", "--force", url(tok, r), `HEAD:${ref}`]);
    return { accepted: p.ok, detail: p.out.replace(/https:\/\/[^@\s]*@/g, "https://<token>@").slice(-200) };
  };
  // A REST ref write needs its commit on the base first: one commit per cell (with the cell's file), pushed once under
  // m3-admin to a scratch ref, then named by every write of that cell. A seed push per write made a pass outlast the
  // hold that keeps A live (r1, 2026-11-27).
  const seeds = {};
  const restRef = async (tok, method, ref, n, cell, file) => {
    if (!seeds[cell]) {
      const sha = commit(file, `seed-${cell.length}`);
      const seed = git(["push", "-q", "--force", url(admin), `HEAD:refs/heads/rowa-scratch-${pass}-${Object.keys(seeds).length}`]);
      if (!seed.ok) return { accepted: false, detail: "seed push failed", status: 0 };
      seeds[cell] = sha;
    }
    const sha = seeds[cell];
    const r = method === "POST"
      ? await api("POST", `/api/v3/repos/m3org/${base}/git/refs`, tok, { ref, sha })
      : await api("PATCH", `/api/v3/repos/m3org/${base}/git/${ref.replace(/^refs\//, "")}`, tok, { sha, force: true });
    return { accepted: r.status >= 200 && r.status < 300, status: r.status, detail: JSON.stringify(r.body?.message ?? r.body).slice(0, 160) };
  };

  const prefix = (t) => t.prefix.replace(/\*+$/, "").replace(/\/?$/, "/");
  // Controls: A's workspace token pushes inside A's own prefix, 10 of 10 accepted, before the cells (A surely live) and
  // again after them, with A's and B's states recorded at the end.
  const control = async (when) => {
    fresh();
    const c = { attempts: 0, accepted: 0 };
    for (let n = 0; n < 10; n++) {
      const g = push(held.wsA, `${prefix(A)}ctl-${when}-${pass}-${n}`, `ctl/${when}-${n}.txt`, n);
      c.attempts++; c.accepted += g.accepted ? 1 : 0;
      ev({ control: n, when, ...g });
    }
    log(`controls ${when}: ${c.accepted}/10`);
    return c;
  };
  const controlsBefore = await control("before");
  const cells = [
    { cell: "A-workspace into B's prefix", tok: held.wsA, ref: (n) => `${prefix(B)}x-${pass}-${n}`, file: (n) => `b/${n}.txt` },
    { cell: "A-workspace workflow file in A's prefix", tok: held.wsA, ref: (n) => `${prefix(A)}wf-${pass}-${n}`, file: (n) => `.github/workflows/x-${n}.yml` },
    { cell: "workspace token with no graftyard_task", tok: held.wsNone, ref: (n) => `${prefix(A)}none-${pass}-${n}`, file: (n) => `none/${n}.txt` },
    { cell: "C's token after C completed", tok: held.wsC, ref: (n) => `${prefix(C)}late-${pass}-${n}`, file: (n) => `c/${n}.txt` },
  ];
  const summary = {};
  const count = (cell, surface, a) => {
    const k = `${cell} | ${surface}`;
    summary[k] ??= { attempts: 0, accepted: 0, refused: 0 };
    summary[k].attempts++;
    summary[k][a.accepted ? "accepted" : "refused"]++;
  };
  for (const c of cells) {
    fresh();
    for (let n = 0; n < N; n++) {
      const g = push(c.tok, c.ref(n), c.file(n), n);
      count(c.cell, "git push", g); ev({ cell: c.cell, surface: "git push", n, ...g });
      // The workflow-file cell is a git-only rule (the push carries the file); its REST twin writes a ref to a commit
      // that holds it, which the same path check sees.
      // A create of a new ref each time: a PATCH of a ref that does not exist answers 404 before any rule, which is not
      // the rule's refusal (r1's first shipped pass, 2026-11-27).
      const r = await restRef(c.tok, "POST", c.ref(n), n, c.cell, c.file(`rest-${n}`));
      count(c.cell, "REST ref write", r); ev({ cell: c.cell, surface: "REST ref write", n, ...r });
    }
    log(`${c.cell}: ${JSON.stringify(Object.fromEntries(Object.entries(summary).filter(([k]) => k.startsWith(c.cell))))}`);
  }
  // A's agent token merges m3-admin's pull request, on every surface that serves a merge.
  let node = null;
  const merges = [
    ["REST PUT .../merge", async () => {
      const r = await api("PUT", `/api/v3/repos/m3org/${base}/pulls/${pr.number}/merge`, held.agentA, { merge_method: "merge" });
      return { accepted: r.status === 200, status: r.status, detail: JSON.stringify(r.body?.message ?? null) };
    }],
    ["GraphQL mergePullRequest", async () => {
      node ??= (await api("GET", `/api/v3/repos/m3org/${base}/pulls/${pr.number}`, admin)).body?.node_id;
      const q = { query: "mutation($id: ID!) { mergePullRequest(input: {pullRequestId: $id}) { pullRequest { merged } } }", variables: { id: node } };
      const r = await api("POST", "/api/graphql", held.agentA, q);
      const merged = r.body?.data?.mergePullRequest?.pullRequest?.merged === true;
      return { accepted: merged, status: r.status, detail: JSON.stringify(r.body?.errors?.[0]?.message ?? null) };
    }],
    ["git fast-forward of main", async () => {
      const f = git(["fetch", "-q", url(admin), `refs/heads/${pr.head}`]);
      if (!f.ok) return { accepted: false, detail: "fetch failed" };
      const p = git(["push", "-q", url(held.agentA), "FETCH_HEAD:refs/heads/main"]);
      return { accepted: p.ok, detail: p.out.replace(/https:\/\/[^@\s]*@/g, "https://<token>@").slice(-200) };
    }],
    ["PATCH .../git/refs/heads/main", async () => {
      const r = await api("PATCH", `/api/v3/repos/m3org/${base}/git/refs/heads/main`, held.agentA, { sha: pr.sha, force: true });
      return { accepted: r.status === 200, status: r.status, detail: JSON.stringify(r.body?.message ?? null) };
    }],
  ];
  fresh();
  for (const [surface, fn] of merges) {
    for (let n = 0; n < N; n++) {
      const a = await fn();
      count("A-agent merges m3-admin's pull request", surface, a);
      ev({ cell: "A-agent merges m3-admin's pull request", surface, n, ...a });
    }
  }
  // The surfaces the edge does not serve (read from crates/api/src/graphql.rs and workers/edge/src/api.rs on main):
  // run 10 times each to record the answer, not counted as the rule's refusal.
  const unserved = {};
  const gql = (m) => api("POST", "/api/graphql", held.wsA, { query: `mutation { ${m} }` });
  const record = [
    ["GraphQL createRef", () => gql(`createRef(input: {repositoryId: "x", name: "${prefix(B)}g", oid: "${"0".repeat(40)}"}) { ref { name } }`)],
    ["GraphQL updateRef", () => gql(`updateRef(input: {refId: "x", oid: "${"0".repeat(40)}"}) { ref { name } }`)],
    ["GraphQL createCommitOnBranch", () => gql(`createCommitOnBranch(input: {branch: {branchName: "x"}, expectedHeadOid: "${"0".repeat(40)}", message: {headline: "x"}}) { commit { oid } }`)],
    ["REST POST .../merges", () => api("POST", `/api/v3/repos/m3org/${base}/merges`, held.agentA, { base: "main", head: pr.head })],
  ];
  for (const [surface, fn] of record) {
    for (let n = 0; n < UNSERVED_N; n++) {
      const r = await fn();
      const answer = `${r.status} ${JSON.stringify(r.body?.errors?.[0]?.message ?? r.body?.message ?? null).slice(0, 120)}`;
      unserved[surface] ??= {};
      unserved[surface][answer] = (unserved[surface][answer] ?? 0) + 1;
      ev({ unserved: surface, n, status: r.status, answer });
    }
  }
  const controls = await control("after");
  const aState = (await api("GET", `/api/graftyard/v1/repos/m3org/${base}/tasks/${A.id}`, admin)).body?.state;
  const bState = (await api("GET", `/api/graftyard/v1/repos/m3org/${base}/tasks/${B.id}`, admin)).body?.state;
  ev({ end_states: { A: aState, B: bState } });
  // Each refusal's reason, so a refusal for a lapsed task is told apart from the rule's.
  const reasons = {};
  for (const l of readFileSync(rows, "utf8").split("\n").filter(Boolean).map((x) => JSON.parse(x))) {
    if (!l.cell || l.accepted) continue;
    const k = `${l.cell} | ${l.surface}`;
    const why = `${l.status ?? "git"} ${(l.detail ?? "").replace(/[0-9a-f]{8}-[0-9a-f-]{27}/g, "<task>").replace(/https:\/\/\S+/g, "<url>").slice(0, 90)}`;
    reasons[k] ??= {}; reasons[k][why] = (reasons[k][why] ?? 0) + 1;
  }
  const pass_ = Object.values(summary).every((s) => s.accepted === 0 && s.attempts === N)
    && controlsBefore.accepted === 10 && controls.accepted === 10;
  const v = { pass, cells: summary, reasons, unserved, controls_before: controlsBefore, controls_after: controls,
    end_states: { A: aState, B: bState }, verdict: pass_ ? "PASS" : "FAIL" };
  writeFileSync(`${out}/l003-${pass}.json`, scan(JSON.stringify(v, null, 2)));
  log(`L0-03 ${pass}: ${v.verdict}; controls ${controlsBefore.accepted}/10 before, ${controls.accepted}/10 after (A ${aState}, B ${bState}); ${JSON.stringify(summary)}`);
  rmSync(work, { recursive: true, force: true });
} else if (step === "widen" || step === "restore") {
  // B0-36's widened pass: both Apps' records and installations widened before the pass's tasks and tokens, and back to
  // the shipped permissions after it.
  await installAs(step === "widen" ? WIDE : null);
} else {
  console.error("step: repos pr task hold release complete cancel results l003 widen restore delete");
  process.exit(2);
}
