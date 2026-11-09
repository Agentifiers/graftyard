// Fails when a nav entry (src/areas/*/nav.ts) points at a route that does not exist, or
// uses a param no layout can fill. Nav targets are plain strings, so tsrs cannot check
// them against the route tree; this does. Run after `tsr generate`.
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

// The params a layout knows: repo pages have $owner and $repo, org pages $org, and the
// user menu fills $owner with the viewer's login.
const FILLABLE = new Set(["$owner", "$repo", "$org"]);

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
      // A match path only marks a tab active, so it may name any param; a target must be fillable.
      const params = (p.match(/\$\w*/g) ?? []).filter((x) => !FILLABLE.has(x));
      if (m[0].startsWith("to") && params.length) {
        console.error(
          `src/areas/${dir.name}/nav.ts: ${p} uses ${params.join(", ")}; layouts fill only ${[...FILLABLE].join(", ")}`,
        );
        bad++;
      }
      if (!routes.has(p)) {
        console.error(`src/areas/${dir.name}/nav.ts: ${p} is not a route`);
        bad++;
      }
    }
  }
}
if (bad) process.exit(1);
console.log(`nav: every target is one of ${routes.size} routes`);
