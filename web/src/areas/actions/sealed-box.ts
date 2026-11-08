// libsodium's crypto_box_seal and crypto_box_seal_open, which GHES uses for Actions
// secrets: the client seals the value to the repository's or organization's public key
// (`GET .../actions/secrets/public-key`) and sends only the box. X25519 (RFC 7748), then
// XSalsa20-Poly1305 with the nonce blake2b(ephemeral_pk || recipient_pk, 24). Written out
// here because the app takes no crypto dependency; checked against PyNaCl.

const P = (1n << 255n) - 19n;
const mod = (a: bigint) => ((a % P) + P) % P;

function powMod(b: bigint, e: bigint) {
  let r = 1n;
  for (b = mod(b); e > 0n; e >>= 1n, b = (b * b) % P) if (e & 1n) r = (r * b) % P;
  return r;
}

const le = (bytes: Uint8Array) => bytes.reduceRight((n, b) => (n << 8n) | BigInt(b), 0n);

function toLe(n: bigint, length: number) {
  const out = new Uint8Array(length);
  for (let i = 0; i < length; i++, n >>= 8n) out[i] = Number(n & 0xffn);
  return out;
}

export function x25519(scalar: Uint8Array, u: Uint8Array) {
  const k = scalar.slice();
  k[0] &= 248;
  k[31] = (k[31] & 127) | 64;
  const n = le(k);
  const x1 = le(u) & ((1n << 255n) - 1n);
  let [x2, z2, x3, z3] = [1n, 0n, x1, 1n];
  let swap = 0n;
  for (let t = 254; t >= 0; t--) {
    const bit = (n >> BigInt(t)) & 1n;
    if (swap ^ bit) [x2, x3, z2, z3] = [x3, x2, z3, z2];
    swap = bit;
    const a = mod(x2 + z2);
    const b = mod(x2 - z2);
    const aa = (a * a) % P;
    const bb = (b * b) % P;
    const e = mod(aa - bb);
    const da = (mod(x3 - z3) * a) % P;
    const cb = (mod(x3 + z3) * b) % P;
    x3 = mod((da + cb) ** 2n);
    z3 = (x1 * mod((da - cb) ** 2n)) % P;
    x2 = (aa * bb) % P;
    z2 = (e * mod(aa + 121665n * e)) % P;
  }
  if (swap) [x2, z2] = [x3, z3];
  return toLe((x2 * powMod(z2, P - 2n)) % P, 32);
}

const BASE = toLe(9n, 32);
export const publicKeyOf = (secret: Uint8Array) => x25519(secret, BASE);

// ---- Salsa20 ----

const SIGMA = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];
const word = (b: Uint8Array, i: number) =>
  (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0;
const rotl = (x: number, n: number) => (x << n) | (x >>> (32 - n));

function rounds(x: Uint32Array) {
  const qr = (a: number, b: number, c: number, d: number) => {
    x[b] ^= rotl(x[a] + x[d], 7);
    x[c] ^= rotl(x[b] + x[a], 9);
    x[d] ^= rotl(x[c] + x[b], 13);
    x[a] ^= rotl(x[d] + x[c], 18);
  };
  for (let i = 0; i < 10; i++) {
    (qr(0, 4, 8, 12), qr(5, 9, 13, 1), qr(10, 14, 2, 6), qr(15, 3, 7, 11));
    (qr(0, 1, 2, 3), qr(5, 6, 7, 4), qr(10, 11, 8, 9), qr(15, 12, 13, 14));
  }
}

function state(key: Uint8Array, input16: Uint8Array) {
  const s = new Uint32Array(16);
  [s[0], s[5], s[10], s[15]] = SIGMA;
  for (let i = 0; i < 4; i++) s[1 + i] = word(key, i * 4);
  for (let i = 0; i < 4; i++) s[11 + i] = word(key, 16 + i * 4);
  for (let i = 0; i < 4; i++) s[6 + i] = word(input16, i * 4);
  return s;
}

function hsalsa20(key: Uint8Array, input16: Uint8Array) {
  const x = state(key, input16);
  rounds(x);
  const out = new Uint8Array(32);
  const view = new DataView(out.buffer);
  [0, 5, 10, 15, 6, 7, 8, 9].forEach((w, i) => view.setUint32(i * 4, x[w], true));
  return out;
}

// XSalsa20 keystream XORed into `data`.
function xsalsa20(key: Uint8Array, nonce: Uint8Array, data: Uint8Array) {
  const subkey = hsalsa20(key, nonce.subarray(0, 16));
  const out = new Uint8Array(data.length);
  const input = new Uint8Array(16);
  input.set(nonce.subarray(16, 24));
  for (let block = 0; block * 64 < data.length; block++) {
    new DataView(input.buffer).setUint32(8, block, true);
    const s = state(subkey, input);
    const x = s.slice();
    rounds(x);
    const stream = new Uint8Array(64);
    const view = new DataView(stream.buffer);
    for (let i = 0; i < 16; i++) view.setUint32(i * 4, (x[i] + s[i]) >>> 0, true);
    for (let i = 0; i < 64 && block * 64 + i < data.length; i++)
      out[block * 64 + i] = data[block * 64 + i] ^ stream[i];
  }
  return out;
}

function poly1305(key: Uint8Array, msg: Uint8Array) {
  const r = le(key.subarray(0, 16)) & 0x0ffffffc0ffffffc0ffffffc0fffffffn;
  const s = le(key.subarray(16, 32));
  const p = (1n << 130n) - 5n;
  let acc = 0n;
  for (let i = 0; i < msg.length; i += 16) {
    const chunk = msg.subarray(i, i + 16);
    acc = ((acc + le(chunk) + (1n << BigInt(8 * chunk.length))) * r) % p;
  }
  return toLe((acc + s) & ((1n << 128n) - 1n), 16);
}

// ---- BLAKE2b, unkeyed, for the 24-byte nonce ----

const MASK = (1n << 64n) - 1n;
const IV = [
  0x6a09e667f3bcc908n,
  0xbb67ae8584caa73bn,
  0x3c6ef372fe94f82bn,
  0xa54ff53a5f1d36f1n,
  0x510e527fade682d1n,
  0x9b05688c2b3e6c1fn,
  0x1f83d9abfb41bd6bn,
  0x5be0cd19137e2179n,
];
const SIGMA_B = [
  [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
  [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
  [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
  [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
  [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
  [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
  [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
  [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
  [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
  [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];
const rotr = (x: bigint, n: bigint) => ((x >> n) | (x << (64n - n))) & MASK;

export function blake2b(input: Uint8Array, outlen: number) {
  const h = IV.slice();
  h[0] ^= 0x01010000n ^ BigInt(outlen);
  const blocks = Math.max(1, Math.ceil(input.length / 128));
  for (let b = 0; b < blocks; b++) {
    const block = new Uint8Array(128);
    block.set(input.subarray(b * 128, b * 128 + 128));
    const m = Array.from({ length: 16 }, (_, i) => le(block.subarray(i * 8, i * 8 + 8)));
    const v = [...h, ...IV];
    v[12] ^= BigInt(Math.min(input.length, (b + 1) * 128));
    if (b === blocks - 1) v[14] ^= MASK;
    const g = (a: number, bb: number, c: number, d: number, x: bigint, y: bigint) => {
      v[a] = (v[a] + v[bb] + x) & MASK;
      v[d] = rotr(v[d] ^ v[a], 32n);
      v[c] = (v[c] + v[d]) & MASK;
      v[bb] = rotr(v[bb] ^ v[c], 24n);
      v[a] = (v[a] + v[bb] + y) & MASK;
      v[d] = rotr(v[d] ^ v[a], 16n);
      v[c] = (v[c] + v[d]) & MASK;
      v[bb] = rotr(v[bb] ^ v[c], 63n);
    };
    for (let r = 0; r < 12; r++) {
      const s = SIGMA_B[r % 10];
      g(0, 4, 8, 12, m[s[0]], m[s[1]]);
      g(1, 5, 9, 13, m[s[2]], m[s[3]]);
      g(2, 6, 10, 14, m[s[4]], m[s[5]]);
      g(3, 7, 11, 15, m[s[6]], m[s[7]]);
      g(0, 5, 10, 15, m[s[8]], m[s[9]]);
      g(1, 6, 11, 12, m[s[10]], m[s[11]]);
      g(2, 7, 8, 13, m[s[12]], m[s[13]]);
      g(3, 4, 9, 14, m[s[14]], m[s[15]]);
    }
    for (let i = 0; i < 8; i++) h[i] ^= v[i] ^ v[i + 8];
  }
  const out = new Uint8Array(64);
  h.forEach((w, i) => out.set(toLe(w, 8), i * 8));
  return out.slice(0, outlen);
}

// ---- crypto_box_seal ----

const concat = (...parts: Uint8Array[]) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  parts.reduce((at, p) => (out.set(p, at), at + p.length), 0);
  return out;
};

const boxKey = (secret: Uint8Array, peer: Uint8Array) =>
  hsalsa20(x25519(secret, peer), new Uint8Array(16));

export function seal(message: Uint8Array, recipient: Uint8Array, ephemeralSecret?: Uint8Array) {
  const esk = ephemeralSecret ?? crypto.getRandomValues(new Uint8Array(32));
  const epk = publicKeyOf(esk);
  const nonce = blake2b(concat(epk, recipient), 24);
  const stream = xsalsa20(boxKey(esk, recipient), nonce, concat(new Uint8Array(32), message));
  const ciphertext = stream.subarray(32);
  return concat(epk, poly1305(stream.subarray(0, 32), ciphertext), ciphertext);
}

// The plaintext, or null when the box was not sealed to this key or was altered.
export function sealOpen(box: Uint8Array, secret: Uint8Array): Uint8Array | null {
  if (box.length < 48) return null;
  const epk = box.subarray(0, 32);
  const mac = box.subarray(32, 48);
  const nonce = blake2b(concat(epk, publicKeyOf(secret)), 24);
  const key = boxKey(secret, epk);
  const plain = xsalsa20(key, nonce, concat(new Uint8Array(32), box.subarray(48)));
  const expected = poly1305(plain.subarray(0, 32), box.subarray(48));
  if (expected.some((b, i) => b !== mac[i])) return null;
  return plain.slice(32);
}

export const toBase64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
export const fromBase64 = (text: string) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

// What the secret forms send: the value sealed to the key GHES returned, base64.
export function sealSecret(value: string, publicKeyBase64: string) {
  return toBase64(seal(new TextEncoder().encode(value), fromBase64(publicKeyBase64)));
}
