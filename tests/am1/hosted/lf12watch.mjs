// LF-12's timing as a time series (am1-fix): every `every` ms (default 2,000), each voter's `/stats?reset=1` and
// `/status` on `FEED_REPO_ID`, so each row covers the window since the last. Run beside `feed.mjs lf12` to see where
// in the run the leader's commit time goes. Stops after `secs` (default 600). Rows: tests/am1/raw/lf12watch-<chain>.jsonl.
//   node tests/am1/hosted/lf12watch.mjs [secs] [every]
import { member, row, sleep } from "./lib.mjs";

const [secsArg, everyArg] = process.argv.slice(2);
const CHAIN = `repo:${process.env.FEED_REPO_ID ?? 1048577}`;
const OUT = new URL(`../raw/lf12watch-${CHAIN.replace(":", "-")}.jsonl`, import.meta.url).pathname;
const end = Date.now() + Number(secsArg ?? 600) * 1000;
const every = Number(everyArg ?? 2000);
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const sum = (xs) => (xs?.length ? { n: xs.length, p50: pct(xs, 0.5), p99: pct(xs, 0.99), max: Math.max(...xs) } : null);

while (Date.now() < end) {
  const t = Date.now();
  const v = await Promise.all([0, 1, 2].map(async (s) => {
    try {
      const st = (await member(CHAIN, s, "/stats?reset=1", undefined, 10_000)).v;
      const ss = (await member(CHAIN, s, "/status", undefined, 10_000)).v;
      return {
        site: s, role: ss?.role, term: ss?.term, commit: ss?.commit, feed: ss?.feed, learners: ss?.config?.config?.normal?.learners?.length,
        commitMs: st.commitMs, round: sum(st.roundMs), confirm: sum(st.confirmMs), applyStep: sum((st.applySteps ?? []).map((a) => a[0])),
        append: Object.fromEntries(Object.entries(st.appendMs ?? {}).map(([k, x]) => [k, sum(x)])),
        inbound: st.inbound, inboundMs: st.inboundMs, failedCalls: st.failedCalls, catchups: st.catchups, installs: st.installs,
        depReads: st.depReads, calls: st.calls,
      };
    } catch (e) { return { site: s, error: String(e).slice(0, 80) }; }
  }));
  row(OUT, { t, members: v });
  await sleep(Math.max(0, every - (Date.now() - t)));
}
