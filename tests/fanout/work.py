import json, lzma, sys
# round-time's turn trace (`Trace::Work`, kind 30, `work_row` in workers/site/src/host.rs): what each leader input step
# produced, and how it lines up with the ticker stalls (`Trace::Lag`, kind 8) that F98 blames on leader CPU per input.
# Usage: work.py TRACE.jsonl[.xz] LOAD_START_MS [SECONDS]
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3] if len(sys.argv) > 3 else 600)
BIG = 1_400_000  # a large cut, the size F98 names
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
for site in sorted({w['site'] for w in work}, key=str):
    ws = [w for w in work if w['site'] == site]
    print(f'site {site}: {len(ws)} leader steps')
    for k in ('appends', 'append_bytes', 'stmts', 'stmt_bytes', 'applied', 'events', 'resolutions'):
        xs = [w[k] for w in ws]
        print(f'  {k}: p50 {pc(xs, .5)} p99 {pc(xs, .99)} max {max(xs)} sum {sum(xs)}')
    big = [w for w in ws if w['append_bytes'] >= BIG]
    print(f'  steps sending >= {BIG} bytes of entries: {len(big)}')
    # A stall of `lag` ms logged at `at` covers the turns since `at - lag`: the work those turns did is what ran
    # instead of the ticker. Steps share the isolate clock inside a turn, so this joins by time, not by turn.
    ls = sorted((at, lag) for s, at, lag in lags if s == site and lag >= 100)
    stalled = [sum(w['append_bytes'] + w['stmt_bytes'] for w in ws if at - lag <= w['at'] <= at) for at, lag in ls]
    print(f'  ticker stalls >= 100 ms: {len(ls)}, {sum(l for _, l in ls)} ms; bytes of work inside each: '
          f'p50 {pc(stalled, .5)} p99 {pc(stalled, .99)}')
