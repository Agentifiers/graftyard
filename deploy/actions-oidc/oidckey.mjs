// An `ActionsOidcKey` slot (design/DESIGN.md, "Install operations", Rotation runbook; decisions/DECISIONS.md,
// "`actions-oidc`"). Draws one RSA-2048 key with openssl, writes it as PEM PKCS#8 to the Secrets Store secret
// `ACTIONS_OIDC_KEY_<slot>` (the site deploy's `--oidc <store id>` binds both slots), and prints the slot's `kid`, the
// RFC 7638 thumbprint the JWKS lists it under. The private key never touches disk outside a 0700 temporary folder
// that is removed before exit, and goes to curl on stdin, never argv. Usage:
//   node deploy/actions-oidc/oidckey.mjs <store id> <slot A|B> --sites <sites.json>
// `--sites` names the install's site scripts. The secret names carry no install prefix and test installs share one
// account and store, so the script refuses when any other script binds the slot's secret, as hostkeys.mjs does
// (workers/site/sshguard.mjs, `refuseForeignHostKey`; decisions/OPEN-QUESTIONS.md, R60). It also refuses to write the
// slot the `--active <A|B>` flag names, so a rotation never replaces the key tokens are being signed under.
// Rotation, with no new script version: write the idle slot with this script, then commit the customer's
// `Rotate { ActionsOidcKey, slot }`; graft-ops probes every site's `/oidc_slots` for the new kid, waits
// `OIDC_PUBLISHED_FOR`, then switches. Needs CLOUDFLARE_API_TOKEN (Secrets Store edit) and CLOUDFLARE_ACCOUNT_ID.
import { createHash, createPrivateKey } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { cfApi, refuseForeignHostKey } from "../../workers/site/sshguard.mjs";

const args = process.argv.slice(2);
const take = (flag) => {
  const i = args.indexOf(flag);
  return i >= 0 ? args.splice(i, 2)[1] : null;
};
const sitesPath = take("--sites");
const active = take("--active");
const [store, slot] = args;
if (!store || !["A", "B"].includes(slot) || !sitesPath) {
  throw new Error("usage: oidckey.mjs <store id> <A|B> --sites <sites.json> [--active <A|B>]");
}
if (active === slot) throw new Error(`slot ${slot} is the active slot; write the idle one`);
const name = `ACTIONS_OIDC_KEY_${slot}`;
const siteScripts = JSON.parse(readFileSync(sitesPath, "utf8")).map((s) => s.script);
await refuseForeignHostKey(cfApi, process.env.CLOUDFLARE_ACCOUNT_ID, { store, secret: name, siteScripts });

const dir = mkdtempSync(join(tmpdir(), "oidckey-"));
let priv;
try {
  const f = join(dir, "key.pem");
  execFileSync("openssl", ["genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", f], { stdio: "ignore" });
  priv = readFileSync(f, "utf8");
} finally {
  rmSync(dir, { recursive: true, force: true });
}
if (!priv.startsWith("-----BEGIN PRIVATE KEY-----")) throw new Error("openssl wrote no PKCS#8 key");
const { e, n } = createPrivateKey(priv).export({ format: "jwk" });
const kid = createHash("sha256").update(`{"e":"${e}","kty":"RSA","n":"${n}"}`).digest("base64url");

const api = (method, path, body) => {
  const a = ["-sS", "-X", method, "-H", "@-", `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/secrets_store/stores/${store}/secrets${path}`];
  const headers = `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}\ncontent-type: application/json\n`;
  if (body === undefined) return JSON.parse(execFileSync("curl", a, { input: headers }));
  const d = mkdtempSync(join(tmpdir(), "oidckey-body-"));
  const f = join(d, "b.json");
  writeFileSync(f, JSON.stringify(body), { mode: 0o600 });
  try {
    return JSON.parse(execFileSync("curl", [...a, "--data-binary", `@${f}`], { input: headers }));
  } finally {
    rmSync(d, { recursive: true, force: true });
  }
};
const found = api("GET", `?search=${name}`).result?.find((s) => s.name === name);
const comment = `Actions OIDC signing key, slot ${slot}, kid ${kid}`;
const r = found
  ? api("PATCH", `/${found.id}`, { value: priv, comment })
  : api("POST", "", [{ name, value: priv, comment, scopes: ["workers"] }]);
if (!r.success) throw new Error(`${name}: ${JSON.stringify(r.errors)}`);
console.log(`${name} written; kid ${kid}`);
