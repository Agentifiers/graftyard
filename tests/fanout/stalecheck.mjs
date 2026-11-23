// stale-log's check: is every `Stale` push in a run the git lease working, not a lost write?
//   node tests/fanout/stalecheck.mjs <a1-pushes-RUN.jsonl> [edge-push-lines.jsonl]   -> one JSON verdict, exit 1 on FAIL
//
// The pushes file is a1.mjs's per-push log: one row per push, `{client, kind, ref, old, new, start, end, status, result,
// err, rid}` (`start`/`end` on Cloudflare's clock, `srvclock.mjs`). The edge file, when given, is the test edge's
// `push_log` lines from Workers Logs, one `{"push", "client", "status", "end", ...}` per line, where `end` is the
// leader's answer (`{"index", "refs"}` for a commit) or null for a push the edge answered before its commit.
//
// A push can be refused `stale` (git's "fetch first") correctly only when the ref is not at the `old` it named. A1's
// pushers each own their ref and name the tip their last `ok` returned, so for each `stale` push the previous push to
// the same ref decides it:
//   - FAIL: the previous push was acked `ok` with the tip this push names as `old`, and nothing else was in flight to
//     the ref: the ref is where the client was told it is, and the group still said it moved. That breaks the write
//     path's invariant, unless it is a race below.
//   - PASS (lost reply): the previous push's reply never reached the client (status 0, or a non-200 the edge answered
//     200), and the edge shows it committed that ref `ok`: the ref moved under the client, so `Stale` is correct.
//   - PASS (race): another push to the same ref overlapped this one in time (humans share refs).
//   - PASS (first): the ref's previous push names a different tip than this push's `old` (the client's own bookkeeping,
//     e.g. a refused push leaves `old` unchanged and so cannot be the cause).
//   - UNKNOWN: the previous push lost its reply and no edge line says what happened to it.
// The run passes when no row is FAIL; UNKNOWN rows are reported, not passed.

import { readFileSync } from "node:fs";

/** Each `stale` push's verdict, from `pushes` (the per-push rows) and `edge` (the edge's lines, by client id). */
export function judge(pushes, edge = []) {
  const byClient = new Map(edge.filter((e) => e.client).map((e) => [e.client, e]));
  const byRef = new Map();
  for (const p of pushes) (byRef.get(p.ref) ?? byRef.set(p.ref, []).get(p.ref)).push(p);
  for (const xs of byRef.values()) xs.sort((a, b) => a.start - b.start);
  const out = [];
  for (const p of pushes) {
    if (p.result !== "stale") continue;
    const same = byRef.get(p.ref);
    const overlap = same.some((q) => q !== p && q.start < p.end && p.start < q.end);
    const before = same.filter((q) => q !== p && q.end <= p.start);
    const prev = before[before.length - 1];
    const row = { client: p.client, ref: p.ref, old: p.old, prev: prev?.client ?? null };
    if (overlap) { out.push({ ...row, verdict: "PASS", why: "race: another push to the ref overlapped it" }); continue; }
    if (!prev) { out.push({ ...row, verdict: "UNKNOWN", why: "no earlier push to the ref in the log" }); continue; }
    const committed = (e) => e?.end?.refs?.length > 0 && e.end.refs.every((r) => r === "Ok");
    if (prev.status === 200 && prev.result === "ok") {
      out.push({ ...row, verdict: prev.new === p.old ? "FAIL" : "PASS",
        why: prev.new === p.old ? "the previous push was acked ok at this push's old tip" : "this push named another tip than the acked one" });
      continue;
    }
    if (prev.status === 200 && prev.result !== "ok") {
      // A refused push leaves the ref where it was. So the ref is at the newest push acked `ok` before this push started,
      // when every push that ended since that ack was refused too; otherwise (no ack in the log) at the refused push's
      // `old`. A Stale is wrong only when this push named that tip. On b9m3f1 a push overlapping an ack was refused, and
      // judging the next push against the refused one's `old` called a correct Stale a FAIL.
      const acked = before.filter((q) => q.status === 200 && q.result === "ok").pop();
      const since = acked ? before.filter((q) => q.end > acked.end) : before;
      const refusedSince = since.every((q) => q.status === 200 && q.result !== "ok");
      const tip = acked && refusedSince ? acked.new : prev.old;
      const at = acked && refusedSince ? `the push acked ok before it (${acked.client})` : `the previous push (${prev.result})`;
      out.push({ ...row, verdict: tip === p.old ? "FAIL" : "PASS",
        why: tip === p.old ? `${at} left the ref at this push's old tip` : `${at} left the ref at another tip than this push's old` });
      continue;
    }
    // The previous push's reply never reached the client: the edge says whether it committed.
    const e = byClient.get(prev.client);
    if (!e) { out.push({ ...row, verdict: "UNKNOWN", why: `the previous push's reply was lost (${prev.err ?? prev.status}) and no edge line names it` }); continue; }
    if (committed(e)) out.push({ ...row, verdict: "PASS", why: `lost reply: the edge answered the previous push ${e.status} and it committed at index ${e.end.index}` });
    // No leader answer on the line (a commit that ran out of time is a 500 with no `end`): it may still have committed.
    else if (!e.end) out.push({ ...row, verdict: "UNKNOWN", why: `the previous push ended ${e.status} at the edge with no answer from the leader` });
    else out.push({ ...row, verdict: "FAIL", why: "lost reply, but the leader answered the previous push without committing it" });
  }
  const n = (v) => out.filter((x) => x.verdict === v).length;
  return { stale: out.length, pass: n("PASS"), fail: n("FAIL"), unknown: n("UNKNOWN"), verdict: n("FAIL") ? "FAIL" : n("UNKNOWN") ? "UNKNOWN" : "PASS", rows: out };
}

const lines = (f) => readFileSync(f, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));

if (import.meta.url === `file://${process.argv[1]}`) {
  const [pushes, edge] = process.argv.slice(2);
  const v = judge(lines(pushes), edge ? lines(edge) : []);
  console.log(JSON.stringify(v));
  process.exit(v.fail ? 1 : 0);
}
