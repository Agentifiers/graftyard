#!/usr/bin/env python3
"""AM3-WP2a, base forks in their network (`build/BUILD-PLAN.md`; `decisions/DECISIONS.md`, "`fork-network-read`").
Head 1's mutants: a fork's network on its chain (`crates/git/src/tiers.rs`, `Genesis` and the `fork` row), in the
access rows (`crates/api/src/hosted.rs`), learned by the edge (`workers/edge/src/lib.rs`, `nets`) and carried by a
shared gate entry (`workers/edge/src/colo.rs`, `gate_body`), and on `Names`' `RepoOwner` row for the id-only routes
(`crates/identity/src/names.rs`, Names 0x10.4). Later heads append their mutants here. Run from the repository root:

    python3 tests/m0carried/plant277.py          # every mutant
    python3 tests/m0carried/plant277.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

TIERS = 'crates/git/src/tiers.rs'
HOSTED = 'crates/api/src/hosted.rs'
EDGE = 'workers/edge/src/lib.rs'
COLO = 'workers/edge/src/colo.rs'
NAMES = 'crates/identity/src/names.rs'
CHAIN = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'agent_load', 'a_fork_chain_names_its_network']
ACCESS = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'access_rows_include_a_forks']
LEARN = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_forks_network_is_learned']
ENTRY = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_gate_entry_carries_a_forks_network']
OWNER = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'm3', 'a_forks_repo_owner_row_carries']
MUTANTS = [
    ("a fork's Genesis writes no fork row", TIERS,
     '                if network.root() != self.repo {\n', '                if false {\n', CHAIN),
    ("a root's Genesis writes a fork row, moving its rows", TIERS,
     '                if network.root() != self.repo {\n', '                if true {\n', CHAIN),
    ("the access rows leave out the fork row (option A's edge, with no RepoOwner read)", HOSTED,
     '        c.extend(core.get("fork", &[]).map(|cols| ("fork".to_owned(), Vec::new(), cols)));\n', '', ACCESS),
    ("the edge keys a fork by its own id", EDGE,
     '        fork(repo).unwrap_or(NetworkId::of_root(repo))\n', '        NetworkId::of_root(repo)\n', LEARN),
    ("the edge stores a root's own id as a fork, so its shared entry grows a net", EDGE,
     '        if network.root() == repo {\n            return;\n        }\n', '', LEARN),
    ("a root's shared gate entry carries a net, changing main's bytes", COLO,
     '        None => serde_json::json!({ "at": at.0 }).to_string(),\n',
     '        None => serde_json::json!({ "at": at.0, "net": null }).to_string(),\n', ENTRY),
    ("a shared gate entry drops a fork's network", COLO,
     '    Some((Timestamp(v["at"].as_u64()?), serde_json::from_value(v["net"].clone()).ok().flatten()))\n',
     '    Some((Timestamp(v["at"].as_u64()?), None))\n', ENTRY),
    ("a root's RepoOwnerIn writes ForkOwner, moving its Names row", NAMES,
     '                if network.root() == *repo {\n                    one(NamesEvent::RepoOwner',
     '                if false {\n                    one(NamesEvent::RepoOwner', OWNER),
    ("a follower stores a ForkOwner whose network is its own id", NAMES,
     '            NamesEvent::ForkOwner { repo, network, .. } if network.root() == *repo => {',
     '            NamesEvent::ForkOwner { repo, network, .. } if network.root() == *repo && false => {', OWNER),
    ("a fork's network is not kept in the Names fold", NAMES,
     '            t.more.networks().put(*repo, *network);\n', '', OWNER),
]

if __name__ == '__main__':
    run(MUTANTS)
