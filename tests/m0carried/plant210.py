#!/usr/bin/env python3
"""BM-5 clause 1: planted bugs in the per-build `ReleaseTag` (`crates/release/src/group.rs`, `Upload::seq`;
`crates/release/src/ops.rs`, `version_tag` and `parse_version_tag`; `workers/site/src/host.rs`, `release_of`), the
leader's older-refuser rule (`crates/raft/src/member/leader.rs`, `classify_refusals`) and the edge's aimed
`FORCE_FORMAT` (`workers/edge/src/lib.rs`, `Config::push_format`). Plants one mutant at a time, runs the tests that
should catch it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from
the repository root:

    python3 tests/m0carried/plant210.py          # every mutant
    python3 tests/m0carried/plant210.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GROUP = 'crates/release/src/group.rs'
OPS = 'crates/release/src/ops.rs'
HOST = 'workers/site/src/host.rs'
LEADER = 'crates/raft/src/member/leader.rs'
EDGE = 'workers/edge/src/lib.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', 'bm5_a_release_s_seq']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'a_member_s_release_is_its_version_tag']
PUSH = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'push', 'bm5_an_unreadable_format']
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'bm5_a_forced_format_aimed']

MUTANTS = [
    ("seq is upload time: every tag takes the next place, so R1 re-uploaded after R2 outranks R2", OPS,
     'fold.uploads.get(&r.0).map_or(fold.uploads.len() as u32 + 1, |u| u.seq)',
     'fold.uploads.len() as u32 + 1', REL),
    ("seq 0 parses as a release", OPS,
     'seq.parse().ok().filter(|s| *s > 0)?', 'seq.parse().ok()?', REL),
    ("a malformed tag panics instead of falling back to RELEASE", HOST,
     'tag.and_then(graftyard_release::ops::parse_version_tag).map_or(RELEASE, |(seq, id)| ReleaseTag { id, seq })',
     'tag.map(|t| graftyard_release::ops::parse_version_tag(t).unwrap()).map_or(RELEASE, |(seq, id)| ReleaseTag { id, seq })',
     SITE),
    ("seq counts from the release id, so two sites on one release can differ", GROUP,
     '                    seq: self.uploads.len() as u32 + 1,\n',
     '                    seq: u.release.0[0] as u32 + 1,\n', REL),
    ("same-release refusers also hand off (`<=`)", LEADER,
     'r.release.seq < mine.seq', 'r.release.seq <= mine.seq', PUSH),
    ("FORCE_FORMAT_SITE is ignored: every push is forced", EDGE,
     'self.force_format.filter(|_| self.force_site.is_none_or(|s| leader == Some(s)))',
     'self.force_format', EDGET),
]


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
