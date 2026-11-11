// Watches member SQLite on all three B0-30 members every 30 s and stops the load (its STOP file) at the first reading
// over CHILD_GROUP_AT, the row's fail line. Each reading goes to tests/am1/raw/b030-cross.jsonl.
import { appendFileSync, existsSync, writeFileSync } from "node:fs";
import { member } from "../hosted/lib.mjs";

const CHILD_GROUP_AT = 8 * 2 ** 30; // 8,589,934,592 B
const OUT = new URL("../raw/b030-cross.jsonl", import.meta.url);
const STOP = new URL("../raw/b030-STOP", import.meta.url);

for (;;) {
  const at = new Date().toISOString();
  const ms = await Promise.all([0, 1, 2].map(async (s) => {
    try {
      const v = (await member("repo:1048586", s, "/bench_seed", { b030: { size: true } }, 30_000)).v ?? {};
      return { site: s, role: v.role, commit: v.commit, dbBytes: v.dbBytes, logBytes: v.logBytes, archivedTo: v.archivedTo, sqliteFull: v.sqliteFull };
    } catch (e) {
      return { site: s, error: String(e).slice(0, 120) };
    }
  }));
  const over = ms.filter((m) => m.dbBytes > CHILD_GROUP_AT);
  appendFileSync(OUT, JSON.stringify({ at, members: ms, over: over.map((m) => m.site) }) + "\n");
  if (over.length && !existsSync(STOP)) {
    writeFileSync(STOP, `CHILD_GROUP_AT crossed at ${at} on site(s) ${over.map((m) => m.site).join(",")}\n`);
  }
  if (existsSync(STOP) && over.length === 3) break;
  await new Promise((r) => setTimeout(r, 30_000));
}
