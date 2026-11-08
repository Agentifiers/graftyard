// BF-33's "completes" branch on graft-m2b (`build/BUILD-PLAN.md`, M3-WP3): a device flow as gh's client, where the
// leader of the new token's `Tokens` shard is killed while it decides the exchange's own `StoreWith`, after the code's
// `UseCode` committed. The poll carries `x-graft-kill: <mode>` and `x-graft-kill-on: token`, so the kill rides on the
// edge's first send of `StoreWith` only (`ident::commit_killable`); the edge re-sends under the same request id, as it
// does for every command. Modes alternate `after_quorum` (the entry committed; the re-send finds its outcome) and
// `before_commit_early` (the entry may be lost; the re-send commits it); every 5th run holds the killed member down
// 12 s. Each run then polls again (the replay) and counts the account's OAuth tokens on its `Identity` fold.
// Pass: every run either gets exactly one token from that poll, with one new listed token, or restarts cleanly
// (`expired_token`, no token); the replay never yields one.
//   node tests/m3/rows/bf33-token.mjs <runs> <out.jsonl>
import { EDGE, log, read, row, secret, state, tokens } from "./lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 100);
const GH = "178c6fc778ccc68e1d6a";
const MODES = (process.env.BF33_KILLS ?? "after_quorum,before_commit_early").split(",");
const form = (o) => new URLSearchParams(o).toString();
async function post(path, body, headers = {}) {
  const r = await fetch(EDGE + path, { method: "POST", signal: AbortSignal.timeout(180_000),
    headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded", ...headers }, body: form(body) });
  const b = await r.json().catch(() => null);
  if (b?.access_token) secret(b.access_token);
  if (b?.device_code) secret(b.device_code);
  let trace = [];
  try { trace = JSON.parse(r.headers.get("x-graft-trace") ?? "[]"); } catch { /* none */ }
  return { status: r.status, body: b, trace };
}
const oauthTokens = async () => ((await read(`identity:${state.member}`, {}))?.credentials ?? []).filter((x) => x.kind === "OAuth").length;
const c = { runs, fired: 0, completed: 0, restarted: 0, other: 0, oneNewToken: 0, replayRefused: 0, tokensFromReplay: 0, extraTokens: 0 };
for (let i = 1; i <= runs; i++) {
  const mode = MODES[(i - 1) % MODES.length];
  const down = i % 5 === 0 ? 12_000 : 0;
  const d = (await post("/login/device/code", { client_id: GH, scope: "repo" })).body;
  await post("/login/oauth/device/approve", { user_code: d.user_code }, { authorization: `token ${tokens.member}` });
  const before = await oauthTokens();
  const t0 = Date.now();
  const one = await post("/login/oauth/access_token", { client_id: GH, device_code: d.device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" },
    { "x-graft-kill": mode, "x-graft-kill-on": "token", "x-graft-down-ms": String(down) });
  const ms = Date.now() - t0;
  const two = await post("/login/oauth/access_token", { client_id: GH, device_code: d.device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
  const after = await oauthTokens();
  const got = /^gho_/.test(one.body?.access_token ?? "");
  const works = got ? (await fetch(`${EDGE}/api/v3/user`, { headers: { authorization: `token ${one.body.access_token}` } })).status : null;
  const outcome = got ? "token" : one.body?.error ?? `status ${one.status}`;
  // The kill fired when the hedge on the token's shard saw the killed member miss: a missed receipt or outcome, or a
  // `suspect` after one (a re-send may then find the outcome through `/outcome_by_request` without a second `send`).
  const t = one.trace ?? [];
  const on = t[0]?.killOn ?? null;
  const sends = t.filter((x) => x.send !== undefined).length;
  const missed = t.filter((x) => x.missed !== undefined || x.outcomeMissed !== undefined || x.suspect !== undefined).length;
  const fired = on === "token" && missed > 0;
  if (fired) c.fired++;
  if (got) c.completed++; else if (one.body?.error === "expired_token") c.restarted++; else c.other++;
  if (got && after - before === 1) c.oneNewToken++;
  if (after - before > 1) c.extraTokens += after - before - 1;
  if (two.body?.access_token) c.tokensFromReplay++; else c.replayRefused++;
  row(out, { i, mode, downMs: down, ms, killOn: on, chain: t[0]?.chain ?? null, sends, missed, fired, outcome, status: one.status, userStatus: works, newListed: after - before,
    replay: two.body?.access_token ? "TOKEN" : two.body?.error ?? two.status });
  if (i % 10 === 0) log(`BF-33 token kill ${i}/${runs}: ${JSON.stringify(c)}`);
}
row(out, { summary: c });
log(`BF-33 token kill: ${JSON.stringify(c)}`);
