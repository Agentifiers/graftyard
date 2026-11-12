// Writes N packs shaped like hot-shards' seed packs (one ~SIZE blob, two trees, one commit, each its own chain of
// commits on one of 50 branches) to DIR/p<i>.pack, so index-pack options can be timed locally.
import { createHash, randomBytes } from "node:crypto";
import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
const [DIR, N, SIZE] = [process.argv[2], Number(process.argv[3]), Number(process.argv[4])];
mkdirSync(DIR, { recursive: true });
const sha1 = (b) => createHash("sha1").update(b).digest();
const obj = (type, body) => sha1(Buffer.concat([Buffer.from(`${type} ${body.length}\0`), body]));
const TYPE = { commit: 1, tree: 2, blob: 3 };
function hdr(type, n) { const out = []; let b = (TYPE[type] << 4) | (n & 15); n >>= 4; while (n) { out.push(b | 0x80); b = n & 0x7f; n >>= 7; } out.push(b); return Buffer.from(out); }
const tips = {};
for (let i = 0; i < N; i++) {
  const br = i % 50;
  const bytes = randomBytes(SIZE);
  const blob = obj("blob", bytes);
  const leaf = Buffer.concat([Buffer.from(`100644 f${br}.bin\0`), blob]);
  const top = Buffer.concat([Buffer.from(`40000 hot\0`), obj("tree", leaf)]);
  const root = obj("tree", top);
  const who = `hot <hot@x> ${1700000000 + i} +0000`;
  const commit = Buffer.from(`tree ${root.toString("hex")}\n${tips[br] ? `parent ${tips[br]}\n` : ""}author ${who}\ncommitter ${who}\n\nhot ${i}\n`);
  const all = [["blob", bytes], ["tree", leaf], ["tree", top], ["commit", commit]];
  const head = Buffer.alloc(12); head.write("PACK"); head.writeUInt32BE(2, 4); head.writeUInt32BE(all.length, 8);
  const body = Buffer.concat([head, ...all.flatMap(([t, b]) => [hdr(t, b.length), deflateSync(b, { level: 1 })])]);
  writeFileSync(`${DIR}/p${String(i).padStart(6, "0")}.pack`, Buffer.concat([body, sha1(body)]));
  tips[br] = obj("commit", commit).toString("hex");
}
writeFileSync(`${DIR}/tips`, Object.entries(tips).map(([b, t]) => `${t} refs/heads/hot/${b}`).join("\n") + "\n");
