#!/usr/bin/env python3
"""M2-WP5's planted bugs: plants one bug at a time in crates/git/src, runs `crates/git/tests/prsync.rs`, restores the
file, and prints which tests failed. Every plant must be caught. plant.py [I] runs only plant I.

Needs CARGO_TARGET_DIR or a writable target/; edits the work tree while it runs, so run it on a clean tree."""
import os, re, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
env = dict(os.environ, PATH=os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"])

PLANTS = [
    ("a consumer that skips an event after a leader change: a new dispatcher starts at the leader's applied index",
     "crates/git/src/prsync.rs",
     "        self.scanned = self.scanned.max(cursor);\n",
     "        self.scanned = self.scanned.max(cursor).max(if self.scanned == Index(0) { applied } else { Index(0) });\n"),
    ("a consumer that skips an event after a leader change: a batch starts at the dispatcher's last send, not the cursor",
     "crates/git/src/prsync.rs",
     "        Some((SyncBatch { from: cursor, through: self.scanned, prs: prs.into_iter().collect() }, how))",
     "        let from = if self.sent_at == now.0 { cursor } else { cursor };\n        let prs: BTreeSet<IssueNumber> = self.pending.range(Index(self.sent.0.max(from.0) + 1)..).flat_map(|(_, v)| v.clone()).collect();\n        Some((SyncBatch { from, through: self.scanned, prs: prs.into_iter().collect() }, how))"),
    ("double-applying a redelivered batch: decide records a test merge already recorded",
     "crates/git/src/prsync.rs",
     "            if head != m.head || recorded {",
     "            let _ = recorded;\n            if head != m.head {"),
    ("double-applying a redelivered batch: the cursor check is skipped",
     "crates/git/src/prsync.rs",
     "        if self.through.is_some_and(|t| t <= cursor) {\n            return (SmallVec::new(), SyncOut { events: 0, cursor });\n        }",
     ""),
    ("a stale merge ref after a head push: the head move is not a trigger",
     "crates/git/src/prsync.rs",
     "                for u in updates.iter().filter(|u| u.new.is_some()) {\n                    out.extend(pulls_from(core, repo, &u.name));\n                }",
     "                let _ = (updates, repo, core);"),
    ("a stale merge ref after a head push: a merge of an older head is recorded",
     "crates/git/src/prsync.rs",
     "            let Some((head, _)) = open_head(m.n) else { continue };",
     "            let Some((_, _)) = open_head(m.n) else { continue };\n            let head = m.head;"),
    ("a stale merge ref after a head push: an unrecordable merge leaves the old head's ref",
     "crates/git/src/prsync.rs",
     "            if open_head(*n).is_some_and(|(h, _)| h == *head) && !recorded && have.is_some() {",
     "            if false && open_head(*n).is_some_and(|(h, _)| h == *head) && !recorded && have.is_some() {"),
    ("a conflict that leaves the ref in place",
     "crates/git/src/tiers.rs",
     "                    MergeabilityResult::Conflict { .. } => w.delete(\"ref\", &[ref_key(&merge_ref)]),",
     "                    MergeabilityResult::Conflict { .. } => {}"),
    ("a conflict that leaves the ref in place: mergeable reads a conflict as true",
     "crates/git/src/prsync.rs",
     "    Some(matches!(m.result, MergeabilityResult::Clean { .. }))",
     "    Some(true || matches!(m.result, MergeabilityResult::Clean { .. }))"),
    ("double-applying a redelivered batch: request ids per (batch, part), so a retry on a changed fold is answered from the first attempt's row",
     "crates/git/src/prsync.rs",
     "    zeroed.request = RequestId(Ulid(0));\n",
     "    zeroed.request = RequestId(Ulid(0));\n    zeroed.heads.clear();\n    zeroed.merges.clear();\n    zeroed.unmerged.clear();\n    zeroed.through = Some(Index(zeroed.from.0 + u64::from(c.through.is_some())));\n"),
    ("the cursor is not the group's: it is never committed",
     "crates/git/src/prsync.rs",
     "            events.push(RepoEvent::ConsumerCursor { consumer: Consumer::PrSync, through: t });",
     "            let _ = Consumer::PrSync;"),
    ("the in-band path starves the re-send: every first attempt restarts the redelivery clock",
     "crates/git/src/prsync.rs",
     "        if how == Delivery::Again || !outstanding {",
     "        if true {"),
    ("a re-send goes in band again, not through the queue",
     "crates/git/src/prsync.rs",
     "        let how = if overdue {\n            Delivery::Again",
     "        let how = if overdue {\n            Delivery::First"),
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
        r = subprocess.run(["cargo", "test", "-q", "-p", "graftyard-git", "--test", "prsync"], cwd=ROOT, env=env,
                           capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        failed = sorted(set(re.findall(r"^    (\w+)$", out, re.M)))
        if "error[" in out:
            failed = ["COMPILE ERROR: " + out[out.index("error["):][:300]]
        missed += not failed
        print(f"{i}. {what}\n   caught by: {', '.join(failed) if failed else 'NOTHING'}", flush=True)
    finally:
        open(p, "w").write(src)
sys.exit(1 if missed else 0)
