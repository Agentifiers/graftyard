// B0-24a's reading (`b024a-judge.mjs`): `node --test tests/m6/hosted/b024a.test.mjs`. Every expected value is worked
// out by hand from the gate (build/BUILD-PLAN.md, M6-WP2: pass at 99.9%, fail under 99% or on a lost acknowledged
// push) and the plan's classes (tests/m6/results/m6-wp2.md), not from the code under test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { judge, verdict } from "./b024a-judge.mjs";

const T0 = Date.parse("2026-11-30T01:45:00Z");
const S = 1000;

// The driver itself loads, and has every subcommand the plan and the day script name. Its imports resolve only when
// it is imported (b024a.mjs imported a missing `../am1/hosted/lib.mjs` until 340eb1e0). `lib.mjs` reads an install's
// token files at import, so this gives it stand-ins in a temporary folder; the driver runs nothing when imported.
test("the driver loads and has every subcommand", async () => {
  const { mkdtempSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const d = mkdtempSync(`${tmpdir()}/b024a-load-`);
  for (const [f, v] of [["tools.token", "t0000000"], ["bed.token", "b0000000"], ["m3.json", "{}"], ["m3-tokens.txt", "member m0000000"], ["app.pem", ""]]) {
    writeFileSync(`${d}/${f}`, v);
  }
  Object.assign(process.env, { TOOLS_TOKEN_FILE: `${d}/tools.token`, BED_TOKEN_FILE: `${d}/bed.token`, M3_STATE: d });
  const { COMMANDS } = await import("./b024a.mjs");
  assert.deepEqual(Object.keys(COMMANDS).sort(), ["bf52", "drop-key", "kill", "load", "report", "setup"]);
  for (const [k, f] of Object.entries(COMMANDS)) assert.equal(typeof f, "function", k);
});

test("the verdict is the gate's, on both sides of each line", () => {
  assert.equal(verdict(1, 0), "pass");
  assert.equal(verdict(0.999, 0), "pass");
  assert.equal(verdict(0.9989, 0), "between");
  assert.equal(verdict(0.99, 0), "between");
  assert.equal(verdict(0.9899, 0), "fail");
  // One lost acknowledged push fails whatever the rate.
  assert.equal(verdict(1, 1), "fail");
});

test("failures fall in one class each, by the kill on the orb's clock", () => {
  // Cloudflare's clock runs 4 s ahead of the orb's: the kill is at T0 + 100 s on the orb's.
  const kills = [{ site: 0, at: T0 + 104 * S, orbClockOffsetMs: 4 * S, open: 1 }];
  const s = (n, start, ms, ok) => ({ n, op: "clone", start: T0 + start * S, ms: ms * S, ok });
  const sessions = [
    s(1, 98, 3, false), // running 98 to 101 s: in flight at 100 s
    s(2, 101, 2, false), // started 1 s after: after the kill, not in flight (with the wrong offset sign it would be)
    s(3, 219, 1, false), // 119 s after: inside the 2 minutes
    s(4, 222, 1, false), // 122 s after: background
    s(5, 102, 1, true), // ok sessions are in no failure class
    s(6, 300, 1, true),
    s(7, 400, 1, true),
    s(8, 500, 1, true),
  ];
  const r = judge({ sessions, kills });
  assert.deepEqual([r.failures.inFlight, r.failures.afterKill, r.failures.background], [1, 2, 1]);
  // The background rate is over the sessions outside every kill window: 4, 6, 7 and 8, one of them failed.
  assert.equal(r.failures.backgroundRate, 1 / 4);
  assert.equal(r.openAtKills, 1);
  assert.equal(r.successRate, 4 / 8);
  assert.equal(r.verdict, "fail");
});

test("a failure in flight at a kill is in flight even inside an earlier kill's window", () => {
  // Kills at 100 s and 160 s; a session from 150 s to 165 s started 50 s after the first and was running at the second.
  const kills = [{ site: 0, at: T0 + 100 * S }, { site: 1, at: T0 + 160 * S }];
  const r = judge({ sessions: [{ n: 1, op: "push", start: T0 + 150 * S, ms: 15 * S, ok: false }], kills });
  assert.deepEqual([r.failures.inFlight, r.failures.afterKill, r.failures.background], [1, 0, 0]);
});

test("an acknowledged push missing from the log fails the row; a failed push found there is reported", () => {
  const p = (n, ok, acked, sha) => ({ n, op: "push", start: T0 + n * 10 * S, ms: S, ok, acked, ref: `refs/heads/b024a/${n}`, sha });
  const sessions = [p(1, true, true, "a1"), p(2, true, true, "b2"), p(3, false, false, "c3")];
  const logged = ["refs/heads/b024a/1 a1", "refs/heads/b024a/3 c3", "refs/heads/b024a/2 zz"];
  const r = judge({ sessions, kills: [], logged });
  assert.deepEqual(r.pushes.lost.map((x) => x.n), [2], "the log names ref 2 at another commit");
  assert.equal(r.pushes.ackedInLog, 1);
  assert.equal(r.pushes.failedFoundInLog, 1);
  assert.equal(r.verdict, "fail");
});

test("sessions that overlap an orb pause are left out of every count and the span", () => {
  const sessions = [];
  for (let i = 0; i < 1000; i++) sessions.push({ n: i, op: "fetch", start: T0 + i * 10 * S, ms: 2 * S, ok: true });
  // A pause from 5,000 s to 5,500 s: sessions 500 to 550 overlap it, 51 of them (499 ends at 4,992 s).
  const pauses = [[T0 + 5000 * S, T0 + 5500 * S]];
  for (const x of sessions) if (x.start >= T0 + 5000 * S && x.start <= T0 + 5500 * S) x.ok = false;
  const r = judge({ sessions, kills: [], pauses });
  assert.equal(r.leftOutForPauses, 51);
  assert.equal(r.sessions, 949);
  assert.equal(r.successRate, 1);
  assert.equal(r.verdict, "pass");
  // First start 0 s, last end 9,992 s, less the 500 s pause.
  assert.equal(r.spanMs, (9992 - 500) * S);
});

test("the kills alone allow 1 - kills·d / (3 fronts · span): d = 1.8 s at one kill every 10 minutes is 99.9%", () => {
  const day = 86_400;
  const sessions = [];
  // A session every 6 s for a day, each 1.8 s, the last ending at the day's end.
  for (let t = 0; t + 1.8 <= day; t += 6) sessions.push({ n: sessions.length, op: "clone", start: T0 + t * S, ms: 1.8 * S, ok: true });
  const span = sessions.at(-1).start + sessions.at(-1).ms - T0;
  const kills = Array.from({ length: 144 }, (_, i) => ({ site: i % 3, at: T0 + (i * 600 + 3) * S, orbClockOffsetMs: 0, open: 0 }));
  const r = judge({ sessions, kills });
  assert.equal(r.meanSessionMs, 1800);
  assert.ok(Math.abs(r.ceilingFromKills - (1 - (144 * 1.8 * S) / (3 * span))) < 1e-12);
  assert.ok(Math.abs(r.ceilingFromKills - 0.999) < 1e-5, String(r.ceilingFromKills));
});
