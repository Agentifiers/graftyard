// rest:repos on graft-m2b (branch m3-repos): create, read, update, and delete over REST, as m3-admin (an owner of
// m3org) and m3-member (a member, not an admin). Each step's status is checked, each 2xx or 4xx body is kept as
// `<dir>/<case>.rest.json` for crates/api/gh/validate.py, and a clone shows the auto_init README. Tokens ride in
// headers and, for git, in a credential helper that reads them from a 0600 file, never argv.
//   node tests/m3/repos/smoke.mjs <replies dir> <out.jsonl>
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EDGE, api, log, row, tokens } from "../rows/lib.mjs";

const [dir, out] = process.argv.slice(2);
mkdirSync(dir, { recursive: true });
const tag = Date.now().toString(36);
const checks = [];
const keep = (name, method, path, r) => {
  writeFileSync(join(dir, `${name}.rest.json`), JSON.stringify({ method, path, status: r.status, body: r.body }, null, 2));
};
const check = (name, ok, extra = {}) => {
  checks.push({ name, ok });
  row(out, { check: name, ok, ...extra });
  log(`${ok ? "PASS" : "FAIL"} ${name} ${JSON.stringify(extra)}`);
};

const work = mkdtempSync(join(tmpdir(), "m3repos-"));
const tok = join(work, "tok");
writeFileSync(join(work, "cred.sh"), "#!/bin/sh\n[ \"$1\" = get ] || exit 0\necho username=x\necho \"password=$(cat \"$GRAFT_TOKFILE\")\"\n", { mode: 0o700 });
const genv = { ...process.env, GRAFT_TOKFILE: tok, GIT_TERMINAL_PROMPT: "0" };
const git = (args, cwd = work) =>
  execFileSync("git", ["-c", "credential.helper=", "-c", `credential.helper=${join(work, "cred.sh")}`, ...args], { cwd, env: genv, stdio: ["ignore", "pipe", "pipe"] }).toString();

try {
  const org = `rs-org-${tag}`;
  // 1. An org repository with auto_init, as m3-admin.
  let r = await api("POST", "/orgs/m3org/repos", tokens.admin, { name: org, description: "m3-repos smoke", auto_init: true, private: true, has_wiki: false });
  keep("create-org", "POST", "/orgs/{org}/repos", r);
  check("POST /orgs/m3org/repos is 201", r.status === 201 && r.body?.full_name === `m3org/${org}` && r.body?.private === true && r.body?.permissions?.admin === true, { status: r.status, id: r.body?.id });
  const id = r.body?.id;
  r = await api("GET", `/repos/m3org/${org}`, tokens.admin);
  keep("get", "GET", "/repos/{owner}/{repo}", r);
  check("GET is 200 with the same id", r.status === 200 && r.body?.id === id && r.body?.description === "m3-repos smoke" && r.body?.has_wiki === false, { status: r.status });
  // The README commit is there over git.
  writeFileSync(tok, tokens.admin, { mode: 0o600 });
  let readme = "";
  try {
    git(["clone", "-q", `${EDGE}/m3org/${org}.git`, join(work, "c")]);
    readme = readFileSync(join(work, "c", "README.md"), "utf8");
  } catch (e) { readme = `clone failed: ${String(e.stderr ?? e).slice(0, 200)}`; }
  check("the clone has the auto_init README", readme === `# ${org}\nm3-repos smoke\n`, { readme: readme.slice(0, 80) });
  let fsck = "";
  try { git(["fsck", "--strict"], join(work, "c")); fsck = "ok"; } catch (e) { fsck = String(e.stderr ?? e).slice(0, 200); }
  check("git fsck --strict on the clone", fsck === "ok", { fsck });
  // A push to the new repository.
  let pushed = false;
  try {
    const c = join(work, "c");
    writeFileSync(join(c, "x.txt"), "x\n");
    git(["add", "x.txt"], c);
    git(["-c", "user.name=rs", "-c", "user.email=rs@x", "commit", "-qm", "x"], c);
    git(["push", "-q", "origin", "HEAD:main"], c);
    pushed = true;
  } catch (e) { log(`push: ${String(e.stderr ?? e).slice(0, 200)}`); }
  check("a push to the new repository lands", pushed);
  // 2. PATCH: description, visibility, merge methods.
  r = await api("PATCH", `/repos/m3org/${org}`, tokens.admin, { description: "patched", private: false, allow_rebase_merge: false, has_issues: false });
  keep("patch", "PATCH", "/repos/{owner}/{repo}", r);
  check("PATCH is 200 with the new settings", r.status === 200 && r.body?.description === "patched" && r.body?.visibility === "public" && r.body?.allow_rebase_merge === false && r.body?.has_issues === false, { status: r.status });
  r = await api("PATCH", `/repos/m3org/${org}`, tokens.admin, { allow_merge_commit: false, allow_squash_merge: false });
  keep("patch-no-merge", "PATCH", "/repos/{owner}/{repo}", r);
  check("PATCH with every merge method off is 422", r.status === 422, { status: r.status });
  // 3. A member who is not an admin: create 403, PATCH and DELETE 403.
  r = await api("POST", "/orgs/m3org/repos", tokens.member, { name: `rs-mem-${tag}` });
  keep("create-member", "POST", "/orgs/{org}/repos", r);
  check("a member's org create is 403", r.status === 403, { status: r.status });
  r = await api("PATCH", `/repos/m3org/${org}`, tokens.member, { description: "member" });
  keep("patch-member", "PATCH", "/repos/{owner}/{repo}", r);
  check("a member's PATCH is 403", r.status === 403, { status: r.status });
  r = await api("DELETE", `/repos/m3org/${org}`, tokens.member);
  keep("delete-member", "DELETE", "/repos/{owner}/{repo}", r);
  check("a member's DELETE is 403", r.status === 403, { status: r.status });
  // 4. A taken name and a bad body.
  r = await api("POST", "/orgs/m3org/repos", tokens.admin, { name: org });
  keep("create-taken", "POST", "/orgs/{org}/repos", r);
  check("creating a taken name is 422", r.status === 422, { status: r.status });
  r = await api("POST", "/orgs/m3org/repos", tokens.admin, {});
  keep("create-no-name", "POST", "/orgs/{org}/repos", r);
  check("creating with no name is 422", r.status === 422, { status: r.status });
  r = await api("POST", "/orgs/m3org/repos", null, { name: `rs-x-${tag}` }, { authorization: `token ghp_${"0".repeat(36)}` });
  check("creating with a bad token is 401", r.status === 401, { status: r.status });
  // 5. DELETE, then 404, a refused push, and the name held.
  r = await api("DELETE", `/repos/m3org/${org}`, tokens.admin);
  keep("delete", "DELETE", "/repos/{owner}/{repo}", r);
  check("DELETE is 204", r.status === 204, { status: r.status });
  r = await api("GET", `/repos/m3org/${org}`, tokens.admin);
  keep("get-deleted", "GET", "/repos/{owner}/{repo}", r);
  check("GET after DELETE is 404", r.status === 404, { status: r.status });
  let refused = false;
  try { git(["push", "-q", "origin", "HEAD:refs/heads/after"], join(work, "c")); } catch { refused = true; }
  check("a push after DELETE is refused", refused);
  r = await api("DELETE", `/repos/m3org/${org}`, tokens.admin);
  check("a second DELETE is 404", r.status === 404, { status: r.status });
  r = await api("POST", "/orgs/m3org/repos", tokens.admin, { name: org });
  check("re-creating a deleted name is 422 while it is held", r.status === 422, { status: r.status });
  // 6. A user repository, then a rename.
  const mine = `rs-user-${tag}`;
  r = await api("POST", "/user/repos", tokens.member, { name: mine, auto_init: false });
  keep("create-user", "POST", "/user/repos", r);
  check("POST /user/repos is 201 under the caller", r.status === 201 && r.body?.full_name === `m3-member/${mine}` && r.body?.organization === undefined, { status: r.status });
  r = await api("POST", "/user/repos", tokens.member, { name: `${mine}-i`, visibility: "internal" });
  keep("create-user-internal", "POST", "/user/repos", r);
  check("an internal user repository is 422", r.status === 422, { status: r.status });
  r = await api("PATCH", `/repos/m3-member/${mine}`, tokens.member, { name: `${mine}-2` });
  keep("patch-rename", "PATCH", "/repos/{owner}/{repo}", r);
  check("PATCH name renames", r.status === 200 && r.body?.name === `${mine}-2`, { status: r.status });
  r = await api("GET", `/repos/m3-member/${mine}-2`, tokens.member);
  check("GET under the new name is 200", r.status === 200 && r.body?.name === `${mine}-2`, { status: r.status });
  r = await api("DELETE", `/repos/m3-member/${mine}-2`, tokens.member);
  check("the user deletes its own repository (204)", r.status === 204, { status: r.status });
} finally {
  rmSync(work, { recursive: true, force: true });
}
const failed = checks.filter((c) => !c.ok).map((c) => c.name);
row(out, { summary: { checks: checks.length, passed: checks.length - failed.length, failed } });
log(`smoke: ${checks.length - failed.length}/${checks.length}`);
