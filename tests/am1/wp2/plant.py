#!/usr/bin/env python3
"""AM1-WP2's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/am1/wp2/plant.py          # every mutant
    python3 tests/am1/wp2/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
AG = 'crates/git/src/agent.rs'
R = 'crates/git/src/repo.rs'
I = 'crates/git/src/issue_ops.rs'
M = 'crates/git/src/merge.rs'
T = 'crates/git/src/tiers.rs'
Z = 'crates/identity/src/authz.rs'
GIT = ['-p', 'graftyard-git', '--test', 'agent_gates', '--test', 'agent_bytes', '--lib']

# (name, file, old, new, test args)
MUTANTS = [
    ('unflag not applied: the fold keeps the old AgentApp row (a stale class)', Z,
     '        AuthzEvent::AgentApp { app, row: None } => t.agent_apps.del(app),',
     '        AuthzEvent::AgentApp { row: None, .. } => {}', GIT),
    ('class ignores the install fold: a confined App is never Agent', AG,
     '            | Some(AgentApp { kind: AgentKind::ThirdParty, confine: Some(_), .. }) => {',
     '            | Some(AgentApp { kind: AgentKind::ThirdParty, confine: Some(_), .. }) if false => {', GIT),
    ('prefix check off', AG,
     '    if prefix.as_ref().is_none_or(|p| !p.matches(name.as_bytes())) {',
     '    if prefix.is_none() {', GIT),
    ('protected-path check off', AG,
     '        && let Some(h) = protected.iter().find(|h| h.refname == *name && !h.paths.is_empty())',
     '        && let Some(h) = protected.iter().find(|h| h.refname == *name && h.paths.is_empty())', GIT),
    ('bypass allowed to agents', AG,
     '    if needs_bypass {\n        let text', '    if needs_bypass && false {\n        let text', GIT),
    ('no live task not required of a Graftyard-run App', AG,
     '    if row.as_ref().is_some_and(|r| r.kind == AgentKind::GraftyardRun) && a.task.is_none() {',
     '    if row.as_ref().is_some_and(|r| r.kind == AgentKind::GraftyardRun) && a.task.is_some() {', GIT),
    ('gate ignores the missing requester: one approval suffices', AG,
     '    1 + usize::from(policy.extra_human_approvals) + usize::from(meta.requester.is_none())',
     '    1 + usize::from(policy.extra_human_approvals)', GIT),
    ('gate not checked in Merge::decide', M,
     '        gate_holds(core, self.n, head)?;\n', '', GIT),
    ('gate counted on any head: a head move does not void it', T,
     '    r.prefix("agent_gate", &[n_key(n), sha_key(head)])', '    r.prefix("agent_gate", &[n_key(n)])', GIT),
    ('AgentGate written by an AgentApp\'s user-to-server token', I,
     '                    && self.credential_app(deps).is_none_or(|a| crate::agent::agent_app(install, a).is_none())\n',
     '', GIT),
    ('Co-authored-by classes', AG,
     '        self != TrailerKey::CoAuthoredBy', '        true', GIT),
    ('a forged Graftyard-Session links without a task', AG,
     '                            Some(id) if own.contains(&id) => Source::Session(id),',
     '                            Some(id) => Source::Session(id),', GIT),
    ('a Session sent on the wire is kept without a task', AG,
     '                .filter(|s| !matches!(s, Source::Session(id) if !own.contains(id)))',
     '                .filter(|_| true)', GIT),
    ('refs/graftyard/ not host-owned', R,
     '    name.starts_with("refs/pull/") || name.starts_with(crate::agent::HOST_REFS)',
     '    name.starts_with("refs/pull/")', GIT),
    ('label allowlist ignored', I,
     '                (!allowed.iter().any(|a| **a == *name)).then(|| {',
     '                (allowed.is_empty() && false).then(|| {', GIT),
    ('32.8 chosen for every push (default bytes change)', R,
     '        } else if self.validated() {\n            REPO_FORMAT_AGENT', '        } else if true {\n            REPO_FORMAT_AGENT', GIT),
    # The permission gate between them grew (a fine-grained token's own scope): the mutant moves the agent check past
    # the whole gate, as it was on main.
    ('the agent check after the permission checks: an App without the permission gets a bare 403', I,
     "        let install = crate::repo::install_authz(deps);\n        if let crate::agent::ActorKind::Agent(_) = crate::agent::actor_kind(&self.actor, install) {\n            agent_refusal(&self.op, v).map_or(Ok(()), |r| Err(IssueErr::Agent(r)))?;\n        }\n        let owner = v.owner();\n        let folds = crate::repo::authz_folds(deps);\n        let authz = folds.iter().copied().find(|a| a.owner() == Some(owner));\n        // An installation token is authorized once, by the GitHub App permission the command needs, intersected with\n        // the installation's current row; past that gate it may do what a role check below would ask of a user.\n        let perm = match app {\n            Some(c) => {\n                let access = Access { repo: v.tiers.repo, owner, core: v.core() };\n                match graftyard_identity::authorize_app(c, &self.app_needs(v), &folds, &access) {\n                    Decision::Allow => Some(RepoPermission::Admin),\n                    Decision::Forbidden => return Err(IssueErr::Forbidden),\n                    Decision::Hidden => return Err(IssueErr::NotFound),\n                }\n            }\n            None => {\n                // A `github_pat_` or `ghu_` acts only within its own scope and permissions, by the same per-resource\n                // names an App's command needs.\n                let access = Access { repo: v.tiers.repo, owner, core: v.core() };\n                let fields = crate::repo::stored_fields(&self.actor, deps);\n                match graftyard_identity::token_allows(fields, &self.app_needs(v), &access) {\n                    Decision::Allow => permission(&self.actor, v, &folds),\n                    Decision::Forbidden => return Err(IssueErr::Forbidden),\n                    Decision::Hidden => return Err(IssueErr::NotFound),\n                }\n            }\n        };\n",
     "        let owner = v.owner();\n        let folds = crate::repo::authz_folds(deps);\n        let authz = folds.iter().copied().find(|a| a.owner() == Some(owner));\n        // An installation token is authorized once, by the GitHub App permission the command needs, intersected with\n        // the installation's current row; past that gate it may do what a role check below would ask of a user.\n        let perm = match app {\n            Some(c) => {\n                let access = Access { repo: v.tiers.repo, owner, core: v.core() };\n                match graftyard_identity::authorize_app(c, &self.app_needs(v), &folds, &access) {\n                    Decision::Allow => Some(RepoPermission::Admin),\n                    Decision::Forbidden => return Err(IssueErr::Forbidden),\n                    Decision::Hidden => return Err(IssueErr::NotFound),\n                }\n            }\n            None => {\n                // A `github_pat_` or `ghu_` acts only within its own scope and permissions, by the same per-resource\n                // names an App's command needs.\n                let access = Access { repo: v.tiers.repo, owner, core: v.core() };\n                let fields = crate::repo::stored_fields(&self.actor, deps);\n                match graftyard_identity::token_allows(fields, &self.app_needs(v), &access) {\n                    Decision::Allow => permission(&self.actor, v, &folds),\n                    Decision::Forbidden => return Err(IssueErr::Forbidden),\n                    Decision::Hidden => return Err(IssueErr::NotFound),\n                }\n            }\n        };\n        let install = crate::repo::install_authz(deps);\n        if let crate::agent::ActorKind::Agent(_) = crate::agent::actor_kind(&self.actor, install) {\n            agent_refusal(&self.op, v).map_or(Ok(()), |r| Err(IssueErr::Agent(r)))?;\n        }\n", GIT),
    ('Push::tiers Core only with provenance (followers refuse the entry)', R,
     '        if self.provenance.is_empty() && !path_set { TierSet::CORE } else { TierSet::CORE.union(TierSet::METADATA) }',
     '        let _ = path_set;\n        TierSet::CORE', GIT),
    ('session_from_url splits a forged value inside a character', AG,
     '    let (head, rest) = value.split_at_checked(origin.len())?;',
     '    let (head, rest) = value.split_at(origin.len());', GIT),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')),
           CARGO_INCREMENTAL='0')
only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if only and n not in only:
        continue
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        # A hang counts as caught (README, "Planted bugs").
        r = subprocess.run(['timeout', '-k', '10', '600', 'cargo', 'test', '-j', '3', *args, '--no-fail-fast'], cwd=ROOT, env=env,
                           capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if 'error[' in out and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        if r.returncode == 124:
            failed = ['(timed out)', *failed]
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
