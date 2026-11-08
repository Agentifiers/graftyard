// BF-35 on graft-m2b with no `Audit` consumer (`build/BUILD-PLAN.md`, M3-WP3): each run adds a new SSH key through
// `POST /user/keys` and a new PAT (stored on its `Tokens` shard and listed on `Identity` as the edge does), then at
// once reads `GET /user/keys` and `gh ssh-key list`, and the account's `Identity` fold for the PAT. A run passes when
// all three show the new key and the PAT. gh runs with its own GH_CONFIG_DIR (0700) holding m3-member's PAT, which
// it reads from there, never from argv. Rows name keys by id and fingerprint, never by text.
//   node tests/m3/rows/bf35.mjs <runs> <out.jsonl>
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { api, log, pat, read, row, sshKey, state, tokens } from "./lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 100);
const MEMBER = state.member;
const host = new URL(process.env.GRAFT_EDGE).host;
const ghDir = mkdtempSync(join(tmpdir(), "bf35-gh-"));
writeFileSync(join(ghDir, "hosts.yml"), `${host}:\n    oauth_token: ${tokens.member}\n    user: m3-member\n    git_protocol: https\n`, { mode: 0o600 });
const env = { ...process.env, GH_CONFIG_DIR: ghDir, GH_NO_UPDATE_NOTIFIER: "1", GH_PROMPT_DISABLED: "1", GH_HOST: host };
delete env.GH_TOKEN; delete env.GITHUB_TOKEN; delete env.GH_ENTERPRISE_TOKEN;
const c = { runs, created: 0, keysListed: 0, ghListed: 0, patListed: 0, all: 0, failures: [] };
try {
  for (let i = 1; i <= runs; i++) {
    const k = sshKey();
    const title = `bf35-${Date.now()}-${i}`;
    const t0 = Date.now();
    const add = await api("POST", "/user/keys", tokens.member, { title, key: `${k.line} bf35@rows` });
    const p = await pat(MEMBER, `bf35 PAT ${i}`);
    const added = Date.now() - t0;
    const list = await api("GET", "/user/keys", tokens.member);
    const inList = Array.isArray(list.body) && list.body.some((x) => x.id === add.body?.id && x.title === title && x.key === k.line);
    let ghOut = "";
    let ghRc = 0;
    try { ghOut = execFileSync("gh", ["ssh-key", "list"], { env, stdio: ["ignore", "pipe", "pipe"] }).toString(); } catch (e) { ghRc = e.status ?? -1; ghOut = String(e.stdout ?? ""); }
    const inGh = ghOut.split("\n").some((l) => l.startsWith(`${title}\t`) && l.includes(`\t${add.body?.id}\t`));
    const id = await read(`identity:${MEMBER}`, {});
    const patIn = (id?.credentials ?? []).some((x) => x.token === p.hash);
    const ok = add.status === 201 && inList && inGh && patIn;
    if (add.status === 201) c.created++;
    if (inList) c.keysListed++;
    if (inGh) c.ghListed++;
    if (patIn) c.patListed++;
    if (ok) c.all++; else c.failures.push({ i, add: add.status, inList, inGh, ghRc, patIn });
    row(out, { i, fp: k.fp.slice(0, 16), id: add.body?.id ?? null, add: add.status, addMs: added, listed: inList, gh: inGh, ghRc, pat: patIn, keys: Array.isArray(list.body) ? list.body.length : null });
    if (i % 10 === 0) log(`BF-35 ${i}/${runs}: ${c.all} pass`);
  }
} finally {
  rmSync(ghDir, { recursive: true, force: true });
}
row(out, { summary: { ...c, failures: c.failures.length, failureRows: c.failures.slice(0, 5) } });
log(`BF-35: ${JSON.stringify({ ...c, failures: c.failures.length })}`);
