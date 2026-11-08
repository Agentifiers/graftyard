// Fails when a nav entry (src/areas/*/nav.ts) points at a route that does not exist, or a
// region a route uses has no fault switch. Nav targets are plain strings, so tsrs cannot
// check them against the route tree; this does. Run after `tsr generate`.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const tree = readFileSync(join(root, "src/routeTree.gen.ts"), "utf8");
// FileRouteTypes.fullPaths is a union over several lines: from "fullPaths:" to the next key.
const block = tree.match(/fullPaths:([\s\S]*?)\n\s*fileRoutesByTo/)?.[1] ?? "";
const routes = new Set(
  (block.match(/'[^']*'/g) ?? []).map((s) => s.slice(1, -1).replace(/\/$/, "") || "/"),
);

if (routes.size === 0) throw new Error("no routes found in src/routeTree.gen.ts; run tsr generate");

const areas = join(root, "src/areas");
let bad = 0;
for (const dir of readdirSync(areas, { withFileTypes: true })) {
  if (!dir.isDirectory()) continue;
  let text;
  try {
    text = readFileSync(join(areas, dir.name, "nav.ts"), "utf8");
  } catch {
    continue;
  }
  for (const m of text.matchAll(/\b(?:to|match)\s*:\s*(\[[^\]]*\]|"[^"]*")/g)) {
    for (const path of m[1].match(/"[^"]*"/g) ?? []) {
      const p = path.slice(1, -1).replace(/\/$/, "") || "/";
      if (!routes.has(p)) {
        console.error(`src/areas/${dir.name}/nav.ts: ${p} is not a route`);
        bad++;
      }
    }
  }
}
if (bad) process.exit(1);
console.log(`nav: every target is one of ${routes.size} routes`);
