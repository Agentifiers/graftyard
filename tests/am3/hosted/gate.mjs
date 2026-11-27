// LM-3's row A on graft-m4a (`tests/am3/results/lm3-gate.md`, "Row A"; the lead's GO, 2026-11-26): L0-03's token
// scopes, after the L0-02 phase (`gate-row.sh l002`, which runs the adapted harness in a Sandbox). Run by
// `tests/am3/hosted/gate-row.sh`, which owns the agents half, the probe and the teardown.
//
//   node tests/am3/hosted/gate.mjs repos <tag>                 m3org/gate-<tag> and m3org/gate-other-<tag>, each with a
//        first commit, and m3-admin's pull request (`gate-pr-<tag>` into main) that the merge cells target
//   node tests/am3/hosted/gate.mjs task <name> <label>         one prompt-only Graftyard-run task by m3-admin; prints its
//        id and prefix as JSON
//   node tests/am3/hosted/gate.mjs hold <name> <id>|release <name> <id>    `refs/heads/hold/<id>` on the base, which
//        the hold-mode harness waits on (AM2's `harness.py`), created or deleted by m3-admin
//   node tests/am3/hosted/gate.mjs complete <name> <id>       the task's `completed` through the base (m3-admin)
//   node tests/am3/hosted/gate.mjs results <name> <id> <out>  the L0-02 harness's class files from the task's
//        `results` branch, into <out>/l002-classes.json; exit 1 until the branch exists
//   node tests/am3/hosted/gate.mjs l003 <out> <tag> <pass> <probe url> <probe token file> <tasks.json>
//        the L0-03 cells, 100 of each over git and REST, the merge cells over every surface, the unserved surfaces
//        10 times each to record their answer, and the 10 controls; one JSON row per attempt in <out>/l003-<pass>.jsonl,
//        the verdict's inputs in <out>/l003-<pass>.json. <pass> is `shipped` or `widened`.
//   node tests/am3/hosted/gate.mjs delete <tag>               both repositories deleted
//
// Tokens come only from the probe's `POST /mint` (AM2's, `{app, installation, body}` under `x-probe-token`), are held in
// memory, are registered with `lib.mjs`'s `secret` so `scan` throws if one reaches a log line, and are never written.
import { readFileSync, writeFileSync, appendFileSync, mkdirSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { EDGE, HOST, api, log, sleep, tokens, secret, scan } from "../../am1/hosted/lib.mjs";

const [step, ...args] = process.argv.slice(2);
const admin = tokens.admin;
const N = Number(process.env.GATE_N ?? 100);
const UNSERVED_N = 10;
// The two installations kill.mjs installs (`INST`): graftyard-agent and graftyard-workspace on m3org.
const INST = { agent: 7190003, workspace: 7190004 };

const repo = (tag) => `gate-${tag}`;
const other = (tag) => `gate-other-${tag}`;

if (step === "repos") {
  const [tag] = args;
  for (const name of [repo(tag), other(tag)]) {
    const r = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
    log(`create m3org/${name}: ${r.status}`);
    if (r.status !== 201) process.exit(1);
  }
  // m3-admin's pull request: a branch with one commit, opened into main, which A's agent token tries to merge.
  const main = await api("GET", `/api/v3/repos/m3org/${repo(tag)}/git/ref/heads/main`, admin);
  const sha = main.body?.object?.sha;
  const br = await api("POST", `/api/v3/repos/m3org/${repo(tag)}/git/refs`, admin, { ref: `refs/heads/gate-pr-${tag}`, sha });
  log(`branch gate-pr-${tag} at ${sha?.slice(0, 7)}: ${br.status}`);
  const pr = await api("POST", `/api/v3/repos/m3org/${repo(tag)}/pulls`, admin, { title: "row A merge target", head: `gate-pr-${tag}`, base: "main" });
  log(`pull request: ${pr.status} #${pr.body?.number ?? "-"}`);
  writeFileSync(`tests/am3/raw/gate-${tag}/pr.json`, JSON.stringify({ number: pr.body?.number ?? null, head: `gate-pr-${tag}`, sha }));
} else if (step === "task") {
  const [name, label] = args;
  const r = await api("POST", `/api/graftyard/v1/repos/m3org/${name}/tasks`, admin, { prompt: `LM-3 row A ${label}`, summary: `row A ${label}` });
  log(`task ${label}: ${r.status} ${r.body?.id ?? JSON.stringify(r.body)}`);
  if (r.status !== 201) process.exit(1);
  console.log(JSON.stringify({ label, id: r.body.id, prefix: r.body.prefix ?? `refs/heads/graftyard/${r.body.id}/` }));
} else if (step === "hold" || step === "release") {
  const [name, id] = args;
  if (step === "hold") {
    const main = await api("GET", `/api/v3/repos/m3org/${name}/git/ref/heads/main`, admin);
    const r = await api("POST", `/api/v3/repos/m3org/${name}/git/refs`, admin, { ref: `refs/heads/hold/${id}`, sha: main.body?.object?.sha });
    log(`hold/${id}: ${r.status}`);
    if (r.status !== 201) process.exit(1);
  } else {
    const r = await api("DELETE", `/api/v3/repos/m3org/${name}/git/refs/heads/hold/${id}`, admin);
    log(`release hold/${id}: ${r.status}`);
  }
} else if (step === "complete") {
  const [name, id] = args;
  const r = await api("PATCH", `/api/graftyard/v1/repos/m3org/${name}/tasks/${id}`, admin, { state: "completed" });
  log(`complete ${id}: ${r.status} ${r.body?.state ?? JSON.stringify(r.body)}`);
} else if (step === "results") {
  // The L0-02 harness's class files, from `refs/heads/graftyard/<id>/results` on the base (attempts.py pushes them
  // through the host arm); exit 1 while the ref does not exist yet.
  const [name, id, out] = args;
  const ref = `graftyard/${id}/results`;
  const ls = await api("GET", `/api/v3/repos/m3org/${name}/contents/out?ref=${encodeURIComponent(ref)}`, admin);
  if (ls.status !== 200 || !Array.isArray(ls.body)) { log(`results ${ref}: ${ls.status}`); process.exit(1); }
  const classes = {};
  for (const f of ls.body.filter((e) => e.name.endsWith(".json"))) {
    const c = await api("GET", `/api/v3/repos/m3org/${name}/contents/out/${f.name}?ref=${encodeURIComponent(ref)}`, admin);
    classes[f.name.replace(/\.json$/, "")] = JSON.parse(Buffer.from(c.body.content, "base64").toString("utf8"));
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
  // `widened` asks each mint for every permission the App has (B0-36's widened pass); `shipped` sends none, so the
  // installation's own set applies. The router never mints these: the probe does, as an attacker holding the key would.
  const perms = pass === "widened"
    ? { contents: "write", pull_requests: "write", workflows: "write", metadata: "read", checks: "write", statuses: "write" }
    : undefined;
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
  // A REST ref write needs the commit on the base first: pushed under m3-admin to a scratch ref, then pointed at.
  const restRef = async (tok, method, ref, n) => {
    const sha = commit(`rest/${n}.txt`, n);
    const seed = git(["push", "-q", "--force", url(admin), `HEAD:refs/heads/rowa-scratch-${pass}`]);
    if (!seed.ok) return { accepted: false, detail: "seed push failed", status: 0 };
    const r = method === "POST"
      ? await api("POST", `/api/v3/repos/m3org/${base}/git/refs`, tok, { ref, sha })
      : await api("PATCH", `/api/v3/repos/m3org/${base}/git/${ref.replace(/^refs\//, "")}`, tok, { sha, force: true });
    return { accepted: r.status >= 200 && r.status < 300, status: r.status, detail: JSON.stringify(r.body?.message ?? r.body).slice(0, 160) };
  };

  const prefix = (t) => t.prefix.replace(/\*+$/, "").replace(/\/?$/, "/");
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
    for (let n = 0; n < N; n++) {
      const g = push(c.tok, c.ref(n), c.file(n), n);
      count(c.cell, "git push", g); ev({ cell: c.cell, surface: "git push", n, ...g });
      // The workflow-file cell is a git-only rule (the push carries the file); its REST twin writes a ref to a commit
      // that holds it, which the same path check sees.
      const r = await restRef(c.tok, n % 2 ? "PATCH" : "POST", c.ref(n), n);
      count(c.cell, "REST ref write", r); ev({ cell: c.cell, surface: "REST ref write", n, ...r });
    }
    log(`${c.cell}: ${JSON.stringify(Object.fromEntries(Object.entries(summary).filter(([k]) => k.startsWith(c.cell))))}`);
  }
  // A's agent token merges m3-admin's pull request, on every surface that serves a merge.
  const merges = [
    ["REST PUT .../merge", async () => {
      const r = await api("PUT", `/api/v3/repos/m3org/${base}/pulls/${pr.number}/merge`, held.agentA, { merge_method: "merge" });
      return { accepted: r.status === 200, status: r.status, detail: JSON.stringify(r.body?.message ?? null) };
    }],
    ["GraphQL mergePullRequest", async () => {
      const node = (await api("GET", `/api/v3/repos/m3org/${base}/pulls/${pr.number}`, admin)).body?.node_id;
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
  // Controls: A's workspace token pushes inside A's own prefix, 10 of 10 accepted, while the hold keeps A live.
  const controls = { attempts: 0, accepted: 0 };
  for (let n = 0; n < 10; n++) {
    const g = push(held.wsA, `${prefix(A)}ctl-${pass}-${n}`, `ctl/${n}.txt`, n);
    controls.attempts++; controls.accepted += g.accepted ? 1 : 0;
    ev({ control: n, ...g });
  }
  const pass_ = Object.values(summary).every((s) => s.accepted === 0 && s.attempts === N) && controls.accepted === 10;
  const v = { pass, cells: summary, unserved, controls, verdict: pass_ ? "PASS" : "FAIL" };
  writeFileSync(`${out}/l003-${pass}.json`, scan(JSON.stringify(v, null, 2)));
  log(`L0-03 ${pass}: ${v.verdict}; controls ${controls.accepted}/10; ${JSON.stringify(summary)}`);
  rmSync(work, { recursive: true, force: true });
} else {
  console.error("step: repos task hold release complete l003 delete");
  process.exit(2);
}
