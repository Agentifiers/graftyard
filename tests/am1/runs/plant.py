#!/usr/bin/env python3
"""am1-runs: planted bugs in the code this branch changed (the GraphQL label route, and the feed ring's hole after an
`InstallSnapshot`), on `tests/am1/rest/plant.py`'s runner. Plants one mutant at a time, runs the tests that gate it,
and restores the file byte for byte. Run from the worktree root:

    python3 tests/am1/runs/plant.py          # every mutant
    python3 tests/am1/runs/plant.py 1        # by number
"""
import importlib.util, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('rest_plant', os.path.join(HERE, '..', 'rest', 'plant.py'))
rest = importlib.util.module_from_spec(spec)
# Load the runner without running its mutants: its loop runs only the numbers on argv, so give it one that matches none.
argv, sys.argv = sys.argv, [sys.argv[0], '-1']
spec.loader.exec_module(rest)
sys.argv = argv

GQL = 'crates/api/src/graphql.rs'
FEED = 'crates/git/src/feed.rs'
SURF = ['cargo', '-p', 'graftyard-api', '--test', 'surfaces', 'ready_for_review']
GATES = ['cargo', '-p', 'graftyard-git', '--test', 'lease_gates', 'ring']

MUTANTS = [
    ('gql: addLabelsToLabelable not routed by labelableId', GQL,
     '["repositoryId", "subjectId", "pullRequestId", "labelableId", "assignableId"]',
     '["repositoryId", "subjectId", "pullRequestId", "assignableId"]',
     SURF),
    ('feed: a ring fed past a hole serves cursors from before it', FEED,
     '        if self.first.is_some() && index.0 > self.through.0 + 1 {',
     '        if false && self.first.is_some() && index.0 > self.through.0 + 1 {', GATES),
    ('feed: the ring restarts at the entry past the hole, not after it', FEED,
     '            self.first = Some(Index(index.0 + 1));', '            self.first = Some(index);', GATES),
]

only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if not only or n in only:
        rest.plant(n, name, path, old, new, args)
