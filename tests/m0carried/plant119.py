#!/usr/bin/env python3
"""M6-WP3, the lifeboat's pure half: planted bugs in the importer's chain check (`crates/raft/src/import.rs`), the
lifeboat member's route rule (`workers/site/src/member.rs`, `lifeboat_route`), lifeboat request signing
(`crates/identity/src/lifeboat.rs`), the 7-day silence alarm (`crates/release/src/ops.rs`, `silent_lifeboats`), the
puller (`lifeboat/src/puller.rs`), and `graft-blob` (`lifeboat/src/blob.rs`). Plants one mutant at a time, runs the
tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the repository root:

    python3 tests/m0carried/plant119.py          # every mutant
    python3 tests/m0carried/plant119.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
IMP = 'crates/raft/src/import.rs'
MEM = 'workers/site/src/member.rs'
AUTH = 'crates/identity/src/lifeboat.rs'
OPS = 'crates/release/src/ops.rs'
PULL = 'lifeboat/src/puller.rs'
BLOB = 'lifeboat/src/blob.rs'
IMP_T = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'import']
MEM_T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'lifeboat']
AUTH_T = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--lib', '--', 'lifeboat']
OPS_T = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', '--', 'lifeboat']
PULL_T = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat', '--test', 'puller']
BLOB_T = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat', '--test', 'blob']

MUTANTS = [
    ("the importer trusts a row's prev, as a release build's member does at or below commit", IMP,
     "            if e.prev != prev {\n",
     "            if false && e.prev != prev {\n", IMP_T),
    ("the importer takes a row that differs from one the copy holds", IMP,
     "                if held.is_some_and(|h| h != d) {\n",
     "                if false && held.is_some_and(|h| h != d) {\n", IMP_T),
    ("the importer goes on past a refused row", IMP,
     "            halt = Some(Halt::Refused(r));\n",
     "            let _ = r;\n", IMP_T),
    ("a lifeboat member's /read takes the read-index path", MEM,
     '        "/read" => Lifeboat::Stale,\n',
     '        "/read" => Lifeboat::Pass,\n', MEM_T),
    ("a lifeboat member lets writes through", MEM,
     "        _ => Lifeboat::Refuse,\n",
     "        _ => Lifeboat::Pass,\n", MEM_T),
    ("a signed request's time is not checked", AUTH,
     "    if at.0.abs_diff(now.0) > CLOCK_SKEW.as_millis() as u64 {\n",
     "    if false && at.0.abs_diff(now.0) > CLOCK_SKEW.as_millis() as u64 {\n", AUTH_T),
    ("the signed message leaves out the query", AUTH,
     "    for part in [host.as_bytes(), &at.0.to_be_bytes(), method.as_bytes(), path_and_query.as_bytes()] {\n",
     "    for part in [host.as_bytes(), &at.0.to_be_bytes(), method.as_bytes(), path_and_query.split('?').next().unwrap().as_bytes()] {\n",
     AUTH_T),
    ("a decoder entry does not reset the silence clock", 'crates/release/src/group.rs',
     "                self.lifeboat_seen.insert(host.clone(), at);\n                self.lifeboat_release.insert(host, release);\n",
     "                self.lifeboat_release.insert(host, release);\n", OPS_T),
    ("the silence alarm pages at exactly 7 days", OPS,
     "now.0.saturating_sub(t.0) > silent",
     "now.0.saturating_sub(t.0) >= silent", OPS_T),
    ("the puller stitches a feed gap instead of pulling it from the edge", PULL,
     "            if first.0 > next.0 {\n",
     "            if false && first.0 > next.0 {\n", PULL_T),
    ("the puller moves a halted chain from the feed", PULL,
     "            if self.halted.contains(c) {\n                continue;\n            }\n",
     "", PULL_T),
    ("graft-blob answers before the directory fsync", BLOB,
     "        Err(e) => return Err(e.into()),\n    }\n    fs.fsync_dir(dir_of(key))?;\n",
     "        Err(e) => return Err(e.into()),\n    }\n", BLOB_T),
    ("graft-blob links before the temp file's fsync", BLOB,
     "    fs.write(&tmp, bytes)?;\n    fs.fsync(&tmp)?;\n    let linked = fs.link(&tmp, key);\n",
     "    fs.write(&tmp, bytes)?;\n    let linked = fs.link(&tmp, key);\n    fs.fsync(&tmp)?;\n", BLOB_T),
]


def main():
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
        print(f'{i:2} {verdict}  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')
    sys.exit(0 if caught == len(list(pick)) else 1)


if __name__ == '__main__':
    main()
