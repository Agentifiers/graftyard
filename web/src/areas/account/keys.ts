// Parsing for the public keys a person pastes on the SSH and GPG keys page. The mock uses
// these to validate and to find a key another account already holds; the page uses them to
// show fingerprints, which the REST `key` schema does not carry.

export const SSH_TYPES = [
  "ssh-ed25519",
  "ssh-rsa",
  "ecdsa-sha2-nistp256",
  "ecdsa-sha2-nistp384",
  "ecdsa-sha2-nistp521",
  "sk-ssh-ed25519@openssh.com",
  "sk-ecdsa-sha2-nistp256@openssh.com",
] as const;

export type SshKey = { type: string; base64: string; comment: string };

const fromBase64 = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

function toBase64(bytes: Uint8Array) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

// An OpenSSH public key line: type, base64 blob, optional comment. The blob starts with
// its own type as a length-prefixed string, which catches a pasted private key or a
// truncated line.
export function parseSshKey(text: string): SshKey | { error: string } {
  const parts = text.trim().split(/\s+/);
  const [type, base64, ...rest] = parts;
  if (!type || !base64) return { error: "Key is invalid. It must begin with a key type." };
  if (!(SSH_TYPES as readonly string[]).includes(type)) {
    return {
      error: `Key is invalid. It must begin with one of ${SSH_TYPES.slice(0, 3).join(", ")}, or another supported type.`,
    };
  }
  let blob: Uint8Array;
  try {
    blob = fromBase64(base64);
  } catch {
    return { error: "Key is invalid. The key data is not base64." };
  }
  const n = blob.length >= 4 ? new DataView(blob.buffer).getUint32(0) : -1;
  const inner = n > 0 && n < 64 ? new TextDecoder().decode(blob.slice(4, 4 + n)) : "";
  if (inner !== type) return { error: "Key is invalid. The key data does not match its type." };
  return { type, base64, comment: rest.join(" ") };
}

// The SHA256 fingerprint `ssh-keygen -l` prints.
export async function sshFingerprint(key: string): Promise<string> {
  const parsed = parseSshKey(key);
  if ("error" in parsed) return "unknown";
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", fromBase64(parsed.base64)));
  return `SHA256:${toBase64(digest).replace(/=+$/, "")}`;
}

export type GpgKeyInfo = {
  keyId: string;
  fingerprint: string;
  createdAt: string;
  algorithm: number;
  emails: string[];
  subkeys: { keyId: string; createdAt: string; algorithm: number }[];
};

const hex = (bytes: Uint8Array) =>
  [...bytes]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase();

// Reads an ASCII-armored OpenPGP public key: the primary key packet (tag 6), user ids
// (tag 13), and subkeys (tag 14). A v4 key's fingerprint is the SHA-1 of 0x99, the
// two-byte length, and the packet body; its key id is the fingerprint's last 8 bytes.
export async function parseGpgKey(armored: string): Promise<GpgKeyInfo | { error: string }> {
  const text = armored.trim();
  if (!text.startsWith("-----BEGIN PGP PUBLIC KEY BLOCK-----")) {
    return {
      error: text.includes("PRIVATE KEY")
        ? "That is a private key. Paste the public key block."
        : "Key is invalid. It must begin with -----BEGIN PGP PUBLIC KEY BLOCK-----.",
    };
  }
  if (!text.includes("-----END PGP PUBLIC KEY BLOCK-----")) {
    return { error: "Key is invalid. The armor has no END line." };
  }
  const lines = text.split(/\r?\n/);
  const start = lines.findIndex((l) => l.trim() === "");
  const body = lines
    .slice(start + 1)
    .filter((l) => !l.startsWith("-----") && !l.startsWith("="))
    .join("");
  let data: Uint8Array;
  try {
    data = fromBase64(body);
  } catch {
    return { error: "Key is invalid. The armor is not base64." };
  }

  const packets: { tag: number; body: Uint8Array }[] = [];
  for (let i = 0; i < data.length; ) {
    const b = data[i++];
    if (!(b & 0x80)) return { error: "Key is invalid. A packet header is malformed." };
    let tag: number;
    let len: number;
    if (b & 0x40) {
      tag = b & 0x3f;
      const o1 = data[i++];
      if (o1 < 192) len = o1;
      else if (o1 < 224) len = ((o1 - 192) << 8) + data[i++] + 192;
      else {
        len = new DataView(data.buffer, i, 4).getUint32(0);
        i += 4;
      }
    } else {
      tag = (b >> 2) & 0x0f;
      const size = [1, 2, 4][b & 3] ?? 0;
      len = 0;
      for (let k = 0; k < size; k++) len = (len << 8) | data[i++];
    }
    packets.push({ tag, body: data.slice(i, i + len) });
    i += len;
  }

  const describe = async (p: Uint8Array) => {
    const framed = new Uint8Array(p.length + 3);
    framed.set([0x99, p.length >> 8, p.length & 0xff]);
    framed.set(p, 3);
    const fp = new Uint8Array(await crypto.subtle.digest("SHA-1", framed));
    const created = new DataView(p.buffer, p.byteOffset + 1, 4).getUint32(0);
    return {
      fingerprint: hex(fp),
      keyId: hex(fp.slice(12)),
      createdAt: new Date(created * 1000).toISOString(),
      algorithm: p[5],
    };
  };

  const primary = packets.find((p) => p.tag === 6);
  if (!primary || primary.body[0] !== 4) {
    return { error: "Key is invalid. It holds no version 4 public key." };
  }
  const info = await describe(primary.body);
  const emails = packets
    .filter((p) => p.tag === 13)
    .map((p) => new TextDecoder().decode(p.body).match(/<([^>]+@[^>]+)>/)?.[1])
    .filter((e): e is string => Boolean(e));
  const subkeys = await Promise.all(
    packets.filter((p) => p.tag === 14).map((p) => describe(p.body)),
  );
  return { ...info, emails, subkeys };
}

// Signing algorithms: RSA (1, 3), DSA (17), ECDSA (19), EdDSA (22).
export const canSign = (algorithm: number) => [1, 3, 17, 19, 22].includes(algorithm);
