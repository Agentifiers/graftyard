// The SSH front's host keys (design/DESIGN.md, Shape, "SSH front"; "Install operations", Rotation runbook,
// `SshHostKeys`). Draws one slot, one Ed25519, one ECDSA P-256, and one RSA 3072 key with ssh-keygen, writes their
// private halves, concatenated in OpenSSH PEM, to the Secrets Store secret `SSH_HOST_KEY_<slot>` (the site deploy's
// `--ssh <store id>` binds both slots), and writes the public JSON both slots' public keys and SHA-256 fingerprints
// make, which the edge deploy's `--ssh <file>` serves at /api/graftyard/v1/meta/ssh. The private keys never touch disk
// outside a 0700 temporary folder that is removed before exit. Usage:
//   node deploy/ssh-front/hostkeys.mjs <store id> <slot A|B> <public JSON> [--active A|B]
// Rotation: draw the idle slot, deploy the edge with the new public JSON (both slots listed) and each site, one at a
// time, so every front announces both; once clients have learned them (UpdateHostKeys), deploy the sites again with
// `--var SSH_HOST_KEY_ACTIVE=<new slot>`. Needs CLOUDFLARE_API_TOKEN (Secrets Store edit) and CLOUDFLARE_ACCOUNT_ID.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [store, slot, out, ...flags] = process.argv.slice(2);
if (!store || !["A", "B"].includes(slot) || !out) throw new Error("usage: hostkeys.mjs <store id> <A|B> <public JSON> [--active A|B]");
const ai = flags.indexOf("--active");
const KEYS = [["ed25519", []], ["ecdsa", ["-b", "256"]], ["rsa", ["-b", "3072"]]];

const dir = mkdtempSync(join(tmpdir(), "hostkeys-"));
let priv = "";
const pub = [];
try {
  for (const [type, bits] of KEYS) {
    const f = join(dir, type);
    execFileSync("ssh-keygen", ["-q", "-t", type, ...bits, "-N", "", "-C", "", "-f", f]);
    priv += readFileSync(f, "utf8");
    const [kind, blob] = readFileSync(`${f}.pub`, "utf8").trim().split(/\s+/);
    const fp = execFileSync("ssh-keygen", ["-l", "-E", "sha256", "-f", `${f}.pub`]).toString().split(/\s+/)[1];
    pub.push({ type: kind, key: `${kind} ${blob}`, fingerprint: fp });
  }
} finally {
  rmSync(dir, { recursive: true, force: true });
}

// The secret's value goes to curl on stdin, never argv; so does the token.
const api = (method, path, body) => {
  const args = ["-sS", "-X", method, "-H", "@-", `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/secrets_store/stores/${store}/secrets${path}`];
  const headers = `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}\ncontent-type: application/json\n`;
  if (body === undefined) return JSON.parse(execFileSync("curl", args, { input: headers }));
  const f = join(mkdtempSync(join(tmpdir(), "hostkeys-body-")), "b.json");
  writeFileSync(f, JSON.stringify(body), { mode: 0o600 });
  try {
    return JSON.parse(execFileSync("curl", [...args, "--data-binary", `@${f}`], { input: headers }));
  } finally {
    rmSync(join(f, ".."), { recursive: true, force: true });
  }
};
const name = `SSH_HOST_KEY_${slot}`;
const found = api("GET", `?search=${name}`).result?.find((s) => s.name === name);
const comment = `graft-sshd host keys, slot ${slot}`;
const r = found
  ? api("PATCH", `/${found.id}`, { value: priv, comment })
  : api("POST", "", [{ name, value: priv, comment, scopes: ["workers"] }]);
if (!r.success) throw new Error(`${name}: ${JSON.stringify(r.errors)}`);

const meta = existsSync(out) ? JSON.parse(readFileSync(out, "utf8")) : { active: "A", slots: {} };
meta.slots[slot] = pub;
if (ai >= 0) meta.active = flags[ai + 1];
writeFileSync(out, JSON.stringify(meta));
console.log(`${name} written; ${out}: active ${meta.active}, slot ${slot}: ${pub.map((p) => p.fingerprint).join(" ")}`);
