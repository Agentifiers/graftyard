//! AM1-WP2's gates in memory, on real members: three `graftyard_raft::Member<AuthzCmd>` of the install's own
//! `Authz(Genesis.account)` chain, and three `Member<RepoCmd>` of `repo:7`, whose leader answers every deps request by
//! reading the install's chain through `fresh` on its own leader, as a site host does once per entry.
//!
//! - LF-2: after `DeleteAgentApp` is acknowledged, the next repository decision no longer classes the App an `Agent`.
//! - B0-36 (pure rerun): each refusal for a Graftyard-run App and a confined vendor App; an unflagged or unconfined App
//!   is not refused; `Co-authored-by` alone never classes; `AgentGate` counts; a default PR merges as before.
//! - B0-37 (pure rerun): 10,000 pushes with malformed trailers and notes, forged sessions, and `refs/graftyard/` writes.

mod common;
use common::*;

// ---- LF-2 ----

/// LF-2: a confined App is refused outside its prefix; once the unflag is acknowledged, the very next decision on the
/// repository classes it `Bot` and accepts the same push. Repeated with the unflag and the push in consecutive
/// entries, and with the push admitted while the unflag's entry was still in flight.
#[test]
fn lf2_the_first_decision_after_an_unflag_no_longer_classes_the_app_an_agent() {
    const N: u32 = 25;
    let mut w = World::new();
    let mut after = 0;
    for i in 0..N {
        w.flag(claude_row());
        let branch = format!("refs/heads/x{i}");
        let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up(&branch, None, Some(oid(1000 + i)))], &["src/a.rs"])));
        assert_eq!(agent_rule(&o), Some(AgentRule::OutsidePrefix), "flagged: {o:?}");
        assert!(refs(&o).iter().all(|r| *r != RefOutcome::Ok));
        w.unflag(CLAUDE);
        let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up(&branch, None, Some(oid(1000 + i)))], &["src/a.rs"])));
        assert_eq!(refs(&o), [RefOutcome::Ok], "after the unflag's ack: {o:?}");
        after += 1;
    }
    // The unflag admitted on the install's leader before the push reaches the repository leader: the push's deps
    // read is answered after the unflag commits, so it decides on the new row too.
    w.flag(claude_row());
    let rid = w.rid();
    let l = w.install.leader();
    let now = w.now;
    let unflag = AuthzCmd { cred: admin(), op: AuthzOp::DeleteAgentApp { app: CLAUDE } };
    w.install.on(l, now, |m, t| m.commit(t, rid, unflag)).unwrap();
    let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up("refs/heads/y", None, Some(oid(9)))], &["a"])));
    assert_eq!(refs(&o), [RefOutcome::Ok], "{o:?}");
    eprintln!("LF-2: {after}/{N} first decisions after the unflag's ack classed the App Bot and accepted the push");
}

/// LF-2's planted bug as a test of the harness: a leader that decides on an install fold read before the unflag
/// (a cached copy) keeps refusing, so the gate can fail.
#[test]
fn lf2_control_a_stale_install_read_keeps_the_old_class() {
    let mut w = World::new();
    w.flag(claude_row());
    w.pinned = Some(w.install.fresh(w.now));
    w.unflag(CLAUDE);
    let o = w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up("refs/heads/z", None, Some(oid(3)))], &["a"])));
    assert_eq!(agent_rule(&o), Some(AgentRule::OutsidePrefix));
}

// ---- B0-36 ----

/// Every refusal B0-36 names that `decide` can see before AM1-WP3 (no tasks, no dispatch, no leases), for the
/// Graftyard-run App and the confined vendor App, 100 times each; then the controls.
#[test]
fn b0_36_agents_are_refused_and_others_are_not() {
    let mut w = World::new();
    w.flag(claude_row());
    // The vendor App's PR, opened on its own prefix, and a protected-path setup.
    w.set("refs/heads/claude/a", oid(20));
    let pr = open_pr(&mut w, app(CLAUDE), "refs/heads/claude/a", false);
    let n = number(&pr);
    let f = w.fold();
    assert_eq!(tiers::agent_meta(&*f.core, IssueNumber(n)).map(|m| m.app), Some(CLAUDE), "AgentMeta on the agent's PR");
    // Labels: `agent` on the allowlist, `release` not.
    let lab = |w: &mut World, s: &str| {
        let op = IssueOp::DefineLabel { id: None, name: LabelName(s.into()), color: [1, 2, 3], description: None };
        match w.commit(issue(user(OWNER), op)) {
            Ok(RepoOut::Issue(c)) => match c.out {
                IssueOut::Id(id) => LabelId(id),
                x => panic!("{x:?}"),
            },
            o => panic!("{o:?}"),
        }
    };
    let ok_label = lab(&mut w, "agent");
    let bad_label = lab(&mut w, "release");
    let policy = AgentPolicy { labels: smallvec!["agent".into()], ..AgentPolicy::default() };
    let mut pc =
        IssueCall::new(IssueOp::SetAgentPolicy(policy), user(OWNER), RequestId(Ulid(0)), serde_json::Value::Null);
    pc.agent = true;
    w.commit(RepoCmd::Issue(Box::new(pc))).expect("policy set by the repository admin");
    // A rule over `claude/**` that lists the App as a pusher: an agent push needs that bypass.
    let rule = IssueOp::SetRule {
        id: None,
        pattern: "claude/locked".into(),
        protection: Some(Protection {
            checks: None,
            reviews: None,
            push: Some(smallvec![Pusher::App(CLAUDE)]),
            enforce_admins: false,
            linear_history: false,
            force_pushes: false,
            deletions: false,
            block_creations: false,
            conversation_resolution: false,
            lock_branch: false,
            signatures: false,
            merge_queue: None,
        }),
    };
    w.commit(issue(user(OWNER), rule)).expect("rule");
    let head = w.get("refs/heads/claude/a").unwrap();
    let status = move |ctx: &str| -> IssueOp<graftyard_git::blob::BlobRef> {
        let creator = AccountId(DbId(0));
        IssueOp::RecordStatus(StatusUpdate {
            id: StatusId(DbId(1 << 30)),
            sha: head,
            context: ctx.into(),
            state: StatusState::Success,
            target_url: None,
            description: None,
            creator,
        })
    };
    let check = move |a: AppId, nm: &str| -> IssueOp<graftyard_git::blob::BlobRef> {
        IssueOp::RecordCheck(CheckRunUpdate {
            run: CheckRunId(DbId(1 << 31)),
            suite: CheckSuiteId(DbId(1 << 31)),
            app: a,
            head_sha: head,
            name: nm.into(),
            version: 1,
            status: CheckStatus::Completed,
            conclusion: Some(graftyard_git::issues::CheckConclusion::Success),
            started: None,
            completed: None,
            external_id: None,
            details_url: None,
            output: None,
        })
    };
    type Case = (&'static str, AgentRule, Box<dyn Fn(&mut World, AppId, u32) -> RepoCmd>);
    let cases: Vec<Case> = vec![
        (
            "approve",
            AgentRule::Approve,
            Box::new(move |w, a, _| {
                let head = tiers::pull_head(&*w.fold().core, IssueNumber(n)).unwrap().head_sha;
                issue(
                    app(a),
                    IssueOp::Review { n: IssueNumber(n), state: ReviewState::Approved, commit: head, body: "".into() },
                )
            }),
        ),
        ("merge", AgentRule::Merge, Box::new(move |w, a, _| merge(w, app(a), n))),
        (
            "ready for review",
            AgentRule::ReadyForReview,
            Box::new(move |_, a, _| {
                issue(app(a), IssueOp::Change { n: IssueNumber(n), change: IssueChange::Draft(false) })
            }),
        ),
        (
            "rule edit",
            AgentRule::Rules,
            Box::new(|_, a, _| issue(app(a), IssueOp::SetRule { id: None, pattern: "x".into(), protection: None })),
        ),
        ("settings", AgentRule::Rules, Box::new(|_, a, _| issue(app(a), IssueOp::Settings(settings())))),
        (
            "release",
            AgentRule::Release,
            Box::new(move |_, a, i| {
                issue(
                    app(a),
                    IssueOp::PutRelease {
                        id: None,
                        tag: name(&format!("refs/tags/v{i}")),
                        target: oid(1),
                        name: "r".into(),
                        body: "".into(),
                        draft: false,
                        prerelease: false,
                        latest: true,
                    },
                )
            }),
        ),
        (
            "label outside the allowlist",
            AgentRule::Label,
            Box::new(move |_, a, _| {
                issue(
                    app(a),
                    IssueOp::ApplyLabels { n: IssueNumber(n), add: smallvec![bad_label], remove: SmallVec::new() },
                )
            }),
        ),
        (
            "graftyard/ status",
            AgentRule::GraftyardContext,
            Box::new(move |_, a, _| issue(app(a), status("graftyard/agent-gate"))),
        ),
        (
            "graftyard/ check run",
            AgentRule::GraftyardContext,
            Box::new(move |_, a, _| issue(app(a), check(a, "graftyard/agent-gate"))),
        ),
        (
            "tag push",
            AgentRule::Tag,
            Box::new(|_, a, i| {
                RepoCmd::Push(push(app(a), vec![up(&format!("refs/tags/t{i}"), None, Some(oid(7)))], &["a"]))
            }),
        ),
        (
            "push outside the prefix",
            AgentRule::OutsidePrefix,
            Box::new(|_, a, i| {
                RepoCmd::Push(push(app(a), vec![up(&format!("refs/heads/main{i}"), None, Some(oid(7)))], &["a"]))
            }),
        ),
    ];
    let mut counts = Vec::new();
    for (label, rule, make) in &cases {
        let mut refused = 0;
        for i in 0..100 {
            let c = make(&mut w, CLAUDE, i);
            if agent_rule(&w.commit(c)) == Some(*rule) {
                refused += 1;
            }
        }
        counts.push((*label, refused));
        assert_eq!(refused, 100, "{label}: {refused}/100");
    }
    // Pushes inside the prefix: a protected path and a bypass are refused, an ordinary change is accepted.
    let mut prot = 0;
    for i in 0..100 {
        let r = format!("refs/heads/claude/p{i}");
        let o = w.commit(RepoCmd::Push(push(
            app(CLAUDE),
            vec![up(&r, None, Some(oid(7)))],
            &["src/x.rs", ".github/workflows/ci.yml"],
        )));
        prot += usize::from(agent_rule(&o) == Some(AgentRule::Protected));
    }
    assert_eq!(prot, 100);
    w.set("refs/heads/claude/locked", oid(30));
    let o = w.commit(RepoCmd::Push(push(
        app(CLAUDE),
        vec![up("refs/heads/claude/locked", Some(oid(30)), Some(oid(31)))],
        &["a"],
    )));
    assert_eq!(agent_rule(&o), Some(AgentRule::Bypass), "{o:?}");
    let o =
        w.commit(RepoCmd::Push(push(app(CLAUDE), vec![up("refs/heads/claude/ok", None, Some(oid(8)))], &["src/a.rs"])));
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    // The allowlisted label is applied.
    let o = w.commit(issue(
        app(CLAUDE),
        IssueOp::ApplyLabels { n: IssueNumber(n), add: smallvec![ok_label], remove: SmallVec::new() },
    ));
    assert!(o.is_ok(), "{o:?}");
    // `refs/graftyard/` is the host's for every actor.
    let o = w.commit(RepoCmd::Push(push(user(OWNER), vec![up("refs/graftyard/sessions/x", None, Some(oid(9)))], &[])));
    assert_eq!(refs(&o), [RefOutcome::Rejected(RejectReason::Hidden)]);

    // The Graftyard-run App: with no live task (none exists before AM1-WP3) every push is refused, and so is
    // everything above.
    register_own(&mut w, RUN, "graftyard-agent");
    w.flag(AgentApp { app: RUN, kind: AgentKind::GraftyardRun, confine: None });
    let mut run_counts = Vec::new();
    for (label, rule, make) in &cases {
        let mut refused = 0;
        for i in 0..100 {
            let c = make(&mut w, RUN, 1_000 + i);
            let r = agent_rule(&w.commit(c));
            // A push is refused first for the missing task, in the design's order.
            let want = if matches!(*rule, AgentRule::OutsidePrefix) { AgentRule::NoLiveTask } else { *rule };
            if r == Some(want) {
                refused += 1;
            }
        }
        run_counts.push((*label, refused));
        assert_eq!(refused, 100, "graftyard-run {label}: {refused}/100");
    }
    let o = w.commit(RepoCmd::Push(push(app(RUN), vec![up("refs/heads/run/a", None, Some(oid(8)))], &["a"])));
    assert_eq!(agent_rule(&o), Some(AgentRule::NoLiveTask));
    w.set("refs/heads/run/b", oid(40));
    let o = open_pr(&mut w, app(RUN), "refs/heads/run/b", false);
    assert_eq!(agent_rule(&o), Some(AgentRule::DraftOnly), "{o:?}");
    assert!(open_pr(&mut w, app(RUN), "refs/heads/run/b", true).is_ok());

    // Controls: an unflagged App, a flagged App without `confine` (Bot), and `Graftyard MCP` are never refused by an
    // agent rule; a user is not either.
    w.flag(AgentApp { app: MCP, kind: AgentKind::ThirdParty, confine: None });
    for (who, label) in
        [(app(PLAIN), "unflagged App"), (app(MCP), "flagged App without confine"), (user(WRITER), "user")]
    {
        let o = w.commit(RepoCmd::Push(push(
            who.clone(),
            vec![up(&format!("refs/heads/ctl-{}", label.len()), None, Some(oid(11)))],
            &["a"],
        )));
        assert_eq!(refs(&o), [RefOutcome::Ok], "{label}: {o:?}");
        assert_eq!(
            agent::actor_kind(&who, None),
            if matches!(who.actor, Actor::App { .. }) { agent::ActorKind::Bot } else { agent::ActorKind::User }
        );
    }
    eprintln!(
        "B0-36 confined vendor App: {counts:?}; protected path {prot}/100; bypass refused; allowlisted label applied"
    );
    eprintln!("B0-36 Graftyard-run App: {run_counts:?}; draft-only refused a ready PR");
}

/// `agent-rows-cache`: the edge's `protected` may come from an `AgentPolicy` read before the latest one committed.
/// A push validated against rev k (no `extra_paths`) is refused when rev k+1 protecting its path committed first, in
/// an earlier entry or earlier in the same entry; a truncated directory a new glob reaches under is refused; a path the
/// new policy does not name, and a push before the policy, still pass.
#[test]
fn a_policy_committed_after_the_edges_read_binds_the_next_push() {
    let mut w = World::new();
    w.flag(claude_row());
    let set = |paths: &[&str]| {
        let policy = AgentPolicy { extra_paths: paths.iter().map(|p| (*p).into()).collect(), ..AgentPolicy::default() };
        let mut c =
            IssueCall::new(IssueOp::SetAgentPolicy(policy), user(OWNER), RequestId(Ulid(0)), serde_json::Value::Null);
        c.agent = true;
        RepoCmd::Issue(Box::new(c))
    };
    let at = |r: &str, n: u32, paths: &[&str]| {
        RepoCmd::Push(push(app(CLAUDE), vec![up(&format!("refs/heads/claude/{r}"), None, Some(oid(n)))], paths))
    };
    // Rev k: nothing extra, so `Makefile` passes.
    assert_eq!(refs(&w.commit(at("k", 1, &["Makefile"]))), [RefOutcome::Ok]);
    // Rev k+1 in its own entry; the push's `protected` was computed on rev k.
    w.commit(set(&["Makefile", "gen/*.rs"])).expect("the admin sets the policy");
    let o = w.commit(at("k1", 2, &["LICENSE", "Makefile"]));
    assert_eq!(agent_rule(&o), Some(AgentRule::Protected), "{o:?}");
    assert!(format!("{o:?}").contains("Makefile is a protected path"), "{o:?}");
    // The policy and the push in one entry, the policy first: decide sees the earlier command. The first command
    // admitted decides alone, so a filler goes first.
    let filler = RepoCmd::Push(push(user(OWNER), vec![up("refs/heads/filler", None, Some(oid(90)))], &[]));
    let all = w.commit_all_acked(vec![
        filler,
        set(&["Makefile", "gen/*.rs", "tools/gen.sh"]),
        at("same", 3, &["tools/gen.sh"]),
    ]);
    let index = |i: usize| all[i].as_ref().map(|a| a.index).ok();
    assert!(index(1).is_some() && index(1) == index(2), "{:?} {:?}", index(1), index(2));
    let pushed = all[2].clone().map(|a| a.out);
    assert_eq!(agent_rule(&pushed), Some(AgentRule::Protected), "{pushed:?}");
    // Past PATHS_MAX the edge sends directories; `gen/` may hold paths `gen/*.rs` protects.
    let many: Vec<String> = (0..1_200).map(|i| format!("{}/f{i}.rs", ["src", "gen", "lib"][i % 3])).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let o = w.commit(at("dirs", 4, &many));
    assert_eq!(agent_rule(&o), Some(AgentRule::Protected), "{o:?}");
    // Controls: paths no glob names (`gen/*.rs` reaches neither a file one directory deeper nor one named `gen`),
    // and a directory list no glob reaches under.
    assert_eq!(refs(&w.commit(at("ok", 5, &["src/a.rs", "Makefile.am", "gen/sub/x.rs", "gen"]))), [RefOutcome::Ok]);
    let few: Vec<String> = (0..1_200).map(|i| format!("{}/f{i}.rs", ["src", "lib"][i % 2])).collect();
    let few: Vec<&str> = few.iter().map(String::as_str).collect();
    assert_eq!(refs(&w.commit(at("okdirs", 6, &few))), [RefOutcome::Ok]);
}

// ---- AgentGate ----

fn gate_rule(o: &RepoOutcome) -> Option<String> {
    match o {
        Err(RepoErr::Merge { err: MergeError::Agent(a), .. }) if a.rule == AgentRule::Gate => Some(a.text.to_string()),
        _ => None,
    }
}

/// A PR an `AgentApp` opened merges only once `AgentGate` holds on its current head: with no recorded requester (no
/// tasks before AM1-WP3) two distinct human writers; an approval through the App's user-to-server token writes no
/// gate; a head move voids the gates on the old head.
#[test]
fn agent_gate_counts_human_writers_on_the_current_head() {
    let mut w = World::new();
    w.flag(claude_row());
    w.set("refs/heads/claude/g", oid(50));
    let n = number(&open_pr(&mut w, app(CLAUDE), "refs/heads/claude/g", false));
    let meta = tiers::agent_meta(&*w.fold().core, IssueNumber(n)).expect("AgentMeta");
    assert_eq!((meta.app, meta.requester), (CLAUDE, None));
    let gates = |w: &mut World| {
        let f = w.fold();
        let head = tiers::pull_head(&*f.core, IssueNumber(n)).unwrap().head_sha;
        tiers::agent_gates(&*f.core, IssueNumber(n), &head).len()
    };
    let m = merge(&mut w, user(WRITER), n);
    let t = gate_rule(&w.commit(m)).expect("refused before any approval");
    assert!(t.contains("0 of 2"), "{t}");
    approve(&mut w, user(WRITER), n).unwrap();
    assert_eq!(gates(&mut w), 1);
    // The same writer again, and a writer approving through CLAUDE's user-to-server token: no new gate.
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(VIA_APP), n).unwrap();
    assert_eq!(gates(&mut w), 1, "a repeat and an AgentApp's user token count nothing");
    let m = merge(&mut w, user(WRITER), n);
    assert!(gate_rule(&w.commit(m)).expect("1 of 2").contains("1 of 2"));
    // A reader's approval does not qualify.
    let _ = approve(&mut w, user(READER), n);
    assert_eq!(gates(&mut w), 1);
    approve(&mut w, user(WRITER2), n).unwrap();
    assert_eq!(gates(&mut w), 2);
    // The App pushes again: the gates were for the old head.
    let o = w.commit(RepoCmd::Push(push(
        app(CLAUDE),
        vec![up("refs/heads/claude/g", Some(oid(50)), Some(oid(51)))],
        &["src/a.rs"],
    )));
    assert_eq!(refs(&o), [RefOutcome::Ok]);
    assert_eq!(gates(&mut w), 0, "a head move voids the gate");
    let m = merge(&mut w, user(WRITER), n);
    assert!(gate_rule(&w.commit(m)).expect("0 of 2 on the new head").contains("0 of 2"));
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(WRITER2), n).unwrap();
    let m = merge(&mut w, user(WRITER), n);
    assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))), "merges once the gate holds");
    // `extra_human_approvals` raises the count.
    let policy = AgentPolicy { extra_human_approvals: 1, ..AgentPolicy::default() };
    let mut pc =
        IssueCall::new(IssueOp::SetAgentPolicy(policy), user(OWNER), RequestId(Ulid(0)), serde_json::Value::Null);
    pc.agent = true;
    w.commit(RepoCmd::Issue(Box::new(pc))).unwrap();
    w.set("refs/heads/claude/h", oid(60));
    let n = number(&open_pr(&mut w, app(CLAUDE), "refs/heads/claude/h", false));
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(WRITER2), n).unwrap();
    let m = merge(&mut w, user(WRITER), n);
    assert!(gate_rule(&w.commit(m)).expect("2 of 3").contains("2 of 3"));
    approve(&mut w, user(OWNER), n).unwrap();
    let m = merge(&mut w, user(OWNER), n);
    assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))));
}

/// L0-15's `AgentGate` half for `Graftyard MCP` (LM-2): a writer's approval through the install's `Graftyard MCP` App
/// (an `AgentApp` of kind `Mcp`), on a scoped `ghu_` whose grant allows approve and merge, writes no `AgentGate`, so the
/// agent's PR stays refused; the same writer's own PAT then counts.
#[test]
fn an_approval_through_graftyard_mcp_writes_no_agent_gate() {
    let mut w = World::new();
    w.flag(claude_row());
    register_own(&mut w, PLAIN, "graftyard-mcp");
    w.flag(AgentApp { app: PLAIN, kind: AgentKind::Mcp, confine: None });
    let hash = [0xA7; 32];
    let fields = Box::new(TokenFields {
        app: Some(PLAIN),
        grant: Some(graftyard_identity::GrantId(DbId(1))),
        write: Some(graftyard_identity::WriteScope::WriteApproveMerge),
        ..TokenFields::default()
    });
    let (account, kind) = (AccountId(DbId(WRITER)), TokenKind::UserToServer);
    w.ident.tokens.apply(&TokensEvent::StoredWith { hash, kind, account, scopes: ScopeSet(1), expires: None, fields });
    let via_mcp = Principal { credential: Credential::Stored(hash), ..user(WRITER) };
    w.set("refs/heads/claude/m", oid(70));
    let n = number(&open_pr(&mut w, app(CLAUDE), "refs/heads/claude/m", false));
    let gates = |w: &mut World| {
        let f = w.fold();
        let head = tiers::pull_head(&*f.core, IssueNumber(n)).unwrap().head_sha;
        tiers::agent_gates(&*f.core, IssueNumber(n), &head).len()
    };
    for _ in 0..100 {
        assert!(approve(&mut w, via_mcp.clone(), n).is_ok(), "the review itself is accepted");
    }
    assert_eq!(gates(&mut w), 0, "Graftyard MCP's approvals never write AgentGate");
    let m = merge(&mut w, via_mcp.clone(), n);
    assert!(gate_rule(&w.commit(m)).expect("refused by the gate").contains("0 of 2"));
    approve(&mut w, user(WRITER), n).unwrap();
    approve(&mut w, user(WRITER2), n).unwrap();
    assert_eq!(gates(&mut w), 2, "the same writer's own PAT counts");
    let m = merge(&mut w, user(WRITER), n);
    assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))));
}

/// A user's PR, opened and reviewed in the default formats and in 32.8, carries no `AgentMeta`, writes no
/// `AgentGate`, and merges with no approval as before.
#[test]
fn a_default_pr_merges_as_before() {
    let mut w = World::new();
    w.flag(claude_row());
    for (i, agent) in [false, true].into_iter().enumerate() {
        let branch = format!("refs/heads/feat{i}");
        w.set(&branch, oid(70 + i as u32));
        let head = w.get(&branch).unwrap();
        let op = IssueOp::OpenPr {
            head: HeadSpec { repo: REPO, branch: name(&branch) },
            base: name("refs/heads/main"),
            title: "t".into(),
            body: "b".into(),
            draft: false,
            head_sha: head,
            base_sha: w.get("refs/heads/main").unwrap(),
        };
        let mut c = IssueCall::new(op, user(WRITER), RequestId(Ulid(0)), serde_json::Value::Null);
        c.agent = agent;
        let n = number(&w.commit(RepoCmd::Issue(Box::new(c))));
        let op = IssueOp::Review { n: IssueNumber(n), state: ReviewState::Approved, commit: head, body: "".into() };
        let mut c = IssueCall::new(op, user(WRITER2), RequestId(Ulid(0)), serde_json::Value::Null);
        c.agent = agent;
        w.commit(RepoCmd::Issue(Box::new(c))).unwrap();
        let f = w.fold();
        assert!(tiers::agent_meta(&*f.core, IssueNumber(n)).is_none());
        assert!(tiers::agent_gates(&*f.core, IssueNumber(n), &head).is_empty());
        let m = merge(&mut w, user(WRITER), n);
        assert!(matches!(w.commit(m), Ok(RepoOut::Merge(_))), "agent={agent}");
    }
    let l = w.repo.leader();
    let agent_events = w.repo.members[l.0 as usize]
        .log()
        .iter()
        .filter_map(|e| match &e.body {
            graftyard_raft::EntryBody::Events(evs) => Some(evs.clone()),
            _ => None,
        })
        .flatten()
        .filter(|e| matches!(e, RepoEvent::AgentMeta { .. } | RepoEvent::AgentGate(_) | RepoEvent::Provenance { .. }))
        .count();
    assert_eq!(agent_events, 0);
}

/// BF-34 through a real leader: an invitation's `expires` is the entry's time plus a week, so the invitee can list
/// and accept it (the leader once decided issue commands at time 0, so every invitation had already lapsed).
#[test]
fn a_repository_invitation_sent_through_the_leader_is_pending_and_accepted() {
    let mut w = World::new();
    // A clock past a week since the epoch, as a hosted leader's is (2026-11-08).
    w.now = Timestamp(1_794_000_000_000);
    w.settle();
    let invite = IssueOp::InviteCollaborator { user: AccountId(DbId(READER)), permission: RepoPermission::Write };
    let id = match w.commit(issue(user(OWNER), invite)) {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Id(id) => id,
            ref x => panic!("{x:?}"),
        },
        other => panic!("{other:?}"),
    };
    let f = w.fold();
    let row: graftyard_identity::InvitationRow =
        tiers::get_rec(&*f.core, "invitation", &[graftyard_fold::Val::Int(id.0)]).expect("a pending invitation");
    assert!(row.expires > w.now, "expires {:?} is not after the entry's time {:?}", row.expires, w.now);
    let accept = IssueOp::AnswerInvitation { id: graftyard_identity::InvitationId(id), accept: true };
    w.commit(issue(user(READER), accept)).expect("the invitee accepts");
    let f = w.fold();
    assert_eq!(graftyard_git::issue_ops::collaborator(&*f.core, AccountId(DbId(READER))), Some(RepoPermission::Write));
}

// ---- Co-authored-by ----

fn raw_commit(message: &[u8]) -> Vec<u8> {
    let mut b = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor A <a@x> 1 +0000\ncommitter A <a@x> 1 +0000\n\n"
        .to_vec();
    b.extend_from_slice(message);
    b
}

/// A commit whose only trailer is `Co-authored-by` is recorded `Claimed` and classes nothing: a user-token push of it
/// stays Human and is accepted; an unflagged App's stays Bot. `Assisted-by` classes the App's push Agent for
/// admission only.
#[test]
fn co_authored_by_alone_never_classes() {
    let mut w = World::new();
    let r = name("refs/heads/co");
    let raw = raw_commit(b"Fix\n\nCo-authored-by: Claude <noreply@anthropic.com>\n");
    let prov = agent::provenance_of(&r, &[(oid(80), &raw)], &[]);
    assert_eq!(prov.len(), 1);
    assert!(!agent::classes(&prov));
    let install = w.install.fresh(w.now);
    let read = Some(&install as &dyn graftyard_identity::AuthzRead);
    assert_eq!(agent::admit_class(&user(WRITER), read, None, &prov, false), graftyard_raft::admit::AdmitClass::Human);
    assert_eq!(agent::admit_class(&app(PLAIN), read, None, &prov, false), graftyard_raft::admit::AdmitClass::Bot);
    let raw2 = raw_commit(b"Fix\n\nAssisted-by: Amp\n");
    let p2 = agent::provenance_of(&r, &[(oid(81), &raw2)], &[]);
    assert_eq!(agent::admit_class(&app(PLAIN), read, None, &p2, false), graftyard_raft::admit::AdmitClass::Agent);
    assert_eq!(agent::admit_class(&user(WRITER), read, None, &p2, false), graftyard_raft::admit::AdmitClass::Human);
    let mut p = push(user(WRITER), vec![up("refs/heads/co", None, Some(oid(80)))], &[]);
    p.provenance = prov;
    let o = w.commit(RepoCmd::Push(p));
    assert_eq!(refs(&o), [RefOutcome::Ok]);
}

// ---- B0-37 ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

const ORIGIN: &str = "https://graftyard.example/o/r";

/// One malformed or forged commit message.
fn hostile_message(g: &mut Rng, i: u32) -> Vec<u8> {
    let keys = [
        "Graftyard-Session",
        "graftyard-session",
        "GRAFTYARD-SESSION",
        "Claude-Session",
        "Amp-Thread-ID",
        "Co-authored-by",
        "Assisted-by",
        "Signed-off-by",
        "X-Unknown",
        "Graftyard Session",
        "-bad",
    ];
    let uuid = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", g.next() as u32, i as u16, 1u16, 2u16, g.next() >> 16);
    let forged = [
        format!("{ORIGIN}/_graftyard/sessions/{uuid}"),
        format!("https://evil.example/o/r/_graftyard/sessions/{uuid}"),
        format!("{ORIGIN}/_graftyard/sessions/{uuid}/../x"),
        format!("{ORIGIN}/_graftyard/sessions/%2e%2e"),
        format!("{ORIGIN}/_graftyard/sessions/3.mine"),
        format!("{ORIGIN}/_graftyard/sessions/mine"),
        format!("{ORIGIN}/_graftyard/sessions/"),
        format!("http://graftyard.example/o/r/_graftyard/sessions/{uuid}"),
        format!("{}/_graftyard/sessions/{uuid}", ORIGIN.to_uppercase()),
        format!("{ORIGIN}/_graftyard/sessions/{uuid}\u{202e}"),
    ];
    match g.below(8) {
        0 => {
            let n = g.below(400) as usize;
            g.bytes(n)
        }
        1 => {
            let mut m = b"subject\n\n".to_vec();
            let k = g.below(40);
            for _ in 0..k {
                let key = keys[g.below(keys.len() as u64) as usize];
                let sep = [": ", ":", " : ", ":\t", "="][g.below(5) as usize];
                let n = g.below(64) as usize;
                let mut v = g.bytes(n);
                v.retain(|b| *b != b'\n');
                m.extend_from_slice(key.as_bytes());
                m.extend_from_slice(sep.as_bytes());
                m.extend_from_slice(&v);
                m.extend_from_slice([b"\n".as_slice(), b"\r\n", b"\n continued\n", b"\r"][g.below(4) as usize]);
            }
            m
        }
        2 | 3 => {
            let f = &forged[g.below(forged.len() as u64) as usize];
            let key = keys[g.below(3) as usize];
            format!("Do it\n\nbody\n\n{key}: {f}\nCo-authored-by: X <x@y>\n").into_bytes()
        }
        4 => {
            let long = "é".repeat(5_000);
            format!("s\n\nAmp-Thread-ID: {long}\nClaude-Session: {long}\n").into_bytes()
        }
        5 => ["", "\n", "\n\n", "only subject", "s\n\n\n\n", "s\r\n\r\nAssisted-by: a\r\n"][g.below(6) as usize]
            .as_bytes()
            .to_vec(),
        6 => {
            // A trailer block buried in prose, under a quarter trailers.
            let mut m = "s\n\n".to_string();
            for _ in 0..8 {
                m.push_str("prose line\n");
            }
            m.push_str(&format!("Graftyard-Session: {}\n", forged[0]));
            m.into_bytes()
        }
        _ => {
            if i % 1_000 == 7 {
                // Past MESSAGE_SCAN_MAX: only the tail is read.
                let mut m = vec![b'x'; agent::MESSAGE_SCAN_MAX + 10];
                m.extend_from_slice(format!("\n\nGraftyard-Session: {}\n", forged[0]).as_bytes());
                m
            } else {
                format!("s\n\nGraftyard-Session: {}\n", forged[0]).into_bytes()
            }
        }
    }
}

fn hostile_notes(g: &mut Rng) -> Vec<(String, bool, ObjectId)> {
    (0..g.below(4))
        .map(|_| {
            let hex: String =
                (0..40).map(|_| "0123456789abcdefABCDEFxz/"[g.below(25) as usize..][..1].to_string()).collect();
            let path = match g.below(4) {
                0 => format!("{}/{}", &hex[..2], &hex[2..]),
                1 => hex,
                2 => format!("{}/{}/{}", &hex[..2], &hex[2..4], &hex[4..]),
                _ => String::from_utf8_lossy(&g.bytes(12)).into_owned(),
            };
            (path, g.below(3) != 0, oid(g.next() as u32))
        })
        .collect()
}

/// B0-37 in memory: 10,000 pushes whose commits carry malformed trailers and notes and forged `Graftyard-Session`
/// trailers, through `validate`'s parser, the push wire, and `decide` on the real members; one in ten also writes
/// `refs/graftyard/`. Zero fail on parsing, every forged trailer stays `Claimed` (a `Session` sent on the wire with no
/// task is dropped), and every `refs/graftyard/` update is refused `Hidden`.
#[test]
fn b0_37_hostile_provenance_and_host_refs() {
    const PUSHES: u32 = 10_000;
    let mut w = World::new();
    w.flag(claude_row());
    let mut g = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut parse_fail, mut forged_seen, mut forged_linked, mut hidden_tried, mut hidden_refused) = (0, 0, 0, 0, 0);
    let mut wire_sessions = 0;
    let mut batch = Vec::new();
    let mut expect_hidden = Vec::new();
    for i in 0..PUSHES {
        let who = match i % 4 {
            0 => user(WRITER),
            1 => app(PLAIN),
            2 => app(MCP),
            _ => app(CLAUDE),
        };
        let prefix = if i % 4 == 3 { "refs/heads/claude/b37-" } else { "refs/heads/b37-" };
        let branch = format!("{prefix}{i}");
        let r = name(&branch);
        let raws: Vec<Vec<u8>> = (0..1 + g.below(3)).map(|_| raw_commit(&hostile_message(&mut g, i))).collect();
        let commits: Vec<(ObjectId, &[u8])> =
            raws.iter().enumerate().map(|(k, b)| (oid(i * 4 + k as u32 + 1_000_000), b.as_slice())).collect();
        let notes = hostile_notes(&mut g);
        let entries: Vec<agent::NoteEntry<'_>> =
            notes.iter().map(|(p, blob, o)| agent::NoteEntry { path: p, blob: *blob, oid: *o }).collect();
        let parsed = std::panic::catch_unwind(|| agent::provenance_of(&r, &commits, &entries));
        let Ok(prov) = parsed else {
            parse_fail += 1;
            continue;
        };
        // The edge's linking with the pusher's sessions: none exist, and a session the App owns does not match a
        // forged URL.
        let pusher = match who.actor {
            Actor::App { app, .. } => Some(app),
            _ => None,
        };
        let own = [graftyard_git::agent::SessionId::Run(0x1234)];
        let mut linked = agent::link(&prov, pusher, &own, Some(ORIGIN));
        for p in &linked {
            for s in &p.sources {
                match s {
                    agent::Source::Claimed { key: agent::TrailerKey::GraftyardSession, .. } => forged_seen += 1,
                    agent::Source::Session(_) => forged_linked += 1,
                    _ => {}
                }
            }
        }
        // One in five also forges a `Session` source on the wire.
        if i % 5 == 0 && !linked.is_empty() {
            linked[0].sources.push(agent::Source::Session(graftyard_git::agent::SessionId::Run(u128::from(i))));
            wire_sessions += 1;
        }
        let mut updates = vec![up(&branch, None, Some(oid(i + 2_000_000)))];
        if i % 10 == 0 {
            updates.push(up(&format!("refs/graftyard/sessions/s{i}"), None, Some(oid(i + 3_000_000))));
            hidden_tried += 1;
        }
        let mut p = push(who, updates, &["src/lib.rs"]);
        p.provenance = linked.into_iter().filter(|x| x.refname == r).collect();
        // The push crosses the member wire.
        let Ok(p) = graftyard_git::repo::Push::from_wire(&p.to_wire()) else {
            parse_fail += 1;
            continue;
        };
        expect_hidden.push(i % 10 == 0);
        batch.push(RepoCmd::Push(p));
        if batch.len() == 100 || i + 1 == PUSHES {
            for (o, hid) in w.commit_all(std::mem::take(&mut batch)).iter().zip(std::mem::take(&mut expect_hidden)) {
                let rs = match o {
                    Ok(RepoOut::Push(p)) => p.refs.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(),
                    _ => {
                        parse_fail += 1;
                        continue;
                    }
                };
                if rs[0] != RefOutcome::Ok {
                    parse_fail += 1;
                }
                if hid {
                    hidden_refused += usize::from(rs[1] == RefOutcome::Rejected(RejectReason::Hidden));
                }
            }
        }
    }
    let f = w.fold();
    let rows = graftyard_fold::Read::prefix(&*f.meta, "provenance", &[]);
    let mut stored_sessions = 0;
    let mut stored_claimed = 0;
    for (_, cols) in &rows {
        let graftyard_fold::Val::Blob(b) = &cols[0] else { panic!("a record") };
        let sources: SmallVec<[agent::Source; 2]> = postcard::from_bytes(b).unwrap();
        for s in sources {
            match s {
                agent::Source::Session(_) => stored_sessions += 1,
                agent::Source::Claimed { key: agent::TrailerKey::GraftyardSession, .. } => stored_claimed += 1,
                _ => {}
            }
        }
    }
    let host_refs = graftyard_fold::Read::prefix(&*f.core, "ref", &[])
        .iter()
        .filter(|(k, _)| matches!(&k[0], graftyard_fold::Val::Blob(b) if b.starts_with(agent::HOST_REFS.as_bytes())))
        .count();
    eprintln!(
        "B0-37: {PUSHES} pushes, {parse_fail} failed on parsing; {forged_seen} forged Graftyard-Session trailers, \
         {forged_linked} linked; {wire_sessions} Session sources forged on the wire, {stored_sessions} stored; \
         {stored_claimed} stored Claimed; refs/graftyard/ {hidden_refused}/{hidden_tried} refused Hidden, {host_refs} \
         in the fold; {} provenance rows",
        rows.len()
    );
    assert_eq!(parse_fail, 0);
    assert!(forged_seen > 1_000, "the run forged trailers: {forged_seen}");
    assert_eq!(forged_linked, 0);
    assert!(wire_sessions > 100);
    assert_eq!(stored_sessions, 0);
    assert!(stored_claimed > 1_000);
    assert_eq!(hidden_refused, hidden_tried);
    assert_eq!(host_refs, 0);
}

/// M4-WP1: an `Agent` may not create, edit, or delete a webhook, and the repository's admin may.
#[test]
fn an_agent_may_not_edit_webhooks() {
    use graftyard_git::hooks::{HookContentType, HookId, HookRecord};
    let mut w = World::new();
    w.flag(claude_row());
    let rec = HookRecord {
        url: graftyard_identity::Url("https://hooks.example/a".into()),
        events: smallvec![graftyard_identity::EventKind("push".into())],
        content_type: HookContentType::Json,
        active: true,
        insecure_ssl: false,
        secret: None,
        retry: false,
    };
    let put = |id| IssueOp::PutHook { id, record: rec.clone() };
    let o = w.commit(issue(app(CLAUDE), put(None)));
    assert_eq!(agent_rule(&o), Some(AgentRule::Rules), "{o:?}");
    let id = match w.commit(issue(user(OWNER), put(None))) {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Id(id) => HookId(id),
            x => panic!("{x:?}"),
        },
        o => panic!("{o:?}"),
    };
    assert_eq!(agent_rule(&w.commit(issue(app(CLAUDE), IssueOp::DeleteHook { id }))), Some(AgentRule::Rules));
    assert!(graftyard_git::hooks::hook(&*w.fold().core, id).is_some(), "the agent deleted nothing");
}

// ---- REST check runs ----

/// `RecordCheck`: an installation token records a check run only as its own App, and a first version must name a run
/// id no row holds yet, on any head SHA.
#[test]
fn a_check_run_is_recorded_only_as_its_own_app_and_under_an_unused_id() {
    let mut w = World::new();
    w.set("refs/heads/main", oid(20));
    let check = |a: AppId, run: i64, sha: u32, version: u64| -> IssueOp<graftyard_git::blob::BlobRef> {
        IssueOp::RecordCheck(CheckRunUpdate {
            run: CheckRunId(DbId(run)),
            suite: CheckSuiteId(DbId(run)),
            app: a,
            head_sha: oid(sha),
            name: "ci".into(),
            version,
            status: CheckStatus::InProgress,
            conclusion: None,
            started: None,
            completed: None,
            external_id: None,
            details_url: None,
            output: None,
        })
    };
    let refusal = |o: Result<RepoOut, RepoErr>| match o {
        Err(RepoErr::Issue { err, .. }) => Some(err),
        Ok(_) => None,
        Err(e) => panic!("{e:?}"),
    };
    assert_eq!(
        refusal(w.commit(issue(app(PLAIN), check(CLAUDE, 900, 20, 1)))),
        Some(IssueErr::Invalid("a check run's app is its actor")),
        "an App may not record a run as another App"
    );
    assert_eq!(refusal(w.commit(issue(app(PLAIN), check(PLAIN, 900, 20, 1)))), None, "its own App");
    assert_eq!(
        refusal(w.commit(issue(app(PLAIN), check(PLAIN, 900, 21, 1)))),
        Some(IssueErr::Exists),
        "a first version under a taken id, even on another commit"
    );
    assert_eq!(refusal(w.commit(issue(app(PLAIN), check(PLAIN, 900, 20, 2)))), None, "an update of the same run");
    assert_eq!(refusal(w.commit(issue(app(PLAIN), check(PLAIN, 900, 20, 1)))), None, "a late re-delivery of version 1");
    let mut renamed = check(PLAIN, 900, 20, 1);
    if let IssueOp::RecordCheck(c) = &mut renamed {
        c.name = "lint".into();
    }
    assert_eq!(refusal(w.commit(issue(app(PLAIN), renamed))), Some(IssueErr::Exists), "another run's id, same commit");
    assert_eq!(refusal(w.commit(issue(app(PLAIN), check(PLAIN, 901, 21, 1)))), None, "a new id");
    // A user's command names its App, as before.
    assert_eq!(refusal(w.commit(issue(user(OWNER), check(CLAUDE, 902, 20, 1)))), None, "a user records any App's run");
}

/// `POST /repos/{o}/{r}/check-runs` (lf4-rest-3): `CreateCheck` takes its run id, and its suite's, from the repository's
/// `IdBlock` cursor in `decide`, on real members. Two creates in one entry get distinct ids, the next entry's create a
/// third, each run's row holds its id, and a re-sent request answers the id its first decide assigned.
#[test]
fn check_run_creates_take_distinct_ids_from_the_block_in_decide() {
    let mut w = World::new();
    let create = |name: &str| -> IssueOp<graftyard_git::blob::BlobRef> {
        IssueOp::CreateCheck(CheckRunUpdate {
            // Whatever the edge sends here is ignored.
            run: CheckRunId(DbId(77)),
            suite: CheckSuiteId(DbId(77)),
            app: PLAIN,
            head_sha: oid(20),
            name: name.into(),
            version: 1,
            status: CheckStatus::InProgress,
            conclusion: None,
            started: None,
            completed: None,
            external_id: None,
            details_url: None,
            output: None,
        })
    };
    let id = |o: &RepoOutcome| match o {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Id(id) => id,
            ref o => panic!("{o:?}"),
        },
        o => panic!("{o:?}"),
    };
    let both = w.commit_all(vec![issue(app(PLAIN), create("a")), issue(app(PLAIN), create("b"))]);
    let (a, b) = (id(&both[0]), id(&both[1]));
    let c = id(&w.commit(issue(app(PLAIN), create("c"))));
    assert!(a != b && c != a && c != b, "distinct: {a:?} {b:?} {c:?}");
    let block = graftyard_types::IdBlock::whole(0, 3).unwrap();
    assert!([a, b, c].iter().all(|i| block.index_of(*i).is_some()), "from the leased block: {a:?} {b:?} {c:?}");
    let f = w.fold();
    for (i, name) in [(a, "a"), (b, "b"), (c, "c")] {
        let rows = tiers::get_rec::<tiers::Timed<CheckRunUpdate>>(
            &*f.meta,
            "check_run",
            &[tiers::sha_key(&oid(20)), tiers::int(i.0)],
        )
        .unwrap_or_else(|| panic!("run {name} under {i:?}"));
        assert_eq!((&*rows.rec.name, rows.rec.run, rows.rec.suite), (name, CheckRunId(i), CheckSuiteId(i)));
    }
    // Under another App's name the create is still refused, and a user may not create one.
    let mut other = create("d");
    if let IssueOp::CreateCheck(u) = &mut other {
        u.app = CLAUDE;
    }
    assert!(matches!(
        w.commit(issue(app(PLAIN), other)),
        Err(RepoErr::Issue { err: IssueErr::Invalid("a check run's app is its actor"), .. })
    ));
    // The same request re-sent answers its first id and commits nothing new.
    let rid = w.rid();
    let l = w.repo.leader();
    let now = w.now;
    let cmd = RepoCmd::Issue(Box::new(IssueCall::new(create("e"), app(PLAIN), rid, serde_json::Value::Null)));
    // It crosses the binding as format 32.18, which a member that reads only up to 32.17 refuses.
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["issue"]["format"], serde_json::json!([0x20, 18]));
    assert_eq!(serde_json::from_value::<RepoCmd>(v).unwrap(), cmd);
    let mut answers = Vec::new();
    for _ in 0..2 {
        let from = w.repo.resolutions.len();
        let t = w.repo.on(l, now, |m, t| m.commit(t, rid, cmd.clone())).expect("admitted").ticket;
        w.settle();
        answers.extend(w.repo.resolutions[from..].iter().filter_map(|r| match r {
            Resolution::Ticket { ticket, result: Ok(a) } if *ticket == t => Some(a.out.clone()),
            _ => None,
        }));
    }
    let [RepoOut::Issue(x), RepoOut::Issue(y)] = &answers[..] else { panic!("{answers:?}") };
    assert!(matches!(x.out, IssueOut::Id(_)) && x.out == y.out, "the duplicate answers the first id: {answers:?}");
    let f = w.fold();
    let named_e = graftyard_fold::Read::prefix(&*f.meta, "check_run", &[]).into_iter().filter(|(k, _)| {
        tiers::get_rec::<tiers::Timed<CheckRunUpdate>>(&*f.meta, "check_run", k).is_some_and(|t| &*t.rec.name == "e")
    });
    assert_eq!(named_e.count(), 1, "applied once");
}

// ---- admission's write marks ----

/// The repository leader's admission, as its `/status` reports it.
fn admission(w: &World) -> graftyard_raft::AdmissionView {
    w.repo.members[w.repo.leader().0 as usize].admission().expect("the leader's admission")
}

fn open_issue(who: i64, t: &str) -> RepoCmd {
    issue(
        user(who),
        IssueOp::OpenIssue { title: t.into(), body: "".into(), labels: SmallVec::new(), assignees: SmallVec::new() },
    )
}

/// `hot` from `RepoFold`'s marks: HOT_PUSHES is 2 a second over HOT_WINDOW's 5 s, so 10 pushes in one entry leave the
/// group cold and 11 turn it hot. Writes that move no ref never do.
#[test]
fn a_repo_group_turns_hot_above_the_push_rate_and_stays_cold_below_it() {
    let mut w = World::new();
    let pushes = |from: u32, n: u32| -> Vec<RepoCmd> {
        let p = |i| RepoCmd::Push(push(user(WRITER), vec![up(&format!("refs/heads/h{i}"), None, Some(oid(i)))], &[]));
        (from..from + n).map(p).collect()
    };
    let o = w.commit_all((0..11).map(|i| open_issue(WRITER, &format!("i{i}"))).collect());
    assert!(o.iter().all(Result::is_ok), "{o:?}");
    assert!(!admission(&w).hot, "11 issues moved no ref");
    // Past the seed's push of `main`, so the window holds only what follows.
    let window = graftyard_raft::admit::HOT_WINDOW.as_millis() as u64;
    w.now = Timestamp(w.now.0 + window);
    let o = w.commit_all(pushes(0, 10));
    assert!(o.iter().all(|o| refs(o) == [RefOutcome::Ok]), "{o:?}");
    assert!(!admission(&w).hot, "10 pushes in 5 s is 2 a second, not above it");
    // Past the window those 10 no longer count, so a fresh 11 is what turns it hot.
    w.now = Timestamp(w.now.0 + window);
    let o = w.commit_all(pushes(100, 11));
    assert!(o.iter().all(|o| refs(o) == [RefOutcome::Ok]), "{o:?}");
    assert!(admission(&w).hot, "11 pushes in 5 s");
}

/// DEMOTE_PER_MIN from `RepoFold`'s marks: a user token the edge classes Human keeps Human through 59 committed writes
/// in the minute, pushes and issues alike, and the 60th demotes it to Agent.
#[test]
fn an_actor_past_demote_per_min_is_demoted() {
    use graftyard_raft::admit::{ActorKey, Admit, AdmitClass, DEMOTE_PER_MIN};
    let mut w = World::new();
    let mut cmds: Vec<RepoCmd> = (0..30)
        .map(|i| RepoCmd::Push(push(user(WRITER), vec![up(&format!("refs/heads/d{i}"), None, Some(oid(i)))], &[])))
        .collect();
    cmds.extend((0..DEMOTE_PER_MIN - 1 - 30).map(|i| open_issue(WRITER, &format!("d{i}"))));
    let o = w.commit_all(cmds);
    assert!(o.iter().all(Result::is_ok), "{o:?}");
    let class = |w: &mut World, i: u32| {
        let a = Admit {
            class: AdmitClass::Human,
            actor: Some(ActorKey(WRITER as u64)),
            push: false,
            inline: 0,
            placement_ok: true,
            budget: None,
        };
        let (l, rid, now) = (w.repo.leader(), w.rid(), w.now);
        let c = open_issue(WRITER, &format!("probe{i}"));
        let class = w.repo.on(l, now, |m, t| m.commit_as(t, rid, c, a)).expect("admitted").class;
        w.settle();
        class
    };
    assert_eq!(class(&mut w, 0), AdmitClass::Human, "59 writes in the minute");
    assert_eq!(class(&mut w, 1), AdmitClass::Agent, "60 writes in the minute");
    // Another user is not counted with it.
    let a = Admit {
        class: AdmitClass::Human,
        actor: Some(ActorKey(WRITER2 as u64)),
        push: false,
        inline: 0,
        placement_ok: true,
        budget: None,
    };
    let (l, rid, now) = (w.repo.leader(), w.rid(), w.now);
    let c = open_issue(WRITER2, "other");
    assert_eq!(w.repo.on(l, now, |m, t| m.commit_as(t, rid, c, a)).expect("admitted").class, AdmitClass::Human);
    w.settle();
}

/// Format 32.22 through real members: a review comment outside a review by a reviewer who approved gets its
/// single-comment review, a `SingleComment`, and the pull request's verdict stays APPROVED, as on GHES. A COMMENTED
/// verdict is replaced by the next one, and a later APPROVE still replaces the single-comment review's. The command
/// crosses the binding as 32.22, and the replay answers the comment's id.
#[test]
fn an_approvers_review_comment_gets_its_review_and_keeps_the_approval() {
    use graftyard_git::issues::{CommentTarget, DiffSide, LineAnchor, ReviewEvent};
    let mut w = World::new();
    w.set("refs/heads/f", oid(70));
    let open = IssueOp::OpenPr {
        head: HeadSpec { repo: REPO, branch: name("refs/heads/f") },
        base: name("refs/heads/main"),
        title: "t".into(),
        body: "b".into(),
        draft: false,
        head_sha: oid(70),
        base_sha: w.get("refs/heads/main").unwrap(),
    };
    let n = IssueNumber(number(&w.commit(issue(user(WRITER), open))));
    let verdicts = |w: &mut World| {
        let f = w.fold();
        graftyard_fold::Read::prefix(&*f.core, "review_verdict", &[tiers::n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| tiers::get_rec::<tiers::VerdictRow>(&*f.core, "review_verdict", &k))
            .map(|r| r.state)
            .collect::<Vec<_>>()
    };
    let review = |state| IssueOp::Review { n, state, commit: oid(70), body: "".into() };
    w.commit(issue(user(WRITER2), review(ReviewState::Approved))).unwrap();
    let at = LineAnchor { commit: oid(70), path: "a.rs".into(), line: 1, start_line: None, side: DiffSide::Right };
    let comment = |reply_to| IssueOp::Comment {
        on: CommentTarget::Review { n, review: None, at: at.clone() },
        body: "nit".into(),
        reply_to,
    };
    let call = |w: &mut World, op| {
        let mut c = IssueCall::new(op, user(WRITER2), w.rid(), serde_json::Value::Null);
        c.reviews = true;
        RepoCmd::Issue(Box::new(c))
    };
    let cmd = call(&mut w, comment(None));
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["issue"]["format"], serde_json::json!([0x20, 22]));
    assert_eq!(serde_json::from_value::<RepoCmd>(v).unwrap(), cmd);
    let id = match w.commit(cmd) {
        Ok(RepoOut::Issue(c)) => match c.out {
            IssueOut::Id(id) => id,
            o => panic!("{o:?}"),
        },
        o => panic!("{o:?}"),
    };
    assert_eq!(verdicts(&mut w), [ReviewState::Approved], "the approval stays");
    // The comment names a COMMENTED review with no body, which the metadata tier lists.
    let f = w.fold();
    let row: tiers::CommentRow = tiers::get_rec(&*f.meta, "comment", &[tiers::int(id.0)]).unwrap();
    let CommentTarget::Review { review: Some(r), .. } = row.on else { panic!("{row:?}") };
    let rr: tiers::ReviewRow = tiers::get_rec(&*f.meta, "review", &[tiers::int(r.0.0)]).unwrap();
    assert_eq!((rr.state, &*rr.body, rr.reviewer), (ReviewState::Commented, "", Some(AccountId(DbId(WRITER2)))));
    let l = w.repo.leader();
    let single = w.repo.members[l.0 as usize].log().iter().any(|e| match &e.body {
        graftyard_raft::EntryBody::Events(evs) => {
            evs.iter().any(|e| matches!(e, RepoEvent::Review(ReviewEvent::SingleComment { id: x, .. }) if *x == r))
        }
        _ => false,
    });
    assert!(single, "committed as SingleComment");
    // A reply by the same approver: its own review, the approval still stands.
    let reply = call(&mut w, comment(Some(graftyard_git::issues::CommentId(id))));
    w.commit(reply).unwrap();
    assert_eq!(verdicts(&mut w), [ReviewState::Approved]);
    // Changes requested, then a comment: still CHANGES_REQUESTED.
    w.commit(issue(user(WRITER2), review(ReviewState::ChangesRequested))).unwrap();
    let c2 = call(&mut w, comment(None));
    w.commit(c2).unwrap();
    assert_eq!(verdicts(&mut w), [ReviewState::ChangesRequested]);
    // A commenter with no verdict: the single-comment review is theirs; an APPROVE then replaces it.
    let mut c3 = IssueCall::new(comment(None), user(OWNER), w.rid(), serde_json::Value::Null);
    c3.reviews = true;
    w.commit(RepoCmd::Issue(Box::new(c3))).unwrap();
    assert_eq!(verdicts(&mut w).iter().filter(|s| **s == ReviewState::Commented).count(), 1);
    w.commit(issue(user(OWNER), review(ReviewState::Approved))).unwrap();
    let mut v = verdicts(&mut w);
    v.sort_by_key(|s| *s as u8);
    assert_eq!(v, [ReviewState::Approved, ReviewState::ChangesRequested]);
}
