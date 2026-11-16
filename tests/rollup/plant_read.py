#!/usr/bin/env python3
# roll-up read half: plants in rehydrate and the sweeper's consumed-pack guard; each must fail
# `cargo test -p graftyard-git`. Run from the worktree root.
import subprocess

CATCHUP = 'crates/git/src/catchup.rs'
SWEEP = 'crates/git/src/sweep.rs'
M = [('produced not added to records', CATCHUP,
      'records.insert(*id, (*chain, p.clone()));', 'let _ = (id, chain);'),
     ('kept rolls consume nothing', CATCHUP,
      'rolls.iter().flat_map(|(_, _, _, c)| c.iter().copied())', 'rolls.iter().flat_map(|_| None::<PackId>)'),
     ('a failed produced pack fails the rehydrate', CATCHUP,
      'Err(e) => e.0,', 'Err(e) => return Err(e),'),
     ('a dropped roll\'s produced pack loads again', CATCHUP,
      '.chain(dropped.iter().copied())', ''),
     ('a markerless roll consumes its packs', CATCHUP,
      'let Some((_, m)) = marks.iter().find(|(x, _)| *x == id) else { continue };',
      'let Some((_, m)) = marks.iter().find(|(x, _)| *x == id) else { kept.push(roll); continue };'),
     ('rolls load newest range first', CATCHUP,
      'rolls.sort_by_key(|(c, _, p, _)| (*c, p.from));', 'rolls.sort_by_key(|(c, _, p, _)| (*c, std::cmp::Reverse(p.from)));'),
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
