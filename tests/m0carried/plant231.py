#!/usr/bin/env python3
"""M4-WP3 actions-cache, the shard's rules (`crates/actions/src/cache.rs`, `crates/actions/src/shard.rs`;
`decisions/DECISIONS.md`, "`actions-cache`"): the default branch searched before a pull request's base, a low-trust
run writing the default branch, eviction by newest use, a commit whose chunks do not tile, a commit of another size
than the reserve's, a stale reservation still holding its key, a hit moving last use every time, a reserve past the entry cap evicting nothing, the edge serving the cache before 21.6 is enabled, a range's slice
read from the wrong offset, and a download capability that does not bind its size. Run from the repository root:

    python3 tests/m0carried/plant231.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

C = 'crates/actions/src/cache.rs'
S = 'crates/actions/src/shard.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'cache']
TE = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'cache']
MUTANTS = [
    ("the edge serves the cache before 21.6 is enabled", 'workers/edge/src/cache.rs',
     'enabled.contains(&graftyard_actions::shard::ACTIONS_FORMAT_CACHE)',
     'enabled.iter().any(|f| f.family == graftyard_actions::shard::ACTIONS_FORMAT_CACHE.family)', TE),
    ("the default branch is searched before the base branch", C,
     'for s in [Some(run_ref), base, Some(default_branch)]', 'for s in [Some(run_ref), Some(default_branch), base]', T),
    ("any run writes the default branch's scope", C,
     'run_ref != default_branch || DEFAULT_SCOPE_WRITERS.contains(&event)', 'let _ = (run_ref, default_branch, event);\n    true', T),
    ("eviction takes the most recently used first", C,
     'live.sort_by_key(|(id, e)| (e.last_used, e.created, *id));',
     'live.sort_by_key(|(id, e)| (std::cmp::Reverse(e.last_used), e.created, *id));', T),
    ("chunks with a gap commit", C,
     '        if s > at {\n            return Err(Tiling::Gap { at });\n        }\n', '', T),
    ("a commit of another size than the reserve's is taken", S,
     '        if size != e.size {\n            return Err', '        if false && size != e.size {\n            return Err', T),
    ("a stale reservation keeps its key", C,
     'CacheState::Reserved { .. } => now.0.saturating_sub(self.created.0) < CACHE_RESERVE_STALE_MS,',
     'CacheState::Reserved { .. } => true,', T),
    ("every hit moves last use", S,
     'if s.at().0.saturating_sub(e.last_used.0) < crate::cache::CACHE_USED_EVERY_MS {',
     'if s.at().0.saturating_sub(e.last_used.0) < 1 {', T),
    ("a reserve past the entry cap evicts nothing", S,
     '.take(live + 1 - CACHE_ENTRIES_MAX)', '.take(0)', T),
    ("a range slice starts at the chunk's start", C, '(i, from - st, to - from + 1)', '(i, 0, to - from + 1)', T),
    ("a download capability does not bind its size", C,
     'format!("graftyard/cache-download/1\\0{chain}\\0{}\\0{size}\\0{exp}", id.0)',
     'format!("graftyard/cache-download/1\\0{chain}\\0{}\\0{exp}", id.0 + 0 * size)', T),
]

if __name__ == '__main__':
    run(MUTANTS)
