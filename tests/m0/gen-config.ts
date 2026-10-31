// Writes one wrangler config per script into .gen/. Usage: node gen-config.ts <phase>
//   phase "own":   each site script defines its member classes and binds only its own (so that the cross-script
//                  bindings of the next phase have classes to point at)
//   phase "cycle": each site script also binds the other two sites' classes with script_name, plus the edge

import { mkdirSync, writeFileSync } from "node:fs";
import { SITE_KEYS, SITES, type MemberClass, type SiteKey } from "./src/sites.ts";

const phase = process.argv[2];
if (phase !== "own" && phase !== "cycle") throw new Error("usage: node gen-config.ts own|cycle");

const COMPAT_DATE = "2026-10-01";
const CLASSES: Record<MemberClass, string> = { member: "Member", idle: "IdleMember", control: "ControlMember" };
const bindingsFor = (sites: SiteKey[], from: SiteKey | "edge") =>
  sites.flatMap((k) => (Object.keys(CLASSES) as MemberClass[]).map((cls) => ({
    name: `${cls.toUpperCase()}_${k.toUpperCase()}`,
    class_name: CLASSES[cls],
    ...(from === k ? {} : { script_name: SITES[k].script }),
  })));

mkdirSync(".gen", { recursive: true });
for (const k of SITE_KEYS) {
  const config = {
    name: SITES[k].script,
    main: "../src/site.ts",
    compatibility_date: COMPAT_DATE,
    workers_dev: false,
    preview_urls: false,
    vars: { SITE: k, RTT_RECHECK_MS: "3600000" },
    durable_objects: { bindings: bindingsFor(phase === "own" ? [k] : SITE_KEYS, k) },
    migrations: [
      { tag: "v1", new_sqlite_classes: ["Member"] },
      { tag: "v2", new_sqlite_classes: ["IdleMember", "ControlMember"] },
    ],
    observability: { enabled: true },
  };
  writeFileSync(`.gen/site-${k}.jsonc`, JSON.stringify(config, null, 2));
}
if (phase === "cycle") {
  writeFileSync(".gen/edge.jsonc", JSON.stringify({
    name: "graft-m0-edge",
    main: "../src/edge.ts",
    compatibility_date: COMPAT_DATE,
    workers_dev: true,
    preview_urls: false,
    durable_objects: { bindings: bindingsFor(SITE_KEYS, "edge") },
    observability: { enabled: true },
  }, null, 2));
}
console.log(`wrote .gen/ for phase ${phase}`);
