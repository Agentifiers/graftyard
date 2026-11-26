// bf77bed's requests (`run.sh`): BF-77's token-half probes through the edge's own wasm, as a job's `GITHUB_TOKEN`
// makes them. On top of coldwrite's seed (owner `cw`, repository `r`, a PAT) it publishes an Ed25519 verifying key on
// `tokens:0` (kid 1), installs the install's Actions App on the owner (`AuthzOp::Install` of `actions_row`), and signs
// three `ghs_` tokens for the repository with BF-77's jobs' permissions (`rw`, `ro`, `default`). Each token then
// sends `POST /repos/cw/r/issues` and `POST /repos/cw/r/statuses/{sha}` to edge script EDGE, and the PAT sends both
// as a user. Prints one line per probe and the rendered `creator` of rw's status; tokens are never printed.
//   node tests/bf77bed/drive.mjs <front url> <bed dir>
// Edge 0 (the base build) must answer what main answers (issue create 404, a token's status 401); edge 1 (the head)
// BF-77's lines (rw 201, the others 403, the issue's `user` and the status's `creator` `github-actions[bot]`). One
// process drives both, because the key and the installation are committed once. Exits 1 on any other answer.
import { generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { readFileSync } from "node:fs";

const [F, B] = process.argv.slice(2);
const seed = JSON.parse(readFileSync(`${B}/seed.json`, "utf8"));
const OP = { account: null, key: null };
const OWNER = 7000001, ACTIONS_APP = 2;
const b64 = (b) => Buffer.from(b).toString("base64url");

async function commit(chain, op, extra = {}) {
  const r = await fetch(`${F}/_bed/identity`, {
    method: "POST", headers: { "x-bed-token": "cw-bed", "x-cw-edge": "0" },
    body: JSON.stringify({ chain, cmd: { cred: OP, op }, ...extra }),
  });
  const v = await r.json();
  if (!v.outcome?.ok) throw new Error(`${chain} ${Object.keys(op)[0]}: ${JSON.stringify(v).slice(0, 400)}`);
}

// The verifying key, kid 1, and the Actions App's installation on the owner (its id is the owner's negated).
const { privateKey, publicKey } = generateKeyPairSync("ed25519");
const pub = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
await commit("tokens:0", { PutVerifyingKey: { kid: 1, key: [...pub], retired: null } }, { create: true });
const scopes = ["actions", "checks", "contents", "deployments", "discussions", "issues", "packages", "pages",
  "pull_requests", "repository_projects", "security_events", "statuses"];
const row = Object.fromEntries([...scopes.map((s) => [s, "Write"]), ["metadata", "Read"]].sort());
await commit(`authz:${OWNER}`, { Install: { id: -OWNER, row: { app: ACTIONS_APP, repos: "All", permissions: row }, app_owner: OWNER } });

function ulid() {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = (BigInt(Date.now()) << 80n) | BigInt("0x" + randomBytes(10).toString("hex"));
  let s = "";
  for (let i = 0; i < 26; i++) { s = A[Number(v & 31n)] + s; v >>= 5n; }
  return s;
}
// `stateless::sign`'s form: `ghs_{app}_` and an EdDSA JWS of the claims' payload.
function ghs(perms) {
  const iat = Math.floor(Date.now() / 1000) - 5;
  const header = { alg: "EdDSA", typ: "JWT", kid: "1" };
  const payload = { iss: ACTIONS_APP, ins: -OWNER, repos: [seed.id], perms, jti: ulid(), iat, exp: iat + 3600 };
  const signed = `${b64(JSON.stringify(header))}.${b64(JSON.stringify(payload))}`;
  return `ghs_${ACTIONS_APP}_${signed}.${b64(sign(null, Buffer.from(signed), privateKey))}`;
}
// BF-77's jobs (`tests/m4/wp3/bf77/perms.yml`): GHES's restricted default, `contents: read`, and the write control.
const jobs = {
  default: ghs({ contents: "read", metadata: "read", packages: "read" }),
  ro: ghs({ contents: "read", metadata: "read" }),
  rw: ghs({ contents: "write", issues: "write", metadata: "read", statuses: "write" }),
};
const sha = "a".repeat(40);
async function call(EDGE, tok, path, body) {
  const r = await fetch(`${F}/api/v3/repos/${seed.login}/${seed.repo}${path}`, {
    method: "POST", body: JSON.stringify(body),
    headers: { authorization: `token ${tok}`, "content-type": "application/json", "user-agent": "curl/8.5.0", "x-cw-edge": EDGE },
  });
  const t = await r.text();
  let v = null;
  try { v = JSON.parse(t); } catch {}
  return { status: r.status, location: r.headers.get("location"), body: v };
}

let bad = 0;
const check = (ok, line) => { console.log(`${ok ? "ok  " : "FAIL"} ${line}`); if (!ok) bad++; };
async function probe(EDGE, WANT) {
const call1 = (tok, path, body) => call(EDGE, tok, path, body);
const want = WANT === "head"
  ? { default: [403, 403], ro: [403, 403], rw: [201, 201] }
  : { default: [404, 401], ro: [404, 401], rw: [404, 401] };
for (const [job, tok] of Object.entries(jobs)) {
  const issue = await call1(tok, "/issues", { title: `bf77 ${job}` });
  const status = await call1(tok, `/statuses/${sha}`, { state: "success", context: `bf77-${job}` });
  check(issue.status === want[job][0] && status.status === want[job][1],
    `${job}: issue ${issue.status}, status ${status.status} (want ${want[job].join(", ")})`);
  if (WANT === "head" && job === "rw") {
    const u = issue.body?.user;
    check(issue.body?.number > 0 && issue.body?.title === "bf77 rw" && u?.login === "github-actions[bot]" && u?.type === "Bot"
      && issue.location === `https://cw.example/api/v3/repos/${seed.login}/${seed.repo}/issues/${issue.body.number}`,
    `rw issue: #${issue.body?.number} user ${u?.login} ${u?.type} location ${issue.location}`);
    const cr = status.body?.creator ?? {};
    check(cr.login === "github-actions[bot]" && cr.type === "Bot" && cr.id === ACTIONS_APP
      && cr.html_url === "https://cw.example/apps/github-actions" && cr.node_id?.startsWith("BOT_")
      && status.body?.state === "success" && status.body?.context === "bf77-rw",
    `rw status creator: ${JSON.stringify({ login: cr.login, id: cr.id, type: cr.type, node_id: cr.node_id, html_url: cr.html_url })}`);
  }
  if (WANT === "head" && job !== "rw") {
    check(issue.body?.message === "Resource not accessible by integration"
      && status.body?.message === "Resource not accessible by integration", `${job}: ${issue.body?.message} / ${status.body?.message}`);
  }
}
// A user's PAT: a REST issue create and a status, as before (the status reply's bytes are plaindiff's to check).
const pat = await call1(seed.token, "/issues", { title: "bf77 pat", labels: [], assignees: [seed.login] });
const patStatus = await call1(seed.token, `/statuses/${sha}`, { state: "pending", context: "bf77-pat" });
if (WANT === "head") {
  check(pat.status === 201 && pat.body?.user?.login === seed.login && pat.body?.assignees?.[0]?.login === seed.login
    && pat.body?.author_association === "OWNER", `pat issue: ${pat.status} user ${pat.body?.user?.login} assignee ${pat.body?.assignees?.[0]?.login}`);
} else {
  check(pat.status === 404, `pat issue: ${pat.status} (want 404 on main)`);
}
check(patStatus.status === 201 && patStatus.body?.creator?.login === seed.login && patStatus.body?.creator?.type === "User",
  `pat status: ${patStatus.status} creator ${patStatus.body?.creator?.login}`);
console.log(`${bad} unexpected answers so far (edge ${EDGE}, want ${WANT})`);
}
await probe("0", "base");
await probe("1", "head");
process.exit(bad ? 1 : 0);
