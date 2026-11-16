#!/usr/bin/env python3
"""b034plan.py BARE OUT [TARGET_BYTES] [CHUNK_BYTES] [FUTURE] -> B0-34's monorepo seed plan: the pushes that carry a
bare torvalds/linux clone's first-parent history into a fresh repository in packs under the edge's 90 MB cap
(Pro's 100 MB plan limit x 9/10, `workers/edge/src/lib.rs`, `Config::read`), until about TARGET_BYTES (default 5e9)
of objects are pushed, then FUTURE (default 600) more first-parent commits for the tip to move along during the row
(10 merges a minute, `build/BUILD-PLAN.md`, B0-34).

Each line of OUT is `<commit> <ref> <estimated bytes>`. A first-parent step whose objects estimate over CHUNK_BYTES
(default 60e6) is split: its second parent's first-parent history is pushed to `refs/heads/b034-side` in steps first,
recursively. Sizes are `git rev-list --disk-usage --objects` of the commit not reachable from what is already pushed,
the on-disk (deltified) size, which a pushed pack's size follows [inference]. The last FUTURE commits go to
a `# future` line; the row's merges push them to `main` one at a time, so the tip moves along real history.
A single commit over CHUNK_BYTES with no second parent (torvalds/linux's root, 53 MB packed) has its tree carried
first in commits of top-level entries under the chunk, on `refs/heads/b034-parts`.
"""
import subprocess, sys

bare, out = sys.argv[1], sys.argv[2]
target = float(sys.argv[3]) if len(sys.argv) > 3 else 5e9
chunk = float(sys.argv[4]) if len(sys.argv) > 4 else 60e6
future = int(sys.argv[5]) if len(sys.argv) > 5 else 600
git = lambda *a: subprocess.run(["git", *a], cwd=bare, capture_output=True, text=True, check=True).stdout
pushed = []  # tips the remote holds, as `--not` bases
plan = []
total = 0


def size(c):
    return int(git("rev-list", "--disk-usage", "--objects", c, "--not", *pushed[-64:]).strip() or 0) if pushed else \
        int(git("rev-list", "--disk-usage", "--objects", c).strip())


def emit(c, ref, n):
    global total
    plan.append((c, ref, n))
    pushed.append(c)
    total += n


def carry_tree(c):
    """A commit over the chunk that history cannot split (linux's root, 53 MB packed): push its tree's objects first
    as commits each holding a part of the top-level entries, on `refs/heads/b034-parts`, so the commit itself then
    carries only what they lack. The part commits are written into the bare clone (`git commit-tree`)."""
    tree = git("rev-parse", f"{c}^{{tree}}").strip()
    entries = [l for l in git("ls-tree", tree).splitlines()]
    part, parent = [], None
    def flush():
        nonlocal part, parent
        if not part:
            return
        t = subprocess.run(["git", "mktree"], cwd=bare, input="\n".join(part) + "\n", capture_output=True, text=True, check=True).stdout.strip()
        args = ["commit-tree", t, "-m", "b034 seed part"] + (["-p", parent] if parent else [])
        env = {"GIT_AUTHOR_NAME": "b034", "GIT_AUTHOR_EMAIL": "b034@x", "GIT_COMMITTER_NAME": "b034", "GIT_COMMITTER_EMAIL": "b034@x",
               "GIT_AUTHOR_DATE": "1700000000 +0000", "GIT_COMMITTER_DATE": "1700000000 +0000", "PATH": "/usr/bin:/bin"}
        parent = subprocess.run(["git", *args], cwd=bare, capture_output=True, text=True, check=True, env=env).stdout.strip()
        emit(parent, "refs/heads/b034-parts", size(parent))
        part = []
    for e in entries:
        oid = e.split()[2]
        n = int(git("rev-list", "--disk-usage", "--objects", oid, "--not", *pushed[-64:]).strip() or 0) if pushed else \
            int(git("rev-list", "--disk-usage", "--objects", oid).strip())
        part.append(e)
        if n > chunk:
            sys.stderr.write(f"one entry over the chunk: {e} {n}\n")
        if sum_part(part) > chunk:
            part.pop(); flush(); part = [e]
    flush()


def sum_part(part):
    objs = [e.split()[2] for e in part]
    return int(git("rev-list", "--disk-usage", "--objects", *objs, "--not", *pushed[-64:]).strip() or 0) if pushed else \
        int(git("rev-list", "--disk-usage", "--objects", *objs).strip())


def push_line(tips, ref):
    """Push first-parent list `tips` (oldest first) in steps of at most `chunk`, splitting any step that is over."""
    i = 0
    while i < len(tips):
        # Grow the step by doubling, then back off to the last one under the chunk.
        lo, n = i, 1
        best = None
        while i + n - 1 < len(tips):
            s = size(tips[i + n - 1])
            if s > chunk:
                break
            best = (i + n - 1, s)
            n *= 2
        if best is None:
            c = tips[i]
            if size(c) > chunk:
                carry_tree(c)
            parents = git("rev-list", "--parents", "-n1", c).split()[1:]
            if len(parents) > 1:
                side = git("rev-list", "--first-parent", "--reverse", parents[1], "--not", *pushed[-64:]).split()
                if side:
                    push_line(side, "refs/heads/b034-side")
            emit(c, ref, size(c))
            i += 1
            continue
        emit(tips[best[0]], ref, best[1])
        i = best[0] + 1
        if total >= target and ref == "refs/heads/main":
            return i
        sys.stderr.write(f"{len(plan)} pushes, {total / 1e9:.2f} GB\n")
    return i


line = git("rev-list", "--first-parent", "--reverse", "HEAD").split()
stop = push_line(line, "refs/heads/main")
fut = line[stop:stop + future]
with open(out, "w") as f:
    for c, ref, n in plan:
        f.write(f"{c} {ref} {n}\n")
    f.write("# future " + " ".join(fut) + "\n")
print(f"{len(plan)} pushes, {total / 1e9:.2f} GB, main stops at {line[stop - 1]}, {len(fut)} future commits")
