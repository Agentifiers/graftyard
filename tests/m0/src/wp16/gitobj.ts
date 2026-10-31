// Raw git objects for the M0-WP16 provenance prototype: SHA-1 check, commit headers, and tree entries.

export interface RawObj { oid: string; type: "commit" | "tree" | "blob" | "tag"; data: string /* base64 body */ }

export const b64 = (s: string): Uint8Array => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const hex = (b: ArrayBuffer | Uint8Array) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, "0")).join("");

/** True when `o.oid` is the SHA-1 of `<type> <len>\0<body>`, as git computes it. */
export async function oidOk(o: RawObj): Promise<boolean> {
  const body = b64(o.data), head = new TextEncoder().encode(`${o.type} ${body.length}\0`);
  const all = new Uint8Array(head.length + body.length);
  all.set(head); all.set(body, head.length);
  return hex(await crypto.subtle.digest("SHA-1", all)) === o.oid;
}

/** A commit's tree and parents from its header. */
export function commitHeader(body: Uint8Array): { tree: string; parents: string[] } {
  const text = new TextDecoder().decode(body.subarray(0, Math.min(body.length, 4096)));
  const head = text.split("\n\n")[0].split("\n");
  return { tree: head.find((l) => l.startsWith("tree "))!.slice(5), parents: head.filter((l) => l.startsWith("parent ")).map((l) => l.slice(7)) };
}

/** A tree's entries: mode, name, and oid. */
export function treeEntries(body: Uint8Array): { mode: string; name: string; oid: string }[] {
  const out: { mode: string; name: string; oid: string }[] = [];
  const dec = new TextDecoder();
  for (let i = 0; i < body.length;) {
    const sp = body.indexOf(32, i), nul = body.indexOf(0, sp);
    out.push({ mode: dec.decode(body.subarray(i, sp)), name: dec.decode(body.subarray(sp + 1, nul)), oid: hex(body.subarray(nul + 1, nul + 21)) });
    i = nul + 21;
  }
  return out;
}

export async function sha256hex(s: string | Uint8Array): Promise<string> {
  return hex(await crypto.subtle.digest("SHA-256", typeof s === "string" ? new TextEncoder().encode(s) : s));
}
