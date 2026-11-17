#!/usr/bin/env python3
"""sweep-wall: planted bugs in the sweeper's run bounds (`crates/git/src/sweep.rs`, `RunRows` and `sweep_networks`).
Each must fail `cargo test -p graftyard-git --lib sweep`; the file is restored byte for byte. Run from the worktree root.
The first mutant is main's behavior: every pack's lookup is asked, and no wall."""
import subprocess

F = 'crates/git/src/sweep.rs'
DEAD = '''        if self.dead.borrow().contains(&chain) {
            return Err(StoreError(format!("{chain}: a lookup failed earlier this run")));
        }
'''
WALL_ASK = '''        if (self.clock)() >= self.deadline {
            return Err(StoreError("the run's wall has passed".into()));
        }
'''
WALL_NET = '''        if (rows.clock)() >= rows.deadline {
            out.next = Some(*n);
            out.walled = true;
            return Ok(out);
        }
'''
M = [('no memo and no wall (main)', [(DEAD, ''), (WALL_ASK, ''), (WALL_NET, '')]),
     ('a failed chain is asked again', [(DEAD, '')]),
     ('a failed lookup is not remembered', [('            self.dead.borrow_mut().insert(chain);\n', '')]),
     ('a run asks rows past its wall', [(WALL_ASK, '')]),
     ('a run starts networks past its wall', [(WALL_NET, '')]),
     ('the wall is checked with >', [('if (rows.clock)() >= rows.deadline', 'if (rows.clock)() > rows.deadline'),
                                     ('if (self.clock)() >= self.deadline', 'if (self.clock)() > self.deadline')]),
     ('the walled run does not keep its cursor', [('            out.next = Some(*n);\n            out.walled = true;\n',
                                                   '            out.walled = true;\n')]),
     ('the deadline starts at the wall, not the run', [('let deadline = clock().saturating_add(budget.wall_ms);',
                                                        'let deadline = budget.wall_ms;')])]
for n, edits in M:
    s = open(F).read()
    t = s
    for o, new in edits:
        assert t.count(o) == 1, (n, o)
        t = t.replace(o, new)
    open(F, 'w').write(t)
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', 'sweep'], capture_output=True)
    open(F, 'w').write(s)
    out = r.stdout.decode() + r.stderr.decode()
    print('DID NOT COMPILE' if 'error[' in out else ('CAUGHT' if r.returncode else 'MISSED'), n)
