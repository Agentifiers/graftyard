import json, lzma, sys, collections
# cut-trace's per-cut rows (`Trace::Cut`, kind 29, `cut_row` in workers/site/src/host.rs): what ended each cut on the
# leader, and what it took and left, from a tsave trace. Usage: cuts.py TRACE.jsonl.xz LOAD_START_MS [SECONDS]
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3] if len(sys.argv) > 3 else 600)
ENDS = ['Empty', 'Soft', 'EntryMax', 'AgentTokens', 'AgentCmds']
CLASSES = ['human', 'mergeq', 'bot', 'agent']
seen = set()
raw = []
for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d:
        continue
    for r in d['rows']:
        t = (d.get('site'),) + tuple(r)
        if r[1] != 29 or t in seen or not t0 <= r[0] < t1:
            continue
        seen.add(t)
        # Through a JS number: read the double the site wrote.
        raw.append((d.get('site'), r[0], r[2], int(float(r[3])), int(float(r[4])), int(float(r[5]))))
# Traces from before the tokens moved (cut-trace, B0-29 run 5) carry them above bit 48 of `batch`, past 2^53, so its
# low bits (the Human count) are rounded, marked `human~`; a trace is old if any of its rows has bits there. Later rows
# carry the tokens in `ms` above bit 42 and 13 bits of `left` per class, and every column is under 2^53 and exact (`cut_row`, workers/site/src/host.rs).
old = any(batch >= 1 << 48 for _, _, _, batch, _, _ in raw)
cuts = []
for site, at, peer, batch, index, ms in raw:
    cuts.append({
        'site': site, 'at': at, 'end': ENDS[peer % 16] if peer % 16 < len(ENDS) else peer % 16,
        'held': (peer // 16) % 4, 'in_flight': peer >= 64,
        'taken': [(batch >> (12 * i)) & 0xfff for i in range(4)],
        'tokens': batch >> 48 if old else ms >> 42,
        'left': [(index >> (16 * i)) & 0xffff for i in range(4)] if old else [(index >> (13 * i)) & 0x1fff for i in range(4)],
        'soft': ms & 0xffffff if old else ms & 0x1fffff, 'bytes': ms >> 24 if old else (ms >> 21) & 0x1fffff,
    })
if old:
    CLASSES[0] = 'human~'
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
