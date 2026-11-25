// BF-46's R45 rows on graft-m4a (`tests/m5/bf46/r45-row.sh`): the App-record commands, the App reads, and the live
// traffic each rotation is measured under. Environment as `tests/am1/hosted/lib.mjs` (graft-m4a's env), plus for
// `load` GRAFT_MCP, GRAFT_AUX, GRAFT_FRONT, R45_SINK and R45_SINK_SECRET_FILE. No secret goes on argv, in a URL, or
// in a log line (`scan`).
//
//   node tests/m5/bf46/r45.mjs commit <file>      one `{chain, cmd}` from r45-row.sh's `putapp`, as the operator
//   node tests/m5/bf46/r45.mjs record <app id>     the App's record on the install account's `Authz` chain: slug,
//                                                  callbacks, and how many client secrets it lists
//   node tests/m5/bf46/r45.mjs load <out> <stop>   until <stop> exists, every 20 s: one OAuth login through graft-mcp
//                                                  behind graft-front (the code's redemption uses graft-mcp's client
//                                                  secret slot, `McpClientSecret`), then one MCP `tools/list` with
//                                                  the token; and one repository hook created with a fresh secret
//                                                  (graft-ops `/wrap`s it, `SecretsWrapKey`) and pinged, whose
//                                                  delivery the receiver checks against that secret (the
//                                                  `HookSequencer` `/unwrap`s it), records the slot its
//                                                  secret was wrapped under, and deletes it
//   node tests/m5/bf46/r45.mjs slots <repo chain>  each hook's SecretsWrapKey slot, from the core `hook` rows
//   node tests/m5/bf46/r45.mjs unhook <repo>       deletes every hook whose url is the row's receiver
//   node tests/m5/bf46/r45.mjs wrapcheck <repo,...>  exits 1 when any hook there is wrapped under slot 0
//   node tests/m5/bf46/r45.mjs counts <out> <since ms>  each kind's calls and failures since then
import { readFileSync } from "node:fs";
import { api, bed, commit, log, member, refusal, row, scan, secret, sleep, tokens, voterNames } from "../../am1/hosted/lib.mjs";

const [cmd, a1, a2] = process.argv.slice(2);

// Every hook row of <repo chain>'s core tier and the `SecretsWrapKey` slot its secret is wrapped under: the postcard
// `HookRow`'s `HookRecord` (url, events, content_type, active, insecure_ssl, then `Option<Wrapped>`, whose first byte is
// the slot). Only each hook's id, url path and slot are returned.
async function hookSlots(chain) {
  const names = await voterNames(chain);
  let core = null;
  for (const s of [0, 1, 2]) {
    try { core = (await member(chain, s, "/read", { q: { rows: true } }, 30_000, names[s]))?.v?.value?.core; } catch { continue; }
    if (core) break;
  }
  if (!core) throw new Error(`${chain}: no member answered the rows read`);
  const varint = (b, at) => { let v = 0n, sh = 0n; for (;;) { const x = b[at.i++]; v |= BigInt(x & 0x7f) << sh; if (!(x & 0x80)) return v; sh += 7n; } };
  const str = (b, at) => { const n = Number(varint(b, at)); const t = Buffer.from(b.slice(at.i, at.i + n)).toString(); at.i += n; return t; };
  return core.filter((r) => r[0] === "hook").map(([, k, cols]) => {
    const b = cols[0].Blob, at = { i: 0 };
    const url = str(b, at);
    for (let n = Number(varint(b, at)); n > 0; n--) str(b, at);
    varint(b, at); at.i += 2;
    return { hook: k[0].Int, url: url.replace(/^https:\/\/[^/]+/, ""), slot: b[at.i++] === 1 ? b[at.i] : null };
  });
}
const INSTALL_ACCOUNT = 1;

if (cmd === "commit") {
  const { chain, cmd: c } = JSON.parse(readFileSync(a1, "utf8"));
  const o = await commit(chain, c);
  console.log(JSON.stringify({ chain, op: Object.keys(c.op)[0], outcome: refusal(o) }));
  if (!o?.ok) process.exit(1);
} else if (cmd === "record") {
  const v = (await bed({ chain: `authz:${INSTALL_ACCOUNT}`, read: { app: Number(a1) } })).value?.app;
  console.log(JSON.stringify({ app: Number(a1), slug: v?.slug ?? null, callbacks: v?.callbacks ?? null, client_secrets: v?.client_secrets ?? null }));
} else if (cmd === "slots") {
  console.log(JSON.stringify(await hookSlots(a1)));
} else if (cmd === "wrapcheck") {
  // Teardown's gate (the lead's condition): every hook on these repositories and the slot its secret is wrapped
  // under. Exits 1 when any is on slot 0, the row's fresh secret that teardown deletes.
  let bad = 0;
  for (const name of a1.split(",")) {
    const r = await api("GET", `/api/v3/repos/${name}`, tokens.admin);
    if (r.status !== 200) { console.log(JSON.stringify({ repo: name, status: r.status })); bad++; continue; }
    const s = await hookSlots(`repo:${r.body.id}`);
    const by = {};
    for (const h of s) by[h.slot ?? "none"] = (by[h.slot ?? "none"] ?? 0) + 1;
    console.log(JSON.stringify({ repo: name, chain: `repo:${r.body.id}`, hooks: s.length, slots: by }));
    bad += s.filter((h) => h.slot === 0).length;
  }
  console.log(bad ? `REFUSE: ${bad} hook(s) on slot 0 or unread` : "no hook on slot 0");
  if (bad) process.exit(1);
} else if (cmd === "unhook") {
  const SINK = process.env.R45_SINK;
  const have = await api("GET", `/api/v3/repos/${a1}/hooks`, tokens.admin);
  let n = 0;
  for (const h of have.body ?? []) if (h.config?.url?.startsWith(`${SINK}/h/`)) { await api("DELETE", `/api/v3/repos/${a1}/hooks/${h.id}`, tokens.admin); n++; }
  console.log(JSON.stringify({ repo: a1, deleted: n, left: (have.body ?? []).length - n }));
} else if (cmd === "counts") {
  const since = Number(a2 ?? 0);
  const rows = readFileSync(a1, "utf8").split("\n").filter(Boolean).map(JSON.parse).filter((r) => r.at >= since);
  const by = {};
  for (const r of rows) {
    const k = (by[r.kind] ??= { calls: 0, failed: 0, failures: [] });
    k.calls++;
    if (r.kind === "hook" || r.kind === "j4_hook") (k.slots ??= {})[r.slot ?? "none"] = ((k.slots ??= {})[r.slot ?? "none"] ?? 0) + 1;
    if (!r.ok) { k.failed++; if (k.failures.length < 5) k.failures.push({ at: new Date(r.at).toISOString(), why: r.why }); }
  }
  console.log(JSON.stringify({ since: since ? new Date(since).toISOString() : null, rows: rows.length, by }));
} else if (cmd === "load") {
  const { login, mcpCall } = await import("../../am2/hosted/mcp.mjs");
  const MCP = process.env.GRAFT_MCP;
  const SINK = process.env.R45_SINK;
  const SINK_SECRET = secret(readFileSync(process.env.R45_SINK_SECRET_FILE, "utf8").trim());
  // A repository of its own: j4 holds BF-57's 20 hooks, the most a repository takes.
  const REPO = process.env.R45_REPO ?? "m3org/r45-hooks";
  let repo = await api("GET", `/api/v3/repos/${REPO}`, tokens.admin);
  if (repo.status !== 200) repo = await api("POST", "/api/v3/orgs/m3org/repos", tokens.admin, { name: REPO.split("/")[1], auto_init: true });
  if (![200, 201].includes(repo.status)) throw new Error(`repo ${REPO}: ${repo.status}`);
  const CHAIN = `repo:${repo.body.id}`;
  log(`hooks on ${REPO} (${CHAIN})`);
  // The customer case: j4's existing hooks, wrapped under slot 1 before the row, pinged one a round through the
  // rotation (the lead's condition). Nothing on j4 is changed. Its receiver is graft-m4-hooksink (r45-sink, BF57_J4).
  const J4SINK = process.env.R45_J4_SINK;
  const j4 = await api("GET", "/api/v3/repos/m3org/j4", tokens.admin);
  const j4Hooks = J4SINK ? ((await api("GET", "/api/v3/repos/m3org/j4/hooks", tokens.admin)).body ?? [])
    .filter((h) => !h.config.url.includes("slow")).map((h) => ({ id: h.id, n: new URL(h.config.url).pathname.split("/")[2] })) : [];
  if (J4SINK && (j4.status !== 200 || !j4Hooks.length)) throw new Error(`j4: ${j4.status}, ${j4Hooks.length} hooks`);
  log(`j4 (repo:${j4.body?.id}): ${j4Hooks.length} hooks pinged in turn`);
  const out = a1, stop = a2;
  const exists = (p) => { try { readFileSync(p); return true; } catch { return false; } };
  const note = (kind, ok, extra = {}) => row(out, { at: Date.now(), kind, ok, ...extra });
  for (let n = 1; !exists(stop); n++) {
    const t0 = Date.now();
    // McpClientSecret: graft-mcp redeems the host's code with its client secret (`host_token`). A wrong or missing
    // slot is the host's 401, which graft-mcp's callback answers as an error, so login throws.
    try {
      const t = await login("write:read", "ts-v2", MCP);
      secret(t.access_token);
      const r = await mcpCall(t.access_token, "tools/list", {}, MCP);
      note("mcp_login", r.status === 200, { n, status: r.status, ms: Date.now() - t0, ...(r.status === 200 ? {} : { why: r.text.slice(0, 120) }) });
    } catch (e) {
      note("mcp_login", false, { n, ms: Date.now() - t0, why: scan(String(e.message ?? e)).slice(0, 160) });
    }
    // SecretsWrapKey: a hook created with the sink's secret, so the edge has graft-ops wrap it; a ping signs with it
    // after the sequencer unwraps it; the sink answers 200 only when X-Hub-Signature-256 matches.
    const t1 = Date.now();
    try {
      const c = await api("POST", `/api/v3/repos/${REPO}/hooks`, tokens.admin,
        { config: { url: `${SINK}/h/${n}`, content_type: "json", secret: SINK_SECRET }, events: ["push"], active: true });
      if (c.status !== 201) throw new Error(`hook create ${c.status} ${JSON.stringify(c.body).slice(0, 100)}`);
      const id = c.body.id;
      const p = await api("POST", `/api/v3/repos/${REPO}/hooks/${id}/pings`, tokens.admin);
      if (p.status !== 204) throw new Error(`ping ${p.status} ${JSON.stringify(p.body).slice(0, 100)}`);
      let got = null;
      for (let i = 0; i < 15 && !got; i++) {
        await sleep(1000);
        const r = await fetch(`${SINK}/seen/${n}`, { headers: { authorization: `Bearer ${SINK_SECRET}` } });
        if (r.status === 200) got = await r.json();
      }
      // The slot this hook's secret was wrapped under, from the repository's core rows, then the hook goes (at most
      // 20 a repository).
      const slot = (await hookSlots(CHAIN).catch(() => [])).find((h) => h.hook === id)?.slot ?? null;
      await api("DELETE", `/api/v3/repos/${REPO}/hooks/${id}`, tokens.admin);
      if (!got) throw new Error("no delivery at the sink within 15 s");
      note("hook", got.sig_ok === true, { n, hook: id, slot, ms: Date.now() - t1, event: got.event, ...(got.sig_ok ? {} : { why: "bad signature" }) });
    } catch (e) {
      note("hook", false, { n, ms: Date.now() - t1, why: scan(String(e.message ?? e)).slice(0, 160) });
    }
    if (j4Hooks.length) {
      const h = j4Hooks[n % j4Hooks.length], t2 = Date.now();
      try {
        const p = await api("POST", `/api/v3/repos/m3org/j4/hooks/${h.id}/pings`, tokens.admin);
        if (p.status !== 204) throw new Error(`ping ${p.status} ${JSON.stringify(p.body).slice(0, 100)}`);
        let got = null;
        for (let i = 0; i < 15 && !got; i++) {
          await sleep(1000);
          const r = await fetch(`${J4SINK}/seen/${h.n}`, { headers: { authorization: `Bearer ${SINK_SECRET}` } });
          if (r.status === 200) { const v = await r.json(); if (v.at >= t2) got = v; }
        }
        const slot = (await hookSlots(`repo:${j4.body.id}`).catch(() => [])).find((x) => x.hook === h.id)?.slot ?? null;
        if (!got) throw new Error("no delivery at graft-m4-hooksink within 15 s");
        note("j4_hook", got.sig_ok === true, { n, hook: h.id, slot, ms: Date.now() - t2, event: got.event, ...(got.sig_ok ? {} : { why: "bad signature" }) });
      } catch (e) {
        note("j4_hook", false, { n, hook: h.id, ms: Date.now() - t2, why: scan(String(e.message ?? e)).slice(0, 160) });
      }
    }
    const wait = 20_000 - (Date.now() - t0);
    if (wait > 0) await sleep(wait);
  }
  log("load stopped");
} else {
  console.error("usage: r45.mjs commit <file> | record <app> | load <out> <stop> | counts <out> <since>");
  process.exit(2);
}
