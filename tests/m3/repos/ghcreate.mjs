// `gh repo create` (gh 2.102.1) on graft-m2b, <runs> rounds of four creates: m3-admin's `m3org/<n> --private` (GraphQL
// `createRepository`, ownerId from `GET /users/m3org`), `m3org/<n> --public --add-readme` (`POST /orgs/m3org/repos`),
// m3-member's `<n> --private` (GraphQL, no owner) and `<n> --public --add-readme` (`POST /user/repos`). Each create must
// exit 0 and print the new URL; `GET /repos/{o}/{n}` must then be 200 with the visibility asked for, and a README
// exactly when `--add-readme` was given (`GET .../commits/main/statuses`'s 200 needs `main`, so the README is checked
// with git `ls-remote`). Every repository is deleted afterwards (204). gh reads each user's token from its own 0600
// GH_CONFIG_DIR, git from a credential helper; never argv.
//   node tests/m3/repos/ghcreate.mjs <runs> <out.jsonl>
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EDGE, api, log, row, tokens } from "../rows/lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 5);
const host = new URL(EDGE).host;
const work = mkdtempSync(join(tmpdir(), "m3ghc-"));
const tok = join(work, "tok");
writeFileSync(join(work, "cred.sh"), "#!/bin/sh\n[ \"$1\" = get ] || exit 0\necho username=x\necho \"password=$(cat \"$GRAFT_TOKFILE\")\"\n", { mode: 0o700 });
const base = { ...process.env, HOME: work, GRAFT_TOKFILE: tok, GIT_TERMINAL_PROMPT: "0", GH_NO_UPDATE_NOTIFIER: "1", GH_PROMPT_DISABLED: "1", GH_HOST: host };
delete base.GH_TOKEN; delete base.GITHUB_TOKEN; delete base.GH_ENTERPRISE_TOKEN;
const ghEnv = {};
for (const [who, login] of [["admin", "m3-admin"], ["member", "m3-member"]]) {
  const d = mkdtempSync(join(tmpdir(), `m3ghc-${who}-`));
  writeFileSync(join(d, "hosts.yml"), `${host}:\n    oauth_token: ${tokens[who]}\n    user: ${login}\n    git_protocol: https\n`, { mode: 0o600 });
  ghEnv[who] = { ...base, GH_CONFIG_DIR: d };
}
const c = { runs, creates: 0, ok: 0, failures: [] };
try {
  for (let i = 1; i <= runs; i++) {
    const t = `${Date.now().toString(36)}${i}`;
    const cases = [
      { who: "admin", owner: "m3org", name: `ghc-op-${t}`, args: ["--private"], vis: "private", readme: false },
      { who: "admin", owner: "m3org", name: `ghc-or-${t}`, args: ["--public", "--add-readme"], vis: "public", readme: true },
      { who: "member", owner: "m3-member", name: `ghc-up-${t}`, args: ["--private"], vis: "private", readme: false, bare: true },
      { who: "member", owner: "m3-member", name: `ghc-ur-${t}`, args: ["--public", "--add-readme"], vis: "public", readme: true, bare: true },
    ];
    for (const k of cases) {
      c.creates++;
      const target = k.bare ? k.name : `${k.owner}/${k.name}`;
      const g = spawnSync("gh", ["repo", "create", target, ...k.args], { env: ghEnv[k.who], encoding: "utf8", timeout: 120_000 });
      const url = (g.stdout ?? "").trim().split("\n").pop();
      const get = await api("GET", `/repos/${k.owner}/${k.name}`, tokens[k.who]);
      writeFileSync(tok, tokens[k.who], { mode: 0o600 });
      const ls = spawnSync("git", ["-c", "credential.helper=", "-c", `credential.helper=${join(work, "cred.sh")}`, "ls-remote", `${EDGE}/${k.owner}/${k.name}.git`], { env: base, encoding: "utf8", timeout: 120_000 });
      const hasMain = /\trefs\/heads\/main$/m.test(ls.stdout ?? "");
      const del = await api("DELETE", `/repos/${k.owner}/${k.name}`, tokens[k.who]);
      const r = { i, case: `${k.who}:${k.args.join(" ")}`, rc: g.status, url, get: get.status, vis: get.body?.visibility, full: get.body?.full_name, main: hasMain, lsRc: ls.status, del: del.status, err: g.status ? (g.stderr ?? "").trim().slice(0, 200) : undefined };
      const ok = g.status === 0 && url === `https://${host}/${k.owner}/${k.name}` && get.status === 200 && get.body?.visibility === k.vis && get.body?.full_name === `${k.owner}/${k.name}` && hasMain === k.readme && del.status === 204;
      if (ok) c.ok++; else c.failures.push(r);
      row(out, { ...r, ok });
      log(`${ok ? "PASS" : "FAIL"} ${JSON.stringify(r)}`);
    }
  }
} finally {
  rmSync(work, { recursive: true, force: true });
  for (const e of Object.values(ghEnv)) rmSync(e.GH_CONFIG_DIR, { recursive: true, force: true });
}
row(out, { summary: { ...c, failures: c.failures.length } });
log(`gh repo create: ${c.ok}/${c.creates}`);
