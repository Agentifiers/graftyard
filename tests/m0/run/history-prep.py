# history-prep.py in.jsonl out.jsonl widen_ms (B0-11, `results/m0-wp2.md`)
# Adds a write for each read value that has no recorded write (the writer client's row was lost when the orb
# interrupted it mid-call), called after that client's previous recorded op returned and returning when the first
# read of it returned; then widens every interval by widen_ms on each side.
import json, sys, bisect
src, dst, W = sys.argv[1], sys.argv[2], int(sys.argv[3])
rows = [json.loads(l) for l in open(src)]
vals = {r['op']['value'] for r in rows if r['op']['kind'] != 'get'}
by_client = {}
for r in rows: by_client.setdefault(r['client'], []).append(r['ret'])
for v in by_client.values(): v.sort()
first = {}
for r in rows:
    if r['op']['kind'] == 'get' and r['ok']:
        v = (r.get('out') or {}).get('value')
        if v is not None and v not in vals and (v not in first or r['ret'] < first[v]['ret']): first[v] = r
added = 0
for v, rd in first.items():
    cid = int(v.split('.')[0]); rets = by_client.get(cid, [])
    i = bisect.bisect_left(rets, rd['call']) - 1
    call = rets[i] if i >= 0 else rd['call'] - 600000
    rows.append({'client': cid, 'call': call, 'ret': rd['ret'], 'op': {'kind': 'put', 'key': rd['op']['key'], 'value': v}, 'ok': True, 'out': {'ok': True}})
    added += 1
with open(dst, 'w') as f:
    for r in rows:
        r['call'] -= W; r['ret'] += W
        f.write(json.dumps(r) + '\n')
print(src, 'synthesized writes', added, 'widen', W, file=sys.stderr)
