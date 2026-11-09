// Serves the SPA build as a static host would: files when they exist, _shell.html otherwise.
// Usage: npm run preview (PORT defaults to 4173).
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../dist/client/", import.meta.url));
const shell = join(root, "_shell.html");
const port = Number(process.env.PORT) || 4173;

if (!existsSync(shell)) {
  console.error(`no ${shell}; run npm run build first`);
  process.exit(1);
}

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8",
  ".woff2": "font/woff2",
};

createServer((req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url ?? "/", "http://x").pathname)).replace(
    /^(\.\.[/\\])+/,
    "",
  );
  let file = join(root, path);
  if (!file.startsWith(root) || !existsSync(file) || statSync(file).isDirectory()) file = shell;
  const immutable = file.includes(`${join(root, "assets")}`);
  // A rebuild empties dist/ while this server runs; answer 503 for that window instead of crashing.
  const stream = createReadStream(file);
  stream.once("error", () => {
    if (!res.headersSent) res.writeHead(503, { "retry-after": "1" });
    res.end();
  });
  stream.once("open", () => {
    res.writeHead(200, {
      "content-type": TYPES[extname(file)] ?? "application/octet-stream",
      "cache-control": immutable ? "public, max-age=31536000, immutable" : "no-cache",
    });
    stream.pipe(res);
  });
}).listen(port, () => console.log(`serving ${root} on http://localhost:${port}`));
