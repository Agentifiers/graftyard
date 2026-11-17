#!/usr/bin/env python3
# Local check for tests/f74/inbound.py on a hand-made tsave file whose answers are known:
# - voter 1's rows are complete; voter 2's skip arrival 4, and voter 2 resets (arrivals restart at 1);
# - voter 0's /raft call (id 7) to voter 1 is sent at 1000 and enters at 1500 as arrival 6, and arrivals 3 to 5
#   (two edge /reads and an edge /suspect) entered in between, after arrivals 1 and 2 before the send;
# - the reply leaves at 1510 and arrives at 2010, so the pair's skew estimate is 0 and the out leg 500 ms.
#   python3 tests/f74/inbound_test.py
import json, os, subprocess, sys, tempfile

E = 25
row = lambda at, frm, call, n, route, open_: [at, E, frm, call, n * 64 + route, open_]
v1 = [row(900, 255, 0, 1, 2, 1), row(950, 255, 0, 2, 2, 2), row(1100, 255, 0, 3, 2, 2), row(1200, 255, 0, 4, 7, 3),
      row(1300, 255, 0, 5, 2, 4), row(1500, 0, 7, 6, 1, 5), [1510, 24, 0, 7, 1500, 0]]
v2 = [row(10, 255, 0, 1, 2, 1), row(20, 255, 0, 2, 2, 1), row(30, 255, 0, 3, 2, 1), row(50, 255, 0, 5, 2, 1),
      row(60, 255, 0, 1, 3, 1), row(70, 255, 0, 2, 3, 1)]
v0 = [[1000, 23, 1, 7, 1, 0], [2010, 21, 1, 7, 1510, 0]]
# Voter 1's /reads at arrivals 3 and 5 name their queries: a leader probe and a read-index `rows` read.
v1 += [[1101, 26, 0, 3, 1, 0], [1301, 26, 0, 5, 3, 1]]
# probe-known: arrivals 3 and 5 came from two isolates, 3 a walk (step 2) from isolate 9 and 5 a spread read (step 3)
# from isolate 4; arrival 4 (a /suspect) from isolate 9 as well, step other.
v1 += [[1100, 27, 255, 3, 9, 2], [1200, 27, 255, 4, 9, 0], [1300, 27, 255, 5, 4, 3]]
with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
    for site, rs in ((0, v0), (1, v1), (2, v2)):
        f.write(json.dumps({"at": "x", "site": site, "rows": rs}) + "\n")
here = os.path.dirname(os.path.abspath(__file__))
tsv = f.name + ".tsv"
out = subprocess.run([sys.executable, os.path.join(here, "inbound.py"), f.name, "--rows", tsv], capture_output=True,
                     text=True, check=True).stdout
assert "  1: rows 6 segments 1 missing 0 backwards 0" in out, out
assert "probe:lead" in out and "rows:idx" in out and "untagged" in out, out
assert "  2: rows 6 segments 2 missing 1 backwards 0" in out, out
assert "  1: walk " in out and "spread " in out and "isolates 2, top 67%, walks from 1 isolates (top 100%)" in out, out
cols, call = [l.rstrip("\n").split("\t") for l in open(tsv)]
c = dict(zip(cols, call))
assert (c["out"], c["arrival"], c["ahead"], c["ahead_edge"]) == ("500", "6", "3", "3"), c
assert (c["/read"], c["/suspect"], c["open"]) == ("2", "1", "5"), c
os.remove(f.name)
os.remove(tsv)
print("inbound_test: ok")
