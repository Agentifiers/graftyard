// B0-30 rerun watcher (operator's checks, 2026-11-14): from the end of the seed, every 5 s, which member of the group
// leads, so every leaderless gap is measured; the group's own split steps as the leader reports them; and a sample of
// seeded comment bodies, read through the parent's leader once before the split and again every 30 min, which must
// match. On a leaderless gap over GAP_MAX_S (30), any SQLITE_FULL, or a body mismatch, it writes tests/am1/raw/b030-STOP
// with the reason, so the load stops at once (it resumes from its state once the file is removed), and
// tests/am1/raw/b030r2-ALERT. Also (operator, 2026-11-14): a log over LOG_MAX_BYTES whose leader's `archivedTo` has not
// moved for ARCHIVE_STALL_MIN (30) minutes, or a log over LOG_MAX_BYTES by LOG_OVER (5%), so members not truncating
// never runs on toward SQLITE_FULL. And (operator, the third drain fix): once the log is under LOG_MAX_BYTES after the
// resume, any later reading over it stops the load. Rows go to tests/am1/raw/b030r2-watch.jsonl.
//   node tests/am1/b030split/watch.mjs
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { log, row, sleep, statuses, voter } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8"));
const C = `repo:${REPO.repo}`;
const OUT = "tests/am1/raw/b030r2-watch.jsonl";
const STOP = "tests/am1/raw/b030-STOP";
const ALERT = "tests/am1/raw/b030r2-ALERT";
const GAP_MAX_S = Number(process.env.GAP_MAX_S ?? 30);
const SAMPLE = 100;
const LOG_MAX_BYTES = 2 * 2 ** 30;
const LOG_OVER = Number(process.env.LOG_OVER ?? 0.05);
const ARCHIVE_STALL_MIN = Number(process.env.ARCHIVE_STALL_MIN ?? 30);
let archived = { to: -1, at: Date.now() }, archiveAlerted = false;
const rows = JSON.parse(readFileSync("tests/am1/raw/b030-seed.json", "utf8")).rows;
const keys = Array.from({ length: SAMPLE }, (_, i) => 2 ** 40 + Math.floor((i * rows) / SAMPLE));

function alert(why) {
  log(`ALERT ${why}`);
  row(OUT, { kind: "alert", at: new Date().toISOString(), why });
  appendFileSync(ALERT, `${new Date().toISOString()} ${why}\n`);
  if (!existsSync(STOP)) writeFileSync(STOP, `watch: ${why}\n`);
}

async function bodies(site) {
  const q = { q: { rows: { keys: keys.map((k) => ["comment", [{ Int: k }]]) } } };
  const v = (await voter(C, site, "/read", q, 120_000).catch((e) => ({ v: { error: String(e) } }))).v;
  if (!Array.isArray(v?.value)) return null;
  const out = {};
  for (const [, key, cols] of v.value) if (cols) out[key[0].Int] = Buffer.from(cols[0].Blob).toString("latin1");
  return out;
}

const BASE = "tests/am1/raw/b030r2-bodies.json";
let base = existsSync(BASE) ? JSON.parse(readFileSync(BASE, "utf8")) : null;
let underOnce = false, capAlerted = false;
let seenLeader = false, since = null, alerted = false, lastSplit = "", lastBodies = 0, longest = 0;
for (;;) {
  const st = await statuses(C);
  const led = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  const now = Date.now();
  if (led) {
    if (since) {
      const s = Math.round((now - since) / 1000);
      longest = Math.max(longest, s);
      row(OUT, { kind: "gap", from: new Date(since).toISOString(), s, to: led.site });
      log(`leader again on ${led.site} after ${s} s`);
      since = null;
      alerted = false;
    }
    seenLeader = true;
    const v = (await voter(C, led.site, "/bench_seed", { b030: { size: true } }, 30_000).catch(() => ({ v: {} }))).v ?? {};
    const sp = JSON.stringify({ child: !!v.split?.state?.child, seed: v.split?.state?.seed?.[0] ?? null, dropped: !!v.split?.state?.dropped, slim: v.split?.slim ? { done: v.split.slim.done, table: v.split.slim.table } : null });
    if (sp !== lastSplit) {
      row(OUT, { kind: "split", at: new Date().toISOString(), site: led.site, split: JSON.parse(sp), dbBytes: v.dbBytes });
      log(`split ${sp} on ${led.site}, db ${v.dbBytes}`);
      lastSplit = sp;
    }
    if (v.sqliteFull > 0) alert(`SQLITE_FULL ${v.sqliteFull} on site ${led.site}`);
    if (typeof v.archivedTo === "number" && v.archivedTo !== archived.to) archived = { to: v.archivedTo, at: now };
    const over = (v.logBytes ?? 0) > LOG_MAX_BYTES;
    if (!archiveAlerted && over && (v.logBytes > LOG_MAX_BYTES * (1 + LOG_OVER) || now - archived.at > ARCHIVE_STALL_MIN * 60_000)) {
      archiveAlerted = true;
      alert(`archive not keeping the log at its cap: log ${v.logBytes} B, archivedTo ${v.archivedTo} unchanged since ${new Date(archived.at).toISOString()}`);
    }
    if (typeof v.logBytes === "number" && !over) underOnce = true;
    if (underOnce && over && !capAlerted) {
      capAlerted = true;
      alert(`the log passed LOG_MAX_BYTES after it was under it: ${v.logBytes} B, archivedTo ${v.archivedTo}`);
    }
    if (now % 600_000 < 5_000) row(OUT, { kind: "log", at: new Date().toISOString(), logBytes: v.logBytes, archivedTo: v.archivedTo, truncatedTo: v.truncatedTo, promotedTo: v.promotedTo, dbBytes: v.dbBytes });
    // Bodies: a baseline before the split drops the parent's text, then a comparison every 30 min.
    if (!base && !v.split?.state?.dropped) {
      const b = await bodies(led.site);
      if (b && Object.keys(b).length === SAMPLE) { base = b; writeFileSync(BASE, JSON.stringify(b)); log(`bodies baseline: ${SAMPLE}`); }
    } else if (base && now - lastBodies > 30 * 60_000) {
      const b = await bodies(led.site);
      if (b) {
        lastBodies = now;
        const bad = Object.keys(base).filter((k) => b[k] !== base[k]);
        row(OUT, { kind: "bodies", at: new Date().toISOString(), same: SAMPLE - bad.length, of: SAMPLE, dropped: !!v.split?.state?.dropped });
        if (bad.length) alert(`body mismatch: ${bad.length} of ${SAMPLE} (first key ${bad[0]})`);
      }
    }
  } else if (seenLeader) {
    since ??= now;
    const s = (now - since) / 1000;
    if (s > GAP_MAX_S && !alerted) { alerted = true; alert(`leaderless ${Math.round(s)} s since ${new Date(since).toISOString()}`); }
  }
  if (now % 600_000 < 5_000) row(OUT, { kind: "tick", at: new Date().toISOString(), longestGapS: longest, leader: led?.site ?? null });
  // The run ends after the drain (tests/am1/b030/drain.mjs), which pushes what load.mjs owed when it stopped at day 30.
  if (existsSync("tests/am1/raw/b030-run.done")) break;
  await sleep(5_000);
}
log(`watch: done, longest gap ${longest} s`);
