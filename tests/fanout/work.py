import json, lzma, sys
# round-time's turn trace (`Trace::Work`, kind 30, `work_row` in workers/site/src/host.rs): what each leader input step
# produced, and how it lines up with the ticker stalls (`Trace::Lag`, kind 8) that F100 blames on leader CPU per input.
# Usage: work.py TRACE.jsonl[.xz] LOAD_START_MS [SECONDS] [STALL_MS]
# A stall of `lag` ms logged at `at` covers the turns since `at - lag`; the isolate's clock does not move inside a turn,
# so a step inside the stall carries a time in that span. Steps are joined to stalls by time, not by turn.
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3] if len(sys.argv) > 3 else 600)
STALL = int(sys.argv[4]) if len(sys.argv) > 4 else 200
BIG = 1_400_000  # a large cut, the size F100 names
seen, work, lags = set(), [], []
for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d:
        continue
    for r in d['rows']:
        t = (d.get('site'),) + tuple(r)
        if r[1] not in (8, 30) or t in seen or not t0 <= r[0] < t1:
            continue
        seen.add(t)
        site, at, peer, batch, index, ms = d.get('site'), r[0], r[2], int(float(r[3])), int(float(r[4])), int(float(r[5]))
        if r[1] == 8:
            lags.append((site, at, ms))
        else:
            work.append({'site': site, 'at': at, 'appends': peer, 'append_bytes': batch,
                         'stmts': index & 0xfffff, 'stmt_bytes': index >> 20,
                         'applied': ms & 0xffff, 'events': (ms >> 16) & 0xffff, 'resolutions': ms >> 32})
if not work:
    sys.exit('no kind 30 rows in the window')
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else 0
# One message's entries: the step's entry bytes over its AppendEntries (each message carries the same new entries).
per_msg = lambda w: w['append_bytes'] // w['appends'] if w['appends'] else 0
for site in sorted({w['site'] for w in work}, key=str):
    ws = sorted((w for w in work if w['site'] == site), key=lambda w: w['at'])
    print(f'site {site}: {len(ws)} leader steps, {ws[0]["at"]} to {ws[-1]["at"]}')
    for k in ('appends', 'append_bytes', 'stmts', 'stmt_bytes', 'applied', 'events', 'resolutions'):
        xs = [w[k] for w in ws]
        print(f'  {k}: p50 {pc(xs, .5)} p99 {pc(xs, .99)} max {max(xs)} sum {sum(xs)}')
    lo, hi = ws[0]['at'], ws[-1]['at']
    ls = sorted((at, lag) for s, at, lag in lags if s == site and lag >= STALL and lo <= at <= hi + 2000)
    inside, n_big, n_bigsum = set(), 0, 0
    for at, lag in ls:
        ix = [i for i, w in enumerate(ws) if at - lag <= w['at'] <= at]
        inside.update(ix)
        n_big += any(per_msg(ws[i]) >= BIG for i in ix)
        n_bigsum += any(ws[i]['append_bytes'] >= BIG for i in ix)
    span_in = sum(lag for _, lag in ls)
    span = max(1, hi - lo)
    byte = lambda w: w['append_bytes'] + w['stmt_bytes']
    win = [byte(ws[i]) for i in sorted(inside)]
    wout = [byte(w) for i, w in enumerate(ws) if i not in inside]
    print(f'  ticker stalls >= {STALL} ms while leading: {len(ls)}, {span_in} ms of {span} ms')
    print(f'  steps inside stalls {len(win)}, bytes {sum(win)} ({sum(win) / max(1, span_in):.0f} B/ms), '
          f'per step p50 {pc(win, .5)} p99 {pc(win, .99)}')
    print(f'  steps outside {len(wout)}, bytes {sum(wout)} ({sum(wout) / max(1, span - span_in):.0f} B/ms), '
          f'per step p50 {pc(wout, .5)} p99 {pc(wout, .99)}')
    print(f'  stalls with a step sending a message of >= {BIG} B of entries: {n_big} of {len(ls)}; '
          f'with a step sending >= {BIG} B over all its messages: {n_bigsum} of {len(ls)}')
