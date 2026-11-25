#!/usr/bin/env python3
"""AM3-WP1 S2, the controller's `TaskBox` (`crates/git/src/taskbox.rs`; LM-3's "a Sandbox killed mid-task resumes"):
a late report taken, a late report not counted, a stopped container not restarted, a silent Sandbox kept, a version
change not followed, a restart that reuses the lost `SandboxId`, a final task's alarm leaving its container, an
`unpack` failure counted as an ack, an `ng` or a ref outside the task's refs counted, a delete counted, and the
progress band read as the report; and the hold under a `ControllerPointer` taken away, its release dropped,
its time limit dropped or reset each alarm, late reports taken while held, or a failed read of the view releasing
it. Run from the repository root:

    python3 tests/m0carried/plant248.py          # every mutant
    python3 tests/m0carried/plant248.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

B = 'crates/git/src/taskbox.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'taskbox']
MUTANTS = [
    ("a report from an old Sandbox is taken", B,
     '        if self.hold.is_some() || self.current() != Some(from) {\n', '        if self.current().is_none() {\n', T),
    ("a late report is not counted", B, '            self.late += 1;\n', '', T),
    ("a stopped container is not restarted", B,
     ' || !running || silent ||', ' || silent ||', T),
    ("a silent Sandbox is kept", B,
     '                let silent = now.saturating_sub(self.heard) >= SILENT_MAX.as_millis() as u64;\n',
     '                let silent = false && now >= SILENT_MAX.as_millis() as u64;\n', T),
    ("a box stays on the version it started on", B,
     ' || self.version.as_deref() != Some(&version) {\n', ' {\n', T),
    ("a restart reuses the lost SandboxId", B, '        self.generation += 1;\n', '', T),
    ("a final task's alarm leaves its container running", B,
     '                if self.task.state.is_final() {\n                    return Ok(if running { Act::Stop } else { Act::Keep });\n',
     '                if self.task.state.is_final() {\n                    return Ok(Act::Keep);\n', T),
    ("an unpack failure still acks", B,
     '    if lines.first().map(String::as_str) != Some("unpack ok") {\n',
     '    if lines.first().is_none() {\n', T),
    ("a ref outside the task's refs is acked", B,
     '        .filter(|name| refs.matches(name.as_bytes()))\n', '        .filter(|name| name.starts_with("refs/"))\n', T),
    ("a delete is acked", B, 'Some((n.clone(), id.non_zero()?))', 'Some((n.clone(), *id))', T),
    ("the progress band is read as the report", B, '            if d[0] == 1 {\n', '            if d[0] != 3 {\n', T),
    ("a held box takes its old Sandbox's reports", B,
     '        if self.hold.is_some() || self.current() != Some(from) {\n',
     '        if self.current() != Some(from) {\n', T),
    ("a pointer to another version holds nothing", B,
     '                    Pointer::To(p) => (p != version).then_some(p),\n',
     '                    Pointer::To(_) => None,\n', T),
    ("a failed read of the view releases a hold", B,
     '                    Pointer::Unread => self.hold.as_ref().map(|h| h.0.clone()).filter(|p| *p != version),\n',
     '                    Pointer::Unread => None,\n', T),
    ("a hold is not released when the pointer goes", B,
     '                if self.hold.take().is_some() || !running ||', '                if !running ||', T),
    ("a held box never gives up", B,
     '                    if now.saturating_sub(since) >= HOLD_MAX.as_millis() as u64 {\n',
     '                    if false && now >= since + HOLD_MAX.as_millis() as u64 {\n', T),
    ("a hold restarts its clock on every alarm", B,
     '                    let since = self.hold.get_or_insert((to.clone(), now)).1;\n',
     '                    self.hold = Some((to.clone(), now));\n                    let since = now;\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
