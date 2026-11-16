#!/usr/bin/env python3
# roll-up read half: plants in rehydrate and the sweeper's consumed-pack guard; each must fail
# `cargo test -p graftyard-git`. Run from the worktree root.
import subprocess

CATCHUP = 'crates/git/src/catchup.rs'
SWEEP = 'crates/git/src/sweep.rs'
M = [('produced not added to records', CATCHUP,
      'records.insert(*id, (*chain, p.clone()));', 'let _ = (id, chain);'),
     ('consumed not extended', CATCHUP,
      'consumed.extend(c.consumed.iter().copied());', 'let _ = &c.consumed;'),
     ('marker requirement removed', CATCHUP,
      '.filter(|id| marks.contains_key(id))', '.filter(|_| true)'),
     ('produced ordered by its marker, not its range', CATCHUP,
      'id.and_then(|id| records.get(&id).map(|(_, p)| p.from).or_else(|| at.get(&id).copied()))',
      'id.and_then(|id| at.get(&id).copied())'),
     ('CONSUMED_KEEP check off', SWEEP,
      'now.0.saturating_sub(rolled_at.0) >= CONSUMED_KEEP', 'now.0.saturating_sub(rolled_at.0) >= 0'),
     ('a None shard passes', SWEEP,
      'c.is_some_and(|i| i >= compacted_index)', 'c.map_or(true, |i| i >= compacted_index)')]
for n, f, o, new in M:
    s = open(f).read(); assert s.count(o) == 1, n
    open(f, 'w').write(s.replace(o, new))
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', '--test', 'catchup'], capture_output=True)
    open(f, 'w').write(s)
    print(('CAUGHT' if r.returncode else 'MISSED'), n)
