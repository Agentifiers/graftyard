#!/usr/bin/env python3
"""chain-site and hit-gate: planted bugs in a composable miss's chain wait on a site that holds no chain
(crates/git/src/compose.rs, `chain_ready`; decisions/DECISIONS.md, "`chain-site`") and in a clone-cache hit's lookup
beside its read gate and its manifest line read from chunks (`beside_gate`, `LineSplit`; "`hit-gate`"). Plants one
mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or
DID NOT COMPILE. Mutant 0 of each change is main's rule. Run from the worktree root:

    python3 tests/hitgate/plant.py             # every mutant
    python3 tests/hitgate/plant.py 5 6         # by number
    python3 tests/hitgate/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
COMPOSE = 'crates/git/src/compose.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'chain', 'a_miss_on', 'a_hits_lookup',
     'a_manifest_line']
BUILD = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--no-run']
ENV = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')),
           PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')

WAIT = """    let wait = match (chain_site, follows) {
        (false, _) => std::time::Duration::ZERO,
        (true, true) if !holds => std::time::Duration::ZERO,
        (true, true) => left.min(CHAIN_WAIT_FORWARDED),
        (true, false) => left,
    };"""
BESIDE = """    use futures_util::future::{Either, select};
    match select(std::pin::pin!(gate), std::pin::pin!(lookup)).await {
        Either::Left((g, l)) => {
            if allows(&g) {
                let v = l.await;
                (g, Some(v))
            } else {
                (g, None)
            }
        }
        Either::Right((v, g)) => {
            let g = g.await;
            let ok = allows(&g);
            (g, ok.then_some(v))
        }
    }"""
MUTANTS = [
    # chain-site
    ("chain-site: main's rule, every site waits as a chain site", COMPOSE, WAIT,
     "    let _ = (chain_site, holds);\n    let wait = if follows { left.min(CHAIN_WAIT_FORWARDED) } else { left };"),
    ('chain-site: a site with no chain waits the forwarded bound', COMPOSE,
     '        (false, _) => std::time::Duration::ZERO,', '        (false, _) => left.min(CHAIN_WAIT_FORWARDED),'),
    ('chain-site: the last site with no chain answers warming, not its container', COMPOSE,
     'futures_util::future::Either::Right(_) => !chain_site && !follows,', 'futures_util::future::Either::Right(_) => false,'),
    ('chain-site: a forwarded miss with no chain goes to its container', COMPOSE,
     'futures_util::future::Either::Right(_) => !chain_site && !follows,',
     'futures_util::future::Either::Right(_) => !chain_site,'),
    ('chain-site: a chain site counted as none', COMPOSE,
     '        (true, false) => left,\n    };', '        (true, false) => left,\n    };\n    let chain_site = !chain_site;'),
    # hit-gate
    ("hit-gate: main's rule, the lookup after the gate", COMPOSE, BESIDE,
     """    let g = gate.await;
    if !allows(&g) {
        return (g, None);
    }
    let v = lookup.await;
    (g, Some(v))"""),
    ('hit-gate: a lookup kept though the gate refused after it', COMPOSE, '            (g, ok.then_some(v))',
     '            let _ = ok;\n            (g, Some(v))'),
    ('hit-gate: a refused gate waits for the lookup to end', COMPOSE,
     '            } else {\n                (g, None)\n            }',
     '            } else {\n                let _ = l.await;\n                (g, None)\n            }'),
    ('hit-gate: the line split keeps the tail byte after the newline', COMPOSE,
     '                LineStep::Line(line, i + 1)', '                LineStep::Line(line, i)'),
    ('hit-gate: no MANIFEST_MAX across chunks', COMPOSE,
     '            None if self.held.len() + chunk.len() >= MANIFEST_MAX => LineStep::TooLong,', ''),
    ('hit-gate: a line split forgets the bytes held before its newline', COMPOSE,
     '                let mut line = std::mem::take(&mut self.held);', '                let mut line = Vec::new();'),
]


def residue():
    out = []
    for n, (name, f, old, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, f)).read().count(old)
        if c != 1:
            out.append(f'{n} {name}: anchor found {c} times')
    return out


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    only = {int(a) for a in sys.argv[1:]}
    caught = 0
    for i, (name, f, old, new) in enumerate(MUTANTS):
        if only and i not in only:
            continue
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            # A test that hangs on a mutant (the virtual clock never reaches a future the mutant waits on) is caught:
            # the rule under test never ends (README, "Planted bugs"). The build runs first, so only the test is timed.
            subprocess.run(['timeout', '-k', '10', '600', *BUILD], cwd=ROOT, env=ENV, capture_output=True, text=True)
            r = subprocess.run(['timeout', '-k', '10', '600', *T], cwd=ROOT, env=ENV, capture_output=True, text=True)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or ('could not compile' in out and 'test failed' not in out):
            print(f'{i} DID NOT COMPILE  {name}', flush=True)
        elif r.returncode != 0:
            caught += 1
            print(f'{i} CAUGHT  {name}' + ('  (timed out)' if r.returncode == 124 else ''), flush=True)
        else:
            print(f'{i} MISSED  {name}', flush=True)
    print(f'{caught} of {len(MUTANTS)} caught')


if __name__ == '__main__':
    main()
