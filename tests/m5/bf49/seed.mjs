// BF-49's seed on a test install (tests/m5/results/m5-wp5-hosted.md): one fresh `m3org` repository with auto_init, an
// inline push and a stored push (a 3 MiB file, over INLINE_MAX), so the feed carries the repository's entries and the
// network holds a stored pack with its `Committed` marker. Prints one JSON line: the repository's name and id and the
// pushes' results. Tokens ride in headers and a credential helper only (`lib.mjs`).
//   GRAFT_EDGE=... M3_STATE=~/.graft-m3f/m3 BED_TOKEN_FILE=... TOOLS_TOKEN_FILE=... node tests/m5/bf49/seed.mjs
import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, log, tokens } from "../../am1/hosted/lib.mjs";

const name = `bf49-${Date.now().toString(36)}`;
const made = await api("POST", "/api/v3/orgs/m3org/repos", tokens.admin, { name, private: true, auto_init: true });
if (made.status !== 201) throw new Error(`create ${name}: ${made.status} ${JSON.stringify(made.body).slice(0, 200)}`);
const id = made.body.id;
log(`created m3org/${name} id ${id}`);
const g = new Git(`m3org/${name}`).as(tokens.admin);
let r;
for (let i = 0; i < 5 && !(r = g.clone()).ok; i++) await new Promise((d) => setTimeout(d, 3000));
if (!r.ok) throw new Error(`clone: ${r.out}`);
const pushes = [];
for (const [file, bytes] of [["small.txt", 2_000], ["big.bin", 3 << 20]]) {
  writeFileSync(join(g.wt, file), randomBytes(bytes));
  g.run(["add", file]);
  g.run(["commit", "-q", "-m", `bf49 ${file}`]);
  const t0 = Date.now();
  const p = g.run(["push", "-q", "origin", "HEAD:main"]);
  pushes.push({ file, bytes, ok: p.ok, ms: Date.now() - t0, out: p.ok ? "" : p.out.slice(0, 300) });
  log(`push ${file} ${bytes} B: ${p.ok ? "ok" : p.out.slice(0, 200)}`);
}
const head = g.remoteHead("refs/heads/main");
console.log(JSON.stringify({ repo: `m3org/${name}`, id, head, pushes }));
