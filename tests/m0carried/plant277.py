#!/usr/bin/env python3
"""AM3-WP2a, base forks in their network (`build/BUILD-PLAN.md`; `decisions/DECISIONS.md`, "`fork-network-read`").
Head 1's mutants: a fork's network on its chain (`crates/git/src/tiers.rs`, `Genesis` and the `fork` row), in the
access rows (`crates/api/src/hosted.rs`), learned by the edge (`workers/edge/src/lib.rs`, `nets`) and carried by a
shared gate entry (`workers/edge/src/colo.rs`, `gate_body`), and on `Names`' `RepoOwner` row for the id-only routes
(`crates/identity/src/names.rs`, Names 0x10.4). The review fixes after head 1: the network fixed on the request by its own
read (`Repo::fix`) and held by its gate entry, the clone front's key built again on it, a Genesis naming another network
refused, and the format order (`decisions/DECISIONS.md`, "`format-order`"). Head 2's: a fork's shard calls name its
network (`crates/git/src/shard.rs`, `shard_query`; the edge's `shard_at_url`; the site's warm), the site keys a fork's
packs by its network (`Family::network`), and the vault and the lifeboat learn a fork's network from its `Genesis`
and owe its packs there. Head 3's: the fork operation (`decisions/DECISIONS.md`, "`fork-create`"): `CreateFork`'s
commit point, its finisher's order, the network chain's `ForkAdded`, the fork chain's first touch, and a fork's copied
refs sending no push. Head 4's: a fork chain's refs and HEAD in its own namespace on the network's disk
(`decisions/DECISIONS.md`, "`fork-refns`"), a fork's container calls naming it and its fetch skipping the clone cache,
`OpenPr`'s fork-head network check, the fork fields, and the export pack scope. Later heads append their mutants here.
Run from the repository root:

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
ENTRY = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_gate_entry_carries_a_forks_network']
FIXED = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_forks_network_is_fixed_by_its_own_read']
FRONT = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_clone_key_built_before_the_gate']
REPO = 'crates/git/src/repo.rs'
CORPUS = 'crates/git/tests/fold_corpus.rs'
ORDER = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'fold_corpus', 'a_new_events_format_is_above']
OWNER = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'm3', 'a_forks_repo_owner_row_carries']
SHARD = 'crates/git/src/shard.rs'
FAMILY = 'workers/site/src/family.rs'
MEMBER = 'workers/site/src/member.rs'
VAULT = 'workers/vault/src/lib.rs'
PASS = 'lifeboat/src/pass.rs'
LGIT = 'workers/lifeboat/src/git.rs'
QUERY = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'agent_load', 'a_forks_shard_query_names_its_network']
SHARDURL = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_forks_shard_calls_name_its_network']
SITENET = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'a_fork_chains_network_keys_its_packs']
WARM = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'a_moves_warm_ask_starts_the_shard']
VFORK = ['cargo', 'test', '-q', '-p', 'graftyard-vault', '--test', 'vault', 'a_fork_s_packs_are_copied_under']
VBACK = ['cargo', 'test', '-q', '-p', 'graftyard-vault', '--test', 'vault', 'a_backfilled_fork_s_genesis_moves']
LOWED = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat', '--test', 'pass', 'a_fork_s_packs_are_owed_under']
LHELD = ['cargo', 'test', '-q', '-p', 'graftyard-lifeboat-worker', '--lib', 'a_fork_s_chain_is_held_and_waits']
AUTHZ = 'crates/identity/src/authz.rs'
FINISH = 'crates/identity/src/finish.rs'
NETWORK = 'crates/git/src/network.rs'
LIFE = 'crates/git/src/lifecycle.rs'
HOOKS = 'crates/git/src/hooks.rs'
TRIGGER = 'crates/actions/src/trigger.rs'
FORKS = 'workers/edge/src/forks.rs'
CREATEFORK = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'audit', 'create_fork_needs_an_admin']
FORKORDER = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'audit', 'a_fork_is_finished_in_order']
NETCHAIN = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'network']
TOUCH = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'lifecycle', 'fork']
NOPUSH = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'hooks_gates', 'a_forks_copied_refs']
NORUN = ['cargo', 'test', '-q', '-p', 'graftyard-actions', '--test', 'pr_trigger', 'push_events_skip']
BODIES = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_forks_commit_and_finish_bodies']
CATCHUP = 'crates/git/src/catchup.rs'
ISSUEOPS = 'crates/git/src/issue_ops.rs'
EXPORTTOK = 'crates/identity/src/export_token.rs'
REFNS = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup', 'a_fork_chains_refs_and_head_land']
ROOTONLY = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup', 'a_root_only_networks_cursor']
FORKPR = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'issues', 'a_pull_request_head_in_another']
FIELDS = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'forks::tests::']
PACKSCOPE = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--lib', 'a_networks_packs_are_covered_as_on_main']
JOIN = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'catchup', 'a_fork_that_does_not_answer']
MUTANTS = [
    ("a fork's Genesis writes no fork row", TIERS,
     '                if network.root() != self.repo {\n', '                if false {\n', CHAIN),
    ("a root's Genesis writes a fork row, moving its rows", TIERS,
     '                if network.root() != self.repo {\n', '                if true {\n', CHAIN),
    ("the access rows leave out the fork row (option A's edge, with no RepoOwner read)", HOSTED,
     '        c.extend(core.get("fork", &[]).map(|cols| ("fork".to_owned(), Vec::new(), cols)));\n', '', ACCESS),
    ("the edge keys a fork by its own id", EDGE,
     '        fork(repo).unwrap_or(NetworkId::of_root(repo))\n', '        NetworkId::of_root(repo)\n', FIXED),
    ("the edge stores a root's own id as a fork", EDGE,
     '        if network.root() == repo {\n            return;\n        }\n', '', FIXED),
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
    # The review fixes (AM2, after head 1).
    ("a request reads the isolate's map every time, so an overflow moves its packs under the fork's own id", EDGE,
     '        self.net.get().unwrap_or_else(|| nets::get(self.id))\n', '        nets::get(self.id)\n', FIXED),
    # `GateIo::hold` and `share` pass `Repo::fork()` through; they need a live `Ctx`, so the entry's own storage is
    # what a native test reads back.
    ("a gate entry drops its fill's network, so a later share puts a bare {at} for a fork", EDGE,
     '                c.insert(k, Entry { at, d, foreign, rows, fork });\n',
     '                c.insert(k, Entry { at, d, foreign, rows, fork: None });\n', FIXED),
    ("a root fixes a network of its own id as a fork, so its entry grows a net", EDGE,
     '        self.net.get().filter(|n| n.root() != self.id)\n', '        self.net.get()\n', FIXED),
    ("the clone front's key on the guessed network is kept, so a stale entry is served", EDGE,
     '        Some(k) if k.network != network => (Some(CloneKey { network, ..k }), false),\n',
     '        Some(k) if false => (Some(CloneKey { network, ..k }), false),\n', FRONT),
    ("a second Genesis may name another network", REPO,
     '                    Some(f) if f.network != *network => {\n', '                    Some(f) if false && f.network != *network => {\n',
     CHAIN),
    ("Genesis back at 32.31, below TaskResumed's 32.33", CORPUS, '        50 => 34,\n', '        50 => 31,\n', ORDER),
    # Head 2: network-keyed packs and shard.
    ("a fork's shard query names only its own id, so its call reaches another disk", SHARD,
     '    if network.root() == repo { format!(', '    if true { format!(', QUERY),
    ("the edge's shard calls drop a fork's network", EDGE,
     '    format!("https://shard{path}?{}", graftyard_git::shard_query(repo.id, repo.network()))\n',
     '    format!("https://shard{path}?repo={}", repo.id.0.0)\n', SHARDURL),
    ("the site keys a fork's packs by its own id", FAMILY,
     '            ChainId::Repo(r) => Some(git::tiers::network(&*fold.core, r)),\n',
     '            ChainId::Repo(r) => Some(graftyard_types::NetworkId::of_root(r)),\n', SITENET),
    ("a move's warm call drops a fork's network", MEMBER,
     '        graftyard_git::shard_query(repo, net)\n',
     '        graftyard_git::shard_query(repo, graftyard_types::NetworkId::of_root(repo))\n', WARM),
    ("the vault reads past a fork's Genesis, so its packs are copied under its own id", VAULT,
     '                st.fork(repo, *network);\n', '', VFORK),
    ("a backfilled fork's copies stay under its own id after its Genesis", VAULT,
     '                self.copies.insert(Copy::Pack { network, id, checksum, bytes });\n',
     '                self.copies.insert(Copy::Pack { network: own, id, checksum, bytes });\n', VBACK),
    ("a backfilled fork's GC key stays under its own id after its Genesis", VAULT,
     '                self.gc.entry(pack_key(network, id)).or_insert(g);\n',
     '                self.gc.entry(pack_key(own, id)).or_insert(g);\n', VBACK),
    ("a backfilled chain's copies go out before its Genesis is read", VAULT,
     '            ChainId::Repo(r) if b.next == Index(1) => Some(NetworkId::of_root(*r)),\n',
     '            ChainId::Repo(r) if false && b.next == Index(1) => Some(NetworkId::of_root(*r)),\n', VBACK),
    ("the lifeboat owes a fork's packs under its own id", PASS,
     '                st.networks.insert(repo, *network);\n', '', LOWED),
    ("the lifeboat gives a network a fork chain's index over its root's", LGIT,
     '            out.entry(net).or_insert(*i);\n', '            out.insert(net, *i);\n', LHELD),
    # Head 3: the fork operation.
    ("CreateFork records the new repository as its parent", AUTHZ,
     '                            parent: *parent,\n', '                            parent: repo,\n', CREATEFORK),
    ("CreateFork takes any account, not only an owner admin", AUTHZ,
     '            Some(a) => s.is_admin(UserId(a)),\n',
     '            Some(a) => s.is_admin(UserId(a)) || matches!(self.op, AuthzOp::CreateFork { .. }),\n', CREATEFORK),
    ("the finisher touches the fork's chain before the network lists it", FINISH,
     '            if !network.forks.contains_key(&f.repo) {\n', '            if false {\n', FORKORDER),
    ("the fork's owner row drops its network, so its packs key under its own id", FINISH,
     '                NamesOp::RepoOwnerIn { repo: f.repo, owner: f.owner, network: f.network },\n',
     '                NamesOp::RepoOwner { repo: f.repo, owner: f.owner },\n', FORKORDER),
    ("a fork never aborts past its deadline", FINISH,
     '            if now.0 >= f.at.0.saturating_add(', '            if false && now.0 >= f.at.0.saturating_add(',
     FORKORDER),
    ("ForkAdded re-lists a fork under another parent", NETWORK,
     '            Some(_) => return Err(NetworkErr::Conflict),\n', '            Some(_) => {}\n', NETCHAIN),
    ("ForkAdded takes a parent outside the network", NETWORK,
     '        if !fold.member(parent) {\n', '        if false {\n', NETCHAIN),
    ("anyone may write the network chain", NETWORK,
     '        if !self.cred.is_operator() {\n            return Err(NetworkErr::Forbidden);\n',
     '        if false {\n            return Err(NetworkErr::Forbidden);\n', NETCHAIN),
    ("the first touch writes no Genesis, so the fork keys its packs under its own id", LIFE,
     '                    smallvec![genesis, RepoEvent::Settings(',
     '                    smallvec![RepoEvent::Settings(',
     TOUCH),
    ("the first touch copies a ref outside branches and tags", LIFE,
     '                    if !(b.starts_with(b"refs/heads/") || b.starts_with(b"refs/tags/")) || !names.insert(r) {\n',
     '                    if b.is_empty() || !names.insert(r) {\n', TOUCH),
    ("the first touch copies a ref named twice", LIFE,
     ' || !names.insert(r) {\n', ' || (!names.insert(r) && false) {\n', TOUCH),
    ("a fork whose network is its own id is touched", LIFE,
     '                if network.root() == repo {\n', '                if false {\n', TOUCH),
    ("a fork's copied refs send push webhooks", HOOKS,
     '            RepoEvent::Refs { .. } if fork => {}\n', '', NOPUSH),
    ("a fork's copied refs start push workflows", TRIGGER,
     '            RepoEvent::Refs { .. } if fork => continue,\n', '', NORUN),
    ("the edge's CreateFork names the parent as its network", FORKS,
     '"parent": parent, "network": network } })\n', '"parent": parent, "network": parent } })\n', BODIES),
    ("a fork chain's refs land where the root's are", SHARD,
     '        return Some(format!("refs/namespaces/{}/{name}", r.0.0));\n', '        return Some(name.to_owned());\n',
     REFNS),
    ("a root push writes a ref inside a fork's namespace", SHARD,
     '    (!chains.iter().filter_map(fork).any(inside)).then(|| name.to_owned())\n',
     '    let _ = inside;\n    Some(name.to_owned())\n', REFNS),
    ("a catch-up applies a fork's ref changes outside its namespace", CATCHUP,
     '                .filter_map(|(n, o)| Some((crate::shard::fork_ref(net, chains, chain, &n)?, o)))\n',
     '                .filter_map(|(n, o)| Some((n, o)).filter(|_| !chains.is_empty()))\n', REFNS),
    ("a rehydrate pages a fork's refs outside its namespace", CATCHUP,
     '                .filter_map(|(n, o)| Some((crate::shard::fork_ref(network, chains, *chain, n)?, o.clone())))\n',
     '                .map(|(n, o)| (n.clone(), o.clone()))\n', REFNS),
    ("a fork that does not answer fails the root's catch-up", CATCHUP,
     '                    Err(_) => continue,\n', '                    Err(e) => return Err(e),\n', JOIN),
    ("a fork new to the disk rehydrates the network", CATCHUP,
     '                if *chain != root && cursor.is_some() && !held {\n',
     '                if *chain != root && cursor.is_some() && !held && false {\n', JOIN),
    ("a fork archived past its cursor rehydrates the network", CATCHUP,
     '                    if *chain == root {\n                        cursor = None;\n',
     '                    if *chain == root || true {\n                        cursor = None;\n', JOIN),
    ("a join leaves the fork's cursor at 0, so its catch-up reads archived entries", CATCHUP,
     '    cursor.advance(chain, head.applied.min(first.unwrap_or(head.applied)));\n', '', JOIN),
    ("a fork's changed HEAD is never named", CATCHUP,
     'let Some(h) = head.filter(|h| cursor.head_of(chain) != Some(h.as_str())) else { continue };\n',
     'let Some(h) = head.filter(|h| chain == chains[0] && cursor.head_of(chain) != Some(h.as_str())) else {\n'
     '            continue;\n        };\n', REFNS),
    ("a fork's HEAD is not recorded, so every catch-up names it again", SHARD,
     '            None => self.fork_heads.push((chain, h)),\n', '            None => drop(h),\n', REFNS),
    ("a root-only cursor saves an empty fork_heads", SHARD,
     '    #[serde(default, skip_serializing_if = "SmallVec::is_empty")]\n', '    #[serde(default)]\n', ROOTONLY),
    ("a root call beside forks names none, so it lists their refs", SHARD,
     '            format!("&forks={}", ids.join(","))\n', '            format!("{}", ids.len() * 0)\n', QUERY),
    ("a fork's fetch reads the network's clone cache", EDGE,
     '    if network.root() != repo {\n        return (None, false);\n',
     '    if false {\n        return (None, false);\n', FRONT),
    ("a fork-head pull request reads no Names shard", ISSUEOPS,
     '            && head.repo != view.repo()\n', '            && false\n', FORKPR),
    ("a fork head of another network opens", ISSUEOPS,
     '                    if head_network != crate::tiers::network(v.core(), v.tiers.repo) {\n',
     '                    if false && head_network != crate::tiers::network(v.core(), v.tiers.repo) {\n', FORKPR),
    ("a fork head's network is read as its root's own id, so any root of another network opens", ISSUEOPS,
     '                        None if names.repo_owner(head.repo).is_some() => NetworkId::of_root(head.repo),\n',
     '                        None if names.repo_owner(head.repo).is_some() => {\n'
     '                            crate::tiers::network(v.core(), v.tiers.repo)\n                        }\n', FORKPR),
    ("a repository with no forks renders fork fields and a new ETag", FORKS,
     '        self != ForkFields::default()\n', '        true\n', FIELDS),
    ("network_count reads the direct count, not the network's", FORKS,
     'let total = answer.and_then(|v| v.get("total")).and_then(Value::as_u64).unwrap_or(direct);\n',
     'let total = direct;\n', FIELDS),
    ("a forked network's packs are covered as an unforked one's, so the root's owner reads a fork's objects",
     EXPORTTOK,
     '    if forked { scope == Scope::Install } else { covers(scope, ChainId::Network(network), Some(root_owner)) }\n',
     '    covers(scope, ChainId::Network(network), Some(root_owner))\n', PACKSCOPE),
    ("a forked network's packs are covered by any owner's token", EXPORTTOK,
     '    if forked { scope == Scope::Install } else { covers(scope, ChainId::Network(network), Some(root_owner)) }\n',
     '    if forked { !matches!(scope, Scope::Repo(_)) } else { covers(scope, ChainId::Network(network), Some(root_owner)) }\n',
     PACKSCOPE),
    ("an unforked network's packs are covered by its root's repository scope", EXPORTTOK,
     '    if forked { scope == Scope::Install } else { covers(scope, ChainId::Network(network), Some(root_owner)) }\n',
     '    if forked { scope == Scope::Install } else { scope != Scope::Owner(AccountId(DbId(8))) }\n', PACKSCOPE),
]

if __name__ == '__main__':
    run(MUTANTS)
