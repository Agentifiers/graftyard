// M0-WP15: sealing for `McpToken`, `McpRefresh`, codes and the consent cookie (design/AGENT-LAYER.md, Shape,
// "graft-mcp": "Sealed with a key from Secrets Store"). AES-256-GCM under a key derived from the script secret; the
// prototype stands a script secret in for the Secrets Store binding.

const te = new TextEncoder(), td = new TextDecoder();
const b64u = (b: Uint8Array): string => btoa(String.fromCharCode(...b)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const unb64u = (s: string): Uint8Array => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

const keys = new Map<string, Promise<CryptoKey>>();
function key(secret: string, purpose: string): Promise<CryptoKey> {
  const k = `${secret}|${purpose}`;
  if (!keys.has(k)) keys.set(k, (async () => {
    const raw = await crypto.subtle.digest("SHA-256", te.encode(`${secret}|graft-mcp|${purpose}`));
    return crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt", "decrypt"]);
  })());
  return keys.get(k)!;
}

/** `prefix` names the kind (`gymcp_` access, `gymcr_` refresh, `gymcc_` code, `gymck_` cookie); the purpose key differs
 *  per kind, so one kind never opens as another. */
export async function seal(secret: string, prefix: string, v: unknown): Promise<string> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, await key(secret, prefix), te.encode(JSON.stringify(v))));
  return `${prefix}${b64u(iv)}.${b64u(ct)}`;
}
export async function open<T>(secret: string, prefix: string, s: string | null | undefined): Promise<T | null> {
  if (!s || !s.startsWith(prefix)) return null;
  const [iv, ct] = s.slice(prefix.length).split(".");
  try {
    const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: unb64u(iv) }, await key(secret, prefix), unb64u(ct));
    return JSON.parse(td.decode(pt)) as T;
  } catch { return null; }
}
export async function sha256b64u(s: string): Promise<string> {
  return b64u(new Uint8Array(await crypto.subtle.digest("SHA-256", te.encode(s))));
}
