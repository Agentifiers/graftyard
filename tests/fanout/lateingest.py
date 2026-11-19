#!/usr/bin/env python3
"""lateingest.py SHARDEV.jsonl[.xz] SHARDPROCS.jsonl MISSLINES.jsonl T0_MS NET [OVER_MS 10000] -> each stored-pack
catch-up of network NET whose ingest started late (`stored.total_ms - index_ms` over OVER_MS), and what held the
network's lock when it queued. That gap is `/ingest`'s wait before `indexIngest`, which is `handleIngest` taking the
lock alone to `adopt` (containers/git/server/main.go), so it waits for every reader already in: validates (shared for
their whole call) and stock upload-packs (shared from the catch-up before stock until the client goes, `release`).

For each, from the same clock (Workers Logs):
  - the validates and upload-packs of NET on that object whose requests span the wait's start;
  - the stock miss lines (`misses.py`'s RAW) whose answer had started (`caught_up`) before the wait began and whose
    first byte (`first_ms`) came after it: a stock reader in the lock computing its pack;
  - the stock processes in `shardprocs` samples inside the wait, after moving the sampler's orb clock onto Workers
    Logs' by the median gap between each sample and the shard's `/status` event it caused.
B0-34 mt3's flip (`decisions/DECISIONS.md`, "`b034-tails`", option 4) fires on a wait over 1 s for the exclusive
lock behind a stock reader."""
import bisect, datetime, json, lzma, statistics, sys

ev_path, procs_path, lines_path, t0, net = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5]
over = int(sys.argv[6]) if len(sys.argv) > 6 else 10_000
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
rel = lambda t: round((t - t0) / 1000, 1)

samples = {0: [], 1: [], 2: []}
for line in open(procs_path):
    try:
        d = json.loads(line)
    except ValueError:
        continue
    if d.get('status') == 200:
        samples[d['site']].append((ms(d['at']), d.get('shallow', 0)))
opener = lzma.open if ev_path.endswith('.xz') else open
events = [json.loads(line) for line in opener(ev_path, 'rt')]

gaps = []
for e in events:
    m = (e.get('source') or {}).get('message')
    if isinstance(m, str) and m.startswith('GET https://shard/status'):
        ts = [t for t, _ in samples['abc'.index(e['_site'])]]
        i = bisect.bisect_right(ts, e['timestamp']) - 1
        if i >= 0 and 0 <= e['timestamp'] - ts[i] < 15_000:
            gaps.append(e['timestamp'] - ts[i])
offset = statistics.median(gaps)
print(json.dumps({'sampler_offset_ms': offset, 'status_events': len(gaps)}))

reqs = []
for e in events:
    ev = e['$workers'].get('event')
    wall = e['$workers'].get('wallTimeMs')
    if isinstance(ev, dict) and ev.get('path') in ('/validate', '/upload-pack') and wall is not None \
            and str((ev.get('search') or {}).get('repo')) == net:
        reqs.append((e['_site'], ev['path'], e['timestamp'], e['timestamp'] + wall))
stock = [l for l in map(json.loads, open(lines_path)) if str(l.get('answer', '')).startswith('stock')
         and l.get('caught_up_ms') is not None and l.get('first_ms') is not None]

for e in events:
    m = (e.get('source') or {}).get('message')
    if not (isinstance(m, str) and m.startswith(f'shard catch-up stored {net}:')):
        continue
    st = json.loads(m.split(': ', 1)[1])['stored']
    wait = st.get('total_ms', 0) - st.get('index_ms', 0)
    if e['timestamp'] < t0 or not st.get('packs') or wait <= over:
        continue
    site = e['_site']
    a, b = e['timestamp'] - st['total_ms'], e['timestamp'] - st['index_ms']
    held = [r for r in reqs if r[0] == site and r[2] < a < r[3]]
    computing = [l for l in stock if l['site'] == site and l['arrived'] + l['caught_up_ms'] < a < l['arrived'] + l['first_ms']]
    procs = [n for t, n in samples['abc'.index(site)] if a <= t + offset <= b]
    print(json.dumps({
        'site': site, 'wait_s': [rel(a), rel(b)], 'wait_ms': wait, 'packs': st['packs'], 'lock_ms': st['lock_ms'],
        'midx_ms': st['midx_ms'],
        'validates_in': sum(1 for r in held if r[1] == '/validate'),
        'last_validate_ends_s': rel(max((r[3] for r in held if r[1] == '/validate'), default=a)),
        'upload_packs_in': sum(1 for r in held if r[1] == '/upload-pack'),
        'stock_computing': [{'caught_up_s': rel(l['arrived'] + l['caught_up_ms']), 'first_byte_s': rel(l['arrived'] + l['first_ms']),
                             'answer': l['answer']} for l in computing],
        'stock_procs_in_samples': procs}))
