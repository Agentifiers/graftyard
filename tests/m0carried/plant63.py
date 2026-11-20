#!/usr/bin/env python3
"""retire-sticks: planted bugs in a retired member's refusal, a failed promotion's backoff, and the sweep's skip of a gone chain
(`decisions/DECISIONS.md`, "`retire-sticks`"). Plants one mutant at a time, runs the test that guards it, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root; the bed's
mutants need tests/m0's wrangler (WRANGLER= to point at another checkout's):

    python3 tests/m0carried/plant63.py          # every mutant
    python3 tests/m0carried/plant63.py 0 5      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LA = 'crates/git/src/log_archive.rs'
RT = 'workers/site/src/retire.rs'
SW = 'crates/git/src/sweep.rs'
ARCHIVE = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'log_archive', '--',
           'a_promotion_that_keeps_failing_backs_off_and_starts_no_stopped_shard_again']
BED = ['bash', 'tests/retirebed/run.sh']
SWEEP = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', 'gone_chain']

MUTANTS = [
    # Main's alarm: every step tries the failed range again, as a client call.
    ('a failed range is retried at once', LA,
     '            Some(f) if now < f.at.saturating_add(backoff(f.times)) => Retry::Wait,\n', '', ARCHIVE),
    ('a retry past its backoff starts stopped shards', LA,
     '            Some(_) => Retry::Warm,\n', '            Some(_) => Retry::Start,\n', ARCHIVE),
    ('the backoff never grows', LA,
     '    RETRY_FIRST_MS.saturating_mul(1 << times.saturating_sub(1).min(16)).min(RETRY_MAX_MS)\n',
     '    RETRY_FIRST_MS.min(RETRY_MAX_MS + times as u64 * 0)\n', ARCHIVE),
    ('a warm retry resets the restart clock, so a failed range never starts a shard again', LA,
     '            started: if started { now } else { last.map_or(0, |f| f.started) },\n',
     '            started: now,\n', ARCHIVE),
    ('an idle step forgets the failure', LA,
     '        Ok(Step::Idle) => *run = Run { failed: run.failed, ..Run::default() },\n',
     '        Ok(Step::Idle) => *run = Run::default(),\n', ARCHIVE),
    # The sweep fills and marks a gone chain's packs, as main did for 1048577 and 3473415.
    ('a gone chain is filled', SW,
     '    if !gone.is_empty() {\n        plan.actions.retain', '    if false {\n        plan.actions.retain', SWEEP),
    ('a gone chain whose lookup failed this run is filled', SW,
     '        if (self.clock)() >= self.deadline {\n            return false;\n        }\n        self.inner.gone(chain).await\n',
     '        if (self.clock)() >= self.deadline || self.dead.borrow().contains(&chain) {\n            return false;\n'
     '        }\n        self.inner.gone(chain).await\n', SWEEP),
    # Main's retire: storage deleted, nothing kept, so the next `/join` builds the group again.
    ('a retire keeps no row', RT,
     '    if !reuse {\n', '    if reuse && !reuse {\n', BED),
    ('a retired member joins', RT,
     '    retired.map(|at| json!({ "retired": at }))\n', '    retired.and(None).map(|at: u64| json!({ "retired": at }))\n',
     BED),
    ('a placement try\'s clear also refuses its names', RT,
     '    if !reuse {\n', '    if true {\n', BED),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    env = dict(os.environ)
    env.setdefault('WRANGLER', os.path.join(ROOT, 'tests/m0/node_modules/.bin/wrangler'))
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900, env=env)
            out = r.stdout + r.stderr
        finally:
            open(p, 'wb').write(orig)
        if r.returncode == 0:
            verdict = 'MISSED'
        elif 'error[E' in out or 'could not compile' in out:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT'
            caught += 1
        print(f'{i} {verdict}: {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
