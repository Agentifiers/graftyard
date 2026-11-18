#!/usr/bin/env python3
"""appendgaps.py TSAVE.jsonl[.xz] FROM TO [MIN_MS] [TERMS.jsonl] -> where a follower's gaps between the leader's AppendEntries went.

For each voter-follower f and each gap of MIN_MS (1,000) or more between two AppendEntries it stepped from the same
leader L (`AppendStepped`, 17), inside [FROM, TO], it finds the `/raft` call that ended the gap: f's last `Entered` (25)
row for a `/raft` call from L at or before the second step, joined by call id to L's `CallSent` (23). Then it splits
the gap, each part on one isolate's own clock:
  - send: on L's clock, the longest stretch with no `/raft` call to f among L's `CallSent` rows from the call that
    started the gap to the one that ended it. A part here is the leader not sending. Inside it, the leader's own
    event-loop lag (`Lag`, 8, the ticker's lateness, and each `CallSent`'s loop-lag field), its storage confirms
    (`Confirm`, 10) and synchronous steps (`Stepped`, 15) that overlap it, and the requests that entered L meanwhile;
  - out: the gap-ending call's out leg, f's `Entered` minus L's `CallSent`, corrected by the pair's skew as
    `tests/f74/inbound.py` estimates it; with the requests that entered f ahead of it and their routes;
  - handler: f's `AppendStepped` minus that call's `Entered`, on f's clock (body read, decode, input gate, step);
  - lost: calls L sent to f inside the gap that never entered f, or entered and ended the gap no sooner.
A gap during which L stopped leading or the term changed (from terms.mjs's samples, when TERMS is given) is set aside:
it is an election, not a stall. A gap's dominant part is the largest of send, out and handler. `CallSent`'s awaiting field is L's `/raft` fetches then
waiting for response headers.
  python3 tests/fanout/appendgaps.py tests/fanout/raw/tsave-m47b33.jsonl.xz 2026-11-18T01:00:18Z 2026-11-18T01:05:18Z"""
import bisect, collections, datetime, json, lzma, sys

ROUTES = ["/raft", "/read", "/commit", "/commit_many", "/outcome", "/outcome_by_request", "/suspect", "/campaign",
          "/status", "/stats", "/trace", "/ping", "/rtt", "/place", "/learners", "/record"]
route = lambda code: ROUTES[code - 1] if 0 < code <= len(ROUTES) else "other"
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
hm = lambda t: datetime.datetime.fromtimestamp(t / 1000, datetime.UTC).strftime("%H:%M:%S.%f")[:-3]
path, T0, T1 = sys.argv[1], iso(sys.argv[2]), iso(sys.argv[3])
MIN = int(sys.argv[4]) if len(sys.argv) > 4 else 1000
TERMS = sys.argv[5] if len(sys.argv) > 5 else None
# WIDE=1: the exact split also takes gaps of up to 20 s whose sender led at the gap's start by terms.mjs's samples,
# though a sample inside the gap missed a member or saw another term (`held` drops those).
WIDE = __import__("os").environ.get("WIDE") == "1"
# terms.mjs samples: (at, {site: (role, term)}).
samples = []
if TERMS:
    for line in open(TERMS):
        r = json.loads(line)
        if r.get("members"):
            samples.append((iso(r["at"]), {m[0]: (m[1], m[2]) for m in r["members"]}))
    samples.sort()


def leader_at(t):
    ts = [x for x, _ in samples]
    i = max(0, bisect.bisect_right(ts, t) - 1)
    return next((k for k, (r, _) in samples[i][1].items() if r == "leader"), None) if samples else None


def held(L, a, b):
    """Whether L led one term through [a, b]: every sample from the last one before a to the first after b."""
    if not samples:
        return True
    ts = [t for t, _ in samples]
    i, j = max(0, bisect.bisect_right(ts, a) - 1), min(len(samples), bisect.bisect_left(ts, b) + 1)
    seen = {m.get(L) for _, m in samples[i:j] if m.get(L, ("?",))[0] != "?"}
    return len(seen) == 1 and next(iter(seen))[0] == "leader"

rows = collections.defaultdict(set)
for line in (lzma.open if path.endswith(".xz") else open)(path, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"])
rows = {s: sorted(rs) for s, rs in rows.items()}
kind = lambda s, k: [x for x in rows.get(s, []) if x[1] == k]


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


# Per caller: /raft calls sent, by (caller, id) and in time order per callee.
sent, sent_to = {}, collections.defaultdict(list)
for s in rows:
    for at, _, peer, cid, awaiting, lag in kind(s, 23):
        sent[(s, cid)] = (at, peer, awaiting, lag)
        sent_to[(s, peer)].append((at, cid))
for v in sent_to.values():
    v.sort()
# Per callee: Entered rows, all of them (for "ahead") and /raft ones by (caller, id).
entered, raft_in, left = {}, {}, {}
for s in rows:
    ent = kind(s, 25)
    entered[s] = ([x[0] for x in ent], ent)
    for at, _, frm, cid, code, open_ in ent:
        if code % 64 == 1 and frm < 128 and cid:
            raft_in[(frm, cid)] = (s, at, code // 64)
    for at, _, peer, cid, _, _ in kind(s, 24):
        left[(peer, cid)] = at
answer = {(s, x[3]): x[0] for s in rows for x in kind(s, 21) if x[3]}
# The pair skew, as inbound.py estimates it: (min out - min back) / 2.
skew = {}
outs, backs = collections.defaultdict(list), collections.defaultdict(list)
for (caller, cid), (at, peer, _, _) in sent.items():
    if (caller, cid) in raft_in:
        outs[(caller, peer)].append(raft_in[(caller, cid)][1] - at)
        if (caller, cid) in answer and (caller, cid) in left:
            backs[(caller, peer)].append(answer[(caller, cid)] - left[(caller, cid)])
for pair in outs:
    if backs[pair]:
        skew[pair] = (min(outs[pair]) - min(backs[pair])) // 2
lagrows = {s: [(x[0], x[5]) for x in kind(s, 8)] for s in rows}
confirms = {s: [(x[0] - x[5], x[0]) for x in kind(s, 10)] for s in rows}
steps = {s: [(x[0], x[5]) for x in kind(s, 15)] for s in rows}


def overlap(spans, a, b):
    return sum(max(0, min(b, e) - max(a, s)) for s, e in spans if e > a and s < b)


gaps = []
for f in rows:
    st = [x for x in kind(f, 17)]
    raft_from = sorted((at, frm, cid) for (frm, cid), (callee, at, _) in raft_in.items() if callee == f)
    rat = [x[0] for x in raft_from]
    for (a, _, la, _, _, _), (b, _, lb, _, _, _) in zip(st, st[1:]):
        if la != lb or b - a < MIN or not (T0 <= a <= T1):
            continue
        L = la
        # The calls from L that entered f just before each step: the start and the end of the gap.
        def call_before(t):
            i = bisect.bisect_right(rat, t)
            while i:
                i -= 1
                at, frm, cid = raft_from[i]
                if frm == L:
                    return at, cid
            return None
        start, end = call_before(a), call_before(b)
        g = {"f": f, "L": L, "a": a, "b": b, "gap": b - a, "held": held(L, a, b)}
        if not end or end == start or (L, end[1]) not in sent:
            g["joined"] = False
            gaps.append(g)
            continue
        g["joined"] = True
        e_at, e_cid = end
        s_end, _, awaiting, lag_at_send = sent[(L, e_cid)]
        k = skew.get((L, f), 0)
        g["out_raw"] = e_at - s_end
        g["out"] = max(0, e_at - s_end - k)
        g["handler"] = b - e_at
        g["awaiting"], g["lag_at_send"] = awaiting, lag_at_send
        # On L's clock: its sends to f from the gap's start call to its end call.
        s_start = sent[(L, start[1])][0] if start and (L, start[1]) in sent else s_end - (b - a)
        seq = [t for t, _ in sent_to[(L, f)] if s_start <= t <= s_end]
        seq = [s_start] + seq + [s_end]
        hole = max((max(0, y - x), x) for x, y in zip(seq, seq[1:]))
        g["send"], g["hole_at"] = hole
        lo, hi = g["hole_at"], g["hole_at"] + g["send"]
        g["sent_in_gap"] = sum(1 for t, _ in sent_to[(L, f)] if s_start < t < s_end)
        g["lost"] = sum(1 for t, c in sent_to[(L, f)] if s_start < t < s_end and (L, c) not in raft_in)
        g["lag_max"] = max([ms for t, ms in lagrows[L] if lo <= t <= hi + 200] + [0])
        g["confirm"] = overlap(confirms[L], lo, hi)
        g["stepped"] = sum(ms for t, ms in steps[L] if lo <= t <= hi)
        lat, lent = entered[L]
        i, j = bisect.bisect_left(lat, lo), bisect.bisect_right(lat, hi)
        g["leader_in"] = collections.Counter(route(x[4] % 64) for x in lent[i:j])
        fat, fent = entered[f]
        i, j = bisect.bisect_left(fat, e_at - g["out"]), bisect.bisect_left(fat, e_at)
        g["ahead"] = collections.Counter(route(x[4] % 64) for x in fent[i:j])
        gaps.append(g)

merged = {}
for s_ in rows:
    m = []
    for b_, e_ in sorted(confirms[s_]):
        if m and b_ <= m[-1][1]:
            m[-1][1] = max(m[-1][1], e_)
        else:
            m.append([b_, e_])
    merged[s_] = m


def gate(s_, t):
    """How long the output gate of member s_ stays closed after t: a storage confirm in flight then."""
    m = merged[s_]
    i = bisect.bisect_right([x[0] for x in m], t) - 1
    return m[i][1] - t if i >= 0 and m[i][0] <= t < m[i][1] else 0

elect = [g for g in gaps if not g["held"]]
joined = [g for g in gaps if g["joined"] and g["held"]]
print(f"follower append gaps >= {MIN} ms in the window: {len(gaps)}; across a term change or leader loss: {len(elect)}"
      f" ({sum(g['gap'] for g in elect)} ms); in one term and joined to the call that ended them: {len(joined)}"
      f"; pair skews {dict(sorted(skew.items()))}")
dom = collections.Counter()
tot = collections.Counter()
for g in joined:
    parts = {"send": g["send"], "out": g["out"], "handler": g["handler"]}
    dom[max(parts, key=parts.get)] += 1
    for k, v in parts.items():
        tot[k] += v
print("dominant part (gaps):", dict(dom), "| summed ms:", dict(tot), "| summed gap ms:", sum(g["gap"] for g in joined))
for k in ("gap", "send", "out", "handler", "awaiting", "lag_at_send", "lag_max", "confirm", "stepped", "sent_in_gap",
          "lost"):
    xs = [g[k] for g in joined]
    print(f"  {k:12} p50 {pct(xs, .5)}  p90 {pct(xs, .9)}  max {max(xs) if xs else None}")
send_dom = [g for g in joined if max(("send", "out", "handler"), key=lambda k: g[k]) == "send"]
if send_dom:
    print(f"\nsend-dominated gaps: {len(send_dom)}; inside the leader's hole:")
    print(f"  ticker lag max p50 {pct([g['lag_max'] for g in send_dom], .5)} p90 {pct([g['lag_max'] for g in send_dom], .9)}"
          f"; share of the hole: lag {sum(min(g['lag_max'], g['send']) for g in send_dom) / sum(g['send'] for g in send_dom):.2f},"
          f" confirm {sum(min(g['confirm'], g['send']) for g in send_dom) / sum(g['send'] for g in send_dom):.2f},"
          f" sync step {sum(min(g['stepped'], g['send']) for g in send_dom) / sum(g['send'] for g in send_dom):.2f}")
    mix = collections.Counter()
    secs = sum(g["send"] for g in send_dom) / 1000
    for g in send_dom:
        mix.update(g["leader_in"])
    print("  requests entering the leader during its holes, per s:",
          {k: round(v / secs, 1) for k, v in mix.most_common(6)})
out_dom = [g for g in joined if max(("send", "out", "handler"), key=lambda k: g[k]) == "out"]
if out_dom:
    mix = collections.Counter()
    for g in out_dom:
        mix.update(g["ahead"])
    n = sum(mix.values())
    print(f"\nout-dominated gaps: {len(out_dom)}; ahead of the call at the follower p50 "
          f"{pct([sum(g['ahead'].values()) for g in out_dom], .5)}, by route",
          {k: round(v / n, 2) for k, v in mix.most_common(5)})
print("\nlongest 12 gaps: f L at gap send out handler | leader lag max, confirm, step ms, awaiting, sent/lost")
for g in sorted(joined, key=lambda g: -g["gap"])[:12]:
    print(f"  {g['f']} {g['L']} {hm(g['a'])} {g['gap']} {g['send']} {g['out']} {g['handler']} | {g['lag_max']} "
          f"{g['confirm']} {g['stepped']} {g['awaiting']} {g['sent_in_gap']}/{g['lost']}")

# The exact split: with s0, s1 the calls that entered f just before the gap's first and second steps, the gap is
# (L sent s1 - L sent s0) + (s1's out leg - s0's out leg) + (s1's handler - s0's handler), each on one clock, so the
# parts sum to the gap with no skew estimate: send spacing, queueing growth on the way in, handler growth.
ex = []
for f in rows:
    st = kind(f, 17)
    raft_from = sorted((at, frm, cid) for (frm, cid), (callee, at, _) in raft_in.items() if callee == f)
    rat = [x[0] for x in raft_from]

    def cb(t, L):
        i = bisect.bisect_right(rat, t)
        while i:
            i -= 1
            if raft_from[i][1] == L:
                return raft_from[i]

    for x, y in zip(st, st[1:]):
        a, b, L = x[0], y[0], x[2]
        wide = WIDE and b - a <= 20_000 and leader_at(a) == L
        if y[2] != L or b - a < MIN or not (T0 <= a <= T1) or not (held(L, a, b) or wide):
            continue
        s0, s1 = cb(a, L), cb(b, L)
        if not s0 or not s1 or s0 == s1 or (L, s0[2]) not in sent or (L, s1[2]) not in sent:
            continue
        t0, t1 = sent[(L, s0[2])][0], sent[(L, s1[2])][0]
        queue = (s1[0] - t1) - (s0[0] - t0)
        g_ = min(gate(L, t1), max(0, queue))
        l_ = min(sent[(L, s1[2])][3], max(0, queue - g_))
        ex.append({"gap": b - a, "send": t1 - t0, "queue": queue, "handler": (b - s1[0]) - (a - s0[0]),
                   "lag": max([ms for t, ms in lagrows[L] if t0 <= t <= t1 + 200] + [0]),
                   "confirm": overlap(confirms[L], t0, t1),
                   "send_confirm": min(max(0, t1 - t0), overlap(merged[L], t0, t1)),
                   "q_gate": g_, "q_lag": l_, "q_rest": queue - g_ - l_})
tot = sum(g["gap"] for g in ex)
print(f"\nexact split of {len(ex)} gaps in one term ({tot} ms), parts summing to each gap:")
for k in ("send", "queue", "handler"):
    xs = [g[k] for g in ex]
    print(f"  {k:8} share {sum(xs) / tot:.2f}  p50 {pct(xs, .5)}  p90 {pct(xs, .9)}  max {max(xs)}")
print("  dominant:", dict(collections.Counter(max(("send", "queue", "handler"), key=lambda k: g[k]) for g in ex)))
S = lambda k: sum(g[k] for g in ex) / tot
print(f"  storage: the leader's confirms during its send spacing {S('send_confirm'):.2f} plus its output gate closed at the"
      f" ending call's send {S('q_gate'):.2f}; the leader's loop lag at that send {S('q_lag'):.2f}; the rest of the out leg"
      f" (the wire and the follower's input queue) {S('q_rest'):.2f}")
sd = [g for g in ex if g["send"] >= 500]
print(f"  gaps whose send spacing is 500 ms or more: {len(sd)}; the leader's ticker lag max there p50 {pct([g['lag'] for g in sd], .5)}"
      f" max {max([g['lag'] for g in sd] + [0])}; its storage confirms overlapping p50 {pct([g['confirm'] for g in sd], .5)}")

# Robustness: the gaps whose calls all reached the follower (no trace hole), the out leg capped at the gap, the same
# without the skew correction, and the follower's own ticker lag and open requests while the call waited.
clean = [g for g in joined if g["lost"] == 0]
cov = lambda g, k: min(g[k], g["gap"])
print(f"\ngaps with no call lost: {len(clean)} of {len(joined)}; summed gap {sum(g['gap'] for g in clean)} ms, of which"
      f" out (capped at the gap) {sum(cov(g, 'out') for g in clean)}, send {sum(cov(g, 'send') for g in clean)},"
      f" handler {sum(cov(g, 'handler') for g in clean)}")
raw = [g["out_raw"] for g in clean]
print(f"  out leg without the skew correction p50 {pct(raw, .5)} p90 {pct(raw, .9)}")
fl = [max([ms for t, ms in lagrows[g['f']] if g['a'] <= t <= g['b']] + [0]) for g in clean]
print(f"  follower ticker lag max inside the gap p50 {pct(fl, .5)} p90 {pct(fl, .9)} max {max(fl) if fl else None}")
fc = [overlap(confirms[g['f']], g['a'], g['b']) for g in clean]
print(f"  follower storage confirm time inside the gap p50 {pct(fc, .5)} p90 {pct(fc, .9)}")
op = []
for g in clean:
    fat, fent = entered[g["f"]]
    i, j = bisect.bisect_left(fat, g["a"]), bisect.bisect_right(fat, g["b"])
    op += [x[5] for x in fent[i:j]]
    g["rate"] = (j - i) / (g["gap"] / 1000)
print(f"  follower open requests at each entry inside the gaps p50 {pct(op, .5)} p90 {pct(op, .9)} max {max(op) if op else None}"
      f"; requests entering it a second p50 {pct([g['rate'] for g in clean], .5)}")

# The withholdings themselves: for each moment a follower took late reports from two known colos inside SUSPECT's 10 s
# (the `withheld` cause in `tests/fanout/campaigns.py`), the leader's calls to it sent after its last step and before
# that moment, on the leader's clock shifted by the pair's skew: entered before the moment, entered after it (and how
# much later), or never.
V = ["leader", "resumed", "started", "bootstrapping", "promised", "busy", "won", "lost", "mayNotLead"]
print("\nwithholding moments: f L at | last step before (ms) | leader calls sent since: entered before / after (+ms) / never"
      " | requests entered f in that span")
moments = []
for f in rows:
    st = [x for x in kind(f, 17)]
    at17 = [x[0] for x in st]
    rep = [x for x in kind(f, 28) if T0 <= x[0] <= T1 and x[4] != 0 and V[x[5]] == "promised"]
    last_ep = -1 << 62
    for x in rep:
        i = bisect.bisect_right(at17, x[0])
        if not i or x[0] - at17[i - 1] < 1000:
            continue
        cs = {y[4] for y in rep if x[0] - 10_000 <= y[0] <= x[0] and bisect.bisect_right(at17, y[0])
              and y[0] - at17[bisect.bisect_right(at17, y[0]) - 1] >= 1000}
        if len(cs) >= 2 and x[0] > last_ep + 20_000:
            last_ep = x[0]
            moments.append((f, st[i - 1][2], x[0], at17[i - 1]))
for f, L, tw, la in moments:
    k = skew.get((L, f), 0)
    calls = [(t, c) for t, c in sent_to[(L, f)] if la - k <= t <= tw - k]
    before = after_ = never = 0
    late = []
    for t, c in calls:
        if (L, c) not in raft_in:
            never += 1
        elif raft_in[(L, c)][1] <= tw:
            before += 1
        else:
            after_ += 1
            late.append(raft_in[(L, c)][1] - tw)
    fat, _ = entered[f]
    n = bisect.bisect_right(fat, tw) - bisect.bisect_left(fat, la)
    print(f"  {f} {L} {hm(tw)} | {tw - la} | {before} / {after_} (+{pct(late, .5)}) / {never} | {n} ({n / max(1, (tw - la) / 1000):.0f}/s)")
