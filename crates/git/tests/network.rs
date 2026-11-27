//! The network chain's `NetworkCmd` (AM3-WP2a): only the operator lists a fork, a fork's parent is a member, a
//! re-sent `ForkAdded` changes nothing, and verify refuses a fork listed under another parent.

use graftyard_git::network::{NetworkCmd, NetworkErr, NetworkEvent, NetworkFold, NetworkOp, NetworkOut};
use graftyard_raft::{Command, Fold, Scratch};
use graftyard_types::{AccountId, CredentialShard, DbId, NetworkId, RepoId};

const ROOT: RepoId = RepoId(DbId(5));
const A: RepoId = RepoId(DbId(6));
const B: RepoId = RepoId(DbId(7));
const C: RepoId = RepoId(DbId(8));
const STRANGER: RepoId = RepoId(DbId(9));

fn operator(repo: RepoId, parent: RepoId) -> NetworkCmd {
    NetworkCmd { cred: CredentialShard { account: None, key: None }, op: NetworkOp::ForkAdded { repo, parent } }
}

fn decide(fold: &NetworkFold, cmd: &NetworkCmd) -> Result<(Vec<NetworkEvent>, NetworkOut), NetworkErr> {
    cmd.decide(&fold.scratch(), &[]).map(|(evs, out)| (evs.into_vec(), out))
}

#[test]
fn the_operator_lists_forks_of_members_once() {
    let mut fold = NetworkFold::new(NetworkId::of_root(ROOT));
    let user =
        NetworkCmd { cred: CredentialShard { account: Some(AccountId(DbId(100))), key: None }, ..operator(A, ROOT) };
    assert_eq!(decide(&fold, &user), Err(NetworkErr::Forbidden));

    let added = NetworkEvent::ForkAdded { repo: A, parent: ROOT };
    assert_eq!(decide(&fold, &operator(A, ROOT)), Ok((vec![added.clone()], NetworkOut { added: true })));
    fold.apply(&added);
    assert_eq!(decide(&fold, &operator(A, ROOT)), Ok((Vec::new(), NetworkOut { added: false })), "re-sent");
    assert_eq!(decide(&fold, &operator(A, B)), Err(NetworkErr::Conflict));

    let (evs, out) = decide(&fold, &operator(B, A)).expect("a fork of a fork");
    assert!(out.added);
    fold.apply(&evs[0]);
    assert_eq!(fold.forks_of(A).collect::<Vec<_>>(), [B]);
    assert_eq!(fold.forks_of(ROOT).collect::<Vec<_>>(), [A]);
    assert_eq!(fold.parent(B), Some(A));
    assert!(fold.member(ROOT) && fold.member(B) && !fold.member(STRANGER));

    assert_eq!(decide(&fold, &operator(C, STRANGER)), Err(NetworkErr::NotMember));
    assert_eq!(decide(&fold, &operator(ROOT, A)), Err(NetworkErr::Invalid));
    assert_eq!(decide(&NetworkFold::default(), &operator(A, ROOT)), Err(NetworkErr::Invalid), "no network");

    let mut s = fold.scratch();
    assert!(s.check(&NetworkEvent::ForkAdded { repo: A, parent: B }).is_err(), "A is a fork of the root");
    assert!(s.check(&NetworkEvent::ForkAdded { repo: ROOT, parent: A }).is_err());
    let c = NetworkEvent::ForkAdded { repo: C, parent: B };
    assert!(s.check(&c).is_ok());
    s.apply(&c);
    assert!(s.check(&NetworkEvent::ForkAdded { repo: C, parent: A }).is_err(), "the entry's earlier add counts");
    assert_eq!(fold.read().forks, fold.forks);
}
