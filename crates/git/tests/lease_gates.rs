//! AM1-WP4's lease rules in memory, on the same real members as `agent_gates.rs` and `task_gates.rs` (`common`): three
//! `graftyard_raft::Member<AuthzCmd>` of the install's chain and three `Member<RepoCmd>` per repository, whose leader
//! reads the install's chain through `fresh` once per entry.
//!
//! - LF-5: `graftyard-plan` blocks on draft pull requests.
//! - L0-13 (lease limits): bad Exclusive acquires, App tokens with no session, plan blocks from non-writers and
//!   writers, a removed holder, an uninstalled App, and an Exclusive lease past `EXCLUSIVE_MAX`.
//! - L0-12 (leases and the feed), the pure parts: 100 conflicting pushes and 100 conflicting REST ref writes per actor
//!   kind under `AgentsExclusive`, an idle 60 s lease followed 10 minutes later by an overlapping Exclusive acquire,
//!   and a 30-minute stall with member calls blackholed, after which every lease live before it is live with
//!   `expires` moved by the stall.
//! - L0-16 (core tier alone): lease checks and agent pushes decide with the metadata tier unloaded, reading it zero
//!   times.
//! - Renew and release rules, a task's cancel releasing its leases, and the feed events of real entries.

mod common;
use common::*;

use std::cell::Cell;
use std::rc::Rc;

use graftyard_fold::{MemStore, Store, TierStore};
use graftyard_git::feed::{self, ActivityEvent, LeaseChange};
use graftyard_git::lease::{
    self, EXCLUSIVE_MAX, LEASES_PER_HOLDER, LeaseErr, LeaseId, LeaseMode, LeaseView, NewLease, PLAN_LEASES_PER_REPO,
};
use graftyard_git::task::{self, AgentTask, NewTask, TaskActor, TaskApiState, TaskKind};
use graftyard_identity::AuthzRead;
use graftyard_identity::SessionId;
use graftyard_types::Index;

const RUN_TASK: u128 = 0x0193_a1b2_c3d4_8e5f_8a9b_0c1d_2e3f_4a5b;
const N: usize = 100;

type Op = IssueOp<graftyard_git::blob::BlobRef>;

fn uid(a: i64) -> UserId {
    UserId(AccountId(DbId(a)))
}

fn run(k: u128) -> SessionId {
    SessionId::Run(RUN_TASK + k)
}

fn ext(app: AppId, name: &str) -> SessionId {
    SessionId::External { app, name: name.into() }
}

fn claimed(a: AppId, s: Option<&SessionId>) -> Principal {
    let mut c = claims(a);
    c.task = s.cloned();
    Principal::installation(&c)
}

fn new_lease(paths: &[&str], mode: LeaseMode, session: Option<&str>, ttl: Option<u32>) -> Op {
    IssueOp::AcquireLease(NewLease {
        paths: paths.iter().map(|p| Box::from(*p)).collect(),
        refs: Vec::new(),
        mode,
        ttl_seconds: ttl,
        intent: "".into(),
        session: session.map(Box::from),
    })
}

fn shared(paths: &[&str], session: Option<&str>) -> Op {
    new_lease(paths, LeaseMode::Shared, session, None)
}

fn exclusive(paths: &[&str], session: Option<&str>) -> Op {
    new_lease(paths, LeaseMode::Exclusive, session, None)
}

fn lease_id(o: &RepoOutcome) -> LeaseId {
    match o {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Lease(id) => id,
            ref x => panic!("not a lease: {x:?}"),
        },
        other => panic!("refused: {other:?}"),
    }
}

fn lease_err(o: &RepoOutcome) -> Option<LeaseErr> {
    match o {
        Err(RepoErr::Issue { err: IssueErr::Lease(e), .. }) => Some(e.clone()),
        _ => None,
    }
}

/// An issue command as the edge sends it once 32.14 is enabled: marked `leases` when `lease::marks` says its entry may
/// write lease events.
fn marked(w: &mut World, who: Principal, op: Op) -> RepoCmd {
    let f = w.fold();
    let mut c = IssueCall::new(op, who, RequestId(Ulid(0)), serde_json::Value::Null);
    c.agent = matches!(c.op, IssueOp::OpenPr { .. } | IssueOp::Review { .. });
    c.leases = lease::marks(&c.op, &*f.core);
    RepoCmd::Issue(Box::new(c))
}

fn view_of<'a>(w: &'a World, f: &'a RepoFold) -> LeaseView<'a> {
    LeaseView { core: &f.core, owner: &w.ident.owner, repo: w.current, at: w.now }
}

fn live(w: &mut World) -> Vec<lease::LeaseRow> {
    let f = w.fold();
    view_of(w, &f).live()
}

fn seed_run_task(w: &mut World, s: &SessionId, requester: i64) {
    let t = AgentTask {
        session: s.clone(),
        kind: TaskKind::GraftyardRun,
        state: TaskApiState::InProgress,
        prefix: task::run_prefix(s),
        requester: Some(uid(requester)),
        app: RUN,
        installation: claims(RUN).installation,
        creator: TaskActor::User(uid(requester)),
        issue: None,
        snapshot: Index(1),
        prompt: Some("work".into()),
        pr: None,
        summary: "".into(),
    };
    w.record_in(graftyard_git::repo::REPO_FORMAT_TASKS, vec![RepoEvent::TaskCreated(t)]);
}

fn create_ext(w: &mut World, app_id: AppId, name: &str, requester: Option<i64>) {
    let op = IssueOp::CreateTask(NewTask {
        session: name.into(),
        requester: requester.map(uid),
        issue: None,
        summary: "".into(),
    });
    w.commit(issue(app(app_id), op)).expect("an external task");
}

fn set_policy(w: &mut World, leases: agent::LeaseEnforce) {
    let policy = AgentPolicy { leases, ..AgentPolicy::default() };
    let mut c =
        IssueCall::new(IssueOp::SetAgentPolicy(policy), user(OWNER), RequestId(Ulid(0)), serde_json::Value::Null);
    c.agent = true;
    w.commit(RepoCmd::Issue(Box::new(c))).expect("the admin sets the policy");
}

/// The Graftyard-run App with two live tasks, the confined vendor App, an App flagged without `confine`, and the
/// install's `Graftyard MCP` (PLAIN, registered as the install's own), every one installed on the repository.
fn lease_world() -> World {
    let mut w = World::new();
    register_own(&mut w, RUN, "graftyard-agent");
    w.flag(AgentApp { app: RUN, kind: AgentKind::GraftyardRun, confine: None });
    w.flag(claude_row());
    w.flag(AgentApp { app: MCP, kind: AgentKind::ThirdParty, confine: None });
    register_own(&mut w, PLAIN, "graftyard-mcp");
    w.flag(AgentApp { app: PLAIN, kind: AgentKind::Mcp, confine: None });
    seed_run_task(&mut w, &run(0), WRITER);
    seed_run_task(&mut w, &run(1), WRITER2);
    w
}

/// A push of `n` refs' creates by `who` touching `paths`, marked 32.14 and sent across the member wire.
fn leased_push(w: &mut World, who: Principal, refs: &[String], paths: &[&str], rest: bool) -> RepoOutcome {
    let ups = refs.iter().map(|r| up(r, None, Some(oid(77)))).collect();
    let mut p = push(who, ups, paths);
    p.leases = true;
    if rest {
        // A REST ref write (`POST`/`PATCH .../git/refs`) is the same `Push`, with no pack.
        p.pack = None;
    }
    let p = graftyard_git::repo::Push::from_wire(&p.to_wire()).expect("crosses the member wire");
    w.commit(RepoCmd::Push(p))
}

// ---- LF-5 ----

fn plan(inner: &str) -> String {
    format!("Plan\n\n```graftyard-plan\n{inner}\n```\n")
}

/// LF-5: a valid block, a repeated key, a bad glob, a second block (after an invalid first one), and a block over
/// 4 KiB, each in a draft pull request's body: one lease, from the valid block, and every body stored unchanged.
#[test]
fn lf5_plan_blocks_make_one_lease_and_leave_bodies_alone() {
    let mut w = lease_world();
    let big = format!("paths: a/**\n{}", "# padding line\n".repeat(300));
    let bodies = [
        ("valid", plan("paths: pkg/auth/**, docs/auth.md\nintent: Rework the session cache")),
        ("repeated key", plan("paths: a/**\npaths: b/**")),
        ("bad glob", plan("paths: /etc/passwd")),
        ("second block", format!("{}\n{}", plan("paths: x\npaths: y"), plan("paths: c/**"))),
        ("over 4 KiB", plan(&big)),
    ];
    let mut numbers = Vec::new();
    for (i, (label, body)) in bodies.iter().enumerate() {
        let br = format!("refs/heads/lf5-{i}");
        w.set(&br, oid(500 + i as u32));
        let op = IssueOp::OpenPr {
            head: HeadSpec { repo: REPO, branch: name(&br) },
            base: name("refs/heads/main"),
            title: (*label).into(),
            body: body.as_str().into(),
            draft: true,
            head_sha: oid(500 + i as u32),
            base_sha: w.get("refs/heads/main").unwrap(),
        };
        let c = marked(&mut w, user(WRITER), op);
        numbers.push(number(&w.commit(c)));
    }
    let f = w.fold();
    let leases = view_of(&w, &f).live();
    assert_eq!(leases.len(), 1, "{leases:?}");
    let l = &leases[0];
    assert_eq!(l.plan, Some(IssueNumber(numbers[0])));
    assert_eq!((l.lease.mode, &*l.lease.intent), (LeaseMode::Shared, "Rework the session cache"));
    assert_eq!(l.lease.scope.paths.iter().map(|p| p.as_str()).collect::<Vec<_>>(), ["pkg/auth/**", "docs/auth.md"]);
    for (n, (_, body)) in numbers.iter().zip(&bodies) {
        let row: tiers::IssueRow = tiers::get_rec(&*f.meta, "issue", &[tiers::n_key(IssueNumber(*n))]).unwrap();
        assert_eq!(&*row.body, body.as_str(), "the body is stored byte for byte");
    }
    // A body edit replaces the editor's plan lease; an edit to an invalid block changes nothing; one with no block
    // releases it.
    let edit = |w: &mut World, b: &str| {
        let op = IssueOp::Change { n: IssueNumber(numbers[0]), change: IssueChange::Body(b.into()) };
        let c = marked(w, user(WRITER), op);
        w.commit(c).unwrap();
    };
    edit(&mut w, &plan("paths: pkg/billing/**"));
    let now = live(&mut w);
    assert_eq!(now.len(), 1);
    assert_ne!(now[0].lease.id, l.lease.id);
    assert_eq!(now[0].lease.scope.paths[0].as_str(), "pkg/billing/**");
    edit(&mut w, &plan("paths: a\npaths: b"));
    assert_eq!(live(&mut w)[0].lease.id, now[0].lease.id, "an invalid block changes nothing");
    edit(&mut w, "no block now");
    assert!(live(&mut w).is_empty(), "a body with no block releases it");
    // A plan lease lives only while its draft is open: marking it ready ends it, with no command on the lease.
    edit(&mut w, &plan("paths: pkg/ready/**"));
    assert_eq!(live(&mut w).len(), 1);
    let ready = IssueOp::Change { n: IssueNumber(numbers[0]), change: IssueChange::Draft(false) };
    w.commit(issue(user(WRITER), ready)).unwrap();
    assert!(live(&mut w).is_empty(), "a readied pull request's plan lease is not held");
    eprintln!(
        "LF-5: 5 draft bodies, 1 lease (the valid block's), 5 bodies byte-identical; edit replaces, invalid keeps, none releases"
    );
}

// ---- L0-13 ----

/// A writer account `a` with its own token, so many writers have distinct credentials.
fn writer(w: &mut World, a: i64, permission: RepoPermission) -> Principal {
    let id = AccountId(DbId(a));
    let mut hash = [0xA5u8; 32];
    hash[..8].copy_from_slice(&a.to_be_bytes());
    w.ident.names.apply(&NamesEvent::Account { id, suspended: false, sessions_not_before: Timestamp(0) });
    let ev = TokensEvent::Stored { hash, kind: TokenKind::Pat, account: id, scopes: ScopeSet(1), expires: None };
    w.ident.tokens.apply(&ev);
    w.ident.owner.apply(&AuthzEvent::Grant { repo: REPO, grantee: UserId(id), permission: Some(permission) });
    Principal {
        actor: Actor::User(UserId(id)),
        scopes: ScopeSet(1),
        credential: Credential::Stored(hash),
        sso_orgs: SmallVec::new(),
    }
}

/// A bad acquire: its label, caller, command, and the refusal it must get.
type Case = (&'static str, Principal, Op, fn(&LeaseErr) -> bool);

/// Each bad acquire 100 times, all refused with its reason and none written.
#[test]
fn l0_13_bad_acquires_are_refused_100_of_100() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "c1", Some(WRITER2));
    create_ext(&mut w, CLAUDE, "gone", None);
    w.commit(issue(app(CLAUDE), IssueOp::CancelTask { id: ext(CLAUDE, "gone") })).unwrap();
    // The task's one Exclusive lease, for the "second Exclusive" case.
    let s0 = run(0);
    lease_id(&w.commit(issue(claimed(RUN, Some(&s0)), exclusive(&["src/core/**"], None))));
    let cases: Vec<Case> = vec![
        ("Exclusive **", claimed(RUN, Some(&run(1))), exclusive(&["**"], None), |e| matches!(e, LeaseErr::Literal(_))),
        ("Exclusive *", claimed(RUN, Some(&run(1))), exclusive(&["*"], None), |e| matches!(e, LeaseErr::Literal(_))),
        ("Exclusive */x", claimed(RUN, Some(&run(1))), exclusive(&["*/x"], None), |e| {
            matches!(e, LeaseErr::Literal(_))
        }),
        ("Exclusive, no task (user)", user(WRITER), exclusive(&["lib/**"], None), |e| *e == LeaseErr::NoLiveTask),
        ("Exclusive, cancelled task", app(CLAUDE), exclusive(&["lib/**"], Some("gone")), |e| {
            *e == LeaseErr::NoLiveTask
        }),
        ("second Exclusive for one task", claimed(RUN, Some(&s0)), exclusive(&["src/other/**"], None), |e| {
            matches!(e, LeaseErr::OnePerTask(_))
        }),
        ("App token, no session", app(CLAUDE), shared(&["lib/**"], None), |e| *e == LeaseErr::NoSession),
        ("App token, another App's session", app(MCP), shared(&["lib/**"], Some("3.c1")), |e| {
            *e == LeaseErr::NoSession
        }),
        ("no push access", user(READER), shared(&["lib/**"], None), |e| *e == LeaseErr::NoPush),
    ];
    let before = live(&mut w).len();
    let mut report = Vec::new();
    for (label, who, op, want) in cases {
        let outs = w.commit_all((0..N).map(|_| issue(who.clone(), op.clone())).collect());
        let refused = outs.iter().filter(|o| lease_err(o).as_ref().is_some_and(want)).count();
        assert_eq!(refused, N, "{label}: {:?}", outs.first());
        report.push(format!("{label} {refused}/{N}"));
    }
    assert_eq!(live(&mut w).len(), before, "nothing written");
    // An `Agent`'s refs stay inside its prefix.
    let mut out_of_prefix = new_lease(&["lib/**"], LeaseMode::Shared, Some("c1"), None);
    if let IssueOp::AcquireLease(n) = &mut out_of_prefix {
        n.refs = vec!["refs/heads/main".into()];
    }
    assert!(matches!(lease_err(&w.commit(issue(app(CLAUDE), out_of_prefix))), Some(LeaseErr::RefScope(_))));
    eprintln!("L0-13 bad acquires: {}", report.join("; "));
}

/// 600 plan blocks from accounts without push access, then 600 from writers: zero leases from the first, and at
/// most PLAN_LEASES_PER_REPO from the second, each within LEASES_PER_HOLDER per writer.
#[test]
fn l0_13_plan_blocks_need_push_and_stop_at_the_repository_cap() {
    const EACH: usize = 600;
    const PER_ACCOUNT: usize = 5;
    let mut w = lease_world();
    let readers: Vec<Principal> =
        (0..EACH / PER_ACCOUNT).map(|i| writer(&mut w, 2_000 + i as i64, RepoPermission::Read)).collect();
    let writers: Vec<Principal> =
        (0..EACH / PER_ACCOUNT).map(|i| writer(&mut w, 3_000 + i as i64, RepoPermission::Write)).collect();
    // Every head branch in one entry.
    let updates: Vec<RefUpdate> =
        (0..2 * EACH).map(|i| up(&format!("refs/heads/plan-{i}"), None, Some(oid(10_000 + i as u32)))).collect();
    w.record(vec![RepoEvent::Refs { push: RequestId(Ulid(9)), actor: None, updates, pack: None }]);
    let base = w.get("refs/heads/main").unwrap();
    let open = |w: &mut World, who: &[Principal], from: usize| -> usize {
        let mut ok = 0;
        for chunk in (0..EACH).collect::<Vec<_>>().chunks(100) {
            let cmds = chunk
                .iter()
                .map(|i| {
                    let k = from + i;
                    let op = IssueOp::OpenPr {
                        head: HeadSpec { repo: REPO, branch: name(&format!("refs/heads/plan-{k}")) },
                        base: name("refs/heads/main"),
                        title: "t".into(),
                        body: plan(&format!("paths: svc/s{k}/**")).into(),
                        draft: true,
                        head_sha: oid(10_000 + k as u32),
                        base_sha: base,
                    };
                    marked(w, who[i / PER_ACCOUNT].clone(), op)
                })
                .collect();
            ok += w.commit_all(cmds).iter().filter(|o| o.is_ok()).count();
        }
        ok
    };
    let opened = open(&mut w, &readers, 0);
    let from_readers = live(&mut w).len();
    assert!(lease::rows(&*w.fold().core).is_empty(), "no lease row is written for a non-writer's block");
    let opened2 = open(&mut w, &writers, EACH);
    let rows = live(&mut w);
    let from_writers = rows.iter().filter(|r| r.plan.is_some()).count();
    assert_eq!((opened, opened2), (EACH, EACH), "every pull request opens");
    assert_eq!(from_readers, 0);
    assert_eq!(from_writers, usize::from(PLAN_LEASES_PER_REPO));
    let mut per: std::collections::BTreeMap<String, usize> = Default::default();
    for r in &rows {
        *per.entry(r.lease.holder.key()).or_default() += 1;
    }
    assert!(per.values().all(|n| *n <= usize::from(LEASES_PER_HOLDER)));
    eprintln!(
        "L0-13 plan blocks: {EACH} from accounts without push -> {from_readers} leases; {EACH} from writers -> \
         {from_writers} leases (cap {PLAN_LEASES_PER_REPO}); all {} pull requests opened",
        2 * EACH
    );
}

/// A holder whose push access is removed refuses nothing at the next `decide`; an uninstalled App's leases and tasks
/// read released and cancelled with no sweep; an Exclusive lease renewed for 5 h reads Shared after 4 h.
#[test]
fn l0_13_removed_uninstalled_and_old_holders_lose_their_hold() {
    let mut w = lease_world();
    set_policy(&mut w, agent::LeaseEnforce::AgentsExclusive);
    // WRITER2's Exclusive lease through its external task with CLAUDE.
    create_ext(&mut w, CLAUDE, "w2", Some(WRITER2));
    let id = lease_id(&w.commit(issue(user(WRITER2), exclusive(&["pkg/pay/**"], Some("3.w2")))));
    let probe = |w: &mut World, i: u32| {
        leased_push(w, app(CLAUDE), &[format!("refs/heads/claude/rm{i}")], &["pkg/pay/a.rs"], false)
    };
    assert_eq!(agent_rule(&probe(&mut w, 0)), Some(AgentRule::Leased));
    w.ident.owner.apply(&AuthzEvent::Grant {
        repo: REPO,
        grantee: uid(WRITER2),
        permission: Some(RepoPermission::Read),
    });
    assert_eq!(refs(&probe(&mut w, 1)), [RefOutcome::Ok], "the removed holder's lease refuses nothing");
    assert!(live(&mut w).iter().all(|r| r.lease.id != id));
    w.ident.owner.apply(&AuthzEvent::Grant {
        repo: REPO,
        grantee: uid(WRITER2),
        permission: Some(RepoPermission::Write),
    });

    // CLAUDE holds 5 tasks, each with a Shared lease, plus the Exclusive one; then it is uninstalled.
    for i in 0..5 {
        create_ext(&mut w, CLAUDE, &format!("u{i}"), Some(WRITER));
        lease_id(&w.commit(issue(app(CLAUDE), shared(&[&format!("u/{i}/**")], Some(&format!("u{i}"))))));
    }
    lease_id(&w.commit(issue(app(CLAUDE), exclusive(&["u/x/**"], Some("u0")))));
    let through = |r: &lease::LeaseRow| {
        r.lease.holder.app == Some(CLAUDE)
            || matches!(&r.lease.holder.session, Some(SessionId::External { app, .. }) if *app == CLAUDE)
    };
    let held = |w: &mut World| live(w).iter().filter(|r| through(r)).count();
    assert_eq!(held(&mut w), 7, "6 of CLAUDE's own and WRITER2's through CLAUDE's task");
    let inst = claims(CLAUDE).installation;
    let row = w.ident.owner.installation(inst).cloned().unwrap().row;
    w.ident.owner.apply(&AuthzEvent::Installation { id: inst, row: None, not_before: w.now });
    assert_eq!(held(&mut w), 0, "uninstall releases every lease");
    let f = w.fold();
    for i in 0..5 {
        let t = task::task_core(&*f.core, &ext(CLAUDE, &format!("u{i}"))).unwrap();
        assert_eq!(task::effective(&t, &w.ident.owner, REPO, w.now), TaskApiState::Cancelled);
    }
    w.ident.owner.apply(&AuthzEvent::Installation { id: inst, row, not_before: w.now });
    assert_eq!(held(&mut w), 0, "a reinstall does not revive them");

    // A Graftyard-run task's Exclusive lease renewed every 50 minutes for 5 h; another task's push into it is refused
    // until EXCLUSIVE_MAX after the grant, then accepted (it reads Shared).
    let s = run(0);
    let hold = new_lease(&["pkg/core/**"], LeaseMode::Exclusive, None, Some(3_600));
    let id = lease_id(&w.commit(issue(claimed(RUN, Some(&s)), hold)));
    let granted = lease::lease_row(&*w.fold().core, id).unwrap().lease.granted;
    let other = run(1);
    let (mut modes, mut k) = (Vec::new(), 0);
    while w.now.0 - granted.0 < 5 * 3_600_000 {
        w.now = Timestamp(w.now.0 + 50 * 60_000);
        k += 1;
        let o = w.commit(issue(claimed(RUN, Some(&s)), IssueOp::RenewLease { id, ttl_seconds: Some(3_600) }));
        assert_eq!(lease_id(&o), id, "renew {k}");
        let f = w.fold();
        let mode = lease::lease_row(&*f.core, id).unwrap().lease.mode_at(w.now);
        let r = format!("refs/heads/graftyard/{}/h{k}", other.text());
        let o = leased_push(&mut w, claimed(RUN, Some(&other)), &[r], &["pkg/core/x.rs"], false);
        let at = w.now.0 - granted.0;
        let refused = agent_rule(&o) == Some(AgentRule::Leased);
        // The push's entry is a settle later than the renew's; judge each by its own time.
        assert_eq!(refused, at < EXCLUSIVE_MAX.as_millis() as u64 && mode == LeaseMode::Exclusive, "renew {k}: {o:?}");
        modes.push((at / 60_000, mode));
    }
    assert!(modes.iter().all(|(m, mode)| (*mode == LeaseMode::Exclusive) == (*m < 240)), "{modes:?}");
    assert!(modes.last().unwrap().0 >= 300);
    // The same for a holder with no task row here (a claim naming a task this repository does not hold), whose lease
    // only the installation's `not_before` ends. Last, because the reinstall outdates the harness's RUN token too.
    let ghost = run(9);
    let g = lease_id(&w.commit(issue(claimed(RUN, Some(&ghost)), shared(&["ghost/**"], None))));
    let rinst = claims(RUN).installation;
    let rrow = w.ident.owner.installation(rinst).cloned().unwrap().row;
    w.ident.owner.apply(&AuthzEvent::Installation { id: rinst, row: None, not_before: w.now });
    w.now = Timestamp(w.now.0 + 1);
    w.ident.owner.apply(&AuthzEvent::Installation { id: rinst, row: rrow, not_before: w.now });
    assert!(live(&mut w).iter().all(|r| r.lease.id != g), "a reinstall does not revive the ghost claim's lease");
    eprintln!(
        "L0-13: removed holder refuses nothing; uninstall released 7 leases and cancelled 5 tasks; (minutes since grant, mode) {modes:?}"
    );
}

// ---- L0-12, the pure parts ----

/// Under `AgentsExclusive`, 100 conflicting pushes and 100 conflicting REST ref writes per actor kind into another
/// task's Exclusive scope: 100 of 100 refused for the two `ActorKind::Agent` kinds, 0 for the other three. Under
/// `Advisory`, none refused.
#[test]
fn l0_12_agents_exclusive_refuses_only_agents() {
    let mut w = lease_world();
    let holder = run(0);
    // Every harness commit settles at least CALL_DEADLINE, so a pass of 1,000 pushes needs an hour's TTL.
    let hold = issue(claimed(RUN, Some(&holder)), new_lease(&["pkg/auth/**"], LeaseMode::Exclusive, None, Some(3_600)));
    let id = lease_id(&w.commit(hold));
    let other = run(1);
    let kinds: Vec<(&str, Principal, String)> = vec![
        ("Graftyard-run App", claimed(RUN, Some(&other)), format!("refs/heads/graftyard/{}/", other.text())),
        ("confined vendor App", app(CLAUDE), "refs/heads/claude/".into()),
        ("flagged App without confine", app(MCP), "refs/heads/bot/".into()),
        ("Graftyard MCP", app(PLAIN), "refs/heads/mcp/".into()),
        ("human user token", user(WRITER2), "refs/heads/human/".into()),
    ];
    let run_all = |w: &mut World, tag: &str| -> Vec<(usize, usize)> {
        kinds
            .iter()
            .map(|(_, who, prefix)| {
                let mut counts = (0, 0);
                for i in 0..N {
                    for rest in [false, true] {
                        let r = format!("{prefix}{tag}{}{i}", if rest { "r" } else { "p" });
                        let o = leased_push(w, who.clone(), &[r], &["pkg/auth/session.rs", "README.md"], rest);
                        let refused = agent_rule(&o) == Some(AgentRule::Leased);
                        if rest { counts.1 += usize::from(refused) } else { counts.0 += usize::from(refused) }
                    }
                }
                counts
            })
            .collect()
    };
    let advisory = run_all(&mut w, "a");
    assert!(advisory.iter().all(|c| *c == (0, 0)), "Advisory refuses nothing: {advisory:?}");
    set_policy(&mut w, agent::LeaseEnforce::AgentsExclusive);
    let renew = IssueOp::RenewLease { id, ttl_seconds: Some(3_600) };
    assert_eq!(lease_id(&w.commit(issue(claimed(RUN, Some(&holder)), renew))), id);
    let enforced = run_all(&mut w, "e");
    assert_eq!(enforced, [(N, N), (N, N), (0, 0), (0, 0), (0, 0)]);
    // The holder's own pushes into its scope pass; a push outside it passes.
    let o = leased_push(
        &mut w,
        claimed(RUN, Some(&holder)),
        &[format!("refs/heads/graftyard/{}/own", holder.text())],
        &["pkg/auth/a"],
        false,
    );
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    let o = leased_push(&mut w, app(CLAUDE), &["refs/heads/claude/outside".into()], &["pkg/authz/a.rs"], false);
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    // A push past PATHS_MAX reports a truncated directory (`pkg/`), which covers every lease under it.
    let mut p = push(app(CLAUDE), vec![up("refs/heads/claude/wide", None, Some(oid(78)))], &[]);
    p.changed =
        vec![ChangedPaths { refname: name("refs/heads/claude/wide"), paths: vec!["pkg/".into()], truncated: true }];
    p.leases = true;
    assert_eq!(agent_rule(&w.commit(RepoCmd::Push(p))), Some(AgentRule::Leased), "a truncated directory");
    // The refusal names the lease, its holder, and its task; per ref in 32.14, so a second ref of the push passes.
    let o = leased_push(
        &mut w,
        app(CLAUDE),
        &["refs/heads/claude/two-a".into(), "refs/heads/claude/two-b".into()],
        &["pkg/auth/x"],
        false,
    );
    let Ok(RepoOut::Push(p)) = &o else { panic!("{o:?}") };
    let RefOutcome::Rejected(RejectReason::Agent(a)) = &p.refs[0].1 else { panic!("{o:?}") };
    assert!(a.text.starts_with("refused: pkg/auth/** is leased Exclusive by app 4 (task "), "{}", a.text);
    assert!(a.text.contains(&holder.text()));
    for (k, c) in kinds.iter().zip(&enforced) {
        eprintln!("L0-12 {}: {}/{N} pushes and {}/{N} REST ref writes refused", k.0, c.0, c.1);
    }
}

/// An idle 60 s lease, then 10 minutes later an overlapping Exclusive acquire by another task: accepted. While the
/// first was live it conflicted with 409's leases.
#[test]
fn l0_12_a_late_exclusive_acquire_after_an_idle_lease_succeeds() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "late", Some(WRITER));
    let first =
        lease_id(&w.commit(issue(
            claimed(RUN, Some(&run(0))),
            new_lease(&["docs/api/**"], LeaseMode::Exclusive, None, Some(60)),
        )));
    // An acquire whose literal prefix is above the lease's still finds it in the index.
    match lease_err(&w.commit(issue(app(CLAUDE), exclusive(&["docs/**"], Some("late"))))) {
        Some(LeaseErr::Conflict(c)) => assert_eq!(c.iter().map(|l| l.id).collect::<Vec<_>>(), [first]),
        other => panic!("{other:?}"),
    }
    // Shared beside it is fine, and lists it as an overlap (`graftyard_api::leases::render_ok`).
    lease_id(&w.commit(issue(app(CLAUDE), shared(&["docs/api/**"], Some("late")))));
    w.now = Timestamp(w.now.0 + 10 * 60_000);
    let late = lease_id(&w.commit(issue(app(CLAUDE), exclusive(&["docs/api/**"], Some("late")))));
    let f = w.fold();
    assert!(lease::lease_row(&*f.core, first).is_none(), "the expired row was pruned");
    assert!(view_of(&w, &f).live().iter().any(|r| r.lease.id == late));
    eprintln!("L0-12: the overlapping Exclusive acquire 10 min after an idle 60 s lease succeeded");
}

/// A 30-minute stall with every member running and member calls blackholed: every lease live before is live after,
/// with `expires` moved by the recorded stall, and the first command after it judges them so.
#[test]
fn l0_12_a_stall_moves_every_live_lease_by_its_length() {
    const STALL: u64 = 30 * 60_000;
    let mut w = lease_world();
    let ids: Vec<LeaseId> = (0..3)
        .map(|i| lease_id(&w.commit(issue(claimed(RUN, Some(&run(0))), shared(&[&format!("st/{i}/**")], None)))))
        .collect();
    let before: Vec<Timestamp> = {
        let f = w.fold();
        ids.iter().map(|id| lease::lease_row(&*f.core, *id).unwrap().lease.expires).collect()
    };
    // The leader appends an entry whose AppendEntries are lost, and the stall runs past every lease's expiry.
    let l = w.repo.leader();
    let now = w.now;
    let rid = w.rid();
    w.repo.on(l, now, |m, t| m.record(t, rid, REPO_FORMAT_V1, vec![RepoEvent::Settings(settings())])).unwrap();
    let stalled_at = w.now;
    w.repo.inbox.clear();
    w.now = Timestamp(w.now.0 + STALL);
    let now = w.now;
    w.repo.on(l, now, |m, t| m.tick(t));
    let marker = w.repo.members[l.0 as usize].state().stalled_since;
    assert_eq!(marker, Some(stalled_at), "the leader recorded the stall at its unacknowledged entry");
    w.settle();
    // The first command after the stall: a renew of the first lease, which `decide` judges with the stall added.
    let o = w.commit(issue(claimed(RUN, Some(&run(0))), IssueOp::RenewLease { id: ids[0], ttl_seconds: Some(600) }));
    assert_eq!(lease_id(&o), ids[0], "{o:?}");
    let lg = w.repo.leader();
    // The group's first election marked a stall too (a campaign start); count the entries after this one.
    let ext = w.repo.members[lg.0 as usize]
        .log()
        .iter()
        .filter(|e| e.at > stalled_at)
        .filter_map(|e| e.extend)
        .collect::<Vec<_>>();
    assert_eq!(ext.len(), 1, "one entry carried the stall: {ext:?}");
    let by = ext[0].by.as_millis() as u64;
    assert!(by >= STALL, "{ext:?}");
    let f = w.fold();
    let rows: Vec<_> = ids.iter().map(|id| lease::lease_row(&*f.core, *id)).collect();
    assert!(rows.iter().all(Option::is_some), "no lease expired by the stall");
    for (r, b) in rows[1..].iter().zip(&before[1..]) {
        assert_eq!(r.as_ref().unwrap().lease.expires, Timestamp(b.0 + by), "moved by the recorded stall");
    }
    assert_eq!(view_of(&w, &f).live().len(), 3);
    assert_markers_clear(&mut w);
    eprintln!("L0-12 stall: 3 of 3 leases live after a {STALL} ms stall, expires moved by {by} ms (one ExtendLeases)");
}

/// The marker clears on every member once it applies the entry carrying `extend`: at once on the leader, and on a
/// follower when the next append tells it that entry committed.
fn assert_markers_clear(w: &mut World) {
    let l = w.repo.leader();
    assert_eq!(w.repo.members[l.0 as usize].state().stalled_since, None, "the leader's marker cleared on commit");
    w.record(vec![RepoEvent::Settings(settings())]);
    w.settle();
    for m in &w.repo.members {
        assert_eq!(m.state().stalled_since, None, "every member's marker cleared once it applied the extend");
    }
}

/// The same stall, ended by a leader change, as the hosted run on graft-m1 ended (`tests/am1/results/am1-hosted.md`,
/// L0-12 stall): the leader holds an entry in flight while every member call is blackholed past LEASE, so no follower
/// sees a `suspect` report. When calls work again a follower wins the next term. The new leader's first entry carries
/// `extend` covering the stall, and every lease live before the stall is live after it with `expires` moved by it.
#[test]
fn l0_12_a_stall_ended_by_a_leader_change_moves_every_live_lease() {
    const STALL: u64 = 30 * 60_000;
    let mut w = lease_world();
    let ids: Vec<LeaseId> = (0..3)
        .map(|i| lease_id(&w.commit(issue(claimed(RUN, Some(&run(0))), shared(&[&format!("lc/{i}/**")], None)))))
        .collect();
    let before: Vec<Timestamp> = {
        let f = w.fold();
        ids.iter().map(|id| lease::lease_row(&*f.core, *id).unwrap().lease.expires).collect()
    };
    let old = w.repo.leader();
    let now = w.now;
    let rid = w.rid();
    w.repo.on(old, now, |m, t| m.record(t, rid, REPO_FORMAT_V1, vec![RepoEvent::Settings(settings())])).unwrap();
    let stalled_at = w.now;
    w.repo.inbox.clear();
    w.now = Timestamp(w.now.0 + STALL);
    let now = w.now;
    // The calls work again: the old leader's tick records the stall at its entry and re-sends it, both followers store
    // it, and their answers are lost. Then the old leader is cut off for good.
    w.repo.on(old, now, |m, t| m.tick(t));
    assert_eq!(w.repo.members[old.0 as usize].state().stalled_since, Some(stalled_at));
    // An edge's `suspect` report reaches the follower that will win before the re-send does: it marks the end of the
    // stall, and the re-send's earlier entry must still win.
    let new = *SITES.iter().find(|s| **s != old).unwrap();
    w.repo.on(new, now, |m, t| m.suspect(t, graftyard_raft::Colo(1), old, m.term()));
    assert_eq!(w.repo.members[new.0 as usize].state().stalled_since, Some(now));
    let resent: Vec<_> = std::mem::take(&mut w.repo.inbox).into_iter().filter(|e| e.from == old).collect();
    assert!(!resent.is_empty(), "the old leader re-sends its entry");
    for e in resent {
        w.repo.on(e.to, now, |m, t| m.step(t, e.from, e.msg));
    }
    w.repo.inbox.clear();
    // Once the followers' promises lapse, a `suspect` report starts a campaign on one of them, which wins.
    w.now = Timestamp(w.now.0 + graftyard_raft::ELECTION_AFTER.as_millis() as u64 + 1_000);
    let now = w.now;
    w.repo.on(new, now, |m, t| m.suspect(t, graftyard_raft::Colo(1), old, m.term()));
    w.repo.deliver(now);
    assert_eq!(w.repo.leader(), new, "a follower took over");
    let term = w.repo.members[new.0 as usize].term();
    let first = w.repo.members[new.0 as usize].log().iter().find(|e| e.term == term).cloned().unwrap();
    assert!(first.index <= w.repo.members[new.0 as usize].state().commit, "the term's first entry committed");
    let ext = first.extend.unwrap_or_else(|| panic!("the new leader's first entry carries no extend: {first:?}"));
    let by = ext.by.as_millis() as u64;
    assert!(ext.since <= stalled_at && by >= STALL, "the extension covers the stall: {ext:?}");
    w.settle();
    let o = w.commit(issue(claimed(RUN, Some(&run(0))), IssueOp::RenewLease { id: ids[0], ttl_seconds: Some(600) }));
    assert_eq!(lease_id(&o), ids[0], "{o:?}");
    let f = w.fold();
    let rows: Vec<_> = ids.iter().map(|id| lease::lease_row(&*f.core, *id)).collect();
    assert!(rows.iter().all(Option::is_some), "no lease expired by the stall");
    for (r, b) in rows[1..].iter().zip(&before[1..]) {
        assert_eq!(r.as_ref().unwrap().lease.expires, Timestamp(b.0 + by), "moved by the recorded stall");
    }
    let all = w.repo.members[w.repo.leader().0 as usize].log().iter().filter(|e| e.at > stalled_at);
    assert_eq!(all.filter(|e| e.extend.is_some()).count(), 1, "one entry carried the stall");
    assert_markers_clear(&mut w);
    eprintln!("L0-12 stall with a leader change: 3 of 3 leases live after {STALL} ms, expires moved by {by} ms");
}

// ---- L0-16 ----

/// A metadata tier that has not loaded, and counts every read.
#[derive(Debug, Clone, Default)]
struct Unloaded(Rc<Cell<u32>>);

impl Store for Unloaded {
    fn get(&self, _: &str, _: &[graftyard_fold::Val]) -> Option<graftyard_fold::Stored> {
        self.0.set(self.0.get() + 1);
        None
    }
    fn put(&mut self, _: &str, _: graftyard_fold::Key, _: graftyard_fold::Stored) {}
    fn delete(&mut self, _: &str, _: &[graftyard_fold::Val]) {}
    fn scan(&self, _: &str, _: &[graftyard_fold::Val], _: usize) -> Vec<(graftyard_fold::Key, graftyard_fold::Stored)> {
        self.0.set(self.0.get() + 1);
        Vec::new()
    }
}

impl TierStore for Unloaded {
    fn clone_box(&self) -> Box<dyn TierStore> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn loaded(&self) -> bool {
        false
    }
}

/// L0-16: pushes and lease commands decide on the leader's core tier with the metadata tier unloaded: every check
/// passes or refuses as on the full fold, with zero metadata loads.
#[test]
fn l0_16_lease_checks_read_the_core_tier_alone() {
    let mut w = lease_world();
    set_policy(&mut w, agent::LeaseEnforce::AgentsExclusive);
    create_ext(&mut w, CLAUDE, "core", Some(WRITER));
    let id = lease_id(&w.commit(issue(claimed(RUN, Some(&run(0))), exclusive(&["pkg/auth/**"], None))));
    let full = w.fold();
    let loads = Rc::new(Cell::new(0));
    let core: Box<dyn TierStore> = full.core.clone();
    let cut =
        RepoFold::with_tiers(full.repo, full.owner, Box::new(MemStore::default()), Box::new(Unloaded(loads.clone())));
    let cut = RepoFold { core, ..cut };
    let install = w.install.fresh(w.now);
    let decide = |f: &RepoFold, c: &RepoCmd| {
        let deps: Vec<_> = c.deps(f).into_iter().map(|ch| (ch, w.ident.dep(ch, &install))).collect();
        let s = graftyard_raft::Fold::scratch_at(f, w.now, graftyard_raft::TierSet(u8::MAX));
        c.decide(&s, &deps).map(|(evs, _)| evs.len())
    };
    let mut p = push(app(CLAUDE), vec![up("refs/heads/claude/c16", None, Some(oid(5)))], &["pkg/auth/x.rs"]);
    p.leases = true;
    let cmds = vec![
        ("conflicting agent push", RepoCmd::Push(p)),
        ("Shared acquire", issue(app(CLAUDE), shared(&["pkg/auth/**"], Some("core")))),
        ("conflicting Exclusive acquire", issue(app(CLAUDE), exclusive(&["pkg/auth/a/**"], Some("core")))),
        ("renew", issue(claimed(RUN, Some(&run(0))), IssueOp::RenewLease { id, ttl_seconds: None })),
        ("release", issue(claimed(RUN, Some(&run(0))), IssueOp::ReleaseLease { id })),
    ];
    for (label, c) in &cmds {
        let a = decide(&full, c);
        let b = decide(&cut, c);
        assert_eq!(a, b, "{label}: the same decision with the metadata tier unloaded");
    }
    assert_eq!(loads.get(), 0, "zero metadata loads");
    eprintln!("L0-16: {} lease-path decisions identical with the metadata tier unloaded, 0 metadata loads", cmds.len());
}

// ---- renew, release, task cancel, feed ----

#[test]
fn renew_and_release_follow_the_holder_and_admin_rules() {
    let mut w = lease_world();
    let s = run(0);
    let me = claimed(RUN, Some(&s));
    let id = lease_id(&w.commit(issue(me.clone(), shared(&["r/**"], None))));
    let renew = |ttl| IssueOp::RenewLease { id, ttl_seconds: ttl };
    assert_eq!(lease_err(&w.commit(issue(user(WRITER), renew(None)))), Some(LeaseErr::NotHolder));
    assert_eq!(lease_err(&w.commit(issue(me.clone(), renew(Some(30))))), Some(LeaseErr::Invalid("ttl_seconds")));
    assert_eq!(lease_err(&w.commit(issue(me.clone(), renew(Some(3_601))))), Some(LeaseErr::Invalid("ttl_seconds")));
    let at = w.now;
    assert_eq!(lease_id(&w.commit(issue(me.clone(), renew(Some(3_600))))), id);
    let row = lease::lease_row(&*w.fold().core, id).unwrap();
    assert!(row.lease.expires.0 >= at.0 + 3_600_000);
    assert_eq!(lease_err(&w.commit(issue(user(WRITER), IssueOp::ReleaseLease { id }))), Some(LeaseErr::NotHolder));
    assert!(w.commit(issue(user(OWNER), IssueOp::ReleaseLease { id })).is_ok(), "a repository admin releases");
    assert_eq!(lease_err(&w.commit(issue(me.clone(), renew(None)))), Some(LeaseErr::NotFound));
    assert_eq!(lease_err(&w.commit(issue(me, IssueOp::ReleaseLease { id }))), Some(LeaseErr::NotFound));
}

/// Cancelling a task releases its leases in the same command (32.14), and the feed reports each lease change from the
/// real entries: acquired, released by the cancel, and expired.
#[test]
fn a_cancel_releases_the_tasks_leases_and_the_feed_reports_each_change() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "feed", Some(WRITER));
    let s = ext(CLAUDE, "feed");
    let feed_of = |w: &mut World, c: RepoCmd| -> Vec<ActivityEvent> {
        let before = w.fold();
        // The member reads the state before an entry at the previous entry's `at`.
        let l = w.repo.leader();
        let at0 = w.repo.members[l.0 as usize].log().last().unwrap().at;
        w.commit(c).unwrap();
        let after = w.fold();
        let l = w.repo.leader();
        let e = w.repo.members[l.0 as usize]
            .log()
            .iter()
            .rev()
            .find(|e| matches!(e.body, graftyard_raft::EntryBody::Events(_)))
            .unwrap()
            .clone();
        let graftyard_raft::EntryBody::Events(evs) = &e.body else { unreachable!() };
        feed::events(REPO, &w.ident.owner, (&before.core, at0), (&after.core, &after.meta, e.at), evs)
    };
    let c = issue(app(CLAUDE), new_lease(&["f/**"], LeaseMode::Shared, Some("feed"), Some(60)));
    let got = feed_of(&mut w, c);
    assert!(matches!(&got[..], [ActivityEvent::Lease { change: LeaseChange::Acquired, .. }]), "{got:?}");
    let c = issue(app(CLAUDE), shared(&["g/**"], Some("feed")));
    feed_of(&mut w, c);
    let c = issue(app(CLAUDE), shared(&["h/**"], Some("feed")));
    let h = match feed_of(&mut w, c).as_slice() {
        [ActivityEvent::Lease { lease, change: LeaseChange::Acquired }] => lease.id,
        other => panic!("{other:?}"),
    };
    let got = feed_of(&mut w, issue(app(CLAUDE), IssueOp::ReleaseLease { id: h }));
    assert!(matches!(&got[..], [ActivityEvent::Lease { change: LeaseChange::Released, .. }]), "{got:?}");
    // A cancel marked 32.14 releases both in its own command.
    let cancel = marked(&mut w, user(WRITER), IssueOp::CancelTask { id: s.clone() });
    let got = feed_of(&mut w, cancel);
    let changes: Vec<_> = got
        .iter()
        .filter_map(|e| match e {
            ActivityEvent::Lease { change, .. } => Some(*change),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [LeaseChange::Released, LeaseChange::Released], "{got:?}");
    assert!(got.iter().any(|e| matches!(e, ActivityEvent::Task { state: TaskApiState::Cancelled, .. })));
    assert!(lease::session_rows(&*w.fold().core, &s).is_empty(), "the rows are gone");
    // Expiry: a 60 s lease, then any entry after it.
    create_ext(&mut w, CLAUDE, "exp", None);
    let c = issue(app(CLAUDE), new_lease(&["e/**"], LeaseMode::Shared, Some("exp"), Some(60)));
    feed_of(&mut w, c);
    w.now = Timestamp(w.now.0 + 61_000);
    let c = issue(app(CLAUDE), shared(&["z/**"], Some("exp")));
    let got = feed_of(&mut w, c);
    assert!(got.iter().any(|e| matches!(e, ActivityEvent::Lease { change: LeaseChange::Expired, .. })), "{got:?}");
    // An agent push writes `PathSet` and an `agent_push` event.
    let br = "refs/heads/claude/feed";
    let mut p = push(app(CLAUDE), vec![up(br, None, Some(oid(91)))], &["f/a.rs", "f/b.rs"]);
    p.leases = true;
    let got = feed_of(&mut w, RepoCmd::Push(p));
    assert!(matches!(&got[..], [ActivityEvent::AgentPush { paths, .. }] if paths.len() == 2), "{got:?}");
    assert!(got[0].matches(&["f/**"]) && !got[0].matches(&["docs/**"]));
}

/// The serving member's `feed::Ring` (AM1 hosted half), fed each applied entry as the site host feeds it, gives the
/// same events as `feed::events` on the same real entries, in order, and serves a poll past a cursor with `paths`.
#[test]
fn the_ring_a_serving_member_keeps_matches_the_pure_feed() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "ring", Some(WRITER));
    let mut ring = feed::Ring::default();
    let mut want: Vec<(graftyard_types::Index, ActivityEvent)> = Vec::new();
    let step = |w: &mut World, ring: &mut feed::Ring, c: RepoCmd, want: &mut Vec<_>| {
        let before = w.fold();
        let l = w.repo.leader();
        let at0 = w.repo.members[l.0 as usize].log().last().unwrap().at;
        w.commit(c).unwrap();
        let after = w.fold();
        let l = w.repo.leader();
        let e = w.repo.members[l.0 as usize]
            .log()
            .iter()
            .rev()
            .find(|e| matches!(e.body, graftyard_raft::EntryBody::Events(_)))
            .unwrap()
            .clone();
        let graftyard_raft::EntryBody::Events(evs) = &e.body else { unreachable!() };
        for x in feed::events(REPO, &w.ident.owner, (&before.core, at0), (&after.core, &after.meta, e.at), evs) {
            want.push((e.index, x));
        }
        ring.feed(REPO, &w.ident.owner, &after.core, &after.meta, (e.index, e.at, evs));
    };
    let c = issue(app(CLAUDE), new_lease(&["f/**"], LeaseMode::Shared, Some("ring"), Some(60)));
    step(&mut w, &mut ring, c, &mut want);
    let start = ring.through;
    let c = issue(app(CLAUDE), shared(&["h/**"], Some("ring")));
    step(&mut w, &mut ring, c, &mut want);
    w.now = graftyard_types::Timestamp(w.now.0 + 61_000);
    let mut p = push(app(CLAUDE), vec![up("refs/heads/claude/ring", None, Some(oid(92)))], &["f/a.rs"]);
    p.leases = true;
    step(&mut w, &mut ring, RepoCmd::Push(p), &mut want);
    let s = ext(CLAUDE, "ring");
    let cancel = marked(&mut w, user(WRITER), IssueOp::CancelTask { id: s });
    step(&mut w, &mut ring, cancel, &mut want);
    let got: Vec<(graftyard_types::Index, ActivityEvent)> =
        ring.events.iter().map(|(i, _, e)| (*i, e.clone())).collect();
    assert_eq!(got, want);
    assert!(want.iter().any(|(_, e)| matches!(e, ActivityEvent::Lease { change: LeaseChange::Expired, .. })));
    // A poll past the first entry with `paths=f/**` sees the push and the f lease's expiry, not the h lease.
    let past = ring.past(start, &["f/**"]);
    assert!(past.iter().all(|(i, _, e)| *i > start && e.matches(&["f/**"])), "{past:?}");
    assert!(past.iter().any(|(_, _, e)| matches!(e, ActivityEvent::AgentPush { .. })));
    let (floor, newest) = ring.bounds();
    assert!(floor < start && newest == ring.through);
    assert_eq!(feed::learners_for_feed(&[(900, 0), (1_000, 300)]), 5);
    assert_eq!(feed::learners_for_feed(&[(0, 0)]), 0);
}

/// LF-13 hosted, gap mode (`tests/am1/results/fanout.md`, r3 gap 2: one lease event fewer on F's ring than on the
/// third voter): a follower catching up applies many entries in one input, and the host feeds the ring each of them
/// with the tiers that input left. A lease acquired and released inside that batch is gone from those tiers at its
/// acquire's entry, but its release must still reach the ring, as it does on a member that applied them one by one.
#[test]
fn a_lease_acquired_and_released_in_one_catch_up_batch_reports_both() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "batch", Some(WRITER));
    let mut entries = Vec::new();
    let last_events = |w: &mut World| {
        let l = w.repo.leader();
        let e = w.repo.members[l.0 as usize]
            .log()
            .iter()
            .rev()
            .find(|e| matches!(e.body, graftyard_raft::EntryBody::Events(_)))
            .unwrap()
            .clone();
        let graftyard_raft::EntryBody::Events(evs) = e.body else { unreachable!() };
        (e.index, e.at, evs)
    };
    let id = lease_id(&w.commit(issue(app(CLAUDE), shared(&["batch/**"], Some("batch")))));
    entries.push(last_events(&mut w));
    w.commit(issue(app(CLAUDE), IssueOp::ReleaseLease { id })).unwrap();
    entries.push(last_events(&mut w));
    let after = w.fold();
    assert!(lease::lease_row(&*after.core, id).is_none(), "the batch's final tiers no longer hold the lease");
    // Both entries fed with the batch's final tiers, as a follower's catch-up input feeds them.
    let mut batch = feed::Ring::default();
    for (i, at, evs) in &entries {
        batch.feed(REPO, &w.ident.owner, &after.core, &after.meta, (*i, *at, evs));
    }
    let changes: Vec<LeaseChange> = batch
        .past(Index(0), &["batch/**"])
        .into_iter()
        .filter_map(|(_, _, e)| match e {
            ActivityEvent::Lease { lease, change } if lease.id == id => Some(change),
            _ => None,
        })
        .collect();
    assert_eq!(changes, vec![LeaseChange::Acquired, LeaseChange::Released], "the catch-up batch's ring");
}

/// LF-13 hosted, install mode (`tests/am1/results/am1-runs.md`): a follower sent `InstallSnapshot` never feeds its ring
/// the entries the snapshot covered. A cursor from before that hole must get `gone` (410 with the oldest cursor it can
/// serve), never a `hold` past it that skips the events in between.
#[test]
fn a_ring_fed_past_a_hole_refuses_cursors_from_before_it() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "hole", Some(WRITER));
    let mut ring = feed::Ring::default();
    let mut fed = Vec::new();
    for k in 0..3 {
        let c = issue(app(CLAUDE), shared(&[&format!("hole/{k}/**")], Some("hole")));
        w.commit(c).unwrap();
        let after = w.fold();
        let l = w.repo.leader();
        let e = w.repo.members[l.0 as usize]
            .log()
            .iter()
            .rev()
            .find(|e| matches!(e.body, graftyard_raft::EntryBody::Events(_)))
            .unwrap()
            .clone();
        let graftyard_raft::EntryBody::Events(evs) = &e.body else { unreachable!() };
        fed.push((e.index, e.at, evs.clone(), after));
    }
    // The first entry reaches the ring; the second does not (installed); the third does.
    let (i0, at0, evs0, f0) = &fed[0];
    ring.feed(REPO, &w.ident.owner, &f0.core, &f0.meta, (*i0, *at0, evs0));
    let cursor = ring.through;
    let (i2, at2, evs2, f2) = &fed[2];
    assert!(i2.0 > cursor.0 + 1, "the third entry is past a hole");
    ring.feed(REPO, &w.ident.owner, &f2.core, &f2.meta, (*i2, *at2, evs2));
    let (oldest, newest) = ring.bounds();
    let c = feed::FeedCursor { group: REPO, index: cursor }.encode();
    assert!(
        matches!(feed::start(REPO, Some(&c), oldest, newest), feed::Start::Gone { .. }),
        "a cursor from before the hole is gone, not served past it: oldest {oldest:?}, cursor {cursor:?}"
    );
    // A cursor inside the hole, just before the first entry fed after it, would skip that entry's events too.
    let inside = feed::FeedCursor { group: REPO, index: Index(i2.0 - 1) }.encode();
    assert!(matches!(feed::start(REPO, Some(&inside), oldest, newest), feed::Start::Gone { .. }), "{oldest:?}");
    assert!(ring.past(Index(0), &[]).is_empty(), "no event from before the hole, or of the baseline entry, is kept");
    let c2 = feed::FeedCursor { group: REPO, index: *i2 }.encode();
    assert_eq!(feed::start(REPO, Some(&c2), oldest, newest), feed::Start::After(*i2), "the new floor serves");
}

/// LF-13 on graft-m3c: the install's own input moves the member's base past the ring with no entry to feed, and the
/// member follows again at once. A cursor from before the install must get `gone` then, not a `hold` that skips
/// what the snapshot covered, even before any later entry arrives. A lease the snapshot holds and the next entry
/// releases still reports `Released` (the second hosted batch: 2 such events missing on F).
#[test]
fn a_ring_skipped_past_an_install_refuses_older_cursors_before_the_next_entry() {
    let mut w = lease_world();
    create_ext(&mut w, CLAUDE, "skip", Some(WRITER));
    let last = |w: &mut World| {
        let l = w.repo.leader();
        let e = w.repo.members[l.0 as usize]
            .log()
            .iter()
            .rev()
            .find(|e| matches!(e.body, graftyard_raft::EntryBody::Events(_)))
            .unwrap()
            .clone();
        let graftyard_raft::EntryBody::Events(evs) = e.body else { unreachable!() };
        (e.index, e.at, evs, w.fold())
    };
    let mut ring = feed::Ring::default();
    w.commit(issue(app(CLAUDE), shared(&["skip/0/**"], Some("skip")))).unwrap();
    let (i0, at0, evs0, f0) = last(&mut w);
    ring.feed(REPO, &w.ident.owner, &f0.core, &f0.meta, (i0, at0, &evs0));
    let before = feed::FeedCursor { group: REPO, index: ring.through }.encode();
    // The snapshot covers an acquire the ring never saw; the entry after it releases that lease.
    let id = lease_id(&w.commit(issue(app(CLAUDE), shared(&["skip/1/**"], Some("skip")))));
    let (through, at1, _, snap) = last(&mut w);
    ring.skip(REPO, &w.ident.owner, &snap.core, (through, at1));
    let (oldest, newest) = ring.bounds();
    assert!(
        matches!(feed::start(REPO, Some(&before), oldest, newest), feed::Start::Gone { .. }),
        "a cursor from before the install is gone at once: oldest {oldest:?}, newest {newest:?}"
    );
    assert!(ring.past(Index(0), &[]).is_empty(), "no event from before the install is served");
    w.commit(issue(app(CLAUDE), IssueOp::ReleaseLease { id })).unwrap();
    let (i2, at2, evs2, f2) = last(&mut w);
    assert_eq!(i2.0, through.0 + 1, "the next entry follows the snapshot");
    ring.feed(REPO, &w.ident.owner, &f2.core, &f2.meta, (i2, at2, &evs2));
    let (oldest, newest) = ring.bounds();
    let at_snapshot = feed::FeedCursor { group: REPO, index: through }.encode();
    assert_eq!(feed::start(REPO, Some(&at_snapshot), oldest, newest), feed::Start::After(through));
    let served: Vec<(Index, LeaseId, LeaseChange)> = ring
        .past(through, &[])
        .into_iter()
        .filter_map(|(i, _, e)| match e {
            ActivityEvent::Lease { lease, change } => Some((i, lease.id, change)),
            _ => None,
        })
        .collect();
    assert_eq!(served, vec![(i2, id, LeaseChange::Released)], "the snapshot's lease, released after it");
}
