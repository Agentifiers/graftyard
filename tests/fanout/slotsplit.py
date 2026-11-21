#!/usr/bin/env python3
"""slotsplit.py TSAVE LOAD_START_ISO SECONDS [SITE] -> B0-29 m3fb29's commit queue tail, per command, from the leader's
trace: by class, by the frozen batches ahead of it when it arrived, and by what its batch's append waited on.

Per command, from the leader's rows (default site 0):
- arrival: each `/commit` reaching the leader's handler (`Entered`, kind 25, route code 3 (`ROUTES` index + 1), `index // 64` its arrival
  number). Admission comes after the body read and decode, so this reads queue a few ms long [inference].
- its cut: the first `Cut` (kind 29) at or after the arrival. Every cut on this row ended `Empty` (nothing left
  queued), so a cut takes everything that arrived since the one before it.
- its append: the cut's batch is appended as the (held + 1)-th entry append (`AppendQueued`, kind 12, first per last
  index, to a voter) after the cut, `held` the frozen batches before it (`cut_row`).
- queue = append - arrival, split as the leader's `/stats` splits it (`queue_split`): slot to the last apply
  (`Applied`, kind 4) before the append, post from it.
- ahead: the cuts made before the arrival whose batches were not yet appended at it (frozen batches ahead), and
  whether an entry was in flight (appended, not yet applied).
- its cycle's wait, for the entry its batch rode: in-flight = the previous entry's apply minus the cut (when the cut
  came first), after = the append minus the later of the cut and that apply, which is the batch's dependency reads,
  `decide` and encode once nothing else holds it [inference: the trace has no per-read row].
Class is per cut (`taken` per class): a cut whose commands are all one class names theirs exactly; a mixed cut's
commands are counted in each class by its share (weights)."""
import bisect, collections, datetime, json, lzma, sys

path, start, secs = sys.argv[1], sys.argv[2], int(sys.argv[3])
SITE = int(sys.argv[4]) if len(sys.argv) > 4 else 0
t0 = int(datetime.datetime.fromisoformat(start.replace("Z", "+00:00")).timestamp() * 1000)
t1 = t0 + 1000 * secs
CLASSES = ["human", "mergeq", "bot", "agent"]
seen, arrivals, cuts, appends, applied, sizes = set(), [], [], {}, {}, {}
for l in (lzma.open if path.endswith(".xz") else open)(path, "rt"):
    d = json.loads(l)
    if "rows" not in d or "learner" in d or d.get("site") != SITE:
        continue
    for r in d["rows"]:
        if tuple(r) in seen:
            continue
        seen.add(tuple(r))
        at, kind, peer = r[0], r[1], r[2]
        b, i, ms = int(float(r[3])), int(float(r[4])), int(float(r[5]))
        if kind == 25 and i % 64 == 3:
            arrivals.append(at)
        elif kind == 29:
            cuts.append(dict(at=at, held=(peer // 16) % 4, in_flight=peer >= 64,
                             taken=[(b >> (12 * c)) & 0xFFF for c in range(4)]))
        elif kind == 12 and peer < 192:
            appends.setdefault(i, at)
            sizes.setdefault(i, ms)
        elif kind == 4:
            applied.setdefault(i, at)
cuts.sort(key=lambda c: c["at"])
app = sorted((at, i) for i, at in appends.items())
app_t = [a for a, _ in app]
ap_idx = sorted(applied)
ap_t = sorted(applied.values())


def apply_of(last):
    j = bisect.bisect_left(ap_idx, last)
    return applied[ap_idx[j]] if j < len(ap_idx) else None


for c in cuts:
    k = bisect.bisect_right(app_t, c["at"]) + c["held"]
    c["append"], c["last"] = app[k] if k < len(app) else (None, None)
    c["prev_apply"] = apply_of(app[k - 1][1]) if 0 < k <= len(app) else None
    c["prev_append"], c["prev_last"] = app[k - 1] if 0 < k <= len(app) else (None, None)
cut_t = [c["at"] for c in cuts]
cmd = []
for a in sorted(arrivals):
    if not t0 <= a < t1:
        continue
    j = bisect.bisect_left(cut_t, a)
    if j >= len(cuts) or cuts[j]["append"] is None:
        continue
    c = cuts[j]
    P = c["append"]
    k = bisect.bisect_right(ap_t, P) - 1
    last_apply = ap_t[k] if k >= 0 else a
    freed = min(max(last_apply, a), P)
    ahead = sum(1 for x in cuts[max(0, j - 4):j] if x["at"] <= a and x["append"] is not None and x["append"] > a)
    pa = c["prev_apply"] or c["at"]
    cmd.append(dict(q=P - a, slot=freed - a, post=P - freed, ahead=ahead, pre=c["at"] - a,
                    inflight=max(0, pa - c["at"]), after=P - max(c["at"], pa), cut=c))


def pc(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else None


def wpc(pairs, q):
    pairs = sorted(pairs)
    tot = sum(w for _, w in pairs)
    acc = 0
    for v, w in pairs:
        acc += w
        if acc >= q * tot:
            return v
    return None


n = len(cmd)
print(f"{n} /commit arrivals matched in the window ({len(arrivals)} traced, {len(cuts)} cuts, {len(app)} appends)")
print(f"queue p50/p99 {pc([c['q'] for c in cmd], .5)}/{pc([c['q'] for c in cmd], .99)}, "
      f"slot {pc([c['slot'] for c in cmd], .5)}/{pc([c['slot'] for c in cmd], .99)}, "
      f"post {pc([c['post'] for c in cmd], .5)}/{pc([c['post'] for c in cmd], .99)}  (/stats: 266/819, 222/704, 3/327)")
tail = pc([c["slot"] for c in cmd], .99)
print(f"\nby class (weighted by each cut's taken share; 'pure' = cuts of one class only)")
for ci, name in enumerate(CLASSES):
    w = [(c, c["cut"]["taken"][ci] / max(1, sum(c["cut"]["taken"]))) for c in cmd]
    w = [(c, x) for c, x in w if x > 0]
    if not w:
        continue
    pure = [c for c, x in w if x == 1]
    print(f"  {name}: weight {sum(x for _, x in w):.0f}; queue p50/p99 {wpc([(c['q'], x) for c, x in w], .5)}/"
          f"{wpc([(c['q'], x) for c, x in w], .99)}, slot p99 {wpc([(c['slot'], x) for c, x in w], .99)}; "
          f"pure cuts n {len(pure)} queue p99 {pc([c['q'] for c in pure], .99)} slot p99 {pc([c['slot'] for c in pure], .99)}")
print("\nby frozen batches ahead at arrival")
for k, g in sorted(collections.Counter(c["ahead"] for c in cmd).items()):
    cs = [c for c in cmd if c["ahead"] == k]
    print(f"  {k}: n {g} ({100 * g / n:.1f}%), queue p50/p99 {pc([c['q'] for c in cs], .5)}/{pc([c['q'] for c in cs], .99)}, "
          f"slot p50/p99 {pc([c['slot'] for c in cs], .5)}/{pc([c['slot'] for c in cs], .99)}; "
          f"share of slot >= {tail} ms: {sum(1 for c in cs if c['slot'] >= tail)}")
print("\nwhat each command waited through (all, then the slot tail >= p99)")
for label, cs in (("all", cmd), (f"slot >= {tail}", [c for c in cmd if c["slot"] >= tail])):
    print(f"  {label}: n {len(cs)}; arrival->cut p50/p99 {pc([c['pre'] for c in cs], .5)}/{pc([c['pre'] for c in cs], .99)}, "
          f"cut->previous apply (entry in flight) {pc([c['inflight'] for c in cs], .5)}/{pc([c['inflight'] for c in cs], .99)}, "
          f"after that apply to append (deps, decide, encode) {pc([c['after'] for c in cs], .5)}/{pc([c['after'] for c in cs], .99)}")
    s = {k: sum(c[k] for c in cs) for k in ("pre", "inflight", "after", "q")}
    print(f"    summed: arrival->cut {100 * s['pre'] / s['q']:.0f}%, in flight {100 * s['inflight'] / s['q']:.0f}%, "
          f"after the apply {100 * s['after'] / s['q']:.0f}% of queue")
    dom = collections.Counter("in flight" if c["inflight"] >= c["after"] else "after the apply" for c in cs)
    print(f"    cycle waited mostly on: {dict(dom)}")

print("\nthe entry in flight ahead of the slot tail (its append to apply, and its AppendEntries bytes), against all entries")
def ahead_entry(cs):
    es = {c["cut"]["prev_last"]: c["cut"] for c in cs if c["cut"]["prev_last"] is not None and c["cut"]["prev_apply"]}
    rt = [x["prev_apply"] - x["prev_append"] for x in es.values()]
    by = [sizes.get(i, 0) for i in es]
    return len(es), rt, by
for label, cs in (("all", cmd), (f"slot >= {tail}", [c for c in cmd if c["slot"] >= tail])):
    m, rt, by = ahead_entry(cs)
    print(f"  {label}: {m} entries; append->apply p50/p99 {pc(rt, .5)}/{pc(rt, .99)}; bytes p50/p99 {pc(by, .5)}/{pc(by, .99)}; "
          f"over 100 KB {sum(1 for b in by if b > 100_000)}")
