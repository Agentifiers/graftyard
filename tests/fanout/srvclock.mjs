// Cloudflare's clock as the orb sees it. The orb's clock is unsynchronised and ran 7.3 to 7.5 s behind Cloudflare's on
// 2026-11-18 (`tests/am1/results/health-gate-lease.md`, "The rule saw what the trace shows"), so a harness stamp from
// `Date.now()` is not on the clock the members' trace rows use. `measure` bounds the offset (server minus orb, ms)
// from HTTP `Date` headers: each answer's whole second D was stamped at some server time S in [D, D + 1000) and some
// orb time in [sent, received], so the offset lies in (D - received, D + 1000 - sent); the intersection over samples
// timed to land on a server second boundary narrows it to about the round trip.
//   node tests/fanout/srvclock.mjs [url]   -> {"offsetMs": .., "errMs": .., "samples": ..}
// `serverClock(url)` keeps one offset fresh (re-measured in the background every REFRESH_MS) for `now()`.

export const DEFAULT_URL = "https://www.cloudflare.com/cdn-cgi/trace";
export const REFRESH_MS = 60_000;

/** The offset interval from samples `{sent, received, date}` (orb ms, orb ms, the header's epoch ms): `{lo, hi}`, or
 * null if the samples contradict each other. */
export function bound(samples) {
  let lo = -Infinity, hi = Infinity;
  for (const s of samples) {
    const d = Math.floor(s.date / 1000) * 1000;
    lo = Math.max(lo, d - s.received);
    hi = Math.min(hi, d + 1000 - s.sent);
  }
  return lo <= hi ? { lo, hi } : null;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, Math.max(0, ms)));

/** Samples `url`'s Date header until the bound is within `goalMs` or `rounds` pass. After the first, each request is
 * sent so it reaches the server at its next whole second by the current estimate, which halves the bound per round
 * down to the round trip. `fetchFn` and `clock` are the test's seams. */
export async function measure(url = DEFAULT_URL, { rounds = 8, goalMs = 40, fetchFn = fetch, clock = Date.now, wait = sleep } = {}) {
  const samples = [];
  let b = null, rtt = 0;
  for (let i = 0; i < rounds; i++) {
    if (b) {
      // Sent so that, if the offset is the midpoint and the request arrives half a round trip after leaving, it lands
      // on a whole server second: the answer's second then says which half the offset is in, give or take rtt / 2.
      const arrive = clock() + (b.lo + b.hi) / 2 + rtt / 2;
      await wait(Math.ceil(arrive / 1000) * 1000 - arrive);
    }
    const sent = clock();
    const r = await fetchFn(url, { method: "HEAD", signal: AbortSignal.timeout(10_000) });
    const received = clock();
    const date = Date.parse(r.headers.get("date") ?? "");
    if (!Number.isFinite(date)) throw new Error(`${url}: no Date header`);
    samples.push({ sent, received, date });
    rtt = received - sent;
    b = bound(samples);
    if (!b) throw new Error(`${url}: Date headers disagree with each other`);
    if (b.hi - b.lo <= 2 * goalMs) break;
  }
  return { offsetMs: Math.round((b.lo + b.hi) / 2), errMs: Math.round((b.hi - b.lo) / 2), samples: samples.length };
}

/** One offset kept fresh: `ready` resolves after the first measurement; `now()` is the server's clock in ms by the
 * last one; `offset()` the last measurement itself. A failed refresh keeps the last offset. */
export function serverClock(url = DEFAULT_URL, refreshMs = REFRESH_MS) {
  let last = null, at = 0, busy = false;
  const refresh = async () => {
    if (busy) return;
    busy = true;
    try { last = await measure(url); at = Date.now(); } catch { /* keep the last offset */ } finally { busy = false; }
  };
  const ready = refresh();
  const now = () => {
    if (Date.now() - at > refreshMs) refresh();
    return Date.now() + (last?.offsetMs ?? 0);
  };
  return { ready, offset: () => last, now, iso: () => new Date(now()).toISOString() };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  console.log(JSON.stringify(await measure(process.argv[2] ?? DEFAULT_URL)));
}
