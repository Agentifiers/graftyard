#!/usr/bin/env python3
"""R22: planted bugs in the sweeper's fill (`crates/git/src/sweep.rs`, `fill`) and the in-memory bucket's
`copy_new` (`crates/git/src/mem.rs`). Plants one mutant at a time, runs the sweeper's tests, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. `host::R2::copy_new` runs only in a Worker; its
checks were run in local workerd (`tests/am1/results/r22-sweep-fill.md`). Run from the worktree root:

    python3 tests/r22/plant.py          # every mutant
    python3 tests/r22/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SW = 'crates/git/src/sweep.rs'
MEM = 'crates/git/src/mem.rs'
SWEEP = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', 'sweep::']

FILL = '''    to.copy_new(from, &key, Some((m.bytes, m.checksum)))
        .await?
        .ok_or_else(|| StoreError(format!("{key} vanished from the source")))?;'''

MUTANTS = [
    ('fill reads the pack whole into the caller and puts it, as main did', SW, FILL,
     '''    use sha2::{Digest as _, Sha256};
    let body = from.get(&key).await?.ok_or_else(|| StoreError(format!("{key} vanished from the source")))?;
    let sum = graftyard_types::Digest(Sha256::digest(&body).into());
    if sum != m.checksum || body.len() as u64 != m.bytes {
        return Err(StoreError(format!("{key}: source copy does not match its marker")));
    }
    let meta = from.head(&key).await?.map(|o| o.meta).unwrap_or_default();
    to.put_new(&key, &body, sum, &meta).await?;'''),
    ('fill copies without the marker\'s size and checksum', SW, 'Some((m.bytes, m.checksum))', 'None'),
    ('the copy checks the size but not the checksum', MEM,
     '(body.len() as u64 != n || Digest(Sha256::digest(&body).into()) != sum)', '(body.len() as u64 != n && sum == sum)'),
    ('fill writes the marker even when the copy failed', SW, FILL,
     '    let _ = to.copy_new(from, &key, Some((m.bytes, m.checksum))).await;'),
    ('fill skips the stored .idx', SW,
     '    let _ = to.copy_new(from, &R2Key::Idx { network, id }.to_string(), None).await;\n', ''),
    ('the copy drops the source\'s custom metadata', MEM,
     '        self.plant(key, &body, Timestamp(self.now.get()), &info.meta);\n        Ok(Some(PutNew::Created))',
     '        let _ = info;\n        self.plant(key, &body, Timestamp(self.now.get()), &[]);\n        Ok(Some(PutNew::Created))'),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(SWEEP, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
