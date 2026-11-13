// Seeds BF-38's data on a hosted install through product routes only (no `/_bed/`), so it runs on a product edge:
// a repository `m3org/<name>` with a `main` that has a README and a `src/` tree, a pull request from a branch with
// two commits, a review and a review comment, three check runs by App m3-ci (one failing, one running), a commit
// status, and the admin's web password. Prints the facts the Playwright run reads as JSON on stdout.
//   source tests/am1/lf13/env-m4a.sh   (GRAFT_EDGE, M3_STATE)
//   BF38_PASSWORD=... node tests/m4/bf38/seed.mjs [name]
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, ghs, log, tokens } from "../../am1/hosted/lib.mjs";

const name = process.argv[2] ?? `web38-${Date.now().toString(36)}`;
const R = `m3org/${name}`;
const admin = tokens.admin;
const password = process.env.BF38_PASSWORD;
if (!password) throw new Error("set BF38_PASSWORD");
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
  return r.body;
};

// The admin's web password, under the admin's own PAT (`PUT /_t/ui/settings/password`).
must("password", await api("PUT", "/_t/ui/settings/password", admin, { password }), 204);

// A create can outlast the client's timeout (161 s on graft-m4a); the retry then finds the repository made.
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const g = new Git(R).as(admin);
for (let i = 0; ; i++) {
  if (g.clone().ok) break;
  if (i > 10) throw new Error("clone");
  await new Promise((r) => setTimeout(r, 3000));
}
const write = (path, text) => {
  mkdirSync(join(g.wt, path, ".."), { recursive: true });
  writeFileSync(join(g.wt, path), text);
};
const commit = (msg) => {
  g.run(["add", "-A"]);
  const r = g.run(["commit", "-q", "-m", msg]);
  if (!r.ok) throw new Error(`commit: ${r.out}`);
  return g.run(["rev-parse", "HEAD"]).out.trim();
};
write("README.md", `# ${name}\n\nBF-38's repository on a hosted Graftyard edge.\n\n- \`src/\` holds the retry client.\n- \`docs/\` holds its notes.\n`);
write("src/retry.ts", "export const MAX_RETRIES = 2;\n\nexport function delay(attempt: number) {\n  return Math.random() * Math.min(4000, 300 * 2 ** attempt);\n}\n");
write("src/budget.ts", "export class Budget {\n  tokens = 10;\n  take() {\n    if (this.tokens < 1) return false;\n    this.tokens -= 1;\n    return true;\n  }\n}\n");
write("docs/retry.md", "Only idempotent reads retry.\n");
const base = commit("Add the retry client and its notes");
must("push main", { status: g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok ? 200 : 500, body: "" });

g.run(["checkout", "-q", "-b", "web38/retry-after"]);
write("src/retry.ts", "export const MAX_RETRIES = 2;\n\n// A 503's Retry-After wins over the jitter when it is longer.\nexport function delay(attempt: number, retryAfter?: number) {\n  const jitter = Math.random() * Math.min(4000, 300 * 2 ** attempt);\n  return Math.min(10_000, Math.max(jitter, (retryAfter ?? 0) * 1000));\n}\n");
commit("Honor Retry-After on a 503");
write("src/budget.ts", "export class Budget {\n  tokens = 10;\n  take() {\n    if (this.tokens < 1) return false;\n    this.tokens -= 1;\n    return true;\n  }\n  refill() {\n    this.tokens = Math.min(10, this.tokens + 0.1);\n  }\n}\n");
write("docs/budget.md", "A page holds 10 retry tokens.\n");
const head = commit("Refill the retry budget on success");
must("push branch", { status: g.run(["push", "-q", "origin", "HEAD:refs/heads/web38/retry-after"]).ok ? 200 : 500, body: "" });

const repo = must("repo id", await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${name}") { id } }` }));
const repoId = repo.data.repository.id;
const created = must("pr", await api("POST", "/api/graphql", admin, {
  query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number } } }",
  variables: { i: { repositoryId: repoId, baseRefName: "main", headRefName: "web38/retry-after", title: "Honor Retry-After and refill the retry budget",
    body: "Reads that fail with 503 now wait at least as long as `Retry-After` says, and the page's retry budget refills by 0.1 per success." } },
}));
const pr = created.data.createPullRequest.pullRequest;
log(`pull request #${pr.number}`);
must("issue comment", await api("POST", "/api/graphql", admin, {
  query: "mutation($i: AddCommentInput!) { addComment(input: $i) { clientMutationId } }",
  variables: { i: { subjectId: pr.id, body: "BF-38: the checks region should fail alone when its read does." } },
}));
must("review comment", await api("POST", `/api/v3/repos/${R}/pulls/${pr.number}/comments`, admin, {
  body: "Cap this at 10 s so a bad header cannot stall the page.", commit_id: head, path: "src/retry.ts", line: 5, side: "RIGHT",
}), 201);
const ci = must("m3-ci token", await ghs({}), 201);
const runs = [
  { name: "build", conclusion: "success", output: { title: "Build passed", summary: "vite build in 9.8 s." } },
  { name: "test (ubuntu-latest)", conclusion: "failure", output: { title: "1 test failed", summary: "`budget refills to 10` expected 10, got 10.1." } },
  { name: "lint", status: "in_progress", output: { title: "Linting", summary: "oxfmt and tsrs are running." } },
];
for (const run of runs) must(`check ${run.name}`, await api("POST", `/api/v3/repos/${R}/check-runs`, ci.token, { head_sha: head, ...run }), 201);
must("status", await api("POST", `/api/v3/repos/${R}/statuses/${head}`, admin, { state: "success", context: "ci/preview", description: "Preview deployed" }), 201);

// Ask for the test merge once, as a writer's single-PR read does, so `mergeable` is computed while the run starts.
await api("GET", `/api/v3/repos/${R}/pulls/${pr.number}`, admin);
console.log(JSON.stringify({ edge: EDGE, owner: "m3org", repo: name, number: pr.number, base, head, login: "m3-admin" }));
