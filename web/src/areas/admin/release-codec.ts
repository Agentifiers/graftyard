// The mock's byte layout for a signed `Release` entry, so the release log can decode what
// the mock serves. The real encoding is the stored `Bound`, body, and signatures
// (design/DESIGN.md, Shape, "Release group"); this stands in for it:
//
//   "GYRE" | version u8 = 1 | kind length u8 | kind (UTF-8) | body length u32 BE |
//   body (UTF-8 JSON: { bound, body }) | signature count u8 | 96 bytes per signature
//   (32-byte key id, 64-byte Ed25519 signature)
//
// Bytes that do not start with the magic, or use another version, are shown opaque.

const MAGIC = [0x47, 0x59, 0x52, 0x45];

export type Bound = { install: string; group: string; prev: string | null; parent: string | null };
export type Signature = { key: string; sig: string };
export type DecodedEntry = {
  kind: string;
  bound: Bound;
  body: Record<string, unknown>;
  signatures: Signature[];
};

const hex = (b: Uint8Array) => [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
const unhex = (s: string) => Uint8Array.from(s.match(/../g) ?? [], (x) => parseInt(x, 16));

export function encodeEntry(e: DecodedEntry): string {
  const kind = new TextEncoder().encode(e.kind);
  const body = new TextEncoder().encode(JSON.stringify({ bound: e.bound, body: e.body }));
  const out = new Uint8Array(
    4 + 1 + 1 + kind.length + 4 + body.length + 1 + 96 * e.signatures.length,
  );
  const view = new DataView(out.buffer);
  let at = 0;
  out.set(MAGIC, at);
  at += 4;
  out[at++] = 1;
  out[at++] = kind.length;
  out.set(kind, at);
  at += kind.length;
  view.setUint32(at, body.length);
  at += 4;
  out.set(body, at);
  at += body.length;
  out[at++] = e.signatures.length;
  for (const s of e.signatures) {
    out.set(unhex(s.key).slice(0, 32), at);
    out.set(unhex(s.sig).slice(0, 64), at + 32);
    at += 96;
  }
  return btoa(String.fromCharCode(...out));
}

export type Decoded =
  | { ok: true; entry: DecodedEntry; bytes: number }
  | { ok: false; reason: string; bytes: number; head: string };

export function decodeEntry(base64: string): Decoded {
  let bytes: Uint8Array;
  try {
    bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  } catch {
    return { ok: false, reason: "not base64", bytes: 0, head: "" };
  }
  const opaque = (reason: string): Decoded => ({
    ok: false,
    reason,
    bytes: bytes.length,
    head: hex(bytes.slice(0, 16)),
  });
  if (bytes.length < 10 || MAGIC.some((m, i) => bytes[i] !== m)) return opaque("unknown format");
  if (bytes[4] !== 1) return opaque(`format version ${bytes[4]}`);
  try {
    const view = new DataView(bytes.buffer);
    let at = 5;
    const kindLen = bytes[at++]!;
    const kind = new TextDecoder().decode(bytes.slice(at, at + kindLen));
    at += kindLen;
    const bodyLen = view.getUint32(at);
    at += 4;
    const { bound, body } = JSON.parse(new TextDecoder().decode(bytes.slice(at, at + bodyLen)));
    at += bodyLen;
    const count = bytes[at++]!;
    const signatures: Signature[] = [];
    for (let i = 0; i < count; i++, at += 96) {
      signatures.push({
        key: hex(bytes.slice(at, at + 32)),
        sig: hex(bytes.slice(at + 32, at + 96)),
      });
    }
    return { ok: true, entry: { kind, bound, body, signatures }, bytes: bytes.length };
  } catch {
    return opaque("truncated or malformed");
  }
}
