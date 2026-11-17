#!/usr/bin/env python3
"""Planted bugs in tests/fanout/purge.py; each must fail `python3 -m unittest tests/fanout/test_purge.py`. Restores the
file byte for byte. Run from the worktree root."""
import subprocess

F = "tests/fanout/purge.py"
M = [("prefixes without the trailing slash", 'f"{p}/{n}/" for p in by_net', 'f"{p}/{n}" for p in by_net'),
     ("chain prefixes without the trailing slash", 'f"{p}/{c}:{n}/" for p', 'f"{p}/{c}:{n}" for p'),
     ("no repo-meta chain prefixes", 'for c in ("repo", "repo-meta")]', 'for c in ("repo",)]'),
     ("committed markers not purged", '"pack", "committed/pack",', '"pack",'),
     ("any not-joined answer passes", "all(isinstance(v, dict) and v.get(\"joined\") is False for _, v in answers)",
      "any(isinstance(v, dict) and v.get(\"joined\") is False for _, v in answers)"),
     ("a missing joined field passes", 'v.get("joined") is False for _, v in answers', 'not v.get("joined") for _, v in answers'),
     ("a missing site passes", 'if not answers or {s for s, _ in answers} != {0, 1, 2}:', 'if not answers:'),
     ("a dry run deletes", "    if not dry:\n        for i in range", "    if True:\n        for i in range"),
     ("second pass does not round down to the sweep slot", "t // SWEEP_SLOT_S * SWEEP_SLOT_S", "t"),
     ("second pass omits the sweep wall", " + SWEEP_WALL_S + SETTLE_S", " + SETTLE_S"),
     ("no second pass", "    second = [(net, purge_one(net)) for net in nets]", "    second = []"),
     ("wait uses the clock after the first pass", "deadline = second_pass_at(t0)", "deadline = second_pass_at(now())"),
     ("second pass skips the sleep", "        sleep(deadline - now())", "        pass")]
for n, o, new in M:
    s = open(F).read()
    assert s.count(o) == 1, n
    open(F, "w").write(s.replace(o, new))
    r = subprocess.run(["python3", "-m", "unittest", "tests/fanout/test_purge.py"], capture_output=True)
    open(F, "w").write(s)
    print("CAUGHT" if r.returncode else "MISSED", n)
