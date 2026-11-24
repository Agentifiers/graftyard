import bisect, collections, json, lzma, os, sys
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..')))
from tracerows import merge
# Raw traces: tests/fanout/raw under the current directory (run from the repository root), or $B033_RAW.
RAW = os.environ.get('B033_RAW', 'tests/fanout/raw')
def raw_path(tag): return os.path.join(RAW, f'tsave-{tag}.jsonl.xz')
from datetime import datetime
RUNS = {  # tag: (load start ISO, leader site, seconds of site-0 leadership to use)
    'sabA': ('2026-11-21T11:37:06Z', 300), 'blA': ('2026-11-22T02:44:24Z', 300), 'blB': ('2026-11-22T03:25:41Z', 300),
    'blC2': ('2026-11-22T04:15:03Z', 300), 'ncA': ('2026-11-22T01:32:10Z', 216), 'chA': ('2026-11-21T22:07:57Z', 300),
    'crA': ('2026-11-21T23:49:06Z', 300), 'm64r33': ('2026-11-21T00:11:46Z', 144),
    # B0-32 m3fhs1 on graft-m3f, for heavyturns.py: 90 s from this start is the burst's first second at the leader.
    'm3fhs1-b0-32': ('2026-11-21T03:54:39Z', 96),
    # B0-32 m3fsf1 (stage-first measurement): 90 s from this start is 09:03:51Z, the burst's first big cut.
    'm3fsf1-b0-32': ('2026-11-23T09:02:21Z', 96),
    # B0-32 m3fsf2 (stage-first rerun after cw-link-drop): 90 s from this start is 09:44:11Z, the burst's first second.
    'm3fsf2-b0-32': ('2026-11-23T09:42:41Z', 96),
    # B0-32 m3fg3 (measurement on a 55.8 ms IAD leader): 90 s from this start is 12:16:58Z, the burst's first big second.
    'm3fg3-b0-32': ('2026-11-23T12:15:28Z', 96),
    # B0-32 m3fk2B (gate run, a 33.9 ms MIA leader kept at the 40 line): 90 s from this start is 19:19:49Z, the burst's
    # first big apply.
    'm3fk2B-b0-32': ('2026-11-23T19:18:19Z', 96),
    # B0-32 m3fmA (gate run on main 5bed46af, a 28.5 ms MIA leader, 5 learners): 90 s from this start is 23:18:11Z,
    # the burst's first big apply.
    'm3fmA-b0-32': ('2026-11-23T23:16:41Z', 96),
    'qlA': ('2026-11-22T16:24:36Z', 300), 'qlB': ('2026-11-22T17:09:28Z', 300), 'qlC': ('2026-11-22T17:52:14Z', 300),
    'pfA': ('2026-11-23T01:04:05Z', 296),
    'pgA': ('2026-11-23T02:20:20Z', 229),
    # B0-33 row B (edge refusal), for tests/fanout/advertcf.py.
    'erBA': ('2026-11-23T11:20:56Z', 300),
    # read-memo's guarded row (b033-readmemo.md): leader site 0 held term 2 through the 300 s window.
    'mrB': ('2026-11-23T13:15:50Z', 300),
    # read-memo's median run msA: site 0 led term 2 through the window; its wasm instance reset at 14:23:22Z.
    'msA': ('2026-11-23T14:20:22Z', 300)}
COLO = {'sabA': 'EWR', 'blB': 'EWR', 'blA': 'IAD', 'blC2': 'IAD', 'chA': 'IAD', 'm64r33': 'IAD', 'crA': 'ORD', 'ncA': 'ATL', 'qlA': 'MIA', 'qlB': 'MIA', 'qlC': 'EWR', 'pfA': 'ATL', 'pgA': 'ORD', 'mrB': 'ORD', 'msA': 'EWR'}
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
def load(tag, sites=(0,), pad=0):
    a = ms(RUNS[tag][0]); b = a + 1000 * RUNS[tag][1]
    parsed = (json.loads(line) for line in lzma.open(raw_path(tag), 'rt'))
    rows = merge((d['site'], [r for r in d['rows'] if a - pad <= r[0] < b + pad])
                 for d in parsed if 'rows' in d and 'learner' not in d and d.get('site') in sites)
    for s in rows: rows[s].sort(key=lambda r: r[0])
    return a, b, rows
def I(x): return int(float(x))
