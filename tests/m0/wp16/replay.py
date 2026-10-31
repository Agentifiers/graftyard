#!/usr/bin/env python3
"""M0-WP16 (L0-05, L0-06): the replay of real linux topic branches as agent branches against a moving base.

  python3 wp16/replay.py --repo /home/user/wp16/alt.git --topics /home/user/wp16/topics.json \
      --merges /home/user/wp16/merges.txt --from 2026-07-01 --to 2026-08-16 --out /home/user/wp16/events.json

Input is `wp16/topics.py`'s output. A topic (the second parent of a merge into master, 1 to 12 commits, no merges) is one
agent branch `agent/<n>`; each of its commits is one head push, at the commit's committer time. The branch is live from
its first push until master merges it. The moving base B(t) is master's last first-parent commit at or before t.

For each push in [from, to) it writes the head, B(t), the branch's merge base with B(t) (PathSet.merge_base), and the
PathSet paths: `git diff-tree --name-only merge_base head`, cut to directories past PATHS_MAX (truncated). Everything
comes from real git on the source repository (through an alternates repository, so the source is never written).
"""
import argparse, datetime, json, subprocess, sys

ap = argparse.ArgumentParser()
ap.add_argument("--repo", required=True)
ap.add_argument("--topics", required=True)
ap.add_argument("--merges", required=True)
ap.add_argument("--from", dest="frm", required=True)
ap.add_argument("--to", required=True)
ap.add_argument("--out", required=True)
a = ap.parse_args()
PATHS_MAX = 1000


def git(*args):
    return subprocess.run(["git", "-C", a.repo, *args], capture_output=True, text=True, check=True).stdout.strip()


ts = lambda d: int(datetime.datetime.fromisoformat(d).replace(tzinfo=datetime.timezone.utc).timestamp())
W0, W1 = ts(a.frm), ts(a.to)
d = json.load(open(a.topics))
master = [(s, int(t)) for s, t in d["master"]]  # oldest first
merged_at = {}
for line in open(a.merges):
    s = line.split()
    merged_at[s[0]] = int(s[-1])


def base_at(t):
    lo, hi = 0, len(master) - 1
    if master[0][1] > t:
        return master[0][0]
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if master[mid][1] <= t:
            lo = mid
        else:
            hi = mid - 1
    return master[lo][0]


events, refs = [], []
for n, tp in enumerate(d["topics"]):
    end = merged_at[tp["merge"]]
    pushes = [p for p in tp["pushes"] if W0 <= p["t"] < W1 and p["t"] < end]
    if not pushes:
        continue
    refs.append({"ref": f"agent/{n}", "first": tp["pushes"][0]["t"], "merged": end, "merge": tp["merge"]})
    for p in pushes:
        events.append({"t": p["t"], "ref": f"agent/{n}", "head": p["head"], "merged": end})
events.sort(key=lambda e: (e["t"], e["ref"]))
for i, e in enumerate(events):
    e["i"] = i
    e["base"] = base_at(e["t"])
    e["mb"] = git("merge-base", e["head"], e["base"])
    paths = [p for p in git("diff-tree", "-r", "--name-only", e["mb"], e["head"]).split("\n") if p]
    e["truncated"] = len(paths) > PATHS_MAX
    e["paths"] = sorted({p.rsplit("/", 1)[0] for p in paths}) if e["truncated"] else paths
    if i % 200 == 0:
        print(f"{i}/{len(events)}", file=sys.stderr)
# A ref's pushes before the window still set its tip: record each ref's head as of the window start.
start_heads = {}
for n, tp in enumerate(d["topics"]):
    before = [p for p in tp["pushes"] if p["t"] < W0]
    if before and merged_at[tp["merge"]] > W0:
        start_heads[f"agent/{n}"] = before[-1]["head"]
print(f"{len(events)} pushes on {len(refs)} refs in the window; {len(start_heads)} refs live at its start", file=sys.stderr)
json.dump({"from": W0, "to": W1, "events": events, "startHeads": start_heads,
           "masterInWindow": [m for m in master if W0 <= m[1] < W1]}, open(a.out, "w"))
