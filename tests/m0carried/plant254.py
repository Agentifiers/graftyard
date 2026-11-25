#!/usr/bin/env python3
"""BF-76's org substitution policy and the signed pin map (`crates/identity/src/authz.rs`,
`AuthzOp::SetActionsSubstitution`; `crates/actions/src/pin_map.rs`; `decisions/DECISIONS.md`, "`pin-map`"). The
mutants: a policy write that overwrites a field the request left out, a policy a user account may set, a repeat that writes again, a pin map approved
by digest but never checked for its signature, a signature checked after the approval so a bad signer reads as "not
approved", a fork that is another action's, an unapproved v4 SHA pin substituted anyway, a SHA no map names treated as
unapproved (which would annotate v3 pins), and tag substitution on a v3 tag. Run from the repository root:

    python3 tests/m0carried/plant254.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

AZ = 'crates/identity/src/authz.rs'
P = 'crates/actions/src/pin_map.rs'
TI = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'agents']
TP = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--lib', 'pin_map']
MUTANTS = [
    ("a policy write overwrites the field the request left out", AZ,
     'pin_map: pin_map.unwrap_or(was.pin_map) };', 'pin_map: pin_map.unwrap_or(None) };', TI),
    ("a user account takes an org's substitution policy", AZ,
     '            AuthzOp::SetActionsSubstitution { tags, pin_map } => {\n                org_only(s)?;',
     '            AuthzOp::SetActionsSubstitution { tags, pin_map } => {', TI),
    ("a repeat of the same policy writes another event", AZ,
     '                if now == was {\n                    return Ok((SmallVec::new(), AuthzOut::Done));\n                }\n'
     '                Ok((smallvec![AuthzEvent::ActionsSubstitution(now)]',
     '                Ok((smallvec![AuthzEvent::ActionsSubstitution(now)]', TI),
    ("a pin map's signature is never checked", P,
     '    key.verify_strict(doc, &Signature::from_bytes(sig)).map_err(|_| Refused::BadSignature)?;',
     '    let _ = (key, sig);', TP),
    ("the approval is checked before the signature", P,
     '    let key = VerifyingKey::from_bytes(release_key).map_err(|_| Refused::BadSignature)?;',
     '    if digest(doc) != approved {\n        return Err(Refused::NotApproved);\n    }\n'
     '    let key = VerifyingKey::from_bytes(release_key).map_err(|_| Refused::BadSignature)?;', TP),
    ("a pin's fork may be another action's", P,
     '        if upstream_repo != fork_repo {', '        if upstream_repo.is_empty() && fork_repo.is_empty() {', TP),
    ("an unapproved v4 SHA pin is substituted by the known map", P,
     '        return Substitution::Unapproved { fork, fork_sha: known_pin.map(|pin| pin.fork_sha.clone()) };',
     '        if let Some(p) = known_pin {\n            return Substitution::Pin { fork: p.fork.clone(), fork_sha: p.fork_sha.clone() };\n'
     '        }\n        return Substitution::Unapproved { fork, fork_sha: None };', TP),
    ("a SHA no map names is treated as an unapproved v4 pin", P,
     '    Substitution::AsWritten\n}\n\nfn lower_hex_sha',
     '    Substitution::Unapproved { fork, fork_sha: None }\n}\n\nfn lower_hex_sha', TP),
    ("tag substitution takes a v3 tag", P,
     'major(version).is_some_and(|major| major >= 4)', 'major(version).is_some()', TP),
]

if __name__ == '__main__':
    run(MUTANTS)
