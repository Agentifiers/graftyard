// BF-50's R66 probe on graft-m6a (`decisions/OPEN-QUESTIONS.md`, R66; `bf50-row.sh` phases `seed` and `probe`): what a
// real client sees on its first request to a repository whose chains sat idle through the loss and the restore.
// The test-register chains cannot be read by any client (no owner, no name, no git fold), so `seed` makes a real one
// before the load: a user whose `Names` rows live on the install account's shard (which exists), its `Placement`
// (Tri on [0, 1, 2], home A), its opened `Authz` chain, and a PAT on an existing `Tokens` shard, all as the operator
// through the bench's `/op` and `/create` (R14: no operator route); then the repository through the product edge's
// own `POST /api/v3/user/repos` under that PAT. Every chain it touches is homed on A, so the row's loss and restore
// move it like the rest. `probe`, once `home` has committed: each probed chain's members' `/status` (role, leader,
// promise) and a 15 s wait so that read leaves nothing ticking, then `gh api` on the repository and `git ls-remote`,
// each timed from the client, first cold and then once more. The PAT stays in ~/.graft-m6a/bf50 (0600) and reaches gh
// and git through their environment only; no line names the edge's host.
//   BENCH=... EDGE=... node tests/m6/hosted/bf50-probe.mjs seed|probe <raw folder>
import { execFile } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { appendFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, join } from "node:path";

const [MODE, R] = process.argv.slice(2);
if (!["seed", "probe"].includes(MODE) || !R) throw new Error("usage: bf50-probe.mjs seed|probe <raw folder>");
const BENCH = process.env.BENCH, EDGE = process.env.EDGE;
const K = join(homedir(), ".graft-m6a", "bf50");
const BENCH_TOKEN = readFileSync(join(homedir(), ".graft-m6a", "bf50.token"), "utf8").trim();
const PAT_FILE = join(K, `${basename(R)}.probe.pat`);
const OP = { account: null, key: null };
const INSTALL = 1;
// The owner's record, as the bench's `/create` joins an owner chain (`at` 1), so its members' bootstrap check matches.
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
// The judge's exclusions (`bf50judge.py`, EXCLUDED, and m6aL's `tokens:953`): a PAT on one of these would sit on a
// group that cannot move.
const BAD_TOKEN_SHARDS = new Set([133, 169, 281, 417, 476, 518, 600, 628, 741, 745, 819, 919, 953, 1331, 1435, 1598, 2125,
  2310, 2378]);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const nshard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const accountShard = (id) => nshard("graftyard/names/account/", be64(id));
const loginShard = (l) => nshard("graftyard/names/login/", Buffer.from(l.toLowerCase()));
const repoShard = (r) => nshard("graftyard/names/repo/", be64(r));
let SECRET = null;
const line = (v) => {
  const s = JSON.stringify({ at: new Date().toISOString(), ...v });
  if ((SECRET && s.includes(SECRET)) || /ghp_[A-Za-z0-9]{36}/.test(s)) throw new Error("a token reached a log line");
  appendFileSync(join(R, "probe.jsonl"), s + "\n");
  console.log(s);
};

async function bench(path, body) {
  const r = await fetch(BENCH + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${BENCH_TOKEN}` }, signal: AbortSignal.timeout(300_000) });
  return r.json();
}

/** A chain's registry rows on every site, from the one registry shard that holds it (`MemberRegistry::for_chain`). */
async function rowsOf(chain) {
  const shard = createHash("sha256").update(chain).digest()[0] % 64;
  const { rows } = await bench("/snapshot", { sites: [0, 1, 2, 3], from: shard, count: 1 });
  return rows.filter((r) => r.chain === chain).sort((a, b) => b.generation - a.generation);
}

/** One operator command, refused only with one of `allowed`. */
async function op(chain, o, ...allowed) {
  const at = (await rowsOf(chain))[0];
  if (!at) throw new Error(`${chain}: no registry row`);
  const r = await bench("/op", { at: { site: at.site, do_name: at.do_name }, cmd: { cred: OP, op: o } });
  const name = Object.keys(o)[0];
  const ok = !!r.done?.ok || allowed.some((a) => JSON.stringify(r.done?.refused ?? "").includes(a));
  line({ step: "op", chain, op: name, ok, refused: r.done?.refused?.err ?? null, error: r.error ?? null });
  if (!ok) throw new Error(`${chain} ${name}: ${JSON.stringify(r).slice(0, 300)}`);
}

/** A classic PAT on a `Tokens` shard the judge does not exclude. */
function pat() {
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  for (;;) {
    let s = "";
    while (s.length < 30) for (const b of randomBytes(32)) if (b < 248 && s.length < 30) s += A[b % 62];
    let c = ~0; for (const ch of Buffer.from(s)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
    let v = (~c) >>> 0; const tail = Array(6).fill("0"); for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
    const h = createHash("sha256").update("graftyard/token/").update(s).digest();
    const shard = h.readUInt32BE(0) % 3072;
    if (!BAD_TOKEN_SHARDS.has(shard)) return { text: `ghp_${s}${tail.join("")}`, hash: [...h], shard };
  }
}

/** Runs a client with the PAT in its environment, timed from here: `{ms, code, out}`. */
function client(cmd, args, env) {
  const t0 = Date.now();
  return new Promise((ok) => execFile(cmd, args, { env: { PATH: process.env.PATH, HOME: process.env.HOME, ...env }, timeout: 120_000, maxBuffer: 1 << 20 },
    (e, stdout, stderr) => ok({ ms: Date.now() - t0, code: e ? (e.code ?? 1) : 0, out: String(stdout), err: String(stderr).slice(0, 300) })));
}

if (MODE === "seed") {
  // Everything on the install account's `Names` shard, which exists, so the bench's `/op` reaches it; a fresh owner per
  // row tag.
  const shard = accountShard(INSTALL);
  let owner = 7_660_001 + (createHash("sha256").update(basename(R)).digest().readUInt32BE(0) % 1_000_000);
  while (accountShard(owner) !== shard) owner++;
  let n = 0;
  const login = () => `r66-${basename(R)}-${n}`.toLowerCase();
  while (loginShard(login()) !== shard) n++;
  const names = `names:${shard}`;
  await op(names, { PutAccount: { id: owner, suspended: false, sessions_not_before: 0 } });
  await op(names, { InitPlacement: { owner, record: TRI } }, "RecordExists");
  await op(names, { ClaimLogin: { login: login(), kind: "User", account: owner } }, "Taken");
  await op(names, { SetAccountLogin: { account: owner, login: login() } });
  const made = await bench("/create", { chains: [`authz:${owner}`], home: 0 });
  line({ step: "create", chain: `authz:${owner}`, joins: made[0]?.joins, role: made[0]?.role });
  if (made[0]?.role !== "leader") throw new Error(`authz:${owner}: ${JSON.stringify(made).slice(0, 300)}`);
  await op(`authz:${owner}`, { Open: { kind: "User", enterprise: null } }, "AlreadyOpened");
  const tok = pat();
  SECRET = tok.text;
  await op(`tokens:${tok.shard}`, { Store: { hash: tok.hash, kind: "Pat", account: owner, scopes: 1, expires: null } });
  writeFileSync(PAT_FILE, tok.text, { mode: 0o600, flag: "w" });
  // The repository, through the product edge as its owner would make it.
  const name = "r66";
  const t0 = Date.now();
  // `auto_init`, so `main` holds a commit for ls-remote to list.
  const r = await fetch(`${EDGE}/api/v3/user/repos`, { method: "POST", body: JSON.stringify({ name, auto_init: true }),
    headers: { authorization: `token ${tok.text}`, accept: "application/vnd.github+json", "content-type": "application/json" },
    signal: AbortSignal.timeout(180_000) });
  const b = await r.json().catch(() => null);
  line({ step: "repo", status: r.status, ms: Date.now() - t0, full_name: b?.full_name ?? null, id: b?.id ?? null, message: b?.message ?? null });
  if (r.status !== 201) throw new Error(`POST /user/repos: ${r.status}`);
  const seed = { owner, login: login(), repo: name, full_name: b.full_name,
    chains: [`repo:${b.id}`, `authz:${owner}`, `tokens:${tok.shard}`, names, `names:${repoShard(b.id)}`] };
  writeFileSync(join(R, "probe-seed.json"), JSON.stringify(seed));
  line({ step: "seeded", ...seed });
} else {
  const seed = JSON.parse(readFileSync(join(R, "probe-seed.json"), "utf8"));
  const tok = readFileSync(PAT_FILE, "utf8").trim();
  SECRET = tok;
  // What each probed chain's members hold before the first request: idle means no member leads under a live promise.
  for (const chain of seed.chains) {
    const now = Date.now();
    const st = await Promise.all((await rowsOf(chain)).map(async (m) => {
      const v = (await bench("/member", { site: m.site, do_name: m.do_name, path: "/status" })).v ?? {};
      return { site: m.site, do_name: m.do_name, joined: v.joined, role: v.role, leader: v.leader, term: v.term, commit: v.commit,
        promise_left_ms: typeof v.promiseUntil === "number" ? v.promiseUntil - now : null };
    }));
    line({ step: "status", chain, members: st });
  }
  // A `/status` is an input that keeps its member ticking for ACTIVE_FOR_MS (3 s), and a leader's promise is LEASE
  // (9 s): 15 s leaves neither.
  await sleep(15_000);
  const host = new URL(EDGE).host;
  const gh = mkdtempSync(join(tmpdir(), "r66-gh-"));
  const ghEnv = { GH_CONFIG_DIR: gh, GH_HOST: host, GH_ENTERPRISE_TOKEN: tok, GH_NO_UPDATE_NOTIFIER: "1", GH_PROMPT_DISABLED: "1" };
  const gitEnv = { GIT_TERMINAL_PROMPT: "0", GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "http.extraHeader",
    GIT_CONFIG_VALUE_0: `Authorization: token ${tok}` };
  const rest = async (round) => {
    const r = await client("gh", ["api", "-i", "--hostname", host, `repos/${seed.full_name}`], ghEnv);
    const status = Number(r.out.match(/^HTTP\/\S+ (\d+)/)?.[1] ?? 0);
    const body = r.out.slice(r.out.indexOf("\n{") + 1);
    let full = null; try { full = JSON.parse(body).full_name; } catch {}
    const timing = r.out.match(/^server-timing: (.*)$/im)?.[1]?.slice(0, 400) ?? null;
    line({ step: "gh api", round, ms: r.ms, exit: r.code, status, ok: status === 200 && full === seed.full_name, server_timing: timing, err: r.code ? r.err.replaceAll(host, "<edge>") : undefined });
    return status === 200 && full === seed.full_name;
  };
  const git = async (round) => {
    const r = await client("git", ["ls-remote", `${EDGE}/${seed.full_name}.git`], gitEnv);
    line({ step: "git ls-remote", round, ms: r.ms, exit: r.code, refs: r.out.split("\n").filter(Boolean).length, ok: r.code === 0, err: r.code ? r.err.replaceAll(host, "<edge>") : undefined });
    return r.code === 0;
  };
  const ok = [await rest("first"), await git("first"), await rest("again"), await git("again")];
  rmSync(gh, { recursive: true, force: true });
  line({ step: "probe", ok: ok.every(Boolean) });
  process.exit(ok.every(Boolean) ? 0 : 1);
}
