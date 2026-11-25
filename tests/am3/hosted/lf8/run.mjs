// LF-8's hosted loop (`build/BUILD-PLAN.md`, AM3-WP1, LF-8): each forgery of `lf8forge` served to a deployed,
// unmodified `graft-agents-front`, one at a time, and the front's view read back after its refresh read it.
//
//   node tests/am3/hosted/lf8/run.mjs <forge dir> <feed url> <front url> <state dir> <out.jsonl> [main|cold]
//
// <state dir> holds `feed.token`, `front.token` and `webhook.secret` (0600). For each forgery: select it on the
// feed, send signed webhooks through the front (one every 250 ms) until the feed counts a read of it and a status
// read after that read shows a refresh ended since the selection (`oks + fails` moved), then record the status. The
// verdict per row (`ok`):
// - `install`, `keys`, gap `order`: a refresh failed since the selection, `lastError` is the class's refusal and
//   carries the forgery's own `expect` text (so a status answered by another isolate cannot pass a row), and the
//   view's index and pin are the history's;
// - replay `order`: refused as `Equivocation` naming its index (OPEN-QUESTIONS A55), with the front's `paged` count up.
// Then `control`: the group's real next entry must be taken (index + 1), so a refusal did not wedge the view.
// `cold` mode reads only `cold.json` (install B's genesis) through a front that never read a view: index 0 and a
// `WrongInstall` refusal.
import { createHmac, randomUUID } from "node:crypto";
import { appendFileSync, readFileSync } from "node:fs";

const [dir, feed, front, state, out, mode = "main"] = process.argv.slice(2);
const read = (f) => readFileSync(`${state}/${f}`, "utf8").trim();
const feedToken = read("feed.token"), frontToken = read("front.token"), secret = read("webhook.secret");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const history = JSON.parse(readFileSync(`${dir}/history.json`, "utf8"));
const forgeries = JSON.parse(readFileSync(`${dir}/forgeries.json`, "utf8"));
const control = JSON.parse(readFileSync(`${dir}/control.json`, "utf8"));
const cold = JSON.parse(readFileSync(`${dir}/cold.json`, "utf8"));
const held = history.length;

async function lf8(path, body) {
  const r = await fetch(`${feed}${path}`, { method: body ? "POST" : "GET", headers: { "x-lf8-token": feedToken,
    "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
  return r.json();
}
async function hook() {
  const body = JSON.stringify({ action: "opened", issue: { number: 1 }, sampler: "lf8" });
  const sig = "sha256=" + createHmac("sha256", secret).update(body).digest("hex");
  const r = await fetch(front, { method: "POST", body, headers: { "content-type": "application/json",
    "x-github-event": "issues", "x-github-delivery": randomUUID(), "x-hub-signature-256": sig, "user-agent": "graftyard-lf8" } });
  await r.arrayBuffer();
  return r.status;
}
async function status() {
  const r = await fetch(`${front}/_agents_front/status`, { headers: { "x-front-token": frontToken } });
  return r.status === 200 ? r.json() : { http: r.status };
}
const ends = (v) => (v.oks ?? 0) + (v.fails ?? 0);

/// Selects `name`, then sends webhooks until the feed counts a read and a later status shows a refresh ended.
async function serve(name, alone = false) {
  const before = await status();
  await lf8("/_lf8/select", { name, alone });
  const t0 = Date.now();
  for (let i = 0; i < 120; i++) {
    const s = await hook();
    const st = await lf8("/_lf8/state");
    if (st.reads > 0) {
      await sleep(300);
      const v = await status();
      if (v.view && ends(v.view) > ends(before.view ?? {})) return { before: before.view, after: v.view, hook: s, ms: Date.now() - t0, reads: st.reads };
    }
    await sleep(250);
  }
  return { before: before.view, after: (await status()).view, timeout: true, ms: Date.now() - t0 };
}

const want = { install: "WrongInstall", keys: ["UnknownKey", "WrongSigner"], order: ["BadState", "Equivocation"] };
const line = (o) => { appendFileSync(out, JSON.stringify(o) + "\n"); };
if (mode === "main") {
  console.log(JSON.stringify(await lf8("/_lf8/load", { history, pages: {
    ...Object.fromEntries(forgeries.map((f) => [`${f.class}-${f.k}`, [f]])), control, cold } })));
  // The front reads the history first: the view must hold it and pin release 3's controller.
  const h = await serve(null);
  line({ class: "history", ...h });
  const base = h.after;
  if (base?.index !== held || !base?.controller) throw new Error(`the front did not take the history: ${JSON.stringify(base)}`);
  const tally = {};
  for (const f of forgeries) {
    const r = await serve(`${f.class}-${f.k}`);
    const a = r.after ?? {};
    const same = a.index === base.index && a.controller === base.controller;
    const variant = (a.lastError ?? "").split(":")[0];
    // The refusal must be this forgery's own: its class's variant, a failed refresh since the selection, and the
    // text only this forgery's refusal carries (install B_k's id, a stranger's key prefix, or the kind).
    const refusal = [want[f.class]].flat().includes(variant) && a.fails > (r.before?.fails ?? 0)
      && (a.lastError ?? "").includes(f.expect);
    const replay = f.case?.startsWith("replay");
    const ok = !r.timeout && same && refusal && (!replay || a.paged > (r.before?.paged ?? 0));
    const key = `${f.class}/${replay ? "replay" : f.case === "gap" ? "gap" : f.class}`;
    tally[key] ??= { ok: 0, of: 0 };
    tally[key].of += 1;
    tally[key].ok += ok ? 1 : 0;
    line({ class: f.class, k: f.k, case: f.case, sha256: f.sha256, ok, index: a.index, controller: a.controller,
      fails: a.fails, oks: a.oks, lastError: (a.lastError ?? "").slice(0, 160), ms: r.ms, reads: r.reads, timeout: !!r.timeout });
  }
  const c = await serve("control");
  const accepted = c.after?.index === held + 1 && c.after?.controller === base.controller;
  line({ class: "control", ok: accepted, ...c });
  console.log(JSON.stringify({ tally, control: accepted }));
} else {
  const r = await serve("cold", true);
  const a = r.after ?? {};
  const ok = a.index === 0 && (a.lastError ?? "").startsWith("WrongInstall") && a.oks === 0;
  line({ class: "cold", ok, ...r });
  console.log(JSON.stringify({ cold: ok, index: a.index, lastError: (a.lastError ?? "").slice(0, 120) }));
}
