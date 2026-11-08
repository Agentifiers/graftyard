//! AM1-WP2's byte-identity gate: the default (non-agent) forms the agent layer must not change, pinned to the bytes
//! `main` (49d63f6) produced before the agent code existed. A push, issue command, and merge command by a user or a
//! plain App; the events they commit; the entries' digests and both fold tiers' digests after a scripted run; and the
//! report-status bytes. `GRAFTYARD_PIN=1` prints the current values instead of checking them.

use graftyard_fold::fold_digest;
use graftyard_git::issue_ops::call::IssueCall;
use graftyard_git::issue_ops::{IssueOp, RepoView};
use graftyard_git::issues::{
    CommentTarget, HeadSpec, IssueCapture, IssueNumber, MergeMethod, PrCapture, PrState, RepoSettings, ReviewEvent,
    ReviewId, ReviewState, Visibility,
};
use graftyard_git::merge::{EngineVersion, Merge, MergeCmd, MergePlan, TreeDiffDigest};
use graftyard_git::merge_call::MergeCall;
use graftyard_git::pack::{DurablePack, PackId, PackRef};
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{Push, RejectReason, RepoCmd, RepoEvent, RepoFold};
use graftyard_identity::{Actor, Credential, Principal, ScopeSet, UserId};
use graftyard_raft::{EntryBody, Format, LogEntry, PREV_ZERO, Record, Term, TierSet, digest};
use graftyard_types::{AccountId, DbId, Digest, Index, RepoId, RequestId, SiteSet, Tier, Timestamp, Ulid};
use smallvec::{SmallVec, smallvec};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn name(s: &str) -> RefName {
    RefName::parse(s.as_bytes()).unwrap()
}

fn user(a: i64) -> Principal {
    Principal {
        actor: Actor::User(UserId(AccountId(DbId(a)))),
        scopes: ScopeSet(1),
        credential: Credential::Stored([a as u8; 32]),
        sso_orgs: SmallVec::new(),
    }
}

fn app() -> Principal {
    use graftyard_identity::{Access, AppId, InstallationId, Jti, Kid, PermissionSet, RepoScope};
    Principal::installation(&graftyard_identity::stateless::InstallationClaims {
        app: AppId(DbId(900)),
        installation: InstallationId(DbId(901)),
        repos: RepoScope::Selected(smallvec![RepoId(DbId(7))]),
        permissions: PermissionSet([(Box::from("contents"), Access::Write)].into_iter().collect()),
        jti: Jti(Ulid(0x0193_0000_0000_0000_0000_0000_0000_0001)),
        iat: Timestamp(9_000),
        exp: Timestamp(9_000 + 3_600_000),
        kid: Kid(1),
        task: None,
    })
}

fn push(actor: Principal, agent: Option<&str>) -> Push {
    Push {
        updates: vec![
            RefUpdate { name: name("refs/heads/main"), expect: None, new: Some(ObjectId([0xab; 20])) },
            RefUpdate { name: name("refs/heads/claude/x"), expect: Some(ObjectId([1; 20])), new: None },
        ],
        atomicity: Atomicity::PerRef,
        pack: Some(PackRef::Inline(b"PACK".to_vec())),
        actor,
        options: vec!["ci.skip".into()],
        request: RequestId(Ulid(7 << 64)),
        agent: agent.map(Into::into),
        provenance: Vec::new(),
        protected: Vec::new(),
        changed: Vec::new(),
    }
}

fn plan() -> MergePlan {
    MergePlan {
        base_old: ObjectId([1; 20]),
        head: ObjectId([2; 20]),
        commit: ObjectId([3; 20]),
        pack: DurablePack::trusted(PackRef::Stored {
            id: PackId(Ulid(9)),
            checksum: Digest([4; 32]),
            bytes: 10,
            sites: SiteSet(3),
        }),
        method: MergeMethod::Squash,
        up_to_date: true,
        verified: TreeDiffDigest(Digest([5; 32])),
        engine: EngineVersion("git 2.51".into()),
    }
}

fn settings() -> RepoSettings {
    RepoSettings {
        owner: AccountId(DbId(100)),
        enterprise: None,
        visibility: Visibility::Private,
        default_branch: name("refs/heads/main"),
        archived: false,
        description: None,
        homepage: None,
        topics: SmallVec::new(),
        has_issues: true,
        has_wiki: false,
        has_projects: false,
        merge_methods: smallvec![MergeMethod::Merge],
        delete_branch_on_merge: false,
        allow_auto_merge: false,
        allow_update_branch: false,
        check_retention_days: 90,
    }
}

/// Every default command form, as its stored record (`Record::encode`) and its member wire JSON.
fn commands() -> Vec<(&'static str, RepoCmd)> {
    let review = IssueOp::Review {
        n: IssueNumber(1),
        state: ReviewState::Approved,
        commit: ObjectId([2; 20]),
        body: "ok".into(),
    };
    let comment = IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "hi".into(), reply_to: None };
    let open = IssueOp::OpenPr {
        head: HeadSpec { repo: RepoId(DbId(7)), branch: name("refs/heads/claude/x") },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "b".into(),
        draft: true,
        head_sha: ObjectId([2; 20]),
        base_sha: ObjectId([1; 20]),
    };
    let call = |op: IssueOp<graftyard_git::blob::BlobRef>| {
        RepoCmd::Issue(Box::new(IssueCall::new(op, user(101), RequestId(Ulid(5)), serde_json::json!({ "w": 1 }))))
    };
    let merge = Merge { n: IssueNumber(1), plan: plan(), expected_head: Some(ObjectId([2; 20])), actor: user(101) };
    vec![
        ("push v1 user", RepoCmd::Push(push(user(101), None))),
        ("push v2 user", RepoCmd::Push(push(user(101), Some("git/2.51")))),
        ("push stateless app", RepoCmd::Push(push(app(), Some("git/2.51")))),
        ("issue review", call(review)),
        ("issue comment", call(comment)),
        ("issue open pr", call(open)),
        ("issue settings", call(IssueOp::Settings(settings()))),
        ("merge", RepoCmd::Merge(Box::new(MergeCall { cmd: MergeCmd::Merge(merge), render: serde_json::json!(null) }))),
    ]
}

/// The events a default run commits, in order.
fn events() -> Vec<RepoEvent> {
    let issue = IssueCapture {
        id: DbId(1 << 40),
        author: Some(AccountId(DbId(101))),
        title: "t".into(),
        body: "b".into(),
        labels: SmallVec::new(),
        assignees: SmallVec::new(),
    };
    let capture = PrCapture {
        head_sha: ObjectId([2; 20]),
        base_sha: ObjectId([1; 20]),
        state: PrState::Open { draft: false },
        labels: Vec::new(),
        issue,
        changes_from: None,
    };
    vec![
        RepoEvent::Refs {
            push: RequestId(Ulid(7 << 64)),
            actor: Some(AccountId(DbId(101))),
            updates: vec![RefUpdate { name: name("refs/heads/main"), expect: None, new: Some(ObjectId([1; 20])) }],
            pack: Some(PackRef::Inline(b"PACK".to_vec())),
        },
        RepoEvent::Refs {
            push: RequestId(Ulid(8 << 64)),
            actor: None,
            updates: vec![RefUpdate { name: name("refs/heads/claude/x"), expect: None, new: Some(ObjectId([2; 20])) }],
            pack: None,
        },
        RepoEvent::Refused { push: RequestId(Ulid(9)), refs: vec![(name("refs/pull/1/head"), RejectReason::Hidden)] },
        RepoEvent::Settings(settings()),
        RepoEvent::PrOpened {
            n: IssueNumber(1),
            head: HeadSpec { repo: RepoId(DbId(7)), branch: name("refs/heads/claude/x") },
            base: name("refs/heads/main"),
            capture,
        },
        RepoEvent::Review(ReviewEvent::Submitted {
            id: ReviewId(DbId((1 << 40) + 1)),
            n: IssueNumber(1),
            reviewer: Some(AccountId(DbId(102))),
            state: ReviewState::Approved,
            commit: ObjectId([2; 20]),
            body: "lgtm".into(),
        }),
    ]
}

fn entry(index: u64, prev: graftyard_types::Digest, evs: Vec<RepoEvent>) -> LogEntry<RepoEvent> {
    LogEntry {
        term: Term(2),
        index: Index(index),
        prev,
        format: Format { family: 0x20, version: 5 },
        tiers: TierSet::CORE.union(TierSet::METADATA),
        at: Timestamp(1_000 * index),
        requests: smallvec![(RequestId(Ulid(u128::from(index))), 0..evs.len() as u32)],
        extend: None,
        body: EntryBody::Events(evs),
    }
}

/// The run's entries (an id block, then one entry per event), their digests, and both tiers' digests after.
fn run() -> Vec<String> {
    let mut fold = RepoFold::new(RepoId(DbId(7)), AccountId(DbId(100)));
    let mut out = Vec::new();
    let block = LogEntry {
        term: Term(2),
        index: Index(1),
        prev: PREV_ZERO,
        format: Format { family: 0x20, version: 5 },
        tiers: TierSet::CORE.union(TierSet::METADATA),
        at: Timestamp(1),
        requests: SmallVec::new(),
        extend: None,
        body: EntryBody::IdBlock(graftyard_identity::IdBlock::whole(0, 1).unwrap()),
    };
    graftyard_raft::Fold::apply_entry(&mut fold, &block);
    let mut prev = digest(&block);
    for (i, ev) in events().into_iter().enumerate() {
        let e = entry(i as u64 + 2, prev, vec![ev]);
        graftyard_raft::Fold::apply_entry(&mut fold, &e);
        prev = digest(&e);
        out.push(prev.hex());
    }
    out.push(fold_digest(&*fold.core, Tier::Core).hex());
    out.push(fold_digest(&*fold.meta, Tier::Metadata).hex());
    // A view over the result reads the review verdict, as `Merge` does.
    let v = RepoView::new(
        fold.tiers(),
        &fold.core as &dyn graftyard_fold::Read,
        &fold.meta as &dyn graftyard_fold::Read,
        Timestamp(0),
    );
    out.push(format!("{:?}", v.ref_value(&name("refs/heads/main"))));
    out
}

fn current() -> Vec<String> {
    let mut out = Vec::new();
    for (what, c) in commands() {
        let mut b = Vec::new();
        c.encode(&mut b);
        out.push(format!("{what} record {}", hex(&b)));
        out.push(format!("{what} wire {}", serde_json::to_string(&c).unwrap()));
        out.push(format!("{what} format {:?}", graftyard_raft::Command::format(&c)));
    }
    for ev in events() {
        let mut b = Vec::new();
        ev.encode(&mut b);
        out.push(format!("event {}", hex(&b)));
    }
    out.extend(run());
    out
}

#[test]
fn default_commands_events_and_folds_keep_mains_bytes() {
    let got = current();
    if std::env::var("GRAFTYARD_PIN").is_ok() {
        for l in &got {
            println!("{l}");
        }
        return;
    }
    let want: Vec<&str> = include_str!("agent_bytes.golden").lines().collect();
    assert_eq!(got.len(), want.len(), "the pinned set changed size");
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g, w);
    }
}
