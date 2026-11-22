#!/usr/bin/env python3
"""M5-WP3: planted bugs in `graft-ops`'s executor (`crates/release/src/ops.rs`), the `Release` group's rules it leans on
(`crates/release/src/group.rs`), and `graftyard release roll` (`cli/src/release.rs`; `decisions/DECISIONS.md`, "M5-WP3:
`graft-ops` executes intents, as built"). Plants one mutant at a time, runs the tests that guard it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant82.py          # every mutant
    python3 tests/m0carried/plant82.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
OPS = 'crates/release/src/ops.rs'
GROUP = 'crates/release/src/group.rs'
CLI = 'cli/src/release.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib']
ROLL = ['cargo', 'test', '-q', '-p', 'graftyard', '--', 'release_roll']

MUTANTS = [
    ("FormatsEnable waits for no site's report", GROUP,
     '        if let Some(s) = g.sites.iter().find(|s| !reads(self.site_release.get(&s.id.0))) {',
     '        if let Some(s) = g.sites.iter().filter(|s| !s.spare).find(|s| !reads(self.site_release.get(&s.id.0))) {',
     REL),
    ("FormatsEnable waits for no lifeboat", GROUP,
     '        if let Some(h) = self.lifeboats.keys().find(|h| !reads(self.lifeboat_release.get(*h))) {',
     '        if let Some(h) = self.lifeboats.keys().find(|_| false) {', REL),
    ("an Abort leaves the pinned script's deployment as the roll left it", GROUP,
     '                    self.deployed.insert(s, (Some(r), target));', '                    let _ = (s, r, target);', REL),
    ("an undo or Abort records the intent's release as the one the script runs", GROUP,
     '                        IntentState::Aborting { target } => moved.push((i.script(), containers(i), target)),',
     '                        IntentState::Aborting { .. } => {}', REL),
    ("a bundle file the record does not list is uploaded", OPS,
     '        if listed.get(name.as_str()) != Some(&want.as_str()) {',
     '        if listed.contains_key(name.as_str()) && listed.get(name.as_str()) != Some(&want.as_str()) {', REL),
    ("a bundle missing a listed file is uploaded", OPS,
     '    if let Some(missing) = listed.keys().find(|n| !files.iter().any(|(f, _)| f == *n)) {',
     '    if let Some(missing) = listed.keys().find(|_| false) {', REL),
    ("a restart uploads a script again", OPS,
     '    if let Some(s) = c.scripts.keys().find(|s| !tagged.contains_key(s)) {',
     '    if let Some(s) = c.scripts.keys().find(|s| tagged.len() < 99 || !tagged.contains_key(s)) {', REL),
    ("a deploy that is already the deployment is sent again", OPS,
     '            if !same(have, split) {\n                return Action::Deploy',
     '            if true {\n                return Action::Deploy', REL),
    ("the dwell runs from the step's last entry, so a restart after the deploy shortens nothing but a late deploy dwells less",
     OPS, 'let until = Timestamp(c.since.0.max(dep.created.0).saturating_add(*dwell));',
     'let until = Timestamp(c.since.0.saturating_add(*dwell));', REL),
    ("a gate breach waits instead of running the undo", OPS,
     '            if *gated && let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);',
     '            if *gated && let Gate::Breach(_) = &w.gate {\n                return Action::Wait { until: None, why: "breach".into() };', REL),
    ("the first day is not gated", OPS,
     '            if let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);\n            }\n            let until',
     '            let until', REL),
    ("a pinned script's 100% drops N, so its pointer undo can never apply", OPS,
     '                    Some(n) if p < 100 || pinned => vec![(v, p), (n, 100 - p)],',
     '                    Some(n) if p < 100 => vec![(v, p), (n, 100 - p)],', REL),
    ("a site's 100% commits no SiteRelease", OPS,
     '                && fold.site_release.get(&s.0) != Some(release)\n',
     '                && fold.site_release.get(&s.0) != Some(release)\n                && false\n', REL),
    ("an undrain of a site drained by an intent key is signed by the drain key", OPS,
     '            Some(d) if d.by_drain_key => Action::Commit {', '            Some(_) => Action::Commit {', REL),
    ("release roll puts the spares before the member sites", OPS,
     'let sites = g.sites.iter().filter(|s| !s.spare).chain(g.sites.iter().filter(|s| s.spare));',
     'let sites = g.sites.iter().filter(|s| s.spare).chain(g.sites.iter().filter(|s| !s.spare));', REL),
    ("release roll skips a script whose release lists none of its files", OPS,
     '            && !mine.is_empty()\n', '', REL),
    ("release roll's site day carries no Rollback", OPS,
     '            (Some(n), Intent::SiteDay { target, .. }) => vec![ReleaseEntry::Rollback { target: *target, release: n }],',
     '            (Some(_), Intent::SiteDay { .. }) => Vec::new(),', REL),
    ("FormatsEnable's refusal and a refused undo below a format do not page", 'crates/release/src/lib.rs',
     '    matches!(variant, "UnreadableFormat" | "DrainGuard")', '    matches!(variant, "DrainGuard")', REL),
    ("release roll signs from entries it did not check against the anchor", CLI,
     '    r.extend(&t.anchor()?, 0, &entries)', '    r.extend(&graftyard_release::Anchor { customer: graftyard_release::public_key(&[0; 32]), ..t.anchor()? }, 0, &entries)',
     ROLL),
    # The host's run loop (`ops::run`), guarded by crates/release/src/tests/group/run.rs.
    ("a run stops at the first 409 instead of reading the view again", OPS,
     '        Submit::Stale => {\n            log.push(Ran::Stale(kind));\n            true',
     '        Submit::Stale => {\n            log.push(Ran::Stale(kind));\n            false', REL),
    ("a refusal the group pages on writes no page", OPS,
     '            if crate::pages(&variant) {\n                let why', '            if false {\n                let why', REL),
    ("a refused commit lets the run go on", OPS,
     '            log.push(Ran::Refused { kind, variant });\n            false',
     '            log.push(Ran::Refused { kind, variant });\n            true', REL),
    ("a bundle that fails its checks is uploaded anyway", OPS,
     '    let c = check_bundle(fold, &b.record, b.sig, &b.files, &e.scripts())\n        .map_err(|r| Ran::Error(format!("release bundle refused: {r}").into()))?;',
     '    let c = match check_bundle(fold, &b.record, b.sig, &b.files, &e.scripts()) {\n        Ok(c) => c,\n        Err(_) => { let k = fold.keys.clone().unwrap(); let mut f2 = fold.clone(); f2.keys = Some(crate::Keys { release: crate::public_key(&[8; 32]), ..k }); check_bundle(&f2, &b.record, b.sig, &b.files, &e.scripts()).map_err(|r| Ran::Error(format!("{r}").into()))? }\n    };', REL),
    ("a refused bundle ends the run before the intents", OPS,
     '                Ok(Some(r)) | Err(r) => {\n                    log.push(r);\n                    uploads = false;\n                }',
     '                Ok(Some(r)) | Err(r) => {\n                    log.push(r);\n                    break;\n                }', REL),
    ("a breach waits for a deployment read, so no undo goes in while the API is down", OPS,
     '            if *gated && let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);\n            }\n            let Some(dep) = w.deployments.get(script) else {\n                return Action::Wait { until: None, why: format!("no deployment of {script:?} read").into() };\n            };',
     '            let Some(dep) = w.deployments.get(script) else {\n                return Action::Wait { until: None, why: format!("no deployment of {script:?} read").into() };\n            };\n            if *gated && let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);\n            }', REL),
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
