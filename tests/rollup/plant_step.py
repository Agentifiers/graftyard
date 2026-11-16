#!/usr/bin/env python3
# roll-up writer: plants in log_archive's roll_plan and roll step; each must fail
# `cargo test -p graftyard-git --test log_archive`. Run from the worktree root.
import subprocess

F = 'crates/git/src/log_archive.rs'
M = [('cap boundary exclusive', '        if next > ROLL_IN_MAX {', '        if next >= ROLL_IN_MAX {'),
     ('no age rule', '(reached || (prefix >= ROLL_MIN_PACKS && old))', '(reached || prefix >= ROLL_MIN_PACKS)'),
     ('single promotion rolls', '    if prefix < 2 {\n        return None;', '    if prefix < 1 {\n        return None;'),
     ('a roll before a due range', '    match d {\n        Some(d) if !ahead_first',
      '    match d {\n        _ if roll_plan(&v.rollable, v.now).is_some() => return roll(h, &v).await,\n        Some(d) if !ahead_first'),
     ('recorded in the old format', 'h.record(request, REPO_FORMAT_ROLLUP, vec![RepoEvent::Compacted(c)])',
      'h.record(request, REPO_FORMAT_V1, vec![RepoEvent::Compacted(c)])'),
     ('produced not marked', '    let index = h.record(request, REPO_FORMAT_ROLLUP, vec![RepoEvent::Compacted(c)]).await?;\n    h.mark(&pack, index).await;',
      '    let index = h.record(request, REPO_FORMAT_ROLLUP, vec![RepoEvent::Compacted(c)]).await?;')]
for n, o, new in M:
    s = open(F).read(); assert s.count(o) == 1, n
    open(F, 'w').write(s.replace(o, new))
    r = subprocess.run(['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'log_archive'], capture_output=True)
    open(F, 'w').write(s)
    print(('CAUGHT' if r.returncode else 'MISSED'), n)
