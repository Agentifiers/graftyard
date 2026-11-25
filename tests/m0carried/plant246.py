#!/usr/bin/env python3
"""BF-71 `actions-artifacts` and F125's repository delete (`crates/actions/src/artifacts.rs`, `crates/actions/src/shard.rs`,
`crates/identity/src/finish.rs`; `decisions/DECISIONS.md`, "`actions-artifacts`"). The mutants: a container path
naming another repository's prefix admitted, another run admitted, a file recorded before its chunks tile it, a
re-upload's chunks written among the first upload's, a later chunk with no upload started taken; the drop evicting
another repository's entries, a delete that owes no drop once its tombstone is in, and a drop owed with nothing held,
so a retry never ends. The gzip passthrough's mutant (the edge's `encodeBody: "manual"` dropped) is caught by
`tests/edgeboot/run.sh`, which this file does not run. Run from the repository root:

    python3 tests/m0carried/plant246.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

A = 'crates/actions/src/artifacts.rs'
S = 'crates/actions/src/shard.rs'
F = 'crates/identity/src/finish.rs'
TA = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'artifacts']
TS = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'a_repository_drop']
TF = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'audit', 'delete']
MUTANTS = [
    ("a container naming another repository's prefix is admitted", A,
     'if c.chain != token_chain || c.run != job_run || c.repo != run_repo {',
     'if c.chain != token_chain || c.run != job_run {', TA),
    ("another run's container is admitted", A,
     'if c.chain != token_chain || c.run != job_run || c.repo != run_repo {',
     'if c.chain != token_chain || c.repo != run_repo {', TA),
    ("a file is recorded before its chunks tile it", A,
     '    tiles(chunks, size)?;\n    Ok(FileRecord', '    let _ = tiles(chunks, size);\n    Ok(FileRecord', TA),
    ("a re-upload's first chunk joins the upload before it", A,
     'if start == 0 { Some(fresh) } else { pointer }', 'if start == 0 { pointer.or(Some(fresh)) } else { pointer }', TA),
    ("a later chunk with no upload started is taken", A,
     'if start == 0 { Some(fresh) } else { pointer }', 'if start == 0 { Some(fresh) } else { pointer.or(Some(fresh)) }',
     TA),
    ("the drop evicts every repository's entries", S,
     's.cache_entries().filter(|(_, e)| e.repo == *repo).map(|(id, _)| id).collect();',
     's.cache_entries().map(|(id, _)| id).collect();', TS),
    ("a tombstoned delete owes no drop", F,
     '        RepoLife::Deleted if actions_held => Some(Step::Repo(repo, RepoStep::DropActions)),\n', '', TF),
    ("a drop is owed with nothing held", F,
     '        RepoLife::Deleted if actions_held => Some(Step::Repo(repo, RepoStep::DropActions)),\n',
     '        RepoLife::Deleted => Some(Step::Repo(repo, RepoStep::DropActions)),\n', TF),
]

if __name__ == '__main__':
    run(MUTANTS)
