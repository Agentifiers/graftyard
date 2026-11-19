#!/usr/bin/env python3
"""tier-off-leader's planted bugs: each must fail `a_follower_serves_only_its_reads_under_the_caps_and_never_stale`
(graftyard-site) or `a_whole_tier_read_every_member_declines_never_goes_to_the_leader` (graftyard-lookup). Restores
each file byte for byte. Run from the worktree root: `python3 tests/tieroff/plant.py`."""
import subprocess

MB = "workers/site/src/member.rs"
LK = "crates/lookup/src/lib.rs"
SITE = ["cargo", "test", "-q", "-p", "graftyard-site", "--lib", "--", "a_follower_serves_only"]
LOOK = ["cargo", "test", "-q", "-p", "graftyard-lookup", "--lib", "--", "whole_tier_read_every_member"]
GATE = "    if index && (if leading { off_leader } else { !servable }) {"
DECL = "            (false, true) => Spread::Shed(OVERLOAD_RETRY_MS),"
PRED = '    q.get("rows").is_some_and(Value::is_boolean) || q.get("lsRefs").is_some()'
M = [("main's gate: a leader serves every read-index read", MB, GATE, "    if index && !leading && !servable {", SITE),
     ("the leader refuses only when it is past HEAVY_READS", MB, GATE,
      "    if index && (if leading { off_leader && heavies >= HEAVY_READS } else { !servable }) {", SITE),
     ("a follower refuses an off-leader read too", MB, GATE, "    if index && (off_leader || (!leading && !servable)) {", SITE),
     ("main's verdict: a declined whole-tier read goes to the leader", LK, DECL, "            (false, true) => Spread::Leader,", LOOK),
     ("a declined whole-tier read with a leader named counts as absent", LK, DECL, "            (false, true) => Spread::Absent,", LOOK),
     ("ls-refs is not covered", LK, PRED, '    q.get("rows").is_some_and(Value::is_boolean)', LOOK),
     ("a merge's rows are kept off the leader too", LK, PRED, '    q.get("rows").is_some() || q.get("lsRefs").is_some()', LOOK)]
for name, f, old, new, cmd in M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        code = subprocess.run(cmd, capture_output=True, text=True, timeout=1200).returncode
    finally:
        open(f, "w").write(s)
    print("CAUGHT" if code else "MISSED", name, code, flush=True)
