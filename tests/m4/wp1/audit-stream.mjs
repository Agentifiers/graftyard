// M4-WP1's `Audit` stream on graft-m4a: a credential change reaches the Basin Pipelines stream.
//
// Adds a fresh SSH key for the admin (`POST /user/keys`: an `SshKey` entry on the key's fingerprint `Tokens` shard,
// then the `Identity` side), then reads the R2 sink (`graft_m4a_audit_sink`, JSON under `audit/` in bucket
// `graft-m4a-audit`) until rows naming both entries arrive. The consumer runs on the shard leader's alarm once an
// entry is `FINISH_AFTER` (60 s) old, the stream batches, and the sink rolls a file every 30 s, so rows land in about
// two minutes. Pass: the `SshKey` entry's row appears, keyed `chain:index:slot`, with no key material in any row.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp1/audit-stream.mjs <out.jsonl>
import { execFileSync } from "node:child_process";
import { generateKeyPairSync } from "node:crypto";
import { api, log, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const [out] = process.argv.slice(2);
if (!out) throw new Error("usage: audit-stream.mjs <out.jsonl>");
const BUCKET = process.env.AUDIT_BUCKET ?? "graft-m4a-audit";
const T = tokens.admin;

const { publicKey } = generateKeyPairSync("ed25519");
const raw = publicKey.export({ format: "der", type: "spki" }).subarray(12);
const blob = Buffer.concat([Buffer.from([0, 0, 0, 11]), Buffer.from("ssh-ed25519"), Buffer.from([0, 0, 0, 32]), raw]);
const key = `ssh-ed25519 ${blob.toString("base64")}`;
const t0 = Date.now();
const add = await api("POST", "/api/v3/user/keys", T, { title: `audit-stream ${t0}`, key });
log(`add key: ${add.status} id ${add.body?.id}`);
if (add.status !== 201) throw new Error(JSON.stringify(add.body).slice(0, 300));
row(out, { kind: "writes", at: t0, add: add.status, keyId: add.body.id });

const wrangler = (...a) => execFileSync("npx", ["-y", "wrangler@4", ...a], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
// The sink writes `audit/<uuid>.json`, with no date partition on graft-m4a's sink (measured 2026-11-14).
const prefix = "audit/";
/** Every row in the sink's files, listed through the R2 API and read with wrangler. */
async function sinkRows() {
  const list = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets/${BUCKET}/objects?prefix=${encodeURIComponent(prefix)}&per_page=1000`,
    { headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` } }).then((r) => r.json());
  const rows = [];
  for (const o of list.result ?? []) {
    const text = wrangler("r2", "object", "get", `${BUCKET}/${o.key}`, "--remote", "--pipe");
    for (const l of text.split("\n").filter(Boolean)) rows.push({ file: o.key, ...JSON.parse(l) });
  }
  return rows;
}
const mine = (r) => r.at >= t0 - 5_000 && r.action === "SshKey";
let rows = [];
for (let i = 0; i < 40; i++) {
  rows = await sinkRows();
  const hits = rows.filter((r) => r.at >= t0 - 5_000);
  log(`sink: ${rows.length} rows today, ${hits.length} since the writes: ${[...new Set(hits.map((r) => r.action))].join(", ")}`);
  if (hits.some(mine)) break;
  await sleep(15_000);
}
const hits = rows.filter((r) => r.at >= t0 - 5_000);
for (const r of hits) row(out, { kind: "row", seen: Date.now() - t0, ...r });
const ids = hits.map((r) => r.id);
const leak = rows.filter((r) => JSON.stringify(r).includes(blob.toString("base64").slice(0, 24)));
log(`rows since the writes: ${hits.length} (${new Set(ids).size} distinct ids), key material in ${leak.length} rows; ssh rows: ${hits.filter(mine).length}`);
row(out, { kind: "summary", rows: hits.length, distinct: new Set(ids).size, leak: leak.length, ssh: hits.filter(mine).length,
  firstSeenMs: hits.length ? Date.now() - t0 : null });
