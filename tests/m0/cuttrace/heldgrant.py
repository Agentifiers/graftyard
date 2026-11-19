"""Site 0's leader plain reads (SentBy Leader) by query code, classed by the trace's order of rows (one isolate, so
log order is execution order):
  held:    the read's ReadQuery came after a deferring input (a Stepped row whose run of AppendHeld/AppendQueued rows
           has an AppendQueued, `Host::step`'s early sends before `defer_flush`) and before the next Flushed, so its
           grant is released only at that flush (`Host::dispatch`);
  stepped: before that Flushed, another member input came after the read: a /raft body (CallArrived), an answer to
           the member's own call (AnswerArrived), a write input (Stepped), or another read (ReadQuery).
Counts, per class, reads whose isolate posted a Walk within 2 s. Query codes are `READ_QUERIES` + 1 (`workers/site/src/host.rs`): 3 rows (a merge's rows), 4 prsync, 13 ranges.
Usage: heldgrant.py TSAVE"""
import collections, json, lzma, sys
P = sys.argv[1]
seen = set(); s0 = []; walks = collections.defaultdict(list); ent = collections.defaultdict(dict); by = collections.defaultdict(dict)
with (lzma.open if P.endswith(".xz") else open)(P, "rt") as f:
    for l in f:
        d = json.loads(l)
        if "rows" not in d or d.get("learner") is not None: continue
        w = d.get("site")
        for r in d["rows"]:
            t = (w,) + tuple(r)
            if t in seen: continue
            seen.add(t)
            if w == 0: s0.append(r)
            if r[1] == 25: ent[w][r[4] // 64] = (r[0], r[4] % 64, r[2])
            elif r[1] == 27: by[w][r[3]] = (r[4], r[5])
for w in ent:
    for n, (t, route, peer) in ent[w].items():
        sb = by[w].get(n)
        if sb and sb[1] == 2: walks[sb[0]].append(t)
held = False; run = False
c = collections.Counter()
for i, r in enumerate(s0):
    k = r[1]
    if k == 15: run = r[0]; continue
    if run and k == 12 and r[0] == run: held = True
    if run and k not in (12, 13, 18, 23): run = False
    if k == 16: held = False
    if k != 26: continue
    sb = by[0].get(r[3]); e = ent[0].get(r[3])
    if not sb or sb[1] != 1 or not e: continue
    stepped = False
    if held:
        for x in s0[i + 1:i + 5000]:
            if x[1] == 16: break
            if x[1] in (15, 20, 21, 26): stepped = True; break
    walked = any(e[0] < x <= e[0] + 2000 for x in walks[sb[0]])
    c[(r[4], held, stepped, walked)] += 1
for code in sorted({k[0] for k in c}):
    row = []
    for p, s in ((True, True), (True, False), (False, False)):
        row.append(f"held={p!s:5} stepped={s!s:5} walked {c[(code, p, s, True)]}/{c[(code, p, s, True)] + c[(code, p, s, False)]}")
    print(f"q{code}: " + "; ".join(row))
