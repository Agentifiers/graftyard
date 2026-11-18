#!/usr/bin/env python3
"""b032-shed: planted bugs in the forgotten-leader read shed and the agent push bound (`decisions/DECISIONS.md`,
"`b032-shed`"). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant38.py          # every mutant
    python3 tests/m0carried/plant38.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SITE = 'workers/site/src/member.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
LEADER = 'crates/raft/src/member/leader.rs'
EDGE = 'workers/edge/src/lib.rs'
SITE_T = ['cargo', 'test', '-q', '--manifest-path', 'workers/site/Cargo.toml', '--', 'a_read_index_read_on_a_member']
LOOKUP_T = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--', 'a_shed_that_names_a_forgotten_leader']
ADMIT_T = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'admission', '--', 'budget', 'a_command_appended']
EDGE_T = ['cargo', 'test', '-q', '--manifest-path', 'workers/edge/Cargo.toml', '--', 'an_agent_push_past_its_bound']

MUTANTS = [
    ('as on main: a forgotten leader answers notLeader', SITE,
     '        Some(f) if index && nl.leader.is_none() => {', '        Some(f) if false && index && f.0 < 255 => {', SITE_T),
    ('the shed names no forgotten leader, so nothing is reported', LOOKUP,
     '            let f = r.as_ref()?.1.get("forgot")?;', '            let f = r.as_ref()?.1.get("forgotten")?;', LOOKUP_T),
    ('the budget is ignored at admission', LEADER,
     '        if agent && a.budget.is_some_and(|b| expected_wait(ad.agent().len(), cap, ad.round_ms) > b) {',
     '        if agent && a.budget.is_some_and(|_| false) {', ADMIT_T),
    ('the budget is ignored once queued (expiry at AGENT_WAIT)', LEADER,
     '                    Some(b) if class == AdmitClass::Agent => b.min(AGENT_WAIT),',
     '                    Some(_) if class == AdmitClass::Agent => AGENT_WAIT,', ADMIT_T),
    ('a command in the entry in flight is answered Full 1 s after its cut', LEADER,
     '        let old = ad.lanes[AdmitClass::Agent.lane()].take_old(|q| q.until < now);',
     '        let old = ad.lanes[AdmitClass::Agent.lane()].take_old(|q| q.until < now);\n'
     '        let late: Vec<u64> = lead.pending.as_ref().map(|(_, d)| d.iter().map(|x| x.0).collect()).unwrap_or_default();\n'
     '        if old.is_empty() && !late.is_empty() && now.0 > lead.admission.cut_at.0 + 1_000 {\n'
     '            for seq in late {\n'
     '                self.resolve(seq, None, 0, Err(CommitError::Full { retry_after_s: 1 }));\n'
     '            }\n'
     '            return;\n'
     '        }\n'
     '        let ad = &mut self.lead.as_mut().expect("leading").admission;', ADMIT_T),
    ('the edge sends a push past its bound', EDGE,
     '        Some(l) if agent && l.is_zero() => Err(Refusal::agent_full(1)),',
     '        Some(l) if agent && l.is_zero() => Ok(Some(0)),', EDGE_T),
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
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
