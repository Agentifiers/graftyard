#!/usr/bin/env python3
"""sweep-quiet: planted bugs in the sweeper's cached network homes (`workers/site/src/sweep.rs`, `due` and `homes`).
Each must fail `cargo test -p graftyard-site --lib sweep`; the file is restored byte for byte. Run from the worktree
root. The first mutant is main's behavior before the fix: every run reads every network's home."""
import subprocess

F = 'workers/site/src/sweep.rs'
M = [('every run reads every home (main)',
      '    missing.chain(stale.into_iter().map(|(_, n)| n)).take(HOMES_PER_RUN).collect()',
      '    let _ = (missing, stale);\n    all.to_vec()'),
     ('a home never goes stale', 'now.saturating_sub(h.at) >= HOME_FOR_MS', 'false && now.saturating_sub(h.at) >= HOME_FOR_MS'),
     ('a home is stale only past the day', 'now.saturating_sub(h.at) >= HOME_FOR_MS', 'now.saturating_sub(h.at) > HOME_FOR_MS'),
     ('no cap on reads a run', '.take(HOMES_PER_RUN).collect()', '.collect()'),
     ('day-old homes before new networks', 'missing.chain(stale.into_iter().map(|(_, n)| n))',
      'stale.into_iter().map(|(_, n)| n).chain(missing)'),
     ('a failed read drops the cached home', '            Err(e) => unresolved.push((n.0.0, e)),\n        }\n    }\n    (home,',
      '            Err(e) => {\n                home.remove(&n);\n                unresolved.push((n.0.0, e))\n            }\n        }\n    }\n    (home,'),
     ('a fresh read keeps the old time', 'fresh.push((n, Home { site, at: now }));',
      'fresh.push((n, Home { site, at: cached.get(&n).map_or(now, |h| h.at) }));')]
for n, o, new in M:
    s = open(F).read()
    assert s.count(o) == 1, n
    open(F, 'w').write(s.replace(o, new))
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'sweep'], capture_output=True)
    open(F, 'w').write(s)
    out = r.stdout.decode() + r.stderr.decode()
    print('DID NOT COMPILE' if 'error[' in out else ('CAUGHT' if r.returncode else 'MISSED'), n)
