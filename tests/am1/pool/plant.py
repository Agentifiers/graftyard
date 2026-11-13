#!/usr/bin/env python3
"""pool-grants: planted bugs in the per-owner pool budget (the `PoolLedger`, `pool_ask`, `granted_k`, and the slots
`shards_for` serves). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/am1/pool/plant.py          # every mutant
    python3 tests/am1/pool/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
SH = 'crates/git/src/shard.rs'
TESTS = ['-p', 'graftyard-git', '--test', 'agent_load', '--test', 'shard']

MUTANTS = [
    ('ledger: an ask ignores the owner\'s share', SH,
     '        let g = usize::from(*pool_grants(&asks, share).last().expect("one ask"));',
     '        let g = usize::from(*pool_grants(&asks, |_| HOT_POOL).last().expect("one ask"));', TESTS),
    ('ledger: other leases are not kept first (the asker listed first)', SH,
     '        asks.push((network, owner, held.len() as u8, want.min(FANOUT_MAX - 1)));',
     '        asks.insert(0, (network, owner, held.len() as u8, want.min(FANOUT_MAX - 1)));', TESTS),
    ('ledger: new slots may be ones another network holds', SH,
     '.chain(slots.iter().copied()).collect();',
     '.take(0).chain(slots.iter().copied()).collect();', TESTS),
    ('ledger: leases never expire', SH,
     '        self.leases.retain(|l| now.0 < l.at.0 + LOAD_STALE.as_millis() as u64);', '', TESTS),
    ('ledger: an ask does not renew the asker\'s lease', SH,
     '            self.leases.push(PoolLease { network, owner, slots: slots.clone(), at: now });',
     '            self.leases.push(PoolLease { network, owner, slots: slots.clone(), at: mine.map_or(now, |l| l.at) });', TESTS),
    ('ledger: a renewal grows nothing but also renews nothing', SH,
     '            Some(l) => {\n                l.at = now;\n                l.slots.clone()',
     '            Some(l) => {\n                l.slots.clone()', TESTS),
    ('ledger: its own silence counts against leases', SH,
     '        if gap > LEDGER_GAP.as_millis() as u64 {', '        if gap > u64::MAX / 2 {', TESTS),
    ('ledger: an expired network re-asks from what its leader holds', SH,
     '        let held = mine.as_ref().map_or(Vec::new(), |l| l.slots.clone());',
     '        let held = mine.as_ref().map_or((0..want.min(FANOUT_MAX - 1) as u16).collect(), |l| l.slots.clone());', TESTS),
    ('ledger: the heartbeat does not run the ledger (m21: torn-down leases never lapse)', SH,
     '    pub fn beat(&mut self, now: Timestamp) {\n        self.settle(now);\n        self.seen = Some(now);\n    }',
     '    pub fn beat(&mut self, now: Timestamp) {\n        let _ = now;\n    }', TESTS),
    ('ledger: the heartbeat marks the time but drops nothing', SH,
     '    pub fn beat(&mut self, now: Timestamp) {\n        self.settle(now);\n        self.seen = Some(now);\n    }',
     '    pub fn beat(&mut self, now: Timestamp) {\n        self.seen = Some(now);\n    }', TESTS),
    ('ledger: no heartbeat is armed while leases are held', SH,
     '        (!self.leases.is_empty()).then(|| Timestamp(now.0 + LEDGER_BEAT.as_millis() as u64))',
     '        let _ = now;\n        None', TESTS),
    ('ledger: the heartbeat runs past LEDGER_GAP, so a quiet ledger reads as down', SH,
     'pub const LEDGER_BEAT: Duration = Duration::from_secs(45);', 'pub const LEDGER_BEAT: Duration = Duration::from_secs(120);', TESTS),
    ('leader: the larger grant bounds a step up', SH,
     '        [Some(a), Some(b)] if want > k => (1 + a.min(b)).clamp(k, want),',
     '        [Some(a), Some(b)] if want > k => (1 + a.max(b)).clamp(k, want),', TESTS),
    ('leader: an unreachable ledger counts as a grant of nothing', SH,
     '        _ => k,\n    }\n}', '        _ => 1,\n    }\n}', TESTS),
    ('leader: a short grant steps k down', SH,
     '        [Some(a), Some(b)] if want > k => (1 + a.min(b)).clamp(k, want),\n        [Some(_), Some(_)] => want,',
     '        [Some(a), Some(b)] => (1 + a.min(b)).min(want),', TESTS),
    ('leader: a held k is never renewed', SH,
     '    (want > 1 || k > 1).then_some(want - 1)', '    (want > k).then_some(want - 1)', TESTS),
    ('shards_for: pool shards by rendezvous, not the granted slots', SH,
     '        let s = GitCacheShard { site, shard: count + p };',
     '        let s = GitCacheShard { site, shard: by_weight(&key, count..count + HOT_POOL)[usize::from(*p) % usize::from(HOT_POOL)] };', TESTS),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target')),
           CARGO_INCREMENTAL='0')
only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if only and n not in only:
        continue
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    text = orig.decode()
    if text.count(old) != 1:
        print(f'{n} BAD PATTERN ({text.count(old)} matches): {name}', flush=True)
        continue
    try:
        open(f, 'w').write(text.replace(old, new))
        r = subprocess.run(['cargo', 'test', '--no-run', *args], cwd=ROOT, env=env, capture_output=True, text=True)
        if r.returncode != 0:
            print(f'{n} DID NOT COMPILE: {name}\n{r.stderr[-1500:]}', flush=True)
            continue
        r = subprocess.run(['cargo', 'test', *args], cwd=ROOT, env=env, capture_output=True, text=True)
        failed = [l.split()[1] for l in r.stdout.splitlines() if l.startswith('test ') and l.endswith('FAILED')]
        print(f'{n} {"CAUGHT" if r.returncode != 0 else "MISSED"}: {name}' + (f' ({", ".join(failed)})' if failed else ''),
              flush=True)
    finally:
        open(f, 'wb').write(orig)
