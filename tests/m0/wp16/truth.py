#!/usr/bin/env python3
"""M0-WP16 L0-05: queue ground truth, as a merge queue would land two branches on the current base.

  python3 wp16/truth.py --repo /home/user/wp16/alt.git --jobs /home/user/wp16/jobs.json --events /home/user/wp16/events.json \
      --sample /home/user/wp16/predict-r2.json --workers 6 --out /home/user/wp16/jobs-q.json

For a pair (head, other) on base:
  land(x)  = `git merge-tree --write-tree base x`; a conflict means x conflicts with the base itself (Stale).
  queue(a, b) = commit land(a) as a merge commit with parents (base, a), then `git merge-tree --write-tree` of that commit
           and b, where git finds the real merge base. This is what happens when the queue merges a and then b.
Truth is queue(head, other); queue(other, head) is computed too, and any disagreement is reported. Unlike
`--merge-base=base` on two landed trees (wp16/predict.py), a commit that both branches carry (stacked topic branches)
is not counted as a change on both sides.

Rewrites every overlapping pair's `truth` in jobs.json and adds `truthQ` to the 1,000 sampled pairs, with the old
landed truth kept as `landedTruth`.
"""
import argparse, json, os, subprocess
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache

ap = argparse.ArgumentParser()
for f in ("repo", "jobs", "events", "sample", "out"):
    ap.add_argument(f"--{f}", required=True)
ap.add_argument("--workers", type=int, default=6)
a = ap.parse_args()
ENV = {**os.environ, "GIT_AUTHOR_NAME": "q", "GIT_AUTHOR_EMAIL": "q@example.com", "GIT_COMMITTER_NAME": "q",
       "GIT_COMMITTER_EMAIL": "q@example.com", "GIT_AUTHOR_DATE": "1785542400 +0000", "GIT_COMMITTER_DATE": "1785542400 +0000"}


def git(*args, ok=(0,)):
    p = subprocess.run(["git", "-C", a.repo, *args], capture_output=True, text=True, env=ENV)
    if p.returncode not in ok:
        raise RuntimeError(f"{args}: {p.returncode} {p.stderr[:200]}")
    return p


@lru_cache(maxsize=None)
def landed_commit(base, x):
    m = git("merge-tree", "--write-tree", "--no-messages", base, x, ok=(0, 1))
    if m.returncode == 1:
        return None
    return git("commit-tree", m.stdout.split()[0], "-p", base, "-p", x, "-m", "land").stdout.strip()


def queue(base, first, second):
    c1 = landed_commit(base, first)
    if c1 is None:
        return "FirstStale"
    if landed_commit(base, second) is None:
        return "SecondStale"
    m = git("merge-tree", "--write-tree", "--name-only", "--no-messages", c1, second, ok=(0, 1))
    return "Conflict" if m.returncode == 1 else "Clean"


def truth(base, head, other):
    q1, q2 = queue(base, head, other), queue(base, other, head)
    v = {"FirstStale": "HeadStale", "SecondStale": "OtherStale"}.get(q1, q1)
    return v, q2


jobs = json.load(open(a.jobs))["jobs"]
work = [(j["base"], j["head"], o["head"]) for j in jobs for o in j["others"]]
with ThreadPoolExecutor(a.workers) as ex:
    res = list(ex.map(lambda w: truth(*w), work))
k, disagree = 0, []
for j in jobs:
    for o in j["others"]:
        v, q2 = res[k]
        k += 1
        o["landedTruth"], o["truth"] = o["truth"], v
        if v in ("Conflict", "Clean") and q2 in ("Conflict", "Clean") and v != q2:
            disagree.append({"i": j["i"], "ref": j["ref"], "other": o["ref"], "headFirst": v, "otherFirst": q2})
json.dump({"jobs": jobs}, open(a.out, "w"))

# The 1,000 sampled pairs, overlapping or not: the other ref's head is its latest push at or before the job.
ev = json.load(open(a.events))["events"]
heads = {}
for e in ev:
    heads.setdefault(e["ref"], []).append((e["t"], e["head"]))
byi = {j["i"]: j for j in jobs}
sample = json.load(open(a.sample))["pairs"]
sw = []
for p in sample:
    j = byi[p["i"]]
    sw.append((j["base"], j["head"], max(h for h in heads[p["other"]] if h[0] <= j["t"])[1]))
with ThreadPoolExecutor(a.workers) as ex:
    sres = list(ex.map(lambda w: truth(*w), sw))
for p, (v, _) in zip(sample, sres):
    p["truthQ"] = v
json.dump(sample, open(a.out.replace(".json", "-sample.json"), "w"))
from collections import Counter
print(json.dumps({"pairs": len(work), "truth": Counter(v for v, _ in res), "orderDisagree": len(disagree), "disagreeSample": disagree[:5],
                  "sample": Counter((p["landed"], p["raw"], p["truthQ"]) for p in sample).most_common(),
                  "sampleNonOverlapConflicts": sum(1 for p in sample if not p["overlapFiles"] and p["truthQ"] == "Conflict")}, default=str))
