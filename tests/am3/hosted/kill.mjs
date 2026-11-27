// LM-3's kill row on graft-m4a (`build/BUILD-PLAN.md`, AM3-WP1, "a Sandbox killed mid-task resumes"): the install
// half and the row. The agents account half (the Apps, the controller with its `TaskBox`, `graft-agents-front`, the
// agents `graft-ops`) is `tests/am3/hosted/lf7-day.sh` with ROW=kill TB=1; `tests/am3/hosted/kill-row.sh` runs both.
//
//   node tests/am3/hosted/kill.mjs install <agents dir> <app-ids file>   graftyard-agent and graftyard-workspace
//        installed on m3org (`RegisterInstallation`, `Install`, all repositories), as tests/m4/wp4/app.mjs installs
//        its App; the org's agents enabled in-repo (`PATCH /orgs/m3org/agents` as m3-admin)
//   node tests/am3/hosted/kill.mjs repo <name>       m3org/<name> created with a first commit, cloned once (warm)
//   node tests/am3/hosted/kill.mjs row <out dir> <name> <ctrl token file> <controller status url>
//        10 tasks created by m3-admin (`POST .../tasks`, no kind) on m3org/<name>; each task's Sandbox killed once at a
//        planned point (before its first push, between pushes, or during a push, read from the box's `pushing`), its
//        late traffic replayed, and every task followed to its end. One JSON row per event in <out>/row.jsonl; the
//        verdict's inputs in <out>/tasks.json.
//   node tests/am3/hosted/kill.mjs smoke ...          the same with one task (its kill before its first push), before
//        the row spends ten Sandboxes
//   node tests/am3/hosted/kill.mjs delete <name>     m3org/<name> deleted
//
// Times are the orb's; the box's `at` is Cloudflare's. No token is printed (`lib.mjs`'s `scan`).
import { readFileSync, writeFileSync, appendFileSync } from "node:fs";
import { EDGE, OP, Git, api, log, must, sleep, tokens, be64, secret } from "../../am1/hosted/lib.mjs";
import { createHash } from "node:crypto";

const [step, ...args] = process.argv.slice(2);
const admin = tokens.admin;
const ORG = 7100001, ADMIN_ID = 7100003;
// The two installations' ids: next to m3-ci's 7190001 and graftyard-hosted-runners' 7190002.
const INST = { agent: 7190003, workspace: 7190004 };
const keyShard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;

if (step === "install") {
  const [dir, idsFile] = args;
  const [agent, workspace] = readFileSync(idsFile, "utf8").trim().split(/\s+/).map(Number);
  const reg = JSON.parse(readFileSync(`${dir}/agents-register.request.json`, "utf8"));
  const perms = (id) => reg.find((r) => r.op.startsWith("PutApp") && r.cmd.op.PutApp.record.id === id).cmd.op.PutApp.record.permissions;
  for (const [app, inst] of [[agent, INST.agent], [workspace, INST.workspace]]) {
    await must(`names:${keyShard("graftyard/names/installation/", be64(inst))}`,
      { cred: OP, op: { RegisterInstallation: { id: inst, app, owner: ORG } } }, "Exists", "Taken");
    // The Apps are the install account's (`install_account()`, account 1).
    await must(`authz:${ORG}`, { cred: OP, op: { Install: { id: inst, row: { app, repos: "All", permissions: perms(app) }, app_owner: 1 } } });
    log(`App ${app} installed on m3org as ${inst}`);
  }
  const r = await api("PATCH", "/api/graftyard/v1/orgs/m3org/agents", admin, { agents_enabled: true, workspace: "in_repo" });
  log(`PATCH /orgs/m3org/agents: ${r.status} ${JSON.stringify(r.body)}`);
  if (r.status !== 200) process.exit(1);
} else if (step === "uninstall") {
  const r = await api("PATCH", "/api/graftyard/v1/orgs/m3org/agents", admin, { agents_enabled: false });
  log(`PATCH /orgs/m3org/agents: ${r.status} ${JSON.stringify(r.body)}`);
  for (const inst of [INST.agent, INST.workspace]) {
    await must(`authz:${ORG}`, { cred: OP, op: { Uninstall: { id: inst } } }, "NotFound");
    log(`installation ${inst} uninstalled from m3org`);
  }
} else if (step === "repo") {
  const [name] = args;
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
  log(`create m3org/${name}: ${made.status}`);
  if (made.status !== 201) process.exit(1);
  const inst = await api("GET", `/api/v3/repos/m3org/${name}`, admin);
  log(`read m3org/${name}: ${inst.status} default ${inst.body?.default_branch}`);
} else if (step === "exits") {
  // The real-harness smoke's first gate: every `claude` turn's exit code, which `run.py` writes into each work
  // commit's message (`claude exit <n>`), read from every branch of m3org/<name> through the credential helper.
  const [name] = args;
  const g = new Git(`m3org/${name}`).as(admin);
  const c = g.run(["clone", "-q", "--bare", g.url, g.wt], g.dir);
  if (!c.ok) { log(`clone m3org/${name}: ${c.out}`); process.exit(1); }
  const l = g.run(["log", "--all", "--format=%s", "--grep=claude exit"], g.wt);
  const counts = {};
  for (const m of l.out.matchAll(/claude exit (-?\d+)/g)) counts[m[1]] = (counts[m[1]] ?? 0) + 1;
  log(`claude turns by exit code: ${JSON.stringify(counts)}`);
  process.exit(Object.keys(counts).length && Object.keys(counts).every((k) => k === "0") ? 0 : 4);
} else if (step === "delete") {
  const [name] = args;
  const d = await api("DELETE", `/api/v3/repos/m3org/${name}`, admin);
  log(`delete m3org/${name}: ${d.status}`);
} else if (step === "row" || step === "smoke") {
  const [out, name, tokenFile, ctrl] = args;
  const COUNT = step === "smoke" ? 1 : 10;
  const ctoken = secret(readFileSync(tokenFile, "utf8").trim());
  const rows = `${out}/row.jsonl`;
  const ev = (v) => { appendFileSync(rows, JSON.stringify({ t: Date.now(), ...v }) + "\n"); log(JSON.stringify(v)); };
  const box = async (task, what, body) => {
    const r = await fetch(`${ctrl}/_controller/box/${task}/${what}`, {
      method: body === undefined ? "GET" : "POST", headers: { "x-controller-token": ctoken, "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(60_000),
    });
    return { status: r.status, body: await r.json().catch(() => null) };
  };
  // The real-harness case's model stop (REAL=1, `tests/am3/results/lm3-kill-real.md`): every task box's `model`, the
  // usage of each model reply the router forwarded, priced at Sonnet's list price per million tokens ($3 input, $15
  // output, $3.75 cache write, $0.30 cache read). Read after every poll that reads a box; past MODEL_STOP_USD every
  // task is cancelled and the row stops with a `stop` row, which teardown follows.
  const MODEL_STOP_USD = Number(process.env.MODEL_STOP_USD ?? 25);
  const spent = new Map();
  const usd = (m) => m ? (m.input * 3 + m.output * 15 + m.cache_write * 3.75 + m.cache_read * 0.3) / 1e6 : 0;
  const spend = () => [...spent.values()].reduce((a, b) => a + b, 0);
  const metered = async (task, what, body) => {
    const r = await box(task, what, body);
    if (r.body?.box?.model) spent.set(task, usd(r.body.box.model));
    if (process.env.REAL && spend() >= MODEL_STOP_USD) {
      ev({ stop: `model spend $${spend().toFixed(2)} reached the $${MODEL_STOP_USD} stop`, model_usd: spend() });
      for (const t of tasks) await api("POST", `/api/graftyard/v1/repos/m3org/${name}/tasks/${t.id}/cancel`, admin, {});
      writeFileSync(`${out}/${step === "smoke" ? "smoke" : "tasks"}.json`, JSON.stringify({ tasks, model_usd: spend() }, null, 2));
      process.exit(3);
    }
    return r;
  };
  // Ten tasks; kill points spread so each kind lands at least once and the rest at random.
  const kinds = ["before", "between", "during"];
  const plan = Array.from({ length: COUNT }, (_, i) => kinds[i] ?? kinds[Math.floor(Math.random() * 3)]);
  const tasks = [];
  for (let i = 0; i < COUNT; i++) {
    const r = await api("POST", `/api/graftyard/v1/repos/m3org/${name}/tasks`, admin, { prompt: `LM-3 kill row task ${i}`, summary: `kill ${plan[i]}` });
    ev({ create: i, status: r.status, id: r.body?.id, state: r.body?.state, plan: plan[i], msg: r.status === 201 ? undefined : r.body });
    if (r.status !== 201) process.exit(1);
    tasks.push({ i, id: r.body.id, plan: plan[i], kill: null, late: null, end: null });
  }
  // Each box must start (the delivery reaches the controller through the install's App webhook), and each task's kill
  // watch starts as soon as its own box runs, all in parallel. real1's first 10-task row waited for all ten to start
  // first, and five Claude Code tasks were Done before their watch began, so they were never killed.
  await Promise.all(tasks.map(async (t) => {
    for (let k = 0; k < 60; k++) {
      const s = await metered(t.id, "status");
      if (s.body?.box?.task?.state?.Running) { t.first = s.body.box.task.state.Running.sandbox; break; }
      await sleep(2000);
    }
    ev({ started: t.i, sandbox: t.first ?? null });
    if (!t.first) { ev({ stop: `task ${t.i} never started` }); return; }
    // A random point inside the kind (`tests/am3/results/lm3-kill-real.md`, Lines): the first matching poll after a
    // delay drawn from 0 to 20 s. A `before` kill has one window, the first turn before any push, so it takes none.
    t.delay_ms = t.plan === "before" ? 0 : Math.floor(Math.random() * 20_000);
    await sleep(t.delay_ms);
    const deadline = Date.now() + 15 * 60_000;
    while (Date.now() < deadline && !t.kill) {
      const s = await metered(t.id, "status");
      const run = s.body?.box?.task?.state?.Running;
      const pushed = !!run?.last_push;
      const pushing = s.body?.pushing > 0;
      const go = (t.plan === "before" && run && !pushed && !pushing)
        || (t.plan === "between" && pushed && !pushing)
        || (t.plan === "during" && pushing);
      if (go) {
        const k = await box(t.id, "_kill", {});
        const hit = !k.body?.killed ? "not running" : k.body.pushing > 0 ? "during" : k.body.box?.task?.state?.Running?.last_push ? "between" : "before";
        t.kill = { at: k.body?.at, sandbox: run.sandbox, last_push: k.body?.box?.task?.state?.Running?.last_push ?? null, hit };
        ev({ kill: t.i, plan: t.plan, delay_ms: t.delay_ms, ...t.kill });
        // The killed Sandbox's late traffic, replayed through the box's own refusal: a report and a request. Only
        // once the box's alarm restarted the task under a new SandboxId: before it, the killed Sandbox is still the
        // task's, and its replayed `Finished` would be taken (r1's driver fault, `tests/am3/results/lm3-kill.md`).
        for (let k = 0; k < 40; k++) {
          const s = await metered(t.id, "status");
          const now = s.body?.box?.task?.state?.Running?.sandbox;
          if (now && now !== run.sandbox) { t.restarted = { sandbox: now, from_ms: Date.now() }; break; }
          await sleep(3000);
        }
        ev({ restarted: t.i, ...(t.restarted ?? { missed: true }) });
        if (!t.restarted) return;
        const l = await box(t.id, "_late", { sandbox: run.sandbox, url: `${EDGE}/m3org/${name}.git/info/refs?service=git-upload-pack` });
        t.late = { replayed: l.body?.replayed, late: l.body?.box?.late };
        ev({ late: t.i, ...t.late });
      } else {
        await sleep(t.plan === "during" ? 150 : 1000);
      }
    }
    if (!t.kill) ev({ kill: t.i, plan: t.plan, missed: true });
  }));
  // Follow every task to its end: the box's task final (Done), or TASK_IDLE-like stall past 20 minutes.
  const until = Date.now() + 25 * 60_000;
  for (const t of tasks) {
    while (Date.now() < until) {
      const s = await metered(t.id, "status");
      const b = s.body?.box;
      if (b?.task?.state === "Done" || b?.task?.state?.Failed || b?.task?.state === "Cancelled") {
        t.end = { state: b.task.state, generation: b.generation, late: b.late, running: s.body.running };
        break;
      }
      t.last = { state: b?.task?.state, generation: b?.generation, late: b?.late, running: s.body?.running };
      await sleep(10_000);
    }
    ev({ end: t.i, ...(t.end ?? { stalled: t.last }), model_usd: spent.get(t.id) ?? 0 });
  }
  ev({ model_usd: spend(), model_stop_usd: MODEL_STOP_USD });
  writeFileSync(`${out}/${step === "smoke" ? "smoke" : "tasks"}.json`, JSON.stringify(tasks, null, 2));
} else {
  console.error("step: install repo row delete");
  process.exit(2);
}
