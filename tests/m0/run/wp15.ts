// M0-WP15 bed: deploy, set up, and delete graft-wp15-host, graft-wp15-base, graft-wp15-mcp, and graft-wp15-aux.
//   node run/wp15.ts deploy [host|base|mcp|aux ...]    wrangler deploy plus secrets (idempotent)
//   node run/wp15.ts setup                             /m0/setup on host and base, then seed (main, a branch, a PR, an issue)
//   node run/wp15.ts delete                            wrangler delete of all four
// Secrets: M0_TOKEN (tests/m0/.m0-token) on all; MCP_CLIENT_SECRET = derive(M0_TOKEN, "mcp-client-secret") on graft-mcp.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { derive, mint } from "../src/wp15/host/token.ts";
import { CREDS } from "../src/wp15/host/fixture.ts";

export const ROOT = new URL("../", import.meta.url).pathname;
export const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
export const SUB = "<workers-subdomain>";
export const URLS = { host: `https://graft-wp15-host.${SUB}.workers.dev`, base: `https://graft-wp15-base.${SUB}.workers.dev`,
  mcp: `https://graft-wp15-mcp.${SUB}.workers.dev`, aux: `https://graft-wp15-aux.${SUB}.workers.dev` };
const wrangler = (args: string[], input?: string) =>
  execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });

export const tok = (label: string) => mint(TOKEN, CREDS[label]);
export async function m0(base: string, path: string, body?: unknown): Promise<unknown> {
  const r = await fetch(`${base}${path}`, { method: body === undefined ? "GET" : "POST", headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body) });
  const t = await r.text();
  if (!r.ok) throw new Error(`${path}: ${r.status} ${t.slice(0, 300)}`);
  try { return JSON.parse(t); } catch { return t; }
}

async function deploy(which: string[]): Promise<void> {
  for (const w of which) {
    const cfg = `wp15/graft-wp15-${w}.jsonc`;
    process.stdout.write(wrangler(["deploy", "-c", cfg]));
    process.stdout.write(wrangler(["secret", "put", "M0_TOKEN", "-c", cfg], TOKEN));
    if (w === "mcp") process.stdout.write(wrangler(["secret", "put", "MCP_CLIENT_SECRET", "-c", cfg], await derive(TOKEN, "mcp-client-secret")));
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [cmd, ...rest] = process.argv.slice(2);
  if (cmd === "deploy") await deploy(rest.length ? rest : ["aux", "host", "base", "mcp"]);
  else if (cmd === "setup") { for (const b of [URLS.host, URLS.base]) console.log(b, JSON.stringify(await m0(b, "/m0/setup", {}))); }
  else if (cmd === "delete") { for (const n of ["mcp", "host", "base", "aux"]) { try { process.stdout.write(wrangler(["delete", "--name", `graft-wp15-${n}`, "--force"])); } catch (e) { console.error(String(e)); } } }
  else console.error("usage: node run/wp15.ts deploy|setup|delete");
}
