#!/usr/bin/env python3
"""promote-lapse: planted bugs in a due range's promotion as `Caller::Start`. Each must fail
`cargo test -p graftyard-git --test shard`; the file is restored byte for byte. Run from the worktree root."""
import subprocess

F = 'crates/git/src/shard.rs'
M = [('start read as a warm call (skips a stopped shard)', '        Some("start") => Caller::Start,\n', ''),
     ('start touches, as a client', '        Caller::PoolWarm | Caller::Start => Serve::Run { touch: false },',
      '        Caller::PoolWarm => Serve::Run { touch: false },\n        Caller::Start => Serve::Run { touch: true },'),
     ('lapse even with a recent client', '(caller == Caller::Start && idle)', '(caller == Caller::Start)'),
     ('lapse for every caller', '(caller == Caller::Start && idle)', '(idle)'),
     ('never a client counts as recent', 'let idle = last_request.is_none_or(', 'let idle = last_request.is_some_and('),
     ('lapse as long as the inactivity timeout', 'Duration::from_secs(600);', 'Duration::from_secs(6 * 3600);')]
for n, o, new in M:
    s = open(F).read(); assert s.count(o) == 1, n
    open(F, 'w').write(s.replace(o, new))
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard'], capture_output=True)
    open(F, 'w').write(s)
    print(('CAUGHT' if r.returncode else 'MISSED'), n)
