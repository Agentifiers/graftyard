// The edge read gate (origin/main 1c47d28, `authorize_read` on every repository route) on graft-m2b, before the
// Terraform runs. `runs` rounds of each case, against two repositories this driver makes with `POST /orgs/m3org/repos`
// (m3-admin; one private, one public, both auto_init) and a fresh user with no access ("outsider") and a fresh user
// invited and accepted as a collaborator ("collab"):
//   anon-clone-private (401), anon-clone-public (clone ok), outsider-clone-private (404 "Repository not found."),
//   outsider-gh-api (gh api repos/o/r: 404), collab-get (200) and collab-clone (ok), revoked-read (a fresh PAT read
//   200, revoked with DELETE /applications/{gh}/token, then read again 11 s later: 401), outsider-lfs-batch (404),
//   outsider-codeload (404), and one leader read of `{"q":{"rows":{"access":true}}}` on the private repository.
// Tokens ride in headers, a 0600 file for git's credential helper, and gh's own GH_CONFIG_DIR; never argv or a log.
//   node tests/m3/repos/gate.mjs <runs> <out.jsonl>
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BED, EDGE, api, log, pat, row, sleep, tokens, user } from "../rows/lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 10);
const GH_CLIENT = "178c6fc778ccc68e1d6a";
const host = new URL(EDGE).host;
const tag = Date.now().toString(36);
const work = mkdtempSync(join(tmpdir(), "m3gate-"));
const tok = join(work, "tok");
writeFileSync(join(work, "cred.sh"), "#!/bin/sh\n[ \"$1\" = get ] || exit 0\n[ -s \"$GRAFT_TOKFILE\" ] || exit 0\necho username=x\necho \"password=$(cat \"$GRAFT_TOKFILE\")\"\n", { mode: 0o700 });
const genv = { ...process.env, GRAFT_TOKFILE: tok, GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "/bin/false", HOME: work };
delete genv.GH_TOKEN; delete genv.GITHUB_TOKEN; delete genv.GH_ENTERPRISE_TOKEN;
/** `git clone` as `token` (null: anonymous). Returns git's exit code and the HTTP status git reported, if any. */
function clone(url, token, dest) {
  writeFileSync(tok, token ?? "", { mode: 0o600 });
  const r = spawnSync("git", ["-c", "credential.helper=", "-c", `credential.helper=${join(work, "cred.sh")}`, "clone", "-q", url, dest], { env: genv, encoding: "utf8", timeout: 120_000 });
  rmSync(dest, { recursive: true, force: true });
  const err = (r.stderr ?? "").replace(/\s+/g, " ").slice(0, 160);
  const code = err.match(/error: (\d{3})|returned error: (\d{3})|The requested URL returned error: (\d{3})/);
  return { rc: r.status, http: code ? Number(code[1] ?? code[2] ?? code[3]) : null, err: r.status === 0 ? "" : err };
}
/** A raw request with the token in the header (null: anonymous); the status, two headers, and a body prefix. */
async function raw(method, url, token, body, headers = {}) {
  const h = { ...headers };
  if (token) h.authorization = `token ${token}`;
  const r = await fetch(url, { method, headers: h, body, redirect: "manual", signal: AbortSignal.timeout(120_000) });
  const t = await r.text();
  return { status: r.status, www: r.headers.get("www-authenticate"), body: t.slice(0, 120) };
}
const counts = {};
const tally = (k, v) => { counts[k] ??= {}; counts[k][v] = (counts[k][v] ?? 0) + 1; };

try {
  // The repositories and users.
  const priv = `gate-priv-${tag}`, pub = `gate-pub-${tag}`;
  for (const [n, p] of [[priv, true], [pub, false]]) {
    const r = await api("POST", "/orgs/m3org/repos", tokens.admin, { name: n, private: p, auto_init: true });
    if (r.status !== 201) throw new Error(`create ${n}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  }
  const base = 7_170_000 + (Date.now() % 900) * 10;
  const outsider = await user(base + 1, `gate-out-${tag}`), collab = await user(base + 2, `gate-col-${tag}`);
  const tOut = (await pat(outsider, "gate outsider")).text, tCol = (await pat(collab, "gate collab")).text;
  const inv = await api("PUT", `/repos/m3org/${priv}/collaborators/gate-col-${tag}`, tokens.admin, { permission: "pull" });
  // The edge finds a user's repository invitations under `?repo=o/r` (m3-rows' BF-34: no per-user index yet).
  const id = (await api("GET", `/user/repository_invitations?repo=m3org/${priv}`, tCol)).body?.[0]?.id;
  const acc = await api("PATCH", `/user/repository_invitations/${id}?repo=m3org/${priv}`, tCol);
  if (acc.status !== 204) throw new Error(`accept: ${acc.status}`);
  row(out, { setup: { priv, pub, outsider, collab, invite: inv.status, accept: acc.status } });
  log(`setup: invite ${inv.status}, accept ${acc.status}`);
  // gh as the outsider, from its own config dir.
  const ghDir = mkdtempSync(join(tmpdir(), "m3gate-gh-"));
  writeFileSync(join(ghDir, "hosts.yml"), `${host}:\n    oauth_token: ${tOut}\n    user: gate-out-${tag}\n    git_protocol: https\n`, { mode: 0o600 });
  const ghEnv = { ...genv, GH_CONFIG_DIR: ghDir, GH_NO_UPDATE_NOTIFIER: "1", GH_PROMPT_DISABLED: "1", GH_HOST: host };
  const sha = (await api("GET", `/repos/m3org/${priv}/commits/main`, tokens.admin)).body?.sha;
  const lfsBody = JSON.stringify({ operation: "download", transfers: ["basic"], objects: [{ oid: "a".repeat(64), size: 1 }] });
  const lfsHeaders = { accept: "application/vnd.git-lfs+json", "content-type": "application/vnd.git-lfs+json" };

  for (let i = 1; i <= runs; i++) {
    const r = { i };
    const a = clone(`${EDGE}/m3org/${priv}.git`, null, join(work, "c"));
    const info = await raw("GET", `${EDGE}/m3org/${priv}.git/info/refs?service=git-upload-pack`, null);
    r.anonPrivate = { rc: a.rc, http: a.http, infoRefs: info.status, www: info.www };
    tally("anon-clone-private", `rc=${a.rc} http=${a.http} info/refs=${info.status} www=${info.www}`);
    const b = clone(`${EDGE}/m3org/${pub}.git`, null, join(work, "c"));
    r.anonPublic = { rc: b.rc, err: b.err };
    tally("anon-clone-public", `rc=${b.rc}`);
    const o = clone(`${EDGE}/m3org/${priv}.git`, tOut, join(work, "c"));
    const oi = await raw("GET", `${EDGE}/m3org/${priv}.git/info/refs?service=git-upload-pack`, tOut);
    r.outsiderClone = { rc: o.rc, http: o.http, infoRefs: oi.status, body: oi.body.trim() };
    tally("outsider-clone-private", `rc=${o.rc} http=${o.http} info/refs=${oi.status} "${oi.body.trim()}"`);
    const g = spawnSync("gh", ["api", `repos/m3org/${priv}`], { env: ghEnv, encoding: "utf8", timeout: 60_000 });
    const gs = (g.stderr ?? "").match(/HTTP (\d{3})/)?.[1] ?? (g.status === 0 ? "200" : "?");
    r.outsiderGh = { rc: g.status, http: gs };
    tally("outsider-gh-api", `rc=${g.status} HTTP ${gs}`);
    const cg = await api("GET", `/repos/m3org/${priv}`, tCol);
    const cc = clone(`${EDGE}/m3org/${priv}.git`, tCol, join(work, "c"));
    r.collab = { get: cg.status, pull: cg.body?.permissions?.pull, push: cg.body?.permissions?.push, clone: cc.rc, err: cc.err };
    tally("collab-get", `${cg.status}`);
    tally("collab-clone", `rc=${cc.rc}`);
    const lf = await raw("POST", `${EDGE}/m3org/${priv}.git/info/lfs/objects/batch`, tOut, lfsBody, lfsHeaders);
    r.outsiderLfs = lf.status;
    tally("outsider-lfs-batch", `${lf.status}`);
    const cl = await raw("GET", `${EDGE}/_codeload/m3org/${priv}/legacy.tar.gz/${sha}`, tOut);
    const tb = await raw("GET", `${EDGE}/api/v3/repos/m3org/${priv}/tarball/main`, tOut);
    r.outsiderCodeload = { codeload: cl.status, tarball: tb.status };
    tally("outsider-codeload", `codeload=${cl.status} tarball=${tb.status}`);
    // A fresh PAT for the collaborator: read, revoke, read again 11 s later.
    const p = await pat(collab, `gate revoke ${i}`);
    const before = await api("GET", `/repos/m3org/${priv}`, p.text);
    const rv = await fetch(`${EDGE}/api/v3/applications/${GH_CLIENT}/token`, {
      method: "DELETE", signal: AbortSignal.timeout(120_000),
      headers: { authorization: `Basic ${Buffer.from(`${GH_CLIENT}:x`).toString("base64")}`, "content-type": "application/json" },
      body: JSON.stringify({ access_token: p.text }),
    });
    await sleep(11_000);
    const after = await api("GET", `/repos/m3org/${priv}`, p.text);
    const afterGit = await raw("GET", `${EDGE}/m3org/${priv}.git/info/refs?service=git-upload-pack`, p.text);
    r.revoked = { before: before.status, revoke: rv.status, after: after.status, afterGit: afterGit.status };
    tally("revoked-read", `before=${before.status} revoke=${rv.status} after11s=${after.status} git=${afterGit.status}`);
    row(out, r);
    log(`gate ${i}/${runs}: ${JSON.stringify(r)}`);
  }
  rmSync(ghDir, { recursive: true, force: true });
  // One leader read of the access rows, through the bed route (repository read only).
  const q = await fetch(`${EDGE}/_bed/identity`, { method: "POST", headers: { "x-bed-token": BED, "content-type": "application/json" },
    body: JSON.stringify({ chain: `repo:${(await api("GET", `/repos/m3org/${priv}`, tokens.admin)).body.id}`, read: { rows: { access: true } } }) });
  const qv = await q.json();
  const v = qv.value ?? qv;
  const tables = Array.isArray(v?.core) ? [...new Set(v.core.map((r) => r[0]))] : null;
  const perTable = Array.isArray(v?.core) ? Object.fromEntries(tables.map((t) => [t, v.core.filter((r) => r[0] === t).length])) : null;
  row(out, { accessRead: { status: q.status, keys: v && typeof v === "object" ? Object.keys(v) : typeof v, meta: v?.meta ?? null, complete: v?.complete, rows: perTable } });
  log(`access read: ${q.status} keys=${JSON.stringify(v && typeof v === "object" ? Object.keys(v) : v).slice(0, 200)} meta=${JSON.stringify(v?.meta ?? null)} rows=${JSON.stringify(perTable)}`);
} finally {
  rmSync(work, { recursive: true, force: true });
}
row(out, { summary: counts });
log(`summary: ${JSON.stringify(counts)}`);
