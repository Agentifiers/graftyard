// The SSH front's host keys (design/DESIGN.md, Shape, "SSH front"; "Install operations", Rotation runbook,
// `SshHostKeys`). Draws one slot, one Ed25519, one ECDSA P-256, and one RSA 3072 key with ssh-keygen, writes their
// private halves, concatenated in OpenSSH PEM, to the Secrets Store secret `SSH_HOST_KEY_<slot>` (the site deploy's
// `--ssh <store id>` binds both slots), and writes the public JSON both slots' public keys and SHA-256 fingerprints
// make. The edge serves /api/graftyard/v1/meta/ssh from the sites' registries (`GET /ssh_slots`, which read the
// slots' public halves from Secrets Store) and takes the active slot from the `Release` view; the edge deploy's
// `--ssh <file>` JSON is only its fallback while no site answers. The private keys never touch disk outside a 0700
// temporary folder that is removed before exit. Usage:
//   node deploy/ssh-front/hostkeys.mjs <store id> <slot A|B> <public JSON> --sites <sites.json>
// `--sites` names the install's site scripts. The secret names carry no install prefix, and test installs share one
// account and store, so before drawing a key the script refuses when any other script binds the slot's secret
// (workers/site/sshguard.mjs, `refuseForeignHostKey`; decisions/OPEN-QUESTIONS.md, R60).
// Rotation, with no new script version: write the idle slot with this script (every graft-sshd announces it within a
// minute, and the edge lists it), keep the public JSON for the next edge deploy, then commit the customer's
// `Rotate { SshHostKeys, slot }`. graft-ops probes every active site's `/ssh_slots` for the slot's 3 keys, waits
// `SSH_ANNOUNCE_FOR` (30 days) while every graft-sshd announces both slots, then switches; each graft-sshd serves the
// new slot within a minute. No `--var SSH_HOST_KEY_ACTIVE`, no site day. Needs CLOUDFLARE_API_TOKEN (Secrets Store
// edit) and CLOUDFLARE_ACCOUNT_ID.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { cfApi, refuseForeignHostKey } from "../../workers/site/sshguard.mjs";

const args = process.argv.slice(2);
const si = args.indexOf("--sites");
const sitesPath = si >= 0 ? args.splice(si, 2)[1] : null;
const [store, slot, out] = args;
if (!store || !["A", "B"].includes(slot) || !out || !sitesPath) {
  throw new Error("usage: hostkeys.mjs <store id> <A|B> <public JSON> --sites <sites.json>");
}
const siteScripts = JSON.parse(readFileSync(sitesPath, "utf8")).map((s) => s.script);
await refuseForeignHostKey(cfApi, process.env.CLOUDFLARE_ACCOUNT_ID, { store, secret: `SSH_HOST_KEY_${slot}`, siteScripts });
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
writeFileSync(out, JSON.stringify(meta));
console.log(`${name} written; ${out}: slot ${slot}: ${pub.map((p) => p.fingerprint).join(" ")}`);
