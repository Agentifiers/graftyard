// B0-24a's reading (tests/m6/results/m6-wp2.md, "B0-24a on the Spectrum ingress: plan"), fixed before the run and
// kept apart from the driver (`b024a.mjs`) so `b024a.test.mjs` checks it without an install. Pure: no I/O.
//
// The verdict is the gate as written, on every counted session: pass at 99.9% or more with no acknowledged push
// missing from the log, fail under 99% or with one missing, and between otherwise. The rest is evidence:
// - every failure falls in one class, by the kills on the orb's clock (a kill's `at` is Cloudflare's, less the
//   offset measured at the kill): `inFlight` when the session was running at the kill's instant, `afterKill` when it
//   started inside the 2 minutes after one, and `background` otherwise;
// - d, the mean session time, and the success the kills alone allow, 1 - kills·d / (fronts·T), which is d / 1,800 s
//   lost at one kill every 10 minutes on 3 fronts;
// - sessions overlapping a pause of the orb are left out of every count and reported.

export const KILL_WINDOW_MS = 120_000;
export const PASS = 0.999;
export const FAIL = 0.99;

const pct = (xs, p) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.max(0, Math.ceil(p * s.length) - 1)];
};

export function verdict(rate, lost) {
  if (lost > 0 || rate < FAIL) return "fail";
  return rate >= PASS ? "pass" : "between";
}

/**
 * @param {object} x
 * @param {{n:number, op:string, start:number, ms:number, ok:boolean, acked?:boolean, ref?:string, sha?:string}[]} x.sessions
 *   start and ms on the orb's clock
 * @param {{site:number, at:number, orbClockOffsetMs?:number, open?:number, restartMs?:number}[]} x.kills  at on Cloudflare's clock
 * @param {[number, number][]} [x.pauses]  orb pauses, [from, to] on the orb's clock
 * @param {Iterable<string>} [x.logged]  "<ref> <sha>" for every ref update in a committed entry of the repository's log
 * @param {number} [x.fronts]
 */
export function judge({ sessions, kills, pauses = [], logged = [], fronts = 3 }) {
  const paused = (s) => pauses.some(([a, b]) => s.start <= b && s.start + s.ms >= a);
  const counted = sessions.filter((s) => !paused(s));
  const at = kills.map((k) => k.at - (k.orbClockOffsetMs ?? 0)).sort((a, b) => a - b);
  const inFlight = (s) => at.some((t) => s.start <= t && t <= s.start + s.ms);
  const afterKill = (s) => at.some((t) => s.start > t && s.start <= t + KILL_WINDOW_MS);
  const classes = { inFlight: 0, afterKill: 0, background: 0 };
  let quiet = 0;
  for (const s of counted) {
    const c = inFlight(s) ? "inFlight" : afterKill(s) ? "afterKill" : "background";
    if (c === "background") quiet++;
    if (!s.ok) classes[c]++;
  }
  const ok = counted.filter((s) => s.ok).length;
  const rate = counted.length ? ok / counted.length : 0;
  const byOp = {};
  for (const op of ["clone", "fetch", "push"]) {
    const o = counted.filter((s) => s.op === op);
    const g = o.filter((s) => s.ok).map((s) => s.ms);
    byOp[op] = { sessions: o.length, ok: g.length, p50: pct(g, 0.5), p99: pct(g, 0.99), max: g.length ? Math.max(...g) : null };
  }
  const d = counted.length ? counted.reduce((a, s) => a + s.ms, 0) / counted.length : 0;
  const first = Math.min(...counted.map((s) => s.start));
  const last = Math.max(...counted.map((s) => s.start + s.ms));
  const pausedMs = pauses.reduce((a, [f, t]) => a + Math.max(0, Math.min(t, last) - Math.max(f, first)), 0);
  const spanMs = counted.length ? last - first - pausedMs : 0;
  const lossFromKills = spanMs > 0 ? (kills.length * d) / (fronts * spanMs) : 0;
  const log = new Set(logged);
  const pushes = counted.filter((s) => s.op === "push");
  const acked = pushes.filter((s) => s.acked);
  const lost = acked.filter((s) => !log.has(`${s.ref} ${s.sha}`));
  const failedInLog = pushes.filter((s) => !s.acked && s.sha && log.has(`${s.ref} ${s.sha}`));
  return {
    verdict: verdict(rate, lost.length),
    sessions: counted.length,
    ok,
    successRate: rate,
    leftOutForPauses: sessions.length - counted.length,
    pausedMs,
    byOp,
    failures: { ...classes, backgroundRate: quiet ? classes.background / quiet : 0 },
    kills: kills.length,
    openAtKills: kills.reduce((a, k) => a + (k.open ?? 0), 0),
    restartMs: { p50: pct(kills.map((k) => k.restartMs).filter((x) => x != null), 0.5), max: Math.max(0, ...kills.map((k) => k.restartMs ?? 0)) },
    meanSessionMs: d,
    spanMs,
    ceilingFromKills: 1 - lossFromKills,
    pushes: { acked: acked.length, ackedInLog: acked.length - lost.length, lost: lost.map((s) => ({ n: s.n, ref: s.ref, sha: s.sha })), failedFoundInLog: failedInLog.length },
  };
}
