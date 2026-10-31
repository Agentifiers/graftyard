#!/usr/bin/env python3
"""M0-WP16 (L0-05) replay source: real topic branches of torvalds/linux, read from a bare repository through a scratch
repository whose `objects/info/alternates` points at it, so nothing is written into the source clone.

  python3 wp16/topics.py --repo /home/user/wp16/alt.git --from <v7.1 sha> --to <v7.3 sha> --out /home/user/wp16/topics.json

A topic is the second parent T of a merge M in from..to, with its own commits M^1..T: 1 to 12 non-merge commits and no
merges. Each topic is one agent branch, and each of its commits is one push of that branch, at the commit's committer
time. For each push it records the head, the changed paths against the merge base with the first-parent master at that
time, and the time. Master's first-parent history in the window is the moving base.
"""
import argparse, json, subprocess, sys

ap = argparse.ArgumentParser()
ap.add_argument("--repo", required=True)
ap.add_argument("--from", dest="frm", required=True)
ap.add_argument("--to", required=True)
ap.add_argument("--out", required=True)
ap.add_argument("--max-commits", type=int, default=12)
a = ap.parse_args()


def git(*args, inp=None):
    return subprocess.run(["git", "-C", a.repo, *args], input=inp, capture_output=True, text=True, check=True).stdout


master = [l.split() for l in git("rev-list", "--first-parent", "--format=%H %ct", "--no-commit-header", f"{a.frm}..{a.to}").split("\n") if l]
master.reverse()  # oldest first: (sha, time)
merges = [l.split() for l in git("rev-list", "--merges", "--format=%H %P %ct", "--no-commit-header", f"{a.frm}..{a.to}").split("\n") if l]
topics = []
for m in merges:
    sha, p1, p2 = m[0], m[1], m[2]
    commits = [l.split() for l in git("rev-list", "--reverse", "--format=%H %ct %P", "--no-commit-header", f"{p1}..{p2}").split("\n") if l]
    if not 1 <= len(commits) <= a.max_commits or any(len(c) > 3 for c in commits):
        continue
    pushes = []
    for c in commits:
        files = [f for f in git("diff-tree", "--no-commit-id", "--name-only", "-r", c[2], c[0]).split("\n") if f]
        pushes.append({"head": c[0], "t": int(c[1]), "files": files})
    topics.append({"merge": sha, "base": p1, "tip": p2, "pushes": pushes})
print(f"{len(topics)} topics from {len(merges)} merges, {sum(len(t['pushes']) for t in topics)} pushes; master {len(master)} first-parent commits", file=sys.stderr)
json.dump({"master": master, "topics": topics}, open(a.out, "w"))
