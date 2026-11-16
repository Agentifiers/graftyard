// BF-54's re-poll check from the driver's `diag-<box>.txt` files (tests/m4/wp4/bf54.mjs): a runner that got a 503
// must poll again within 30 s. The runner logs only failed polls, so a gap between two logged lines may hold
// successful polls and proves nothing. What the log does show is each retry after a 503:
// - inside one retry chain, `Attempt k` failed and `Attempt k+1` is the runner's next poll (it waits `Retry-After`);
// - after a chain's last attempt, `Back off S seconds` and the next `Attempt 1` line, if it failed too.
// The time from one failed attempt to the next failed one is the runner's wait plus the next request's own time,
// which includes the install's answer: a held poll whose match commit failed answers 503 late. So the verdict is the
// runner's own wait, every `Back off` at most 30 s (in-chain attempts wait the edge's `Retry-After: 15`), and the
// chain gaps over 30 s are reported as `slowAttempts`, the install's slow answers.
//
//   node tests/m4/wp4/bf54-repoll.mjs tests/m4/raw/wp4/bf54
// (The committed run keeps them in `diag.tar.gz`: `tar xzf diag.tar.gz` in that directory first.)
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const dir = process.argv[2] ?? "tests/m4/raw/wp4/bf54";
const LIMIT_S = 30;
// diag line: <runner>|<YYYY-MM-DD HH:MM:SS>|<LEVEL>|<Component>|<message>
const byRunner = new Map();
for (const f of readdirSync(dir).filter((f) => /^diag-.*\.txt$/.test(f))) {
  for (const line of readFileSync(join(dir, f), "utf8").split("\n")) {
    const [runner, at, , comp, ...msg] = line.split("|");
    const t = Date.parse(`${(at ?? "").replace(" ", "T")}Z`);
    if (!runner || Number.isNaN(t)) continue;
    if (!byRunner.has(runner)) byRunner.set(runner, []);
    byRunner.get(runner).push({ t, comp, msg: msg.join("|") });
  }
}
const inChain = [], afterBackoff = [], backoffs = [], over = [], slow = [];
let with503 = 0;
for (const [runner, lines] of byRunner) {
  if (lines.some((l) => /ServiceUnavailable/.test(l.msg))) with503 += 1;
  lines.forEach((l, i) => {
    const a = l.msg.match(/^Attempt (\d+) of GET .*ServiceUnavailable/);
    if (a) {
      const k = Number(a[1]);
      const next = lines.slice(i + 1).find((n) => /^Attempt \d+ of GET/.test(n.msg));
      if (next && Number(next.msg.match(/^Attempt (\d+)/)[1]) === k + 1) {
        const g = (next.t - l.t) / 1000;
        inChain.push(g);
        if (g > LIMIT_S) slow.push({ runner, at: new Date(l.t).toISOString(), attempt: k, gapS: g });
      }
    }
    const b = l.msg.match(/^Back off (\d+(?:\.\d+)?) seconds/);
    if (b) {
      backoffs.push(Number(b[1]));
      if (Number(b[1]) > LIMIT_S) over.push({ runner, at: new Date(l.t).toISOString(), backoffS: Number(b[1]) });
      const next = lines.slice(i + 1).find((n) => /^Attempt 1 of GET/.test(n.msg) || /timed out|Running job/.test(n.msg));
      if (next && /^Attempt 1 of GET/.test(next.msg) && next.t - l.t <= 60_000) afterBackoff.push((next.t - l.t) / 1000);
    }
  });
}
const stat = (xs) => { const v = [...xs].sort((a, b) => a - b); return { n: v.length, p50: v[Math.floor(v.length / 2)] ?? null, p99: v[Math.floor(v.length * 0.99)] ?? null, max: v.at(-1) ?? null }; };
const out = { runnersWith503: with503, retryInChainS: stat(inChain), backoffS: stat(backoffs), firstFailedRetryAfterBackoffS: stat(afterBackoff), overCount: over.length, over: over.slice(0, 50), slowAttempts: slow.length, slow: slow.slice(0, 20) };
out.rePollWithin30s = with503 > 0 && backoffs.length > 0 && over.length === 0;
writeFileSync(join(dir, "repoll.json"), JSON.stringify(out, null, 2));
console.log(JSON.stringify(out));
