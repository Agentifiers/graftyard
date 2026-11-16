// Registers the `graftyard-hosted-runners` App on graft-m4a and installs it on m3org (`design/SHARED.md`, "Actions",
// Hosted runners: "installed on each owner that enables hosted runners with self-hosted-runner write permission"), the
// way tests/m3/hosted/setup.ts registered `m3-ci`: the slug claim, `PutApp` on the owner's `authz:` chain, the App and
// client routes, `RegisterInstallation`, then `Install`, all as the operator through the tools edge. The App's RSA key
// is made here into <out>/app.pem (mode 0600) and only its public half is sent; `graftyard runners init --app-key`
// takes the file. Writes <out>/app.json (App id, installation id, owner). Idempotent.
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp4/app.mjs ~/.graft-m4a/hosted
// With HOOK=<url> it also does R24's step once the edge enabled Authz 0x12.13 and the site set GRAFTYARD_APP_HOOKS:
// `actions: read` and `events: [workflow_job]` on the record and the installation, then the webhook through GHES's own
// `PATCH /app/hook/config` under the App's JWT, with the secret read from <out>/webhook.secret (the file `graftyard
// runners init --webhook-secret` sets as `WEBHOOK_SECRET`). The secret is sent only in that request's body.
import { createHash, createPrivateKey, createPublicKey, createSign, generateKeyPairSync, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { EDGE, log, rid, secret, tools } from "../../am1/hosted/lib.mjs";

const out = process.argv[2];
if (!out) throw new Error("usage: app.mjs <out dir>");
mkdirSync(out, { recursive: true, mode: 0o700 });
const OP = { account: null, key: null };
const ORG = 7100001, ADMIN = 7100003;
const SLUG = "graftyard-hosted-runners";
const INSTALLATION = 7190002;
const HOOK = process.env.HOOK;
const PERMS = { organization_self_hosted_runners: "Write", metadata: "Read", ...(HOOK ? { actions: "Read" } : {}) };

const keyShard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const refusal = (o) => o?.refused?.err?.split?.(/[ ({]/)[0];
async function must(chain, cmd, ...allowed) {
  await tools("/create", { chain });
  const o = (await tools("/commit", { chain, cmd, rid: rid() })).outcome;
  if (!o?.ok && !allowed.includes(refusal(o) ?? "")) throw new Error(`${chain}: ${JSON.stringify(o).slice(0, 300)}`);
  return o;
}

const pem = `${out}/app.pem`;
if (!existsSync(pem)) {
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  writeFileSync(pem, privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
}
const der = createPublicKey(createPrivateKey(readFileSync(pem))).export({ type: "spki", format: "der" });
const statePath = `${out}/app.json`;
const state = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : {};
const slugChain = `names:${keyShard("graftyard/names/app-slug/", Buffer.from(SLUG))}`;
if (!state.app) {
  const o = await must(slugChain, { cred: OP, op: { ClaimSlug: { slug: SLUG, owner: ADMIN } } }, "Taken");
  state.app = o?.ok?.out?.App ?? (await tools("/read", { chain: slugChain, q: { slug: SLUG } })).value?.slug?.app;
  if (!state.app) throw new Error(`ClaimSlug: ${JSON.stringify(o)}`);
}
const app = state.app;
const client = `Iv1.${app.toString(16).padStart(16, "0")}`;
const secretHash = [...createHash("sha256").update(randomBytes(20).toString("hex")).digest()];
const record = {
  id: app, slug: SLUG, name: "Graftyard hosted runners", client, keys: [{ id: 1, public: [...der], created: Date.now() }],
  permissions: PERMS, events: HOOK ? ["workflow_job"] : [], webhook: null, webhook_secret: null,
  client_secrets: [secretHash], callbacks: [], public: false,
};
// `PutApp` writes the whole record and clears a webhook set before, so with HOOK the `PATCH` below runs after it.
await must(`authz:${ADMIN}`, { cred: OP, op: { PutApp: { record } } });
await must(`names:${keyShard("graftyard/names/app/", be64(app))}`, { cred: OP, op: { RegisterApp: { app, owner: ADMIN } } });
await must(`names:${keyShard("graftyard/names/client/", Buffer.from(client))}`, { cred: OP, op: { RegisterClient: { client, owner: ADMIN, app } } });
await must(`names:${keyShard("graftyard/names/installation/", be64(INSTALLATION))}`, { cred: OP, op: { RegisterInstallation: { id: INSTALLATION, app, owner: ORG } } });
await must(`authz:${ORG}`, { cred: OP, op: { Install: { id: INSTALLATION, row: { app, repos: "All", permissions: PERMS }, app_owner: ADMIN } } });
Object.assign(state, { installation: INSTALLATION, owner: "m3org", owner_id: ORG, client });
writeFileSync(statePath, JSON.stringify(state, null, 2), { mode: 0o600 });
log(`App ${SLUG} = ${app}, installed on m3org as ${INSTALLATION} with ${JSON.stringify(PERMS)}`);

if (HOOK) {
  const b = (x) => Buffer.from(x).toString("base64url"), now = Math.floor(Date.now() / 1000);
  const h = b(JSON.stringify({ alg: "RS256", typ: "JWT" })), p = b(JSON.stringify({ iat: now - 30, exp: now + 540, iss: app }));
  const jwt = secret(`${h}.${p}.${b(createSign("RSA-SHA256").update(`${h}.${p}`).sign(readFileSync(pem)))}`);
  const hookSecret = secret(readFileSync(`${out}/webhook.secret`, "utf8").trim());
  const r = await fetch(`${EDGE}/api/v3/app/hook/config`, {
    method: "PATCH",
    headers: { authorization: `Bearer ${jwt}`, accept: "application/vnd.github+json", "content-type": "application/json" },
    body: JSON.stringify({ url: HOOK, content_type: "json", insecure_ssl: "0", secret: hookSecret }),
  });
  const v = await r.json().catch(() => null);
  if (r.status !== 200 || v?.url !== HOOK || v?.secret !== "********") throw new Error(`PATCH /app/hook/config: ${r.status}`);
  log(`App ${SLUG} webhook ${v.url}, content_type ${v.content_type}, secret set`);
}
