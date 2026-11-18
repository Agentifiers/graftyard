// M0-WP5, B0-17: deploy graft-m0-link-a/b/c (src/link.ts) and compare a member call by binding and by signed HTTPS
// through the target site's Worker, for each ordered pair of sites.
//
//   node run/link.ts deploy
//   node run/link.ts run [--n 50] [--bytes 1024] [--run X]
//   node run/link.ts delete

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { generateKeyPairSync } from "node:crypto";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  n: { type: "string", default: "50" }, bytes: { type: "string", default: "1024" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const ROOT = new URL("../", import.meta.url).pathname;
const SITES = ["a", "b", "c"] as const;
const SUBDOMAIN = `${process.env.CF_WORKERS_SUBDOMAIN}`;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const name = (k: string) => `graft-m0-link-${k}`;
const wrangler = (args: string[], input?: string) =>
  execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });

function deploy(): void {
  mkdirSync(`${ROOT}.gen`, { recursive: true });
  // One Ed25519 key pair per site; private keys go in as secrets, public keys as a variable.
  const keysPath = `${ROOT}.gen/link-keys.json`;
  if (!existsSync(keysPath)) {
    const keys = Object.fromEntries(SITES.map((k) => {
      const { privateKey, publicKey } = generateKeyPairSync("ed25519");
      return [k, { priv: privateKey.export({ format: "der", type: "pkcs8" }).toString("base64"), pub: publicKey.export({ format: "der", type: "spki" }).toString("base64") }];
    }));
    writeFileSync(keysPath, JSON.stringify(keys));
  }
  const keys = JSON.parse(readFileSync(keysPath, "utf8")) as Record<string, { priv: string; pub: string }>;
  const pubs = JSON.stringify(Object.fromEntries(SITES.map((k) => [k, keys[k].pub])));
  for (const phase of ["own", "cycle"] as const) {
    for (const k of SITES) {
      const path = `.gen/link-${k}.jsonc`;
      writeFileSync(`${ROOT}${path}`, JSON.stringify({
        name: name(k), main: "../src/link.ts", compatibility_date: "2026-10-01",
        // A fetch to another Worker of this account goes out through the front door, as one from another account would.
        compatibility_flags: ["global_fetch_strictly_public"],
        workers_dev: true, preview_urls: false,
        vars: { SITE: k, LINK_PUBS: pubs, SUBDOMAIN },
        durable_objects: { bindings: (phase === "own" ? [k] : SITES).map((s) => ({ name: `MEMBER_${s.toUpperCase()}`, class_name: "Member",
          ...(s === k ? {} : { script_name: name(s) }) })) },
        migrations: [{ tag: "v1", new_sqlite_classes: ["Member"] }],
      }, null, 2));
      wrangler(["deploy", "-c", path]);
      if (phase === "own") {
        wrangler(["secret", "put", "LINK_KEY", "-c", path], keys[k].priv);
        wrangler(["secret", "put", "M0_TOKEN", "-c", path], TOKEN);
      }
    }
  }
}

async function run(): Promise<void> {
  const out: unknown[] = [];
  for (const from of SITES) for (const to of SITES) {
    if (from === to) continue;
    const r = await fetch(`https://${name(from)}.${SUBDOMAIN}.workers.dev/`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` },
      body: JSON.stringify({ run: opt.run, from, to, n: Number(opt.n), bytes: Number(opt.bytes) }) });
    const j = r.ok ? await r.json() : { error: `${r.status} ${await r.text()}` };
    console.log(from, ">", to, JSON.stringify(j));
    out.push({ from, to, ...(j as object) });
  }
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(`${ROOT}results/raw/b0-17-${opt.run}.json`, JSON.stringify({ row: "B0-17", run: opt.run, n: Number(opt.n), bytes: Number(opt.bytes), at: new Date().toISOString(), pairs: out }, null, 1));
}

switch (positionals[0]) {
  case "deploy": deploy(); break;
  case "run": await run(); break;
  case "delete": for (const k of SITES) wrangler(["delete", "--name", name(k), "--force"]); break;
  default: throw new Error("usage: node run/link.ts deploy|run|delete");
}
