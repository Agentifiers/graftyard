# resets.py TAG [FROM_ISO TO_ISO]: wasm resets per member from memguard-TAG.jsonl (a drop of 20 MiB or more between samples).
import json, sys
tag = sys.argv[1]
lo, hi = (sys.argv[2], sys.argv[3]) if len(sys.argv) > 3 else ('', 'Z')
prev = {}
n = 0
for l in open(f'tests/fanout/raw/memguard-{tag}.jsonl'):
    try:
        d = json.loads(l)
    except Exception:
        continue
    for m in d.get('members', []):
        s, mib = m['site'], m.get('mib')
        if mib is None:
            continue
        p = prev.get(s)
        if p and p[1] - mib >= 20:
            inw = lo <= d['at'] <= hi
            n += inw
            print(('LOAD ' if inw else 'out  ') + f"site {s} role {m.get('role')} at {d['at']}: {p[1]} -> {mib} MiB (last high sample {p[0]})")
        prev[s] = (d['at'], mib)
print('resets in window', n)
