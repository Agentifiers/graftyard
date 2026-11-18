import json, lzma, sys, collections
# cut-trace's per-cut rows (`Trace::Cut`, kind 29, `cut_row` in workers/site/src/host.rs): what ended each cut on the
# leader, and what it took and left, from a tsave trace. Usage: cuts.py TRACE.jsonl.xz LOAD_START_MS [SECONDS]
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3] if len(sys.argv) > 3 else 600)
ENDS = ['Empty', 'Soft', 'EntryMax', 'AgentTokens']
CLASSES = ['human~', 'mergeq', 'bot', 'agent']
seen = set()
cuts = []
for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d:
        continue
    for r in d['rows']:
        t = (d.get('site'),) + tuple(r)
        if r[1] != 29 or t in seen or not t0 <= r[0] < t1:
            continue
        seen.add(t)
        at, _, peer, batch, index, ms = r
        # The trace reaches here through a JS number: `batch` (tokens above bit 48) is past 2^53, so take the double
        # the site wrote, not the decimal's exact integer. Its low bits are then rounded to the double's step (8 to 32
        # at these sizes), which only the Human count (bits 0-11) falls inside; `human~` marks it approximate.
        batch, index, ms = int(float(batch)), int(float(index)), int(float(ms))
        cuts.append({
            'site': d.get('site'), 'at': at, 'end': ENDS[peer % 16] if peer % 16 < len(ENDS) else peer % 16,
            'held': (peer // 16) % 4, 'in_flight': peer >= 64,
            'taken': [(batch >> (12 * i)) & 0xfff for i in range(4)], 'tokens': batch >> 48,
            'left': [(index >> (16 * i)) & 0xffff for i in range(4)],
            'soft': ms & 0xffffff, 'bytes': ms >> 24,
        })
if not cuts:
    sys.exit('no kind 29 rows in the window')
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]
print(f'{len(cuts)} cuts; ends: {dict(collections.Counter(c["end"] for c in cuts))}')
print(f'early cuts (an entry in flight): {sum(c["in_flight"] for c in cuts)}; held before: '
      f'{dict(collections.Counter(c["held"] for c in cuts))}')
for k in ('soft', 'bytes', 'tokens'):
    xs = [c[k] for c in cuts]
    print(f'{k}: p50 {pc(xs, .5)} p90 {pc(xs, .9)} p99 {pc(xs, .99)} max {max(xs)}')
for i, name in enumerate(CLASSES):
    taken, left = [c['taken'][i] for c in cuts], [c['left'][i] for c in cuts]
    print(f'{name}: taken {sum(taken)} (p99 {pc(taken, .99)} a cut), left after a cut p50 {pc(left, .5)} '
          f'p99 {pc(left, .99)} max {max(left)}')
per = collections.defaultdict(list)
for c in cuts:
    per[c['end']].append(sum(c['taken']))
for e, xs in per.items():
    print(f'{e}: {len(xs)} cuts, commands a cut p50 {pc(xs, .5)} p99 {pc(xs, .99)}')
