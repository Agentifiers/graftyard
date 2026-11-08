// BF-2 on graft-m2b (`build/BUILD-PLAN.md`, M3-WP2): revoke a token while 100 of its pushes wait in admission; none
// may be acknowledged at an index after the revocation's. Each run:
// 1. a new `ghp_` for m3-member (stored and listed as the operator);
// 2. 100 pushes under it reach m3org/j4's leader in one `/commit_many` (`/_bed/rows/queue`), whose first dependency
//    read is held `hold_ms`, so they wait in admission behind it;
// 3. `delay_ms` later the token is revoked on its `Tokens` shard (`Revoke`, as `DELETE /applications/{client}/token`
//    commits), at index R of that shard;
// 4. the leader's `deps_log` names, for every entry it decided, the index of the token's shard it read. A push
//    acknowledged (`ok`) from an entry that read the shard at or past R is the fail value; so is one acknowledged
//    while its entry's read is missing. Every push decided from a read at or past R must be refused 401.
// `delay_ms` cycles 2,000 and 0 (revoked while the first read is held: every push must be refused) and 9,000, a
// control revoked after the queue decided, whose pushes are acknowledged from reads below R, which shows the check
// can tell the two apart.
//   node tests/m3/rows/bf2.mjs <runs> <out.jsonl>
import { BED, EDGE, OP, commit, log, pat, row, sleep, state } from "./lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 5);
const N = 100;
const c = { runs, queued: 0, acked: 0, refused401: 0, ackedAfter: 0, ackedUnread: 0, otherOutcomes: 0, runsOk: 0 };
for (let i = 1; i <= runs; i++) {
  const t = await pat(state.member, `bf2 ${i}`);
  const hash = Buffer.from(t.hash, "hex");
  const shard = hash.readUInt32BE(0) % 3072;
  const delay = [2_000, 0, 9_000][(i - 1) % 3];
  const hold = 6_000;
  const queue = fetch(`${EDGE}/_bed/rows/queue`, {
    method: "POST", signal: AbortSignal.timeout(300_000),
    headers: { "x-bed-token": BED, authorization: `token ${t.text}`, "content-type": "application/json" },
    body: JSON.stringify({ repo: state.repo, n: N, hold_deps_ms: hold, hold_ms: 120_000, tag: `r${Date.now()}` }),
  }).then((r) => r.json());
  await sleep(delay);
  const rv = await commit(`tokens:${shard}`, { cred: OP, op: { Revoke: { hash: [...hash] } } });
  const R = rv?.ok?.index;
  const q = await queue;
  const results = q.results ?? [];
  const chain = `tokens:${shard}`;
  // The `Tokens` index each entry read, by the entry's index.
  const readAt = new Map();
  for (const d of q.deps ?? []) {
    const r = (d.reads ?? []).find((x) => x.chain === chain);
    if (r) readAt.set(d.index, r.index);
  }
  let acked = 0, refused = 0, after = 0, unread = 0, other = 0;
  const entries = new Set();
  for (const x of results) {
    const ok = x.done?.ok;
    const ref = x.done?.refused;
    const idx = ok?.index ?? ref?.index;
    if (idx !== undefined) entries.add(idx);
    const seen = readAt.get(idx);
    if (ok) {
      acked++;
      if (seen === undefined) unread++;
      else if (R !== undefined && seen >= R) after++;
    } else if (ref?.status === 401) refused++;
    else other++;
  }
  // The control must have acknowledged pushes, every one from a read below R.
  const control = delay > hold;
  const runOk = R !== undefined && results.length === N && after === 0 && unread === 0 && other === 0 && acked + refused === N
    && (control ? acked > 0 : true);
  Object.assign(c, { queued: c.queued + results.length, acked: c.acked + acked, refused401: c.refused401 + refused,
    ackedAfter: c.ackedAfter + after, ackedUnread: c.ackedUnread + unread, otherOutcomes: c.otherOutcomes + other,
    runsOk: c.runsOk + (runOk ? 1 : 0) });
  const reads = [...readAt.entries()].sort((a, b) => a[0] - b[0]);
  row(out, { i, delayMs: delay, holdMs: hold, shard, revokeIndex: R ?? null, revoke: rv?.ok ? "ok" : JSON.stringify(rv).slice(0, 80),
    leader: q.leader, ms: q.ms, results: results.length, acked, refused401: refused, ackedAfter: after, ackedUnread: unread, other,
    entries: [...entries].sort((a, b) => a - b), tokenReads: reads, control, ok: runOk });
  log(`BF-2 ${i}/${runs}: revoke@${R} acked ${acked} refused ${refused} after ${after} unread ${unread} other ${other} entries ${entries.size} ok ${runOk}`);
}
row(out, { summary: c });
log(`BF-2: ${JSON.stringify(c)}`);
