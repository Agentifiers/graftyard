import bisect, json, lzma, sys
# entrycycle.py TRACE.jsonl[.xz] LOAD_START_MS SECONDS [SITE]: what one entry in flight waits on, per entry, from the
# leader's trace. For each entry the leader appended with entries (`AppendQueued`, kind 12; `index` its last entry),
# it pairs the cut before it (`Cut`, kind 29: commands taken, all bytes), the voters' ends of that round (`AppendEnded`,
# kind 14), the apply of its last entry (`Applied`, kind 4), and the next entry's append. Prints, by commands per entry,
# the round (append to the first voter's end: the quorum with the leader), the apply (quorum to `Applied`), the gap
# from that apply to the next entry's append (with one entry in flight, the next entry's dependency reads, `decide`
# and encode), and the append-to-append cycle; then how the summed cycle splits, and entries, commands and bytes a second.
# The isolate's clock moves only across I/O, so an apply's own CPU shows as 0 here and lands in the next gap.
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3])
SITE = int(sys.argv[4]) if len(sys.argv) > 4 else None
op = lzma.open if path.endswith('.xz') else open
seen, cuts, queued, ended, applied = set(), [], {}, {}, {}
for l in op(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or (SITE is not None and d.get('site') != SITE):
        continue
    for r in d['rows']:
        k = (d.get('site'),) + tuple(r)
        if k in seen or not t0 - 60_000 <= r[0] < t1 + 60_000:
            continue
        seen.add(k)
        at, kind, peer = r[0], r[1], r[2]
        b, i, ms = int(float(r[3])), int(float(r[4])), int(float(r[5]))
        if kind == 29:
            cuts.append((at, sum((b >> (12 * c)) & 0xfff for c in range(4)), (ms >> 21) & 0x1fffff))
        elif kind == 12 and peer < 192:
            # Each voter's first append carrying `i` as its last entry, and its round.
            queued.setdefault(i, (at, b))
        elif kind == 14 and peer < 192 and i == 0:
            ended.setdefault(b, []).append(at)
        elif kind == 4:
            applied.setdefault(i, at)
cuts.sort()
ct = [c[0] for c in cuts]
ap_idx = sorted(applied)
rows = []
for last in sorted(queued):
    at, rnd = queued[last]
    if not t0 <= at < t1:
        continue
    c = bisect.bisect_right(ct, at) - 1
    if c < 0:
        continue
    cut_at, cmds, nbytes = cuts[c]
    acked = min((e for e in ended.get(rnd, []) if e >= at), default=None)
    j = bisect.bisect_left(ap_idx, last)
    app = applied[ap_idx[j]] if j < len(ap_idx) else None
    nxt = cuts[c + 1][0] if c + 1 < len(cuts) else None
    rows.append(dict(cut=cut_at, cmds=cmds, bytes=nbytes, append=at, acked=acked, applied=app, next_cut=nxt, last=last))
# The next entry's append: one entry in flight, so it leaves only after this one applied.
for r, n in zip(rows, rows[1:]):
    r['next_append'] = n['append']


def pc(xs, q):
    xs = sorted(x for x in xs if x is not None)
    return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else None


def stage(r):
    return dict(
        cut_to_append=r['append'] - r['cut'],
        round=(r['acked'] - r['append']) if r['acked'] else None,
        apply=(r['applied'] - r['acked']) if r['applied'] and r['acked'] else None,
        apply_to_next_cut=(r['next_cut'] - r['applied']) if r['next_cut'] and r['applied'] else None,
        applied_to_next_append=(r['next_append'] - r['applied']) if r.get('next_append') and r['applied'] else None,
        cycle=(r['next_append'] - r['append']) if r.get('next_append') else None,
    )


print(f'{len(rows)} entries appended in the window, {len(cuts)} cuts')
bands = [(0, 4), (5, 9), (10, 19), (20, 39), (40, 64), (65, 9999)]
keys = ['round', 'apply', 'applied_to_next_append', 'cycle', 'cut_to_append']
print('commands | n | ' + ' | '.join(f'{k} p50/p90' for k in keys) + ' | bytes p50')
for lo, hi in bands + [(0, 9999)]:
    rs = [r for r in rows if lo <= r['cmds'] <= hi]
    if not rs:
        continue
    st = [stage(r) for r in rs]
    cells = ' | '.join(f'{pc([s[k] for s in st], .5)}/{pc([s[k] for s in st], .9)}' for k in keys)
    print(f'{lo}-{hi if hi < 9999 else "+"} | {len(rs)} | {cells} | {pc([r["bytes"] for r in rs], .5)}')
after = sum(1 for r in rows if r['applied'] and r['next_cut'] and r['next_cut'] >= r['applied'])
both = sum(1 for r in rows if r['applied'] and r['next_cut'])
print(f'next cut at or after this entry applied: {after} of {both}')
st = [stage(r) for r in rows if r.get('next_append') and r['acked'] and r['applied']]
tot = {k: sum(x[k] for x in st) for k in ('round', 'apply', 'applied_to_next_append', 'cycle')}
print(f'append-to-append cycle over {len(st)} entries: ' + ', '.join(f'{k} {100 * tot[k] / tot["cycle"]:.0f}%' for k in ('round', 'apply', 'applied_to_next_append')))
span = (rows[-1]['append'] - rows[0]['append']) / 1000 if len(rows) > 1 else 0
print(f'entries a second {len(rows) / span:.2f}, commands a second {sum(r["cmds"] for r in rows) / span:.1f}, bytes a second {sum(r["bytes"] for r in rows) / span / 1000:.0f} KB' if span else '')
