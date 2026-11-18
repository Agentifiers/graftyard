// A second App for M3 close's stateless issue writes: `m3-triage`, owned by `m3-admin`, registered the M3-WP1 way
// and installed on `m3org` with `issues: write` and `metadata: read`, beside setup.ts's `m3-ci`, which has `contents`
// only. Reuses setup.ts's RSA key (<state>/app.pem): the record holds only its public half. Idempotent. Adds
// `triageApp` and `triageInstallation` to <state>/m3.json.
//
//   node tests/m3/hosted/triage.ts --out ~/.graft-m3h

import { createHash, createPrivateKey, createPublicKey, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { out: { type: "string" } } });
if (!opt.out) throw new Error("usage: triage.ts --out DIR");
const ROOT = new URL("./", import.meta.url).pathname;
const EDGE = process.env.GRAFT_TOOLS_EDGE ?? `https://graft-m1-wp7-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const OP = { account: null, key: null };
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);

async function edge(path: string, body: unknown): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(EDGE + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(300_000) });
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
  await edge("/create", { chain });
  const r = await edge("/commit", { chain, cmd, rid: rid() });
  const o = r.outcome;
  if (!o?.ok && !allowed.includes(refusal(o) ?? "")) throw new Error(`${chain}: ${JSON.stringify(o).slice(0, 300)}`);
  return o;
}
const be64 = (n: number) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const keyShard = (tag: string, body: Buffer) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;

const statePath = `${opt.out}/m3.json`;
const state = JSON.parse(readFileSync(statePath, "utf8"));
const ADMIN = state.admin, ORG = state.org;
const der = createPublicKey(createPrivateKey(readFileSync(`${opt.out}/app.pem`))).export({ type: "spki", format: "der" });
const secretHash = [...createHash("sha256").update(readFileSync(`${opt.out}/app.secret`, "utf8")).digest()];
const slug = "m3-triage";
const slugChain = `names:${keyShard("graftyard/names/app-slug/", Buffer.from(slug))}`;
if (!state.triageApp) {
  const o = await must(slugChain, { cred: OP, op: { ClaimSlug: { slug, owner: ADMIN } } }, "Taken");
  state.triageApp = o?.ok?.out?.App ?? (await edge("/read", { chain: slugChain, q: { slug } })).value?.slug?.app;
  if (!state.triageApp) throw new Error(`ClaimSlug: ${JSON.stringify(o)}`);
}
const app: number = state.triageApp;
const client = `Iv1.${app.toString(16).padStart(16, "0")}`;
const perms = { issues: "Write", metadata: "Read" };
const record = {
  id: app, slug, name: "M3 Triage", client, keys: [{ id: 1, public: [...der], created: Date.now() }], permissions: perms,
  events: [], webhook: null, webhook_secret: null, client_secrets: [secretHash], callbacks: [], public: true,
};
await must(`authz:${ADMIN}`, { cred: OP, op: { PutApp: { record } } });
await must(`names:${keyShard("graftyard/names/app/", be64(app))}`, { cred: OP, op: { RegisterApp: { app, owner: ADMIN } } });
await must(`names:${keyShard("graftyard/names/client/", Buffer.from(client))}`, { cred: OP, op: { RegisterClient: { client, owner: ADMIN, app } } });
state.triageInstallation ??= 7190002;
const inst = state.triageInstallation;
await must(`names:${keyShard("graftyard/names/installation/", be64(inst))}`, { cred: OP, op: { RegisterInstallation: { id: inst, app, owner: ORG } } });
await must(`authz:${ORG}`, { cred: OP, op: { Install: { id: inst, row: { app, repos: "All", permissions: perms }, app_owner: ADMIN } } });
writeFileSync(statePath, JSON.stringify(state, null, 2));
log(`App ${slug} = ${app}, client ${client}, installed on m3org as ${inst} with issues: write`);
