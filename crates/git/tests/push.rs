//! `Push` decided by the real Raft core, through the stand-in group: what commits, what each ref reports, and what an
//! edge's re-send does.

use graftyard_git::group::{
    CallErrorWire, CommitReply, OutcomeReply, PushOutcome, ReadReply, TooLargeWire, UnsupportedWire,
};
use graftyard_git::pkt::{Pkt, Reader};
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{ATOMIC_REFS_MAX, Push, REPO_FORMAT_V1, RefOutcome, RejectReason};
use graftyard_git::standin::{Seed, SeedUser, StandIn};
use graftyard_git::{PackId, PackRef};
use graftyard_identity::{Actor, Credential, Principal, ScopeSet, UserId};
use graftyard_raft::{COMMAND_MAX, ENTRY_MAX, Record};
use graftyard_types::{AccountId, DbId, Digest, RequestId, SiteSet, Timestamp, Ulid};
use smallvec::SmallVec;

const OWNER: i64 = 100;
const WRITER: i64 = 101;
const READER: i64 = 102;
const REVOKED: i64 = 103;

fn hash(n: u8) -> [u8; 32] {
    [n; 32]
}

fn seed() -> Seed {
    let user = |account, n, write, revoked| SeedUser {
        account,
        token_hash: Digest(hash(n)).hex(),
        revoked,
        suspended: false,
        write,
    };
    Seed {
        repo: 7,
        owner: OWNER,
        users: vec![user(OWNER, 1, false, false), user(WRITER, 2, true, false), user(READER, 3, false, false), {
            user(REVOKED, 4, true, true)
        }],
        reads: vec![(REPO_FORMAT_V1.family, REPO_FORMAT_V1.version)],
    }
}

fn group() -> StandIn {
    let mut g = StandIn::restore(&seed(), Vec::new(), Timestamp(10_000));
    assert!(g.ensure_leader());
    g
}

fn who(account: i64, n: u8) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(account)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored(hash(n)),
        sso_orgs: SmallVec::new(),
    }
}

fn oid(n: u32) -> ObjectId {
    let mut o = [0u8; 20];
    o[..4].copy_from_slice(&n.to_be_bytes());
    o[19] = 1;
    ObjectId(o)
}

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn up(n: &str, expect: Option<ObjectId>, new: Option<ObjectId>) -> RefUpdate {
    RefUpdate { name: name(n), expect, new }
}

fn push(updates: Vec<RefUpdate>, atomicity: Atomicity, actor: Principal) -> Push {
    Push {
        updates,
        atomicity,
        pack: Some(PackRef::Inline(b"PACK\0\0\0\x02\0\0\0\0".to_vec())),
        actor,
        options: Vec::new(),
        request: RequestId(Ulid(0)),
        agent: None,
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
        leases: false,
    }
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn rid() -> RequestId {
    RequestId(Ulid(u128::from(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) << 64))
}

fn commit(g: &mut StandIn, mut p: Push) -> (RequestId, PushOutcome) {
    let r = rid();
    p.request = r;
    match g.commit(r, p) {
        CommitReply::Done(o) => (r, o),
        other => panic!("not done: {other:?}"),
    }
}

/// The ok/ng lines of a member's rendered report-status section.
fn status_lines(o: &PushOutcome) -> Vec<String> {
    let PushOutcome::Ok(ok) = o else { panic!("{o:?}") };
    let b = ok.out.status_bytes().unwrap();
    let mut r = Reader::new(&b);
    let mut out = Vec::new();
    while let Some(p) = r.next_pkt().unwrap() {
        if let Pkt::Data(_) = p {
            out.push(String::from_utf8(p.text().unwrap().to_vec()).unwrap());
        }
    }
    out
}

fn refs_of(o: &PushOutcome) -> Vec<RefOutcome> {
    match o {
        PushOutcome::Ok(ok) => ok.out.refs.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_atomic_push_with_one_stale_expect_changes_nothing_and_says_ng_on_every_ref() {
    let mut g = group();
    let (_, o) =
        commit(&mut g, push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
    let before = g.refs();
    let p = push(
        vec![
            up("refs/heads/main", Some(oid(1)), Some(oid(2))),
            up("refs/heads/release", Some(oid(9)), Some(oid(3))), // stale: release does not exist
            up("refs/heads/hotfix", None, Some(oid(4))),
        ],
        Atomicity::AllOrNothing,
        who(OWNER, 1),
    );
    let (_, o) = commit(&mut g, p);
    use RejectReason::*;
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(Atomic), RefOutcome::Rejected(Stale), RefOutcome::Rejected(Atomic)]);
    assert_eq!(g.refs(), before, "no ref moved");
    assert_eq!(
        status_lines(&o),
        [
            "unpack ok",
            "ng refs/heads/main atomic push failure",
            "ng refs/heads/release fetch first",
            "ng refs/heads/hotfix atomic push failure"
        ]
    );
    // The entry committed with no events, and a `request` row, so a re-send is answered from it rather than decided
    // again against a fold that may have moved.
    let last = g.log(1).pop().unwrap();
    assert_eq!((last.kind, last.ref_updates, last.requests), ("events", 0, 1));
}

#[test]
fn a_non_atomic_push_applies_the_refs_that_pass() {
    let mut g = group();
    commit(&mut g, push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    let p = push(
        vec![
            up("refs/heads/main", Some(oid(7)), Some(oid(2))), // stale
            up("refs/heads/feature", None, Some(oid(3))),
            up("refs/pull/1/head", None, Some(oid(4))), // hidden
        ],
        Atomicity::PerRef,
        who(WRITER, 2),
    );
    let (_, o) = commit(&mut g, p);
    assert_eq!(
        refs_of(&o),
        [RefOutcome::Rejected(RejectReason::Stale), RefOutcome::Ok, RefOutcome::Rejected(RejectReason::Hidden)]
    );
    let refs = g.refs();
    assert_eq!(refs.get("refs/heads/main"), Some(&oid(1).hex()));
    assert_eq!(refs.get("refs/heads/feature"), Some(&oid(3).hex()));
    assert!(!refs.contains_key("refs/pull/1/head"));
    assert_eq!(status_lines(&o)[2], "ok refs/heads/feature");
}

#[test]
fn deletes_and_creates_check_their_expects_in_order_inside_one_entry() {
    let mut g = group();
    commit(&mut g, push(vec![up("refs/heads/a", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    // Delete a, then a second push in the same group recreating a expecting it absent: both apply, in order.
    let (_, o) = commit(&mut g, push(vec![up("refs/heads/a", Some(oid(1)), None)], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
    let (_, o) = commit(&mut g, push(vec![up("refs/heads/a", None, Some(oid(5)))], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
    assert_eq!(g.refs().get("refs/heads/a"), Some(&oid(5).hex()));
    // A create of an existing ref is stale.
    let (_, o) = commit(&mut g, push(vec![up("refs/heads/a", None, Some(oid(6)))], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(RejectReason::Stale)]);
}

#[test]
fn ten_thousand_refs_commit_atomically_and_ten_thousand_and_one_are_refused_whole() {
    let mut g = group();
    let updates = |n: usize| -> Vec<RefUpdate> {
        (0..n).map(|i| up(&format!("refs/tags/t{i:05}"), None, Some(oid(i as u32 + 1)))).collect()
    };
    // The pack a 10,000-ref push carries is stored: the updates alone are about 700 KB, so an inline pack of up to
    // 1 MiB would take it over COMMAND_MAX.
    let stored = PackRef::Stored {
        id: PackId(Ulid(1 << 90)),
        checksum: Digest([3; 32]),
        bytes: 1 << 20,
        sites: SiteSet::of([graftyard_types::SiteId(0), graftyard_types::SiteId(1)]),
    };
    let mut p = push(updates(ATOMIC_REFS_MAX), Atomicity::AllOrNothing, who(OWNER, 1));
    p.pack = Some(stored);
    let len = p.encoded_len();
    assert!(len < COMMAND_MAX, "{len}");
    let (_, o) = commit(&mut g, p);
    assert!(refs_of(&o).iter().all(|r| *r == RefOutcome::Ok));
    assert_eq!(g.refs().len(), ATOMIC_REFS_MAX);
    let e = g.log(1).pop().unwrap();
    assert_eq!(e.ref_updates, ATOMIC_REFS_MAX);
    assert!(e.bytes < ENTRY_MAX, "entry of {} bytes", e.bytes);
    // 10,001 refs: refused by decide whole, with no row, even when the edge did not refuse it first.
    let mut p = push(updates(ATOMIC_REFS_MAX + 1), Atomicity::AllOrNothing, who(OWNER, 1));
    p.pack = None;
    let before = g.log(1).len();
    let (_, o) = commit(&mut g, p);
    assert!(matches!(&o, PushOutcome::Refused(r) if r.status == 422 && r.err.contains("10001")), "{o:?}");
    assert_eq!(g.refs().len(), ATOMIC_REFS_MAX);
    assert_eq!(g.log(before as u64 + 1).iter().map(|e| e.ref_updates).sum::<usize>(), 0);
    // Non-atomic pushes are not capped by ATOMIC_REFS_MAX.
    let mut p = push(
        (0..ATOMIC_REFS_MAX + 1).map(|i| up(&format!("refs/heads/b{i:05}"), None, Some(oid(1)))).collect(),
        Atomicity::PerRef,
        who(OWNER, 1),
    );
    p.pack = None;
    let (_, o) = commit(&mut g, p);
    assert_eq!(refs_of(&o).len(), ATOMIC_REFS_MAX + 1);
}

#[test]
fn a_command_over_command_max_is_refused_by_the_member_before_decide() {
    let mut g = group();
    let mut p = push(
        (0..20_000).map(|i| up(&format!("refs/heads/feature/long-branch-name-{i:06}"), None, Some(oid(1)))).collect(),
        Atomicity::PerRef,
        who(OWNER, 1),
    );
    p.pack = None;
    assert!(p.encoded_len_without_pack() > COMMAND_MAX);
    let before = g.log(1).len();
    let r = rid();
    p.request = r;
    match g.commit(r, p) {
        CommitReply::Error(CallErrorWire::TooLarge(TooLargeWire { len, max })) => {
            assert!(len > max && max == COMMAND_MAX)
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(g.log(1).len(), before, "no entry");
}

#[test]
fn an_unknown_format_is_unsupported_and_changes_nothing() {
    let mut g = group();
    commit(&mut g, push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    let before = (g.log(1).len(), g.refs());
    let mut p = push(vec![up("refs/heads/main", Some(oid(1)), Some(oid(2)))], Atomicity::PerRef, who(OWNER, 1));
    p.agent = Some("git/2.51.0".into()); // format 2, which release N does not read
    let r = rid();
    p.request = r;
    match g.commit(r, p) {
        CommitReply::Error(CallErrorWire::Unsupported(UnsupportedWire { family, version })) => {
            assert_eq!((family, version), (0x20, 2))
        }
        other => panic!("{other:?}"),
    }
    assert_eq!((g.log(1).len(), g.refs()), before);
    assert_eq!(g.unsupported, 1);
}

#[test]
fn credentials_and_permissions_are_checked_at_decide() {
    let mut g = group();
    let one = || vec![up("refs/heads/main", None, Some(oid(1)))];
    let (_, o) = commit(&mut g, push(one(), Atomicity::PerRef, who(READER, 3)));
    let status = |o: &PushOutcome| match o {
        PushOutcome::Refused(r) => r.status,
        other => panic!("{other:?}"),
    };
    assert_eq!(status(&o), 403, "read-only");
    let (_, o) = commit(&mut g, push(one(), Atomicity::PerRef, who(REVOKED, 4)));
    assert_eq!(status(&o), 401, "revoked");
    // A token that belongs to someone else.
    let (_, o) = commit(&mut g, push(one(), Atomicity::PerRef, who(WRITER, 1)));
    assert_eq!(status(&o), 401, "someone else's token");
    let anon = Principal { actor: Actor::Anonymous, ..who(OWNER, 1) };
    let (_, o) = commit(&mut g, push(one(), Atomicity::PerRef, anon));
    assert_eq!(status(&o), 401, "anonymous");
    assert!(g.refs().is_empty());
    let (_, o) = commit(&mut g, push(one(), Atomicity::PerRef, who(WRITER, 2)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
}

#[test]
fn a_resent_request_is_answered_from_its_row_and_applied_once() {
    let mut g = group();
    let mut p = push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1));
    let r = rid();
    p.request = r;
    let CommitReply::Done(first) = g.commit(r, p.clone()) else { panic!() };
    let entries = g.log(1).len();
    // The edge's commit timed out, so it asks by request id, then re-sends under the same id.
    match g.outcome_by_request(r, &p) {
        OutcomeReply::Done(o) => assert_eq!(refs_of(&o), refs_of(&first)),
        other => panic!("{other:?}"),
    }
    let CommitReply::Done(again) = g.commit(r, p.clone()) else { panic!() };
    assert_eq!(refs_of(&again), [RefOutcome::Ok], "the duplicate replays the first outcome");
    assert_eq!(g.log(1).iter().map(|e| e.ref_updates).sum::<usize>(), 1, "applied once");
    assert!(g.log(1).len() >= entries);
    // git's own retry over a new connection is a new request with the same expects: Stale.
    let (_, o) = commit(&mut g, p);
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(RejectReason::Stale)]);
}

#[test]
fn the_group_survives_a_restart_from_its_rows() {
    let mut g = group();
    commit(&mut g, push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    let writes = g.drain_writes();
    // Rebuild every member's rows from the write journal, as the host's storage does.
    let mut rows = vec![graftyard_git::standin::MemberRows::default(); 3];
    for (s, w) in writes {
        let r = &mut rows[s.0 as usize];
        match w {
            graftyard_raft::Write::State(st) => r.state = Some(st),
            graftyard_raft::Write::Append(e) => {
                assert_eq!(e.index.0 as usize, r.log.len() + 1);
                r.log.push(e)
            }
            graftyard_raft::Write::Truncate(i) => r.log.truncate(i.0 as usize - 1),
            graftyard_raft::Write::Apply(_) => {}
            graftyard_raft::Write::Install(_) => unreachable!("no member installs a snapshot here"),
        }
    }
    let mut g2 = StandIn::restore(&seed(), rows, Timestamp(10_000 + 60_000));
    assert!(g2.ensure_leader());
    match g2.read() {
        ReadReply::Refs(r) => assert_eq!(r.value, vec![("refs/heads/main".to_owned(), oid(1).hex())]),
        other => panic!("{other:?}"),
    }
    let (_, o) = commit(
        &mut g2,
        push(vec![up("refs/heads/main", Some(oid(1)), Some(oid(2)))], Atomicity::PerRef, who(OWNER, 1)),
    );
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
}

#[test]
fn the_wire_form_round_trips_and_refuses_an_unknown_format() {
    let mut p = push(
        vec![up("refs/heads/main", None, Some(oid(1))), up("refs/heads/x", Some(oid(2)), None)],
        Atomicity::AllOrNothing,
        who(OWNER, 1),
    );
    p.options = vec!["ci.skip".into()];
    p.request = RequestId(Ulid(42));
    assert_eq!(Push::from_wire(&p.to_wire()), Ok(p.clone()));
    p.agent = Some("git/2.39.5".into());
    assert_eq!(Push::from_wire(&p.to_wire()), Ok(p.clone()));
    // The serde form a member's JSON `cmd` carries.
    let j = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Push>(&j).unwrap(), p);
    let mut w = p.to_wire();
    w[3] = 9;
    assert!(matches!(Push::from_wire(&w), Err(graftyard_git::repo::WireError::Format(_))));
    let w = p.to_wire();
    for cut in [0, 3, 10, w.len() - 1] {
        assert!(Push::from_wire(&w[..cut]).is_err(), "{cut}");
    }
}

#[test]
fn a_principal_with_more_than_64_sso_orgs_round_trips() {
    let mut actor = who(OWNER, 1);
    actor.sso_orgs = (0..65).map(|i| graftyard_identity::OrgId(AccountId(DbId(1_000 + i)))).collect();
    let mut p = push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, actor.clone());
    p.options = vec!["ci.skip".into()];
    p.request = RequestId(Ulid(42));
    assert_eq!(Push::from_wire(&p.to_wire()), Ok(p));
    let b = graftyard_git::repo::principal_bytes(&actor);
    assert_eq!(graftyard_git::repo::principal_from_bytes(&b), Ok(actor));
}

#[test]
fn a_ref_that_is_a_directory_or_file_of_another_is_refused_as_a_name_conflict() {
    use RejectReason::*;
    let mut g = group();
    commit(&mut g, push(vec![up("refs/heads/other", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    // A file under an existing ref, and a directory over an existing ref: both refused, nothing else moves.
    let (_, o) = commit(
        &mut g,
        push(
            vec![
                up("refs/heads/other/conflict", None, Some(oid(2))),
                up("refs/tags", None, Some(oid(2))),
                up("refs/heads/fine", None, Some(oid(2))),
            ],
            Atomicity::PerRef,
            who(OWNER, 1),
        ),
    );
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(NameConflict), RefOutcome::Ok, RefOutcome::Ok]);
    assert_eq!(status_lines(&o)[1], "ng refs/heads/other/conflict refname conflict");
    // `refs/tags` is now a ref; a tag under it is its file.
    let (_, o) = commit(&mut g, push(vec![up("refs/heads", None, Some(oid(2)))], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(NameConflict)], "a directory over refs/heads/fine and other");
    commit(&mut g, push(vec![up("refs/tags", Some(oid(2)), None)], Atomicity::PerRef, who(OWNER, 1)));
    // Atomic: git's t5541 #20 shape. As upstream receive-pack, every ref says `atomic transaction failed`.
    let (_, o) = commit(
        &mut g,
        push(
            vec![up("refs/heads/atomic", None, Some(oid(3))), up("refs/heads/other/x", None, Some(oid(3)))],
            Atomicity::AllOrNothing,
            who(OWNER, 1),
        ),
    );
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(AtomicTransaction), RefOutcome::Rejected(AtomicTransaction)]);
    assert_eq!(
        status_lines(&o)[1..],
        ["ng refs/heads/atomic atomic transaction failed", "ng refs/heads/other/x atomic transaction failed"]
    );
    assert!(!g.refs().contains_key("refs/heads/atomic"));
    // Two creates in one push that conflict with each other: both refused.
    let (_, o) = commit(
        &mut g,
        push(
            vec![up("refs/tags/v", None, Some(oid(4))), up("refs/tags/v/1", None, Some(oid(4)))],
            Atomicity::PerRef,
            who(OWNER, 1),
        ),
    );
    assert_eq!(refs_of(&o), [RefOutcome::Rejected(NameConflict), RefOutcome::Rejected(NameConflict)]);
    // Deleting the file and creating the directory in one push is allowed, as git allows it.
    let (_, o) = commit(
        &mut g,
        push(
            vec![up("refs/heads/other", Some(oid(1)), None), up("refs/heads/other/now", None, Some(oid(5)))],
            Atomicity::AllOrNothing,
            who(OWNER, 1),
        ),
    );
    assert_eq!(refs_of(&o), [RefOutcome::Ok, RefOutcome::Ok]);
    assert_eq!(g.refs().get("refs/heads/other/now"), Some(&oid(5).hex()));
    // A prefix that is not at a '/' is no conflict.
    let (_, o) = commit(&mut g, push(vec![up("refs/heads/oth", None, Some(oid(6)))], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
}

/// A replay answers each ref with the reason its first decide gave, not `Stale`: the old replay reported every ref
/// absent from the applied updates as stale, so a lost reply on t5541 #20 printed `fetch first`.
#[test]
fn a_replay_reports_each_refused_refs_own_reason() {
    use RejectReason::*;
    let mut g = group();
    commit(&mut g, push(vec![up("refs/heads/other", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    let cases = [
        // t5541 #20: every ref refused, so the entry applied nothing.
        push(
            vec![up("refs/heads/atomic", None, Some(oid(3))), up("refs/heads/other/x", None, Some(oid(3)))],
            Atomicity::AllOrNothing,
            who(OWNER, 1),
        ),
        push(
            vec![up("refs/heads/other/y", None, Some(oid(4))), up("refs/heads/fine", None, Some(oid(4)))],
            Atomicity::PerRef,
            who(OWNER, 1),
        ),
        push(
            vec![up("refs/heads/new", None, Some(oid(5))), up("refs/heads/other", Some(oid(9)), Some(oid(5)))],
            Atomicity::AllOrNothing,
            who(OWNER, 1),
        ),
        push(
            vec![up("refs/pull/1/head", None, Some(oid(6))), up("refs/heads/other", None, Some(oid(6)))],
            Atomicity::PerRef,
            who(OWNER, 1),
        ),
    ];
    let want = [
        vec![RefOutcome::Rejected(AtomicTransaction), RefOutcome::Rejected(AtomicTransaction)],
        vec![RefOutcome::Rejected(NameConflict), RefOutcome::Ok],
        vec![RefOutcome::Rejected(Atomic), RefOutcome::Rejected(Stale)],
        vec![RefOutcome::Rejected(Hidden), RefOutcome::Rejected(Stale)],
    ];
    for (p, want) in cases.into_iter().zip(want) {
        let (r, first) = commit(&mut g, p.clone());
        assert_eq!(refs_of(&first), want);
        let mut again = p;
        again.request = r;
        // The edge's commit timed out: it asks by request id, then re-sends under the same id. It asks without the
        // pack (`lookup_body` in `workers/edge`), and the replay reads only the ref names, so the answer is the same.
        for ask in [again.clone(), Push { pack: None, ..again.clone() }] {
            match g.outcome_by_request(r, &ask) {
                OutcomeReply::Done(o) => {
                    assert_eq!(refs_of(&o), want, "outcome_by_request");
                    assert_eq!(status_lines(&o), status_lines(&first), "outcome_by_request's report-status");
                }
                other => panic!("{other:?}"),
            }
        }
        let CommitReply::Done(o) = g.commit(r, again) else { panic!() };
        assert_eq!(refs_of(&o), want, "re-send");
    }
}

/// `Promoted` and `Archived` as repository events: the fold records the last of each, `first_index` follows the last
/// archive, and `verify`'s check refuses a range that skips or overlaps, an inline promoted pack, an archive past the
/// last promotion, and an archive key that names another chain, range, or digest.
#[test]
fn promoted_and_archived_follow_each_other_and_archive_only_promoted_ranges() {
    use graftyard_git::repo::{RepoEvent, RepoFold};
    use graftyard_git::{Archived, Promoted, R2Key};
    use graftyard_raft::{Fold, Scratch};
    use graftyard_types::{ChainId, Index, RepoId};

    let repo = RepoId(DbId(7));
    let mut fold = RepoFold::new(repo, AccountId(DbId(OWNER)));
    assert_eq!(fold.first_index(), Index(1));
    let stored = PackRef::Stored { id: PackId(Ulid(9)), checksum: Digest([1; 32]), bytes: 10, sites: SiteSet(3) };
    let promoted = |from, to, pack: &PackRef| {
        RepoEvent::Promoted(Promoted { from: Index(from), to: Index(to), pack: pack.clone() })
    };
    let key = |from, to, d: u8| R2Key::Archive {
        chain: ChainId::Repo(repo),
        from: Index(from),
        to: Index(to),
        digest: Digest([d; 32]),
    };
    let archived = |from, to, k: R2Key| {
        RepoEvent::Archived(Archived {
            key: k,
            from: Index(from),
            to: Index(to),
            from_at: Timestamp(1),
            to_at: Timestamp(2),
            digest: Digest([5; 32]),
        })
    };
    let check = |f: &RepoFold, ev: &RepoEvent| f.scratch().check(ev);
    // A committed entry of one event, chained to the last: what the tiers apply.
    let mut prev = graftyard_raft::PREV_ZERO;
    let mut index = 0;
    let mut commit = |f: &mut RepoFold, ev: RepoEvent| {
        index += 1;
        let e = graftyard_raft::LogEntry {
            term: graftyard_raft::Term(2),
            index: Index(index),
            prev,
            format: graftyard_git::repo::REPO_FORMAT_V1,
            tiers: graftyard_raft::TierSet::CORE,
            at: Timestamp(index),
            requests: SmallVec::new(),
            extend: None,
            body: graftyard_raft::EntryBody::Events(vec![ev]),
        };
        prev = graftyard_raft::digest(&e);
        f.apply_entry(&e);
    };
    // No archive before a promotion covers the range.
    assert!(check(&fold, &archived(1, 8, key(1, 8, 5))).is_err());
    assert!(check(&fold, &promoted(2, 8, &stored)).is_err(), "the first range starts at 1");
    assert!(check(&fold, &promoted(1, 8, &PackRef::Inline(vec![1]))).is_err(), "a promoted pack is stored");
    assert!(check(&fold, &promoted(1, 8, &stored)).is_ok());
    // In one entry, an archive sees the promotion before it.
    let mut s = fold.scratch();
    s.apply(&promoted(1, 8, &stored));
    assert!(s.check(&archived(1, 8, key(1, 8, 5))).is_ok());
    drop(s);
    commit(&mut fold, promoted(1, 8, &stored));
    assert!(check(&fold, &archived(1, 9, key(1, 9, 5))).is_err(), "past the last promotion");
    assert!(check(&fold, &archived(1, 8, key(1, 8, 6))).is_err(), "the key names another digest");
    assert!(check(&fold, &archived(1, 8, key(1, 7, 5))).is_err(), "the key names another range");
    assert!(check(&fold, &archived(1, 8, key(1, 8, 5))).is_ok());
    commit(&mut fold, archived(1, 8, key(1, 8, 5)));
    assert_eq!(fold.first_index(), Index(9));
    assert!(check(&fold, &archived(1, 8, key(1, 8, 5))).is_err(), "an archive does not repeat");
    assert!(check(&fold, &promoted(1, 12, &stored)).is_err(), "a promotion does not overlap the last one");
    assert!(check(&fold, &promoted(9, 12, &stored)).is_ok());
    assert!(fold.refs().is_empty(), "neither event changes a ref");
}

/// An `Archived` with a snapshot key records the tier's snapshot without moving the log's archive ranges, its key
/// must name this chain at `to` from 0, and each new one drops the rows `RESTORE_WINDOW` no longer keeps: every
/// snapshot inside the window, and the newest one before it.
#[test]
fn a_snapshot_archived_keeps_one_snapshot_at_or_below_every_index_in_the_window() {
    use graftyard_git::repo::{RepoEvent, RepoFold};
    use graftyard_git::{Archived, R2Key, tiers};
    use graftyard_raft::{Fold, Scratch};
    use graftyard_types::{ChainId, Index, RepoId, Tier};

    let repo = RepoId(DbId(7));
    let mut fold = RepoFold::new(repo, AccountId(DbId(OWNER)));
    let day = 86_400_000;
    let snap = |chain: RepoId, to: u64, at: u64| {
        RepoEvent::Archived(Archived {
            key: R2Key::Snapshot {
                chain: ChainId::Repo(chain),
                tier: Tier::Metadata,
                from: Index(0),
                to: Index(to),
                digest: Digest([to as u8; 32]),
            },
            from: Index(0),
            to: Index(to),
            from_at: Timestamp(at),
            to_at: Timestamp(at),
            digest: Digest([to as u8; 32]),
        })
    };
    assert!(fold.scratch().check(&snap(RepoId(DbId(8)), 1, 0)).is_err(), "another chain's snapshot");
    let mut prev = graftyard_raft::PREV_ZERO;
    let mut index = 0;
    // Snapshots at 1, 2, ..., 50, each one day after the last, entry `n` at day `n`.
    for n in 1..=50u64 {
        index += 1;
        let ev = snap(repo, n, n * day);
        assert!(fold.scratch().check(&ev).is_ok());
        let e = graftyard_raft::LogEntry {
            term: graftyard_raft::Term(2),
            index: Index(index),
            prev,
            format: graftyard_git::repo::REPO_FORMAT_V1,
            tiers: graftyard_raft::TierSet::CORE,
            at: Timestamp(n * day),
            requests: SmallVec::new(),
            extend: None,
            body: graftyard_raft::EntryBody::Events(vec![ev]),
        };
        prev = graftyard_raft::digest(&e);
        fold.apply_entry(&e);
    }
    let kept: Vec<u64> = tiers::snapshots(&*fold.core, Tier::Metadata).iter().map(|(to, _, _)| to.0).collect();
    // The window is 35 days back from day 50: days 15..=50 are inside, and 14 is the newest before it.
    assert_eq!(kept, (14..=50).collect::<Vec<_>>());
    assert!(tiers::snapshots(&*fold.core, Tier::Core).is_empty());
    assert_eq!(fold.first_index(), Index(1), "a snapshot does not move the log's archive ranges");
    assert!(fold.archived().is_none());
}

/// BF-3, push half: a user suspended while their pushes wait for an entry, and every push after, is refused 401.
/// The `Account` row is a dep every decide reads, so the suspension needs no cache to expire.
#[test]
fn a_suspended_users_pushes_are_all_refused_including_those_admitted_before_the_suspension() {
    let mut g = group();
    let refused = |o: &PushOutcome| matches!(o, PushOutcome::Refused(r) if r.status == 401);
    // Admitted, not yet decided: the suspension lands before their deps are read.
    let waiting: Vec<_> = (0..100u32)
        .map(|i| {
            let r = rid();
            let mut p =
                push(vec![up(&format!("refs/heads/w{i}"), None, Some(oid(i + 1)))], Atomicity::PerRef, who(WRITER, 2));
            p.request = r;
            g.admit(r, p).expect("admitted")
        })
        .collect();
    g.suspend(AccountId(DbId(WRITER)));
    g.run();
    let mut n = 0;
    for t in &waiting {
        let o = g.resolved(*t).expect("resolved");
        assert!(refused(o), "{o:?}");
        n += 1;
    }
    for i in 0..100u32 {
        let (_, o) = commit(
            &mut g,
            push(vec![up(&format!("refs/heads/f{i}"), None, Some(oid(i + 1)))], Atomicity::PerRef, who(WRITER, 2)),
        );
        assert!(refused(&o), "{o:?}");
        n += 1;
    }
    assert_eq!(n, 200);
    assert!(g.refs().is_empty(), "no ref moved: {:?}", g.refs());
    // The owner is not suspended and still pushes.
    let (_, o) =
        commit(&mut g, push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who(OWNER, 1)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
}

/// BF-30's repository half: once `Settings` names the new owner and enterprise, a push's `deps` name them and no
/// longer the old owner, and only the new owner's grants let it write; the old owner's admin is refused.
#[test]
fn after_settings_a_pushs_deps_name_the_new_owner_and_enterprise() {
    use graftyard_git::issues::{MergeMethod, RepoSettings, Visibility};
    use graftyard_git::repo::{RepoEvent, RepoFold, RepoScratch};
    use graftyard_identity::{
        AccountKind, AuthzEvent, AuthzFold, IdentityDep, NamesEvent, NamesFold, OrgRole, RepoPermission, TokenKind,
        TokensEvent, TokensFold,
    };
    use graftyard_raft::{Command, Fold as _};
    use graftyard_types::{ChainId, Index, NameShard, RepoId};
    let (alpha, beta, e2) = (AccountId(DbId(OWNER)), AccountId(DbId(200)), AccountId(DbId(300)));
    let repo = RepoId(DbId(7));
    let mut fold = RepoFold::new(repo, alpha);
    let settings = |owner, enterprise| RepoSettings {
        owner,
        enterprise,
        visibility: Visibility::Private,
        default_branch: name("refs/heads/main"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec::smallvec![MergeMethod::Merge],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    };
    let mut index = 0;
    let mut prev = graftyard_raft::PREV_ZERO;
    let mut commit = |f: &mut RepoFold, ev: RepoEvent| {
        index += 1;
        let e = graftyard_raft::LogEntry {
            term: graftyard_raft::Term(2),
            index: Index(index),
            prev,
            format: graftyard_git::issue_ops::call::REPO_FORMAT_ISSUES,
            tiers: graftyard_raft::TierSet::CORE.union(graftyard_raft::TierSet::METADATA),
            at: Timestamp(index),
            requests: SmallVec::new(),
            extend: None,
            body: graftyard_raft::EntryBody::Events(vec![ev]),
        };
        prev = graftyard_raft::digest(&e);
        f.apply_entry(&e);
    };
    // Every user's account and token, and each org's `Authz`: OWNER admins alpha, WRITER has Write on beta through a
    // team-less grant, and beta belongs to e2.
    let mut names = NamesFold::new(NameShard(0));
    let mut tokens = TokensFold::new(graftyard_types::TokenShard(0));
    for (a, n) in [(OWNER, 1u8), (WRITER, 2)] {
        let id = AccountId(DbId(a));
        names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
        let ev = TokensEvent::Stored {
            hash: hash(n),
            kind: TokenKind::Pat,
            account: id,
            scopes: ScopeSet(1),
            expires: None,
        };
        tokens.apply(&ev);
    }
    let org = |o: AccountId, e: Option<AccountId>| {
        let mut f = AuthzFold::new(o);
        f.apply(&AuthzEvent::Opened { kind: AccountKind::Org, enterprise: e });
        f
    };
    let mut fa = org(alpha, None);
    fa.apply(&AuthzEvent::Member { user: UserId(alpha), role: Some(OrgRole::Admin) });
    let mut fb = org(beta, Some(e2));
    fb.apply(&AuthzEvent::Member { user: UserId(AccountId(DbId(WRITER))), role: Some(OrgRole::Member) });
    let w = UserId(AccountId(DbId(WRITER)));
    fb.apply(&AuthzEvent::Grant { repo, grantee: w, permission: Some(RepoPermission::Write) });
    let fe = org(e2, None);
    let read = |c: ChainId| match c {
        ChainId::Names(_) => IdentityDep::Names(names.clone()),
        ChainId::Tokens(s) => {
            let mut t = tokens.clone();
            t.shard = Some(s);
            IdentityDep::Tokens(t)
        }
        ChainId::Authz(o) if o == alpha => IdentityDep::Authz(fa.clone()),
        ChainId::Authz(o) if o == beta => IdentityDep::Authz(fb.clone()),
        ChainId::Authz(o) if o == e2 => IdentityDep::Authz(fe.clone()),
        other => panic!("{other}"),
    };
    let decide = |f: &RepoFold, p: &Push| {
        let deps: Vec<_> = p.deps(f).into_iter().map(|c| (c, read(c))).collect();
        let s: RepoScratch<'_> = f.scratch();
        (p.deps(f), p.decide(&s, &deps).map(|_| ()))
    };
    let one = |who: Principal| push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who);
    // Before the transfer: the owner's admin writes; WRITER, granted only on beta, does not.
    let (d, r) = decide(&fold, &one(who(OWNER, 1)));
    assert!(d.contains(&ChainId::Authz(alpha)) && r.is_ok(), "{d:?} {r:?}");
    assert!(decide(&fold, &one(who(WRITER, 2))).1.is_err());
    // `Settings` names alpha and no enterprise, then the transfer's `Settings` names beta and e2.
    commit(&mut fold, RepoEvent::Settings(settings(alpha, None)));
    commit(&mut fold, RepoEvent::Settings(settings(beta, Some(e2))));
    let (d, r) = decide(&fold, &one(who(WRITER, 2)));
    assert!(d.contains(&ChainId::Authz(beta)) && d.contains(&ChainId::Authz(e2)), "{d:?}");
    assert!(!d.contains(&ChainId::Authz(alpha)), "no stale owner: {d:?}");
    assert!(r.is_ok(), "{r:?}");
    let (_, r) = decide(&fold, &one(who(OWNER, 1)));
    assert!(r.is_err(), "the old owner's admin no longer writes: {r:?}");
}

// ---- M3-WP4: pushes under a stateless installation token ----

fn installation_claims() -> graftyard_identity::stateless::InstallationClaims {
    use graftyard_identity::{Access, AppId, InstallationId, Jti, Kid, PermissionSet, RepoScope};
    graftyard_identity::stateless::InstallationClaims {
        app: AppId(DbId(900)),
        installation: InstallationId(DbId(901)),
        repos: RepoScope::Selected(smallvec::smallvec![graftyard_types::RepoId(DbId(7))]),
        permissions: PermissionSet([(Box::from("contents"), Access::Write)].into_iter().collect()),
        jti: Jti(Ulid(0x0193_0000_0000_0000_0000_0000_0000_0001)),
        iat: Timestamp(9_000),
        exp: Timestamp(9_000 + 3_600_000),
        kid: Kid(1),
        task: None,
    }
}

/// A group whose release reads the stateless push format, as one does after its site day.
fn stateless_group() -> StandIn {
    let mut s = seed();
    let f = graftyard_git::repo::REPO_FORMAT_STATELESS;
    s.reads.push((f.family, f.version));
    let mut g = StandIn::restore(&s, Vec::new(), Timestamp(10_000));
    assert!(g.ensure_leader());
    g
}

fn installed_group() -> StandIn {
    let mut g = stateless_group();
    let c = installation_claims();
    g.install(
        c.installation,
        graftyard_identity::InstallationRow { app: c.app, repos: c.repos.clone(), permissions: c.permissions.clone() },
    );
    g
}

/// The push format of a stateless principal carries the App and the claims, and round-trips; versions 1 and 2 do
/// not decode one.
#[test]
fn a_push_under_an_installation_token_has_its_own_format_and_round_trips() {
    let p = push(
        vec![up("refs/heads/a", None, Some(oid(1)))],
        Atomicity::PerRef,
        Principal::installation(&installation_claims()),
    );
    assert_eq!(graftyard_raft::Command::format(&p), graftyard_git::repo::REPO_FORMAT_STATELESS);
    assert_eq!(Push::from_wire(&p.to_wire()), Ok(p.clone()));
    let mut w = p.to_wire();
    w[3] = 1;
    assert!(Push::from_wire(&w).is_err(), "format 1 does not carry an App actor");
}

/// BF-27 on the repository chain: 1,000 pushes under one installation token are admitted and wait; its `jti` is
/// revoked on `jti_shard`; every one of them, and every push after, is refused with 401. Before the revocation the
/// token pushes to a repository its installation selects.
#[test]
fn bf27_a_revoked_jti_refuses_all_1000_queued_pushes() {
    let mut g = installed_group();
    let c = installation_claims();
    let who = || Principal::installation(&c);
    let (_, o) = commit(&mut g, push(vec![up("refs/heads/main", None, Some(oid(1)))], Atomicity::PerRef, who()));
    assert_eq!(refs_of(&o), [RefOutcome::Ok], "the token pushes before the revocation");
    let waiting: Vec<_> = (0..1_000u32)
        .map(|i| {
            let r = rid();
            let mut p = push(vec![up(&format!("refs/heads/q{i}"), None, Some(oid(i + 2)))], Atomicity::PerRef, who());
            p.request = r;
            g.admit(r, p).expect("admitted")
        })
        .collect();
    g.revoke_jti(c.jti, c.exp);
    g.run();
    let refused =
        waiting.iter().filter(|t| matches!(g.resolved(**t), Some(PushOutcome::Refused(r)) if r.status == 401)).count();
    assert_eq!(refused, 1_000);
    let (_, o) = commit(&mut g, push(vec![up("refs/heads/late", None, Some(oid(1)))], Atomicity::PerRef, who()));
    assert!(matches!(&o, PushOutcome::Refused(r) if r.status == 401), "{o:?}");
    assert_eq!(g.refs().len(), 1, "only the push before the revocation moved a ref: {:?}", g.refs());
    eprintln!("BF-27 push: {refused}/1000 queued pushes refused 401 after JtiRevoked");
}

/// An installation token reaches only what its claims and the installation's row both select, and an App actor
/// with a stored-token credential, or no installation, is refused.
#[test]
fn an_installation_token_pushes_only_inside_its_claims_and_installation() {
    let c = installation_claims();
    // A release that does not read the format answers `Unsupported`, and nothing is decided.
    let mut g = group();
    let p = push(vec![up("refs/heads/a", None, Some(oid(1)))], Atomicity::PerRef, Principal::installation(&c));
    assert!(matches!(g.commit(rid(), p), CommitReply::Error(CallErrorWire::Unsupported(_))));
    // Not installed on the owner: 404-class refusal.
    let mut g = stateless_group();
    let (_, o) = commit(
        &mut g,
        push(vec![up("refs/heads/a", None, Some(oid(1)))], Atomicity::PerRef, Principal::installation(&c)),
    );
    assert!(matches!(&o, PushOutcome::Refused(r) if r.status == 403 || r.status == 404), "{o:?}");
    // Claims that select another repository.
    let mut g = installed_group();
    let other = graftyard_identity::stateless::InstallationClaims {
        repos: graftyard_identity::RepoScope::Selected(smallvec::smallvec![graftyard_types::RepoId(DbId(8))]),
        ..c.clone()
    };
    let (_, o) = commit(
        &mut g,
        push(vec![up("refs/heads/a", None, Some(oid(1)))], Atomicity::PerRef, Principal::installation(&other)),
    );
    assert!(matches!(&o, PushOutcome::Refused(r) if r.status == 403 || r.status == 404), "{o:?}");
    // Read-only claims.
    let ro = graftyard_identity::stateless::InstallationClaims {
        permissions: graftyard_identity::PermissionSet(
            [(Box::from("contents"), graftyard_identity::Access::Read)].into_iter().collect(),
        ),
        ..c.clone()
    };
    let (_, o) = commit(
        &mut g,
        push(vec![up("refs/heads/a", None, Some(oid(1)))], Atomicity::PerRef, Principal::installation(&ro)),
    );
    assert!(matches!(&o, PushOutcome::Refused(r) if r.status == 403 || r.status == 404), "{o:?}");
    assert!(g.refs().is_empty());
}

/// BF-31 on the repository chain: a `github_pat_` pushes only to its `resource_owner`'s repositories in `repos`, and
/// only with `contents: write`; its user's own `Write` grant does not widen it.
#[test]
fn a_fine_grained_token_pushes_only_inside_its_scope_and_permissions() {
    use graftyard_identity::{Access, PermissionSet, RepoScope, TokenFields, TokenKind};
    let mut g = group();
    let writer = AccountId(DbId(WRITER));
    let scoped = |n: u8, repo: i64, level: Access| {
        (
            hash(n),
            TokenFields {
                resource_owner: Some(AccountId(DbId(OWNER))),
                repos: Some(RepoScope::Selected(smallvec::smallvec![graftyard_types::RepoId(DbId(repo))])),
                permissions: Some(PermissionSet([(Box::from("contents"), level)].into_iter().collect())),
                ..TokenFields::default()
            },
        )
    };
    let status = |o: &PushOutcome| match o {
        PushOutcome::Refused(r) => r.status,
        _ => 200,
    };
    let one = |r: &str| vec![up(r, None, Some(oid(1)))];
    for (n, repo, level, want) in [(20, 7, Access::Read, 403), (21, 8, Access::Write, 403), (22, 7, Access::Write, 200)]
    {
        let (h, f) = scoped(n, repo, level);
        g.store_token(h, TokenKind::FineGrained, writer, f);
        let (_, o) = commit(&mut g, push(one(&format!("refs/heads/t{n}")), Atomicity::PerRef, who(WRITER, n)));
        assert_eq!(status(&o), want, "token {n}: repo {repo}, contents {level:?}: {o:?}");
    }
    // A `contents: write` token for another owner's repositories, naming no repositories, does not reach this one.
    let (h, mut f) = scoped(23, 7, Access::Write);
    f.resource_owner = Some(AccountId(DbId(OWNER + 1000)));
    f.repos = None;
    g.store_token(h, TokenKind::FineGrained, writer, f);
    let (_, o) = commit(&mut g, push(one("refs/heads/t23"), Atomicity::PerRef, who(WRITER, 23)));
    assert_eq!(status(&o), 403, "another owner's token: {o:?}");
    // A `ghp_` of the same user is limited by the user's grant alone.
    let (_, o) = commit(&mut g, push(one("refs/heads/classic"), Atomicity::PerRef, who(WRITER, 2)));
    assert_eq!(refs_of(&o), [RefOutcome::Ok]);
}

/// BF-2 in memory: 100 pushes under one token wait in admission; the token is revoked; every one is refused 401, so
/// none is acknowledged at an index after the revocation's, and the user's other token still pushes.
#[test]
fn bf2_a_revoked_token_refuses_all_100_queued_pushes() {
    let mut g = group();
    let waiting: Vec<_> = (0..100u32)
        .map(|i| {
            let r = rid();
            let mut p =
                push(vec![up(&format!("refs/heads/q{i}"), None, Some(oid(i + 1)))], Atomicity::PerRef, who(WRITER, 2));
            p.request = r;
            g.admit(r, p).expect("admitted")
        })
        .collect();
    g.revoke(hash(2));
    g.run();
    let refused =
        waiting.iter().filter(|t| matches!(g.resolved(**t), Some(PushOutcome::Refused(r)) if r.status == 401)).count();
    assert_eq!(refused, 100);
    assert!(g.refs().is_empty(), "no queued push moved a ref: {:?}", g.refs());
    let (_, o) =
        commit(&mut g, push(vec![up("refs/heads/late", None, Some(oid(1)))], Atomicity::PerRef, who(WRITER, 2)));
    assert!(matches!(&o, PushOutcome::Refused(r) if r.status == 401), "{o:?}");
}

/// `Fanout { k }` (format 32.15) sets the core `fanout` row, which `fanout_k` reads and a shard's `ranges` read
/// answers; before any, k is 1. It changes no ref and its bytes are tag 39 then k.
#[test]
fn a_fanout_entry_sets_the_committed_k() {
    use graftyard_git::repo::{REPO_FORMAT_FANOUT, RepoEvent, RepoFold};
    use graftyard_raft::Fold;
    use graftyard_types::{Index, RepoId};

    let mut fold = RepoFold::new(RepoId(DbId(7)), AccountId(DbId(OWNER)));
    assert_eq!(fold.fanout_k(), 1);
    let mut prev = graftyard_raft::PREV_ZERO;
    for (i, k) in [3u8, 2].into_iter().enumerate() {
        let ev = RepoEvent::Fanout { k };
        let mut b = Vec::new();
        ev.encode(&mut b);
        assert_eq!(b, vec![39, k]);
        let e = graftyard_raft::LogEntry {
            term: graftyard_raft::Term(2),
            index: Index(i as u64 + 1),
            prev,
            format: REPO_FORMAT_FANOUT,
            tiers: graftyard_raft::TierSet::CORE,
            at: Timestamp(1),
            requests: SmallVec::new(),
            extend: None,
            body: graftyard_raft::EntryBody::Events(vec![ev]),
        };
        prev = graftyard_raft::digest(&e);
        fold.apply_entry(&e);
        assert_eq!(fold.fanout_k(), k);
    }
    assert!(fold.refs().is_empty());
    assert_eq!(graftyard_git::tiers::tier_of(&RepoEvent::Fanout { k: 2 }), graftyard_raft::TierSet::CORE);
}
