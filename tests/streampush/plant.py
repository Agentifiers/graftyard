#!/usr/bin/env python3
"""stream-push: planted bugs in the edge's streamed receive-pack (`graftyard_git::smart::BodyReader`, the cap count on
a streamed body's rest, and `PackStore::place_streamed`). Plants one mutant at a time, runs the tests that gate it,
and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/streampush/plant.py          # every mutant
    python3 tests/streampush/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SMART = 'crates/git/src/smart.rs'
STORE = 'crates/git/src/store.rs'
EDGE_LIB = 'workers/edge/src/lib.rs'
SMART_T = ['cargo', '-p', 'graftyard-git', '--lib', 'smart::']
STORE_T = ['cargo', '-p', 'graftyard-git', '--lib', 'store::']
EDGE_T = ['cargo', '-p', 'graftyard-edge', '--lib', 'tests::']

MUTANTS = [
    ('head: the edge never streams, it buffers every body to its end (the code before this branch)', SMART,
     '            Some(rp) if self.buf.len() - rp.pack_at > INLINE_MAX => Read::Stream,',
     '            Some(rp) if self.buf.len() - rp.pack_at > usize::MAX - 1 => Read::Stream,', SMART_T),
    ('head: a pack of exactly INLINE_MAX streams, so an inline pack is placed as a stored one', SMART,
     '            Some(rp) if self.buf.len() - rp.pack_at > INLINE_MAX => Read::Stream,',
     '            Some(rp) if self.buf.len() - rp.pack_at >= INLINE_MAX => Read::Stream,', SMART_T),
    ('head: the body streams as soon as its commands end, so small packs are not inline', SMART,
     '            Some(rp) if self.buf.len() - rp.pack_at > INLINE_MAX => Read::Stream,',
     '            Some(rp) if self.buf.len() > rp.pack_at => Read::Stream,', SMART_T),
    ('head: a long body with bad commands is held whole', SMART,
     '                Err(e) if self.buf.len() > INLINE_MAX => {', '                Err(e) if false => {', SMART_T),
    ('head: a short body with bad commands is answered before it ends (the probe\'s bare flush included)', SMART,
     '                Err(e) if self.buf.len() > INLINE_MAX => {', '                Err(e) if true => {', SMART_T),
    ('head: the cap is off by one', SMART, '        if self.n > self.cap {', '        if self.n >= self.cap {',
     SMART_T),
    ('head: the cap counts only the bytes it holds, not a bad body it stopped holding', SMART,
     '        if self.n > self.cap {', '        if self.buf.len() as u64 + c.len() as u64 > self.cap {', SMART_T),
    ('head: the gzip reader parses, so a long gzip body streams undecoded', SMART,
     '        BodyReader { next_try: usize::MAX, ..BodyReader::new(cap) }', '        BodyReader::new(cap)', SMART_T),
    ('edge: a streamed body\'s rest is counted from zero, not from the head the edge read', EDGE_LIB,
     '    let mut n = read;', '    let mut n = 0 * read;', EDGE_T),
    ('tee: a third part buffer, so memory grows with the pack while a slow bucket lags', STORE,
     '                        None if made < 2 => {', '                        None if made < 3 => {', STORE_T),
    ('tee: the SHA-256 check is skipped, so validate\'s index for other bytes seals the pack', STORE,
     '            if ix.checksum != checksum || ix.bytes != n {', '            if ix.bytes != n {', STORE_T),
    ('tee: the length check is skipped', STORE,
     '            if ix.checksum != checksum || ix.bytes != n {', '            if ix.checksum != checksum {', STORE_T),
    ('tee: a bucket whose part put failed stays live', STORE,
     '                Err(StoreError(e)) => ups.borrow_mut()[i] = Upload::Out { why: e, open: Some(id) },',
     '                Err(StoreError(_)) => {}', STORE_T),
    ('tee: the uploads left open when the reader ends are not aborted', STORE,
     '            drop(puts);\n            self.abort_open(ups, key).await;', '            drop(puts);', STORE_T),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.path.join(ROOT, '.worktrees/target-rows'))


def run(t):
    _, _, pkg, kind, name = t
    args = ['cargo', 'test', '-q', '-p', pkg, kind, name]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
