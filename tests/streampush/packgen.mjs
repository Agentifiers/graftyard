// A deterministic git pack of pseudo-random blobs, for pushes too large to send from the orb (stream-push's 500 MB and
// 2 GB rows). `node packgen.mjs manifest <branch> <MiB>` runs the generator once and prints the manifest: each blob's
// Adler-32, the tree and commit objects, the commit id, and the pack's trailer. `pack(manifest)` then yields the same
// bytes again from the manifest alone, which `driver.js` runs inside Cloudflare without hashing anything.
// Blobs are 64 MiB of xorshift32 words, stored uncompressed in zlib "stored" blocks so the pack is as big as its data.

export const PIECE = 1 << 20;
export const BLOB = 64 << 20;

function sizes(mib) {
  const out = [];
  for (let left = mib * 1048576; left > 0; left -= BLOB) out.push(Math.min(left, BLOB));
  return out;
}

// One blob's data in PIECE-sized pieces.
function* blobData(i, size) {
  let x = (Math.imul(i + 1, 0x9e3779b9) ^ 0x5bd1e995) >>> 0 || 1;
  for (let at = 0; at < size; at += PIECE) {
    const w = new Uint32Array(Math.min(PIECE, size - at) / 4);
    for (let k = 0; k < w.length; k++) {
      x ^= x << 13; x >>>= 0; x ^= x >>> 17; x ^= x << 5; x >>>= 0;
      w[k] = x;
    }
    yield new Uint8Array(w.buffer);
  }
}

function objHeader(type, size) {
  const out = [];
  let b = (type << 4) | (size & 15);
  size = Math.floor(size / 16);
  while (size > 0) { out.push(b | 0x80); b = size & 0x7f; size = Math.floor(size / 128); }
  out.push(b);
  return Uint8Array.from(out);
}

// One piece framed as zlib stored blocks; `last` sets BFINAL on its last block.
function stored(piece, last) {
  const n = Math.ceil(piece.length / 65535) || 1;
  const out = new Uint8Array(piece.length + n * 5);
  let o = 0;
  for (let i = 0; i < n; i++) {
    const d = piece.subarray(i * 65535, Math.min(piece.length, (i + 1) * 65535));
    out[o++] = last && i === n - 1 ? 1 : 0;
    out[o++] = d.length & 255; out[o++] = d.length >> 8;
    out[o++] = ~d.length & 255; out[o++] = (~d.length >> 8) & 255;
    out.set(d, o); o += d.length;
  }
  return out;
}

const be32 = (v) => Uint8Array.from([v >>> 24, (v >>> 16) & 255, (v >>> 8) & 255, v & 255]);
const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

/** The pack's bytes, from `m` (the manifest), in pieces of about PIECE bytes. */
export function* pack(m) {
  const blobs = sizes(m.mib);
  yield Uint8Array.from([0x50, 0x41, 0x43, 0x4b, 0, 0, 0, 2, ...be32(blobs.length + 2)]);
  for (const [i, size] of blobs.entries()) {
    yield Uint8Array.from([...objHeader(3, size), 0x78, 0x01]);
    let at = 0;
    for (const p of blobData(i, size)) {
      at += p.length;
      yield stored(p, at === size);
    }
    yield be32(m.adler[i]);
  }
  yield b64(m.tree);
  yield b64(m.commit);
  yield b64(m.trailer);
}

/** The receive-pack request's commands: create `refs/heads/<branch>` at the commit. */
export function head(m) {
  const line = `${"0".repeat(40)} ${m.oid} refs/heads/${m.branch}\0report-status side-band-64k agent=git/2.47.0\n`;
  const b = new TextEncoder().encode(line);
  return new Uint8Array([...new TextEncoder().encode((b.length + 4).toString(16).padStart(4, "0")), ...b,
    ...new TextEncoder().encode("0000")]);
}

async function manifest(branch, mib) {
  const { createHash } = await import("node:crypto");
  const { deflateSync } = await import("node:zlib");
  const blobs = sizes(mib);
  const adler = [], ids = [];
  for (const [i, size] of blobs.entries()) {
    const h = createHash("sha1").update(`blob ${size}\0`);
    let a = 1, b = 0;
    for (const p of blobData(i, size)) {
      h.update(p);
      for (let k = 0; k < p.length; k += 5552) {
        const end = Math.min(p.length, k + 5552);
        for (let j = k; j < end; j++) { a += p[j]; b += a; }
        a %= 65521; b %= 65521;
      }
    }
    adler.push(((b << 16) | a) >>> 0);
    ids.push(h.digest());
  }
  const obj = (type, body) => [createHash("sha1").update(`${type} ${body.length}\0`).update(body).digest(),
    Buffer.concat([objHeader(type === "tree" ? 2 : 1, body.length), deflateSync(body)])];
  const names = blobs.map((_, i) => `f${i}`).sort((x, y) => Buffer.compare(Buffer.from(x), Buffer.from(y)));
  const treeBody = Buffer.concat(names.map((n) => Buffer.concat([Buffer.from(`100644 ${n}\0`), ids[Number(n.slice(1))]])));
  const [treeId, tree] = obj("tree", treeBody);
  const who = "sp <sp@x> 1763085600 +0000";
  const [oid, commit] = obj("commit", Buffer.from(`tree ${treeId.toString("hex")}\nauthor ${who}\ncommitter ${who}\n\nstream-push ${branch}\n`));
  const m = { branch, mib, adler, tree: tree.toString("base64"), commit: commit.toString("base64"), trailer: "", oid: oid.toString("hex") };
  const h = createHash("sha1");
  let n = 0;
  // With no trailer yet, the last piece is empty, so this hashes the pack ahead of its trailer.
  for (const p of pack(m)) { h.update(p); n += p.length; }
  m.trailer = h.digest().toString("base64");
  m.bytes = n + 20;
  return m;
}

if (typeof process !== "undefined" && process.argv?.[2] === "manifest") {
  console.log(JSON.stringify(await manifest(process.argv[3], Number(process.argv[4]))));
} else if (typeof process !== "undefined" && process.argv?.[2] === "write") {
  // node packgen.mjs write <manifest.json> > x.pack : the pack itself, to check the generator with git index-pack.
  const { readFileSync, writeSync } = await import("node:fs");
  for (const p of pack(JSON.parse(readFileSync(process.argv[3], "utf8")))) writeSync(1, p);
}
