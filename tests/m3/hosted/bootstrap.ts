// `graftyard admin bootstrap` against the graft-m1 install, end to end (`design/DESIGN.md`, "Install operations",
// First admin): the CLI draws the link and writes its `PutCode` request; this commits it through the M1-WP7 tools edge
// as the operator (no operator route exists yet, R14); then the link signs m3-admin in once at the edge's
// `POST /login/oauth/link`, and a second use is refused. The test install has no genesis-made install account
// group, so the install account (id 1, block 0) is given what `after_genesis` would: its `Placement` row and an
// opened `Authz` chain. Idempotent. The link's secret stays in a 0600 file under <state>; it never reaches argv.
//
//   node tests/m3/hosted/bootstrap.ts --out ~/.graft-m3h --cli <graftyard binary> [--runs N]

import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { out: { type: "string" }, cli: { type: "string" }, runs: { type: "string" } } });
if (!opt.out || !opt.cli) throw new Error("usage: bootstrap.ts --out DIR --cli BIN [--runs N]");
const ROOT = new URL("./", import.meta.url).pathname;
const TOOLS = process.env.GRAFT_TOOLS_EDGE ?? `https://graft-m1-wp7-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const OP = { account: null, key: null };
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
const INSTALL = 1;
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);

async function tools(path: string, body: unknown): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(TOOLS + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(300_000) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 300)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 4) throw e;
      log(`${path}: ${String(e).slice(0, 200)}; retrying`);
      await new Promise((r) => setTimeout(r, 2000 * (i + 1)));
    }
  }
}
function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}
const refusal = (o: any): string | undefined => o?.refused?.err?.split?.(/[ ({]/)[0];
async function must(chain: string, cmd: unknown, ...allowed: string[]): Promise<any> {
  await tools("/create", { chain });
  const r = await tools("/commit", { chain, cmd, rid: rid() });
  const o = r.outcome;
  if (!o?.ok && !allowed.includes(refusal(o) ?? "")) throw new Error(`${chain}: ${JSON.stringify(r).slice(0, 400)}`);
  return o;
}
const be64 = (n: number) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const accountShard = (id: number) => createHash("sha256").update("graftyard/names/account/").update(be64(id)).digest().readUInt32BE(0) % 1024;

const state = JSON.parse(readFileSync(`${opt.out}/m3.json`, "utf8"));
const ADMIN: number = state.admin;
// The install account, as genesis acceptance makes it (`workers/edge/src/release.rs`, `after_genesis`).
await must(`names:${accountShard(INSTALL)}`, { cred: OP, op: { InitPlacement: { owner: INSTALL, record: TRI } } }, "RecordExists");
await must(`authz:${INSTALL}`, { cred: OP, op: { Open: { kind: "Org", enterprise: null } } }, "AlreadyOpened");
log(`install account ${INSTALL}: Placement row and Authz opened`);

async function link(body: string): Promise<{ status: number; body: any }> {
  const r = await fetch(`${EDGE}/login/oauth/link`, { method: "POST", body, headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json" }, signal: AbortSignal.timeout(120_000) });
  const t = await r.text();
  let b: any = t;
  try { b = JSON.parse(t); } catch {}
  return { status: r.status, body: b };
}

const runs = Number(opt.runs ?? 1);
const tally = { put: 0, first201: 0, second401: 0, wrong401: 0 };
for (let n = 1; n <= runs; n++) {
  const dir = mkdtempSync(join(tmpdir(), "m3-bootstrap-"));
  try {
    const linkFile = join(dir, "link"), req = join(dir, "bootstrap.request.json");
    execFileSync(opt.cli, ["admin", "bootstrap", "--account", String(ADMIN), "--link-out", linkFile, "--out", req], { stdio: ["ignore", "ignore", "inherit"] });
    const r = JSON.parse(readFileSync(req, "utf8"));
    const o = await must(r.chain, r.cmd);
    if (o?.ok) tally.put++;
    const secret = readFileSync(linkFile, "utf8").trim();
    const form = (l: string) => new URLSearchParams({ link: l, install: String(INSTALL) }).toString();
    const first = await link(form(secret));
    const second = await link(form(secret));
    const wrong = await link(form(secret.slice(0, -1) + (secret.endsWith("0") ? "1" : "0")));
    if (first.status === 201 && first.body?.account === ADMIN && first.body?.method === "Link") tally.first201++;
    if (second.status === 401) tally.second401++;
    if (wrong.status === 401) tally.wrong401++;
    log(`run ${n}: PutCode ${o?.ok ? "ok" : JSON.stringify(o)} on ${r.chain}; first ${first.status} ${JSON.stringify(first.body).slice(0, 160)}; again ${second.status}; wrong link ${wrong.status}`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
log(`runs=${runs} put=${tally.put} first_201=${tally.first201} second_401=${tally.second401} wrong_401=${tally.wrong401}`);
