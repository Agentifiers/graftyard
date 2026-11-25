import json, sys
d = sys.argv[1]
passes = []
for l in open(f"{d}/workerd.log"):
    if '"lifeboat_pass"' in l:
        try:
            passes.append(json.loads(l))
        except Exception:
            pass
st = [l.split() for l in open(f"{d}/status.log") if len(l.split()) == 4]
cur = [(int(t), int(c)) for t, c, _, _ in st]
changes = []
for (t0, c0), (t1, c1) in zip(cur, cur[1:]):
    if c1 != c0:
        changes.append((t1, c1))
stub = [json.loads(l) for l in open(f"{d}/stub.log")]
served = [s["served"] for s in stub]
t = [s["t"] for s in stub]
probes = sum(p["lifeboat_pass"]["probes"] for p in passes)
span = (t[-1] - t[0]) / 1000
print(f"passes {len(passes)} probes {probes} served {served[-1]} over {span:.0f} s; peak in flight {max(s['peak'] for s in stub)}")
print("current_at changes (wall t, new current_at, cycle s since previous):")
prev = None
for tw, c in changes:
    print(f"  {tw} {c} {'' if prev is None else round((c - prev) / 1000, 1)}")
    prev = c
# pass wall time: probes per pass / served rate while busy, from stub served deltas per second while >0
busy = [b - a for a, b in zip(served, served[1:]) if b - a > 0]
idle = sum(1 for a, b in zip(served, served[1:]) if b == a)
print(f"serving seconds {len(busy)}, idle seconds {idle}, mean served per busy second {sum(busy)/max(1,len(busy)):.1f}")
print(f"pass wall time ~ {400 / (sum(busy)/max(1,len(busy))):.1f} s for 400 probes")
print(f"lag_s first {passes[0]['lag_s']} last {passes[-1]['lag_s']}")
