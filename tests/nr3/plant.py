#!/usr/bin/env python3
"""narrow-reads-3: planted bugs in the follower-read floor (`crates/raft`: `ReadGrant::floor`, `Member::fresh_after`),
the member's narrow answer cache keyed by it (`workers/site/src/family.rs`, `host.rs`), the edge's genesis binding
(`crates/lookup`), a learner's entries ahead of its read-index answer, and F67 rule 12 (`crates/raft`). Plants one mutant at a time,
runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
The host's wiring of a waited read (`Host::answer_after` passing the grant's floor) runs only in a Durable Object; it
is covered hosted by `tests/nr3/fresh.mjs`. Run from the worktree root:

    python3 tests/nr3/plant.py          # every mutant
    python3 tests/nr3/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
INSTALL = 'crates/raft/src/member/install.rs'
LEADER = 'crates/raft/src/member/leader.rs'
FAM = 'workers/site/src/family.rs'
HOST = 'workers/site/src/host.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
ELECTION = 'crates/raft/src/member/election.rs'
RAFT = ('graftyard-raft', '--test', 'leadership')
LK = ('graftyard-lookup', '--lib', 'tests')
SITE = ('graftyard-site', '--lib', 'tests')
SCEN = ('graftyard-raft', '--test', 'scenarios')

MUTANTS = [
    ('raft: a follower grant\'s floor is its own applied index, not the leader\'s read index', INSTALL,
     '                let g = grant(self, i);', '                let g = grant(self, applied.min(i).min(Index(0)));', RAFT),
    ('raft: fresh_after answers after a restart', LEADER,
     '        if grant.boot != self.boot {\n            return Err(self.not_leader());\n        }\n        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })',
     '        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })', RAFT),
    ('raft: fresh_after answers at the grant\'s index, not the state now', LEADER,
     '        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })',
     '        Ok(Fresh { fold: &self.fold.fold, index: grant.index })', RAFT),
    ('cache: any cached answer serves, below the read\'s floor too', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(t, _, l, _)| t == text && *l == loaded)', SITE),
    ('cache: a floor one above the answer is still served (off by one)', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(t, at, l, _)| t == text && *at + 1 >= floor && *l == loaded)', SITE),
    ('cache: an answer built while the metadata tier loaded serves once it is loaded', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(t, at, _, _)| t == text && *at >= floor)', SITE),
    ('cache: another query\'s answer serves', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(_, at, l, _)| *at >= floor && *l == loaded)', SITE),
    ('cache: the answer is stamped with the floor, not the index it was built at', FAM,
     '    let at = graftyard_fold::meta(&*fold.core).applied.0;\n    let bytes',
     '    let at = floor.max(graftyard_fold::meta(&*fold.core).applied.0 + 1);\n    let bytes', SITE),
    ('spacing: a read whose floor the cache covers waits for a build', FAM,
     '    if narrow_hit(repo, &text, floor, fold.meta.loaded()).is_some() {\n        return None;\n    }', '', SITE),
    ('spacing: never spaced', FAM,
     '            Some(at) if now < at + NARROW_EVERY_MS => Some(at + NARROW_EVERY_MS - now),', '', SITE),
    ('shared: answers in one input are shared across floors', HOST,
     '    let key = format!("{floor} {q}");', '    let key = format!("{} {q}", floor * 0);', SITE),
    # Genesis on the edge (`crates/lookup`: `bind_genesis`, `settle_bound`).
    ('genesis: the binding is taken without accept_genesis (any install, any signer)', LOOKUP,
     '        graftyard_release::accept_genesis(anchor, &b)\n            .map_err(|e| format!("GRAFTYARD_GENESIS_SIGNED: {}: {e}", e.variant()))',
     '        graftyard_release::SignedEntry::decode(&b).map_err(|e| e.to_string()).and_then(|e| match e.body { graftyard_release::ReleaseEntry::Genesis(g) => Ok(*g), _ => Err("not a genesis".into()) }).map(|g| { let _ = anchor; g })', LK),
    ('genesis: a refused binding is tried again on the next request', LOOKUP,
     '        Err(e) => {\n            BOUND_OFF.with(|o| o.set(true));\n            Err(e)\n        }',
     '        Err(e) => Err(e),', LK),
    ('genesis: a different view never replaces the binding', LOOKUP,
     '    BOUND_OFF.with(|o| o.set(true));\n    GENESIS.with(|c| *c.borrow_mut() = Some(Held::View(view)));\n    true',
     '    let _ = view;\n    false', LK),
    ('genesis: an unreadable view drops the binding', LOOKUP,
     '    let Some(view) = view else { return false };',
     '    let Some(view) = view else {\n        GENESIS.with(|c| *c.borrow_mut() = None);\n        return true;\n    };', LK),
    # A learner's entries ride ahead of its read-index answer (`Member::append_for_read`).
    ('read entries: none sent before the answer', INSTALL,
     '                if let Some((i, _)) = index {\n                    self.append_for_read(to, i);\n                }\n', '', SCEN),
    ('read entries: sent after the calls in flight (f1ab1a61), not from the acknowledged entry', LEADER,
     '        if through < index {\n            self.send_append(peer, true);',
     '        if through < index {\n            self.send_append(peer, false);', SCEN),
    ('read entries: none while a call is out', LEADER,
     '        let through = lead.pipe.get(&peer).and_then(|p| p.last()).map_or(lead.matched(peer), |p| p.1);',
     '        let through = if lead.sent.contains_key(&peer) { index } else { lead.matched(peer) };', SCEN),
    ('read entries: sent only when two or more are missing (off by one)', LEADER,
     '        if through < index {\n            self.send_append(peer, true);',
     '        if through.next() < index {\n            self.send_append(peer, true);', SCEN),
    ('read entries: a lossy learner gets them too', LEADER,
     '        if quorum::is_voter(self.log_config(), peer) || self.link_lossy(peer) {\n            return;\n        }\n        let Some(lead) = self.lead.as_ref() else { return };\n        // Below',
     '        if quorum::is_voter(self.log_config(), peer) {\n            return;\n        }\n        let Some(lead) = self.lead.as_ref() else { return };\n        // Below', SCEN),
    # Liveness (F67 rule 12).
    ('raft: an emptied read-index batch stays in flight', INSTALL,
     '            if reads.is_empty() {\n                r.inflight = None;\n            }', '', RAFT),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def run(t):
    pkg, kind, name = t
    args = ['cargo', 'test', '-q', '-p', pkg, kind, name] if kind == '--test' else ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
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
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
