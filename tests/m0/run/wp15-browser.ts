// M0-WP15: the user's browser for the A7 consent flow, scripted with fetch. Given the authorization URL a real MCP
// client opened, it loads the consent page, checks its framing headers, submits Allow with the page's Origin, follows
// the host's OAuth with the user's GHES session cookie (a harness PAT for `bob`), and returns the client's callback URL
// with `code`, `state`, and `iss`. It never reads the client's own state, so the client still checks state and iss.
//
//   node run/wp15-browser.ts <authorization-url>    prints the callback URL (used by the Go and Python drivers)

import { tok, URLS } from "./wp15.ts";

export interface ConsentSeen { csp: string | null; xfo: string | null; page: string }

export async function consentAndCallback(authUrl: string, user = "bob", decision: "allow" | "deny" = "allow"): Promise<{ callback: string; seen: ConsentSeen }> {
  const r1 = await fetch(authUrl, { redirect: "manual" });
  const page = await r1.text();
  if (r1.status !== 200) throw new Error(`authorize: ${r1.status} ${page.slice(0, 200)}`);
  const seen = { csp: r1.headers.get("content-security-policy"), xfo: r1.headers.get("x-frame-options"), page };
  const p = /name=p value="([^"]+)"/.exec(page)?.[1];
  if (!p) throw new Error("no consent form");
  const r2 = await fetch(`${URLS.mcp}/consent`, { method: "POST", redirect: "manual",
    headers: { "content-type": "application/x-www-form-urlencoded", origin: URLS.mcp, "sec-fetch-site": "same-origin" },
    body: new URLSearchParams({ p: p.replace(/&#(\d+);/g, (_, c) => String.fromCharCode(Number(c))), decision }).toString() });
  const loc2 = r2.headers.get("location") ?? "";
  if (decision === "deny") return { callback: loc2, seen };
  const cookie = (r2.headers.get("set-cookie") ?? "").split(";")[0];
  if (!loc2.startsWith(URLS.host)) throw new Error(`consent: ${r2.status} ${loc2} ${await r2.text()}`);
  const r3 = await fetch(loc2, { redirect: "manual", headers: { cookie: `user_session=${encodeURIComponent(await tok(user))}` } });
  const loc3 = r3.headers.get("location") ?? "";
  if (!loc3.startsWith(`${URLS.mcp}/callback`)) throw new Error(`host authorize: ${r3.status} ${loc3} ${await r3.text()}`);
  const r4 = await fetch(loc3, { redirect: "manual", headers: { cookie } });
  const loc4 = r4.headers.get("location") ?? "";
  if (r4.status !== 302) throw new Error(`callback: ${r4.status} ${await r4.text()}`);
  return { callback: loc4, seen };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { callback } = await consentAndCallback(process.argv[2]);
  process.stdout.write(callback + "\n");
}
