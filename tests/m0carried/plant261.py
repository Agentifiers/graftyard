#!/usr/bin/env python3
"""LM-3, L0-14's demoted user (`crates/git/src/task.rs`, `decide`'s Graftyard-run create; the count from
`crates/raft/src/member/leader.rs`, `decide`, through `Scratch::set_actor_writes` and `Command::actor`): a demoted
creator allowed, the leader never handing the count to the command, the command naming no actor, and the count read
past the window so a writer under DEMOTE_PER_MIN is refused, and the entry's earlier writes left uncounted. Run from the repository root:

    python3 tests/m0carried/plant261.py          # every mutant
    python3 tests/m0carried/plant261.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

TASK = 'crates/git/src/task.rs'
LEAD = 'crates/raft/src/member/leader.rs'
CALL = 'crates/git/src/issue_ops/call.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'task_gates', 'demote']
MUTANTS = [
    ("a demoted creator is allowed", TASK, '            if ctx.writes >= graftyard_raft::admit::DEMOTE_PER_MIN {',
     '            if ctx.writes >= u32::MAX {', T),
    ("the leader hands the command no count", LEAD,
     '                    s.set_actor_writes(self.fold.actor_writes(a, from, self.now) + earlier);',
     '                    s.set_actor_writes(0 * self.fold.actor_writes(a, from, self.now) + earlier);', T),
    ("a create names no actor", CALL,
     '            (IssueOp::CreateRun(_), Actor::User(u)) => Some(graftyard_raft::admit::ActorKey(u.0.0.0 as u64)),',
     '            (IssueOp::CreateRun(_), Actor::User(_)) => None,', T),
    ("one write short demotes", TASK, '            if ctx.writes >= graftyard_raft::admit::DEMOTE_PER_MIN {',
     '            if ctx.writes + 1 >= graftyard_raft::admit::DEMOTE_PER_MIN {', T),
    ("the entry's earlier writes are not counted", LEAD, '                                *in_entry.entry(a).or_insert(0) += 1;',
     '                                let _ = in_entry.entry(a).or_insert(0);', T),
]

if __name__ == '__main__':
    run(MUTANTS)
