// Prototype tokens for M0-WP10. A real install verifies a stateless `ghs_` by its signing key and reads its claims; here
// a token is `gym0_<base64url(JSON credential)>.<base64url(HMAC-SHA256)>`, keyed by the harness secret, so the driver
// can mint every kind of credential the rows name. The edge classifies the actor from the verified claims and headers.

import type { Credential } from "./model.ts";

const b64u = (b: Uint8Array): string => btoa(String.fromCharCode(...b)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const unb64u = (s: string): Uint8Array => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

export async function hmac(key: string, data: string): Promise<Uint8Array> {
  const k = await crypto.subtle.importKey("raw", new TextEncoder().encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(data)));
}

/** A secret derived from the harness secret: App webhook secrets and the MCP client secret. */
export async function derive(secret: string, label: string): Promise<string> {
  return [...await hmac(secret, label)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export async function mint(secret: string, cred: Credential): Promise<string> {
  const body = b64u(new TextEncoder().encode(JSON.stringify(cred)));
  return `gym0_${body}.${b64u(await hmac(secret, body))}`;
}

export async function verify(secret: string, token: string): Promise<Credential | null> {
  const m = /^gym0_([A-Za-z0-9_-]+)\.([A-Za-z0-9_-]+)$/.exec(token);
  if (!m) return null;
  const want = b64u(await hmac(secret, m[1]));
  if (want !== m[2]) return null;
  try { return JSON.parse(new TextDecoder().decode(unb64u(m[1]))) as Credential; } catch { return null; }
}

/** The token from `Authorization: Basic` (git sends it as the password), `Bearer`, or `token`. */
export function tokenOf(req: Request): string | null {
  const h = req.headers.get("authorization") ?? "";
  const b = /^Basic (.+)$/i.exec(h);
  if (b) { try { const s = atob(b[1]); return s.slice(s.indexOf(":") + 1); } catch { return null; } }
  const t = /^(?:Bearer|token) (.+)$/i.exec(h);
  return t ? t[1] : null;
}
