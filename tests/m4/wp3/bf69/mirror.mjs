// BF-69's action mirror on graft-m4a (the lead's ruling, 2026-11-25): an `actions` org (account 7100010) seeded as
// tests/m3/hosted/setup.ts seeds m3org, with m3-admin as its admin, and public repositories `actions/<name>` holding
// github.com/actions/<name>'s commits with their tags, so a workflow's `uses: actions/cache@v4` resolves on the install
// (the launch service reads only the install, `crates/actions/src/launch.rs`). Idempotent.
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf69/mirror.mjs <bare clone of actions/<name>> <name> <tag>...
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { OP, api, bed, log, must, accountShard, loginShard, repoShard, state, tokens, EDGE } from "../../../am1/hosted/lib.mjs";

const [bare, name, ...tags] = process.argv.slice(2);
if (!bare || !name || !tags.length) throw new Error("usage: mirror.mjs <bare clone> <name> <tag>...");
// MIRROR_ORG and MIRROR_ORG_ID name another org: m4a holds `actions/*` Deleted for 90 days after BF-69's teardown,
// so the parallel-save row mirrors as `actions-mirror` (7100011).
const LOGIN = process.env.MIRROR_ORG ?? "actions";
const ORG = Number(process.env.MIRROR_ORG_ID ?? 7100010);
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };

await must(`names:${accountShard(ORG)}`, { cred: OP, op: { PutAccount: { id: ORG, suspended: false, sessions_not_before: 0 } } });
await must(`names:${loginShard(LOGIN)}`, { cred: OP, op: { ClaimLogin: { login: LOGIN, kind: "Org", account: ORG } } }, "Taken");
await must(`names:${accountShard(ORG)}`, { cred: OP, op: { SetAccountLogin: { account: ORG, login: LOGIN } } });
await must(`names:${accountShard(ORG)}`, { cred: OP, op: { InitPlacement: { owner: ORG, record: TRI } } }, "RecordExists");
await must(`authz:${ORG}`, { cred: OP, op: { Open: { kind: "Org", enterprise: null } } }, "AlreadyOpened");
await must(`authz:${ORG}`, { cred: OP, op: { SetMember: { user: state.admin, role: "Admin" } } });
log(`${LOGIN} = ${ORG}, m3-admin its admin`);

// The repository, public, through the GHES route as the org's admin (auto_init off: the mirror's history is pushed).
const made = await api("POST", `/api/v3/orgs/${LOGIN}/repos`, tokens.admin, { name, private: false });
if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) {
  throw new Error(`create ${LOGIN}/${name}: ${made.status} ${JSON.stringify(made.body).slice(0, 300)}`);
}
const repo = made.body?.id ?? (await bed({ chain: `authz:${ORG}`, read: { repo: name } })).value?.repo?.repo;
log(`${LOGIN}/${name} = ${repo}`);

// Push each tag and its commit as a branch head too, so the default branch has a tip, through a 0600 credential file.
const dir = execFileSync("mktemp", ["-d"]).toString().trim();
const tok = join(dir, ".tok");
writeFileSync(tok, tokens.admin, { mode: 0o600 });
writeFileSync(join(dir, ".cred.sh"), `#!/bin/sh\n[ "$1" = get ] || exit 0\necho username=x\necho "password=$(cat "${tok}")"\n`, { mode: 0o700 });
const git = (...a) => execFileSync("git", ["-c", `credential.helper=${join(dir, ".cred.sh")}`, "-C", bare, ...a], { stdio: ["ignore", "pipe", "pipe"] }).toString();
const url = `${EDGE}/${LOGIN}/${name}.git`;
const refspecs = [`refs/tags/${tags[0]}^{commit}:refs/heads/main`, ...tags.map((t) => `refs/tags/${t}:refs/tags/${t}`)];
for (let i = 0; ; i++) {
  try { git("push", "--force", url, ...refspecs); break; } catch (e) {
    if (i >= 4) throw new Error(`push: ${String(e.stderr ?? e).slice(0, 300)}`);
    await new Promise((r) => setTimeout(r, 3000 * (i + 1)));
  }
}
execFileSync("rm", ["-rf", dir]);
for (const t of tags) log(`${LOGIN}/${name}@${t} = ${git("rev-parse", `refs/tags/${t}^{commit}`).trim()}`);
const out = `${process.env.HOME}/.graft-m4a/bf69-mirrors.txt`;
let prev = ""; try { prev = readFileSync(out, "utf8"); } catch {}
if (!prev.split("\n").includes(`${name} ${repo}`)) writeFileSync(out, prev + `${name} ${repo}\n`, { mode: 0o600 });
