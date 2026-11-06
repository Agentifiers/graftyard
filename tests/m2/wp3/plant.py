#!/usr/bin/env python3
"""M2-WP3's planted bugs: plants one bug at a time in crates/git/src, runs `crates/git/tests/merge_queue.rs`, restores
the file, and prints which tests failed. Every plant must be caught. plant.py [I] runs only plant I.

Needs CARGO_TARGET_DIR or a writable target/. It edits the work tree while it runs, so run it on a clean tree."""
import os, re, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
env = dict(os.environ, PATH=os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"])

PLANTS = [
    # BF-23
    ("a merge on a stale success: a queued or running conclusion counts as passing",
     "crates/git/src/merge.rs",
     "    if rows.iter().any(|r| r.conclusion.is_none()) {",
     "    if false && rows.iter().any(|r| r.conclusion.is_none()) {"),
    ("a merge on a stale success: a re-request does not replace the success it re-runs",
     "crates/git/src/tiers.rs",
     "                if newer {",
     "                if newer && get_rec::<RequiredRow>(w, \"required_check\", &key).is_none() {"),
    ("a merge on a stale success: the consumer ejects on a re-requested check instead of waiting",
     "crates/git/src/queue_consumer.rs",
     "        MergeError::Protection(RuleViolation::CheckPending { .. } | RuleViolation::CheckExpected { .. }) => {",
     "        MergeError::Protection(RuleViolation::CheckExpected { .. }) => {"),
    # BM-2
    ("a stale parent_tip: a Build that does not extend the last built entry",
     "crates/git/src/queue.rs",
     "        if last != Some(first.parent_tip) {",
     "        if last.is_none() {"),
    ("a stale parent_tip: a Build whose chain breaks inside it",
     "crates/git/src/queue.rs",
     "        if i > 0 && e.parent_tip != entries[i - 1].group_tip {",
     "        if false && i > 0 && e.parent_tip != entries[i - 1].group_tip {"),
    ("a stale parent_tip: a QueueMerge plan whose base_old is not its entry's parent_tip",
     "crates/git/src/queue.rs",
     "            if plan.base_old != e.parent_tip || plan.commit != e.group_tip {",
     "            if plan.commit != e.group_tip {"),
    # BF-21
    ("a non-fast-forward base move: a QueueMerge on a base that moved outside the queue",
     "crates/git/src/queue.rs",
     "            if e.parent_tip != tip {\n                return Err((n, if i == 0 { MergeError::BaseModified } else { MergeError::StaleChain }));",
     "            if i > 0 && e.parent_tip != tip {\n                return Err((n, if i == 0 { MergeError::BaseModified } else { MergeError::StaleChain }));"),
    ("a group landing past a failed check: HeadGreen reads only the last entry's checks",
     "crates/git/src/queue.rs",
     "                    if let Some(v) = held(&rules, &own) {",
     "                    if let Some(v) = held(&rules, &own).filter(|_| false) {"),
    ("a partial group: Landed removes only the head of the queue",
     "crates/git/src/queue.rs",
     "                if row.n == *through {",
     "                if true || row.n == *through {"),
    ("a failing pull request merged: the consumer never ejects on a failed queue-ref check",
     "crates/git/src/queue_consumer.rs",
     "            Err(name) => ejects.push(eject(e.n, EjectReason::CheckFailed { name })),",
     "            Err(_) => {}"),
    ("a queued pull request lost: Ejected removes every later entry, not only the one named",
     "crates/git/src/queue.rs",
     "                if seq > at_seq && row.built.take().is_some() {\n                    put(w, &base, seq, &row);",
     "                if seq > at_seq && row.built.take().is_some() {\n                    w.delete(\"queue\", &[base_key(&base), int(seq)]);"),
    # Direct merges
    ("a merge on a moved base: decide does not re-check the plan's base",
     "crates/git/src/merge.rs",
     "        if v.ref_value(&ph.base) != Some(self.plan.base_old) {",
     "        if v.ref_value(&ph.base).is_none() {"),
    # BF-1
    ("a follower refuses the group's entry: a landing's entry is tagged Core only, though Merged touches Metadata",
     "crates/git/src/merge.rs",
     "            MergeCmd::Queue(_) | MergeCmd::Cursor(_) => TierSet::CORE,",
     "            MergeCmd::Queue(_) | MergeCmd::Cursor(_) => TierSet::CORE,\n            MergeCmd::Land(_) => TierSet::CORE,"),
    ("a follower refuses the group's entry: every Merged of a group expects the first base tip",
     "crates/git/src/queue.rs",
     "                base: RefUpdate { name: base.clone(), expect: Some(e.parent_tip), new: Some(e.group_tip) },",
     "                base: RefUpdate { name: base.clone(), expect: Some(landed[0].1.parent_tip), new: Some(e.group_tip) },"),
]

only = int(sys.argv[1]) if sys.argv[1:] else None
missed = 0
for i, (what, path, old, new) in enumerate(PLANTS):
    if only is not None and i != only:
        continue
    p = os.path.join(ROOT, path)
    src = open(p).read()
    assert src.count(old) == 1, f"plant {i} does not match once: {what}"
    open(p, "w").write(src.replace(old, new))
    try:
        r = subprocess.run(["cargo", "test", "-q", "-p", "graftyard-git", "--test", "merge_queue"], cwd=ROOT, env=env,
                           capture_output=True, text=True, timeout=900)
        out = r.stdout + r.stderr
        failed = sorted(set(re.findall(r"^    ([\w:]+)$", out, re.M)))
        if "error[" in out:
            failed = ["COMPILE ERROR: " + out[out.index("error["):][:300]]
        missed += not failed
        print(f"{i}. {what}\n   caught by: {', '.join(failed) if failed else 'NOTHING'}", flush=True)
    finally:
        open(p, "w").write(src)
sys.exit(1 if missed else 0)
