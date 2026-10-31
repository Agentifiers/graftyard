#!/usr/bin/env python3
"""M0-WP16 L0-05: `GitCacheShard::predict(head, base, against)` replayed on real linux topic branches, with ground truth
from real git.

  python3 wp16/predict.py --repo /home/user/wp16/alt.git --events /home/user/wp16/events.json \
      --pairs 1000 --workers 6 --out /home/user/wp16/predict.json

The replay walks `wp16/replay.py`'s pushes in time order and keeps the fold: each live agent ref's PathSet row (head,
merge base, paths, truncated). A ref leaves the fold when master merges it. One push is one InsightJob, with
base = master's tip at the push.

Verdicts, all from real `git merge-tree --write-tree --name-only`:
  landed(x) = merge-tree(base, x): x's tree on the current base, or a conflict of x with the base itself.
  pair(head, other) = merge-tree --merge-base=base landed(head) landed(other): do the two branches' own changes conflict
    once both are on the current base. Superseded as ground truth by wp16/truth.py's queue order: this method counts a
    commit both branches carry (stacked topic branches) as a change on both sides, so 124 of its 201 conflicts merge
    clean (tests/m0/results/m0-wp16.md).
  raw(head, other) = merge-tree head other: the two tips as they are, which also counts the base's drift between their
    merge bases as one side's change. Reported for comparison.
A head that conflicts with the base gets `Stale` for every pair (the test merge reports that conflict). An other ref that
conflicts with the base gives that pair `Stale`.

The predictor's verdict for every overlapping pair is computed, so priority orders and PREDICT_PAIRS values are scored
offline on the same ground truth: a pair past the cap reports Stale. Ground truth: 1,000 sampled live pairs, half drawn
from overlapping pairs and half from all live pairs; for non-overlapping sampled pairs the truth is computed directly.
"""
import argparse, json, random, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache

ap = argparse.ArgumentParser()
ap.add_argument("--repo", required=True)
ap.add_argument("--events", required=True)
ap.add_argument("--pairs", type=int, default=1000)
ap.add_argument("--workers", type=int, default=6)
ap.add_argument("--seed", type=int, default=16)
ap.add_argument("--out", required=True)
a = ap.parse_args()


def mt(*args):
    p = subprocess.run(["git", "-C", a.repo, "merge-tree", "--write-tree", "--name-only", "--no-messages", *args], capture_output=True, text=True)
    if p.returncode not in (0, 1):
        raise RuntimeError(f"merge-tree {args}: {p.returncode} {p.stderr[:200]}")
    lines = [l for l in p.stdout.split("\n") if l]
    return p.returncode == 1, lines[0], lines[1:9]


@lru_cache(maxsize=None)
def landed(base, x):
    c, tree, files = mt(base, x)
    return None if c else tree


def pair(base, head, other):
    ta, to = landed(base, head), landed(base, other)
    if ta is None:
        return "HeadStale", []
    if to is None:
        return "OtherStale", []
    c, _, files = mt(f"--merge-base={base}", ta, to)
    return ("Conflict" if c else "Clean"), files


def raw(head, other):
    c, _, files = mt(head, other)
    return ("Conflict" if c else "Clean"), files


def overlap(p, q):
    """PathGlob overlap of two PathSet rows whose entries are literal paths, or directories when truncated."""
    if not p["truncated"] and not q["truncated"]:
        return len(p["set"] & q["set"])
    n = 0
    for x in p["paths"]:
        for y in q["paths"]:
            if x == y or (p["truncated"] and y.startswith(x + "/")) or (q["truncated"] and x.startswith(y + "/")):
                n += 1
    return n


d = json.load(open(a.events))
events = d["events"]
merged = {e["ref"]: e["merged"] for e in events}
rows = {}
for e in events:
    e["set"] = set(e["paths"])
jobs = []
for e in events:
    for ref in [r for r in rows if merged.get(r, 0) <= e["t"]]:
        rows.pop(ref)
    live = [(r, row) for r, row in rows.items() if r != e["ref"]]
    over = [(r, row["head"], row["t"], overlap(e, row)) for r, row in live]
    jobs.append({"i": e["i"], "t": e["t"], "ref": e["ref"], "head": e["head"], "base": e["base"], "live": len(live),
                 "over": [(r, h, t, n) for r, h, t, n in over if n], "non": [(r, h) for r, h, t, n in over if not n]})
    rows[e["ref"]] = e

work = [(j["i"], j["base"], j["head"], r, h) for j in jobs for (r, h, t, n) in j["over"]]
print(f"{len(jobs)} jobs, {len(work)} overlapping pairs", file=sys.stderr)
t0 = time.time()
with ThreadPoolExecutor(a.workers) as ex:
    res = list(ex.map(lambda w: pair(w[1], w[2], w[4]), work))
verd = {(w[0], w[3]): r for w, r in zip(work, res)}
print(f"pair verdicts in {time.time() - t0:.0f} s", file=sys.stderr)

random.seed(a.seed)
over_pairs = [(j, r, h) for j in jobs for (r, h, t, n) in j["over"]]
all_pairs = over_pairs + [(j, r, h) for j in jobs for (r, h) in j["non"]]
sample = random.sample(over_pairs, min(a.pairs // 2, len(over_pairs)))
seen = {(j["i"], r) for j, r, _ in sample}
sample += random.sample([p for p in all_pairs if (p[0]["i"], p[1]) not in seen], a.pairs - len(sample))


def truth(p):
    j, r, h = p
    lp = verd.get((j["i"], r)) or pair(j["base"], j["head"], h)
    return lp, raw(j["head"], h)


with ThreadPoolExecutor(a.workers) as ex:
    gt = list(ex.map(truth, sample))
pairs = []
for (j, r, h), (lp, rw) in zip(sample, gt):
    o = next((x for x in j["over"] if x[0] == r), None)
    pairs.append({"i": j["i"], "t": j["t"], "ref": j["ref"], "other": r, "overlapFiles": o[3] if o else 0,
                  "landed": lp[0], "raw": rw[0], "files": lp[1] or rw[1]})
json.dump({"pairs": pairs,
           "jobs": [{"i": j["i"], "t": j["t"], "ref": j["ref"], "live": j["live"],
                     "over": [[r, t, n, verd[(j["i"], r)][0]] for (r, h, t, n) in j["over"]]} for j in jobs]}, open(a.out, "w"))
print("done", file=sys.stderr)
