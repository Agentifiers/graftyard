//! The `webhooks` surface (`graftyard_api::webhooks`) on a repository chain's tiers: every webhook the committed
//! entries carry, rendered as the consumer renders it, the delivery request and its `X-Hub-Signature-256`, and the
//! REST `hook` object. Each payload is written to `$GRAFTYARD_REPLIES/<case>.webhook.json` when it is set, for
//! `gh/validate.py` against the pinned GHES 3.22 description's `x-webhooks` (and dotcom's for `merge_group`).

mod common;

use std::collections::BTreeMap;

use common::*;
use graftyard_api::webhooks::{hook_json, payload, ping, request};
use graftyard_api::wire::Host;
use graftyard_git::hooks::{HookContentType, HookId, HookRecord, delivery_guid, hook_row, webhooks};
use graftyard_git::issue_ops::{IssueOp, IssueOut};
use graftyard_git::issues::{
    AppId, CheckConclusion, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, CommentTarget, HeadSpec,
    IssueChange, IssueNumber, StatusId, StatusState, StatusUpdate, Url,
};
use graftyard_git::queue::{EjectReason, MergeQueueEntry, QueueEvent, queue_ref};
use graftyard_git::refs::RefUpdate;
use graftyard_git::repo::RepoEvent;
use graftyard_git::sequencer::{HookDelivery, NewDelivery, Sequencer};
use graftyard_identity::sealed::{HookSecret, HookSecrets, Wrapped, hmac_sha256};
use graftyard_identity::{EventKind, UserId};
use graftyard_raft::{EntryBody, TierSet};
use graftyard_types::{AccountId, ChainId, DbId, Index, Timestamp};
use serde_json::{Value, json};
use smallvec::smallvec;

/// The id `PutHook` takes on `seeded`'s chain: the core tier's next id.
const HOOK: HookId = HookId(DbId(98_304));

fn host() -> Host {
    Host::new("ghes.example.com", false).unwrap()
}

/// The in-memory `graft-ops` stub: XOR under a fixed pad, so a wrong slot opens nothing, as M3-WP4 stubbed `KeySource`.
struct Ops;

impl HookSecrets for Ops {
    fn wrap(&self, plain: &[u8], nonce: [u8; 12]) -> Result<Wrapped, graftyard_identity::stateless::KeyError> {
        Ok(Wrapped { slot: 1, nonce, ciphertext: plain.iter().map(|b| b ^ 0x5a).collect() })
    }
    fn unwrap(&self, w: &Wrapped) -> Result<HookSecret, graftyard_identity::stateless::KeyError> {
        if w.slot != 1 {
            return Err(graftyard_identity::stateless::KeyError::Unwrap("no such slot".into()));
        }
        Ok(HookSecret(w.ciphertext.iter().map(|b| b ^ 0x5a).collect()))
    }
}

fn record(url: &str, secret: Option<Wrapped>) -> HookRecord {
    HookRecord {
        url: Url(url.into()),
        events: smallvec![EventKind("*".into())],
        content_type: HookContentType::Json,
        active: true,
        insecure_ssl: false,
        secret,
        retry: false,
    }
}

/// The schema key of a webhook: `push`, or `pull-request-opened`.
fn key(event: &str, action: Option<&str>) -> String {
    let e = event.replace('_', "-");
    match action {
        Some(a) => format!("{e}-{}", a.replace('_', "-")),
        None => e,
    }
}

/// A repository with a hook (whose creation pings it), pushes, a branch made and deleted, an issue edited and closed,
/// a comment, a pull request opened, synchronized, made a draft and ready again, closed and reopened, a status, a
/// check run created and completed, and a merge queue's group built, ejected, and landed.
fn seeded() -> (Chain, usize) {
    let mut c = Chain::new(1);
    c.push("refs/heads/main", oid(1));
    c.push("refs/heads/f", oid(2));
    let secret = Ops.wrap(b"It's a Secret to Everybody", [3; 12]).unwrap();
    let id =
        match c.commit(OWNER, IssueOp::PutHook { id: None, record: record("https://ci.example/hook", Some(secret)) }) {
            Ok(IssueOut::Id(i)) => i,
            o => panic!("{o:?}"),
        };
    assert_eq!(id, HOOK.0, "the hook takes the core tier's next id");
    let from = c.log.len();
    c.push("refs/heads/main", oid(3));
    c.push("refs/heads/tmp", oid(4));
    let del = RepoEvent::Refs {
        push: graftyard_types::RequestId(graftyard_types::Ulid(77)),
        actor: Some(AccountId(DbId(WRITER))),
        updates: vec![RefUpdate { name: name("refs/heads/tmp"), expect: Some(oid(4)), new: None }],
        pack: None,
    };
    c.entry(EntryBody::Events(vec![del]), TierSet::CORE);
    let open = IssueOp::OpenIssue {
        title: "it breaks".into(),
        body: "steps".into(),
        labels: smallvec::SmallVec::new(),
        assignees: smallvec![AccountId(DbId(WRITER))],
    };
    c.commit(READER, open).unwrap();
    c.commit(READER, IssueOp::Change { n: IssueNumber(1), change: IssueChange::Title("it really breaks".into()) })
        .unwrap();
    c.commit(
        READER,
        IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "me too".into(), reply_to: None },
    )
    .unwrap();
    c.commit(OWNER, IssueOp::Close { n: IssueNumber(1) }).unwrap();
    c.commit(OWNER, IssueOp::Change { n: IssueNumber(1), change: IssueChange::State { open: true, reason: None } })
        .unwrap();
    let pr = IssueOp::OpenPr {
        head: HeadSpec { repo: REPO, branch: name("refs/heads/f") },
        base: name("refs/heads/main"),
        title: "fix it".into(),
        body: "fixes #1".into(),
        draft: false,
        head_sha: oid(2),
        base_sha: oid(3),
    };
    c.commit(WRITER, pr).unwrap();
    c.push("refs/heads/f", oid(5));
    c.commit(WRITER, IssueOp::Change { n: IssueNumber(2), change: IssueChange::Draft(true) }).unwrap();
    c.commit(WRITER, IssueOp::Change { n: IssueNumber(2), change: IssueChange::Draft(false) }).unwrap();
    c.commit(WRITER, IssueOp::Change { n: IssueNumber(2), change: IssueChange::Body("fixes #1, really".into()) })
        .unwrap();
    c.commit(WRITER, run(10, 5, None)).unwrap();
    c.commit(
        WRITER,
        IssueOp::RecordCheck(CheckRunUpdate { version: 2, ..check(10, 5, Some(CheckConclusion::Success)) }),
    )
    .unwrap();
    let status = StatusUpdate {
        id: StatusId(DbId(12)),
        sha: oid(5),
        context: "jenkins".into(),
        state: StatusState::Success,
        target_url: Some(Url("https://jenkins.example/1".into())),
        description: Some("build".into()),
        creator: AccountId(DbId(WRITER)),
    };
    c.commit(WRITER, IssueOp::RecordStatus(status)).unwrap();
    let base = name("refs/heads/main");
    let enq = QueueEvent::Enqueued {
        n: IssueNumber(2),
        base: base.clone(),
        head: oid(5),
        actor: UserId(AccountId(DbId(WRITER))),
    };
    let e = MergeQueueEntry { pr: IssueNumber(2), group_tip: oid(6), parent_tip: oid(3) };
    let qref = queue_ref(&base, &e).unwrap();
    let built = QueueEvent::GroupBuilt {
        base: base.clone(),
        entries: smallvec![e],
        refs: vec![RefUpdate { name: qref.clone(), expect: None, new: Some(oid(6)) }],
        pack: None,
    };
    let ejected = QueueEvent::Ejected {
        n: IssueNumber(2),
        reason: EjectReason::Dequeued,
        refs: vec![RefUpdate { name: qref.clone(), expect: Some(oid(6)), new: None }],
    };
    // A bot `Refs` entry on a queue ref (as `MergeQueue` cleans one up): no `push` for it.
    let bot = RepoEvent::Refs {
        push: graftyard_types::RequestId(graftyard_types::Ulid(78)),
        actor: None,
        updates: vec![RefUpdate { name: qref.clone(), expect: None, new: Some(oid(9)) }],
        pack: None,
    };
    c.entry(EntryBody::Events(vec![bot]), TierSet::CORE);
    for q in [enq, built, ejected] {
        let t = graftyard_git::tiers::tier_of(&RepoEvent::Queue(q.clone()));
        c.entry(EntryBody::Events(vec![RepoEvent::Queue(q)]), t);
    }
    c.commit(OWNER, IssueOp::Close { n: IssueNumber(2) }).unwrap();
    c.commit(OWNER, IssueOp::Change { n: IssueNumber(2), change: IssueChange::State { open: true, reason: None } })
        .unwrap();
    // R24: one Actions job's three `workflow_job` steps, as `RunOwner` appends them.
    for u in workflow_job_steps() {
        c.entry(EntryBody::Events(vec![RepoEvent::WorkflowJob(u)]), TierSet::METADATA);
    }
    (c, from)
}

fn workflow_job_steps() -> Vec<graftyard_git::workflow_job::WorkflowJobUpdate> {
    use graftyard_git::workflow_job::{JobAction, WorkflowJobUpdate};
    let queued = WorkflowJobUpdate {
        job: DbId(4_101),
        run: DbId(4_100),
        attempt: 1,
        action: JobAction::Queued,
        name: "build".into(),
        workflow: ".github/workflows/ci.yml".into(),
        head_sha: oid(5),
        head_ref: "refs/heads/f".into(),
        labels: smallvec!["ubuntu-latest".into()],
        runner: None,
        conclusion: None,
        created: Timestamp(1_700_000_000_000),
        started: None,
        completed: None,
    };
    let started = WorkflowJobUpdate {
        action: JobAction::InProgress,
        runner: Some(DbId(77)),
        started: Some(Timestamp(1_700_000_004_000)),
        ..queued.clone()
    };
    let done = WorkflowJobUpdate {
        action: JobAction::Completed,
        conclusion: Some(CheckConclusion::Success),
        completed: Some(Timestamp(1_700_000_064_000)),
        ..started.clone()
    };
    vec![queued, started, done]
}

fn check(id: i64, sha: u8, conclusion: Option<CheckConclusion>) -> CheckRunUpdate {
    CheckRunUpdate {
        run: CheckRunId(DbId(id)),
        suite: CheckSuiteId(DbId(1)),
        app: AppId(DbId(5)),
        head_sha: oid(sha),
        name: "ci".into(),
        version: 1,
        status: if conclusion.is_some() { CheckStatus::Completed } else { CheckStatus::InProgress },
        conclusion,
        started: Some(Timestamp(1_700_000_000_000)),
        completed: conclusion.map(|_| Timestamp(1_700_000_060_000)),
        external_id: None,
        details_url: Some(Url(format!("https://ci.example/run/{id}").into())),
        output: None,
    }
}

fn run(id: i64, sha: u8, conclusion: Option<CheckConclusion>) -> IssueOp {
    IssueOp::RecordCheck(check(id, sha, conclusion))
}

/// Every entry's webhooks, rendered against the fold as of that entry (a fresh chain replayed to it), as the consumer
/// reads the log.
fn rendered() -> Vec<(String, Value)> {
    let (c, _) = seeded();
    let (h, d) = (host(), Dir::new(OWNER));
    let mut replay = Chain::new(1);
    replay.log.clear();
    let mut out = Vec::new();
    for e in c.log.iter().skip(1) {
        replay.entry(e.body.clone(), e.tiers);
        let r = replay.repo(0, &h, &d);
        let hook = hook_row(r.core, HOOK);
        for w in webhooks(r.core, REPO, e) {
            let p = payload(&r, &w, hook.as_ref().map(|row| (HOOK, row)), e.at).expect("the fold has it");
            out.push((key(w.event, w.action), p));
        }
    }
    out
}

#[test]
fn every_webhook_the_entries_carry_renders_against_its_schema_key() {
    let all = rendered();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (k, _) in &all {
        *counts.entry(k.clone()).or_default() += 1;
    }
    let keys: Vec<&str> = counts.keys().map(|s| s.as_str()).collect();
    assert_eq!(
        keys,
        [
            "check-run-completed",
            "check-run-created",
            "create",
            "delete",
            "issue-comment-created",
            "issues-closed",
            "issues-edited",
            "issues-opened",
            "issues-reopened",
            "merge-group-checks-requested",
            "merge-group-destroyed",
            "ping",
            "pull-request-closed",
            "pull-request-converted-to-draft",
            "pull-request-dequeued",
            "pull-request-edited",
            "pull-request-enqueued",
            "pull-request-opened",
            "pull-request-ready-for-review",
            "pull-request-reopened",
            "pull-request-synchronize",
            "push",
            "status",
            "workflow-job-completed",
            "workflow-job-in-progress",
            "workflow-job-queued",
        ]
    );
    assert_eq!(counts["push"], 6, "main twice, f twice, tmp made and deleted, and never a queue ref");
    assert!(all.iter().all(|(_, v)| !v["ref"].as_str().is_some_and(|r| r.contains("gh-readonly-queue"))));
    // `edited` requires `changes` in GHES 3.22's schemas.
    for (k, v) in all.iter().filter(|(k, _)| k.ends_with("-edited")) {
        assert!(v["changes"].is_object(), "{k} has no changes");
    }
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let mut n: BTreeMap<String, usize> = BTreeMap::new();
        for (k, body) in &all {
            let i = n.entry(k.clone()).or_default();
            *i += 1;
            let v = json!({ "key": k, "body": body });
            std::fs::write(format!("{dir}/hook-{k}-{i}.webhook.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
        }
    }
}

#[test]
fn a_push_names_both_shas_and_a_synchronize_the_pull_request_it_moved() {
    let all = rendered();
    let push: Vec<&Value> = all.iter().filter(|(k, _)| k == "push").map(|(_, v)| v).collect();
    let tmp_gone = push.iter().find(|p| p["ref"] == "refs/heads/tmp" && p["deleted"] == true).unwrap();
    assert_eq!(tmp_gone["after"], "0".repeat(40));
    assert_eq!(tmp_gone["head_commit"], Value::Null);
    let sync = &all.iter().find(|(k, _)| k == "pull-request-synchronize").unwrap().1;
    assert_eq!((sync["number"].as_u64(), sync["before"].as_str()), (Some(2), Some(oid(2).hex().as_str())));
    assert_eq!(sync["after"], oid(5).hex());
    assert_eq!(sync["sender"]["login"], "alice");
    assert!(sync["repository"].get("permissions").is_none(), "a payload names no caller's permissions");
}

/// BF-78: a `push` run's `github.event`, built as the `push` webhook with the commits the git cache read. Each commit
/// is its own object's tree, message, author and committer, with no `added`, `removed` or `modified` (GHES leaves them
/// out of Actions' payload), `forced` is what the trigger found, and the rest of the webhook's fields stand. It is
/// written for `gh/validate.py` with the webhooks as `push`.
#[test]
fn an_actions_push_payload_carries_the_pushed_commits_and_forced() {
    let (c, _) = seeded();
    let (h, d) = (host(), Dir::new(OWNER));
    let r = c.repo(0, &h, &d);
    let tree = "4b825dc642cb6eb9a060e54bf8d69288fbe4904c";
    let raw = |msg: &str, t: u64| {
        format!(
            "tree {tree}\nparent {}\nauthor Ann <ann@x.example> {t} +0000\ncommitter Cy <cy@x.example> {} +0000\n\n{msg}\n",
            oid(1).hex(),
            t + 60
        )
    };
    let first = graftyard_api::webhooks::push_commit(&r, oid(20), raw("one", 1_700_000_000).as_bytes()).unwrap();
    let head = graftyard_api::webhooks::push_commit(&r, oid(21), raw("two\n\nbody", 1_700_000_100).as_bytes()).unwrap();
    assert!(graftyard_api::webhooks::push_commit(&r, oid(22), b"not a commit").is_none());
    let v = graftyard_api::webhooks::actions_push(
        &r,
        &graftyard_git::refs::RefName::parse(b"refs/heads/main").unwrap(),
        (oid(1), oid(21)),
        Some(AccountId(DbId(WRITER))),
        Timestamp(1_700_000_200_000),
        true,
        vec![first, head.clone()],
        head,
    );
    assert_eq!(
        (v["forced"].as_bool(), v["created"].as_bool(), v["deleted"].as_bool()),
        (Some(true), Some(false), Some(false))
    );
    assert_eq!(
        v["commits"].as_array().unwrap().iter().map(|c| c["id"].clone()).collect::<Vec<_>>(),
        [json!(oid(20).hex()), json!(oid(21).hex())]
    );
    let hc = &v["head_commit"];
    assert_eq!((hc["message"].as_str(), hc["tree_id"].as_str()), (Some("two\n\nbody"), Some(tree)));
    assert_eq!((hc["author"]["email"].as_str(), hc["committer"]["name"].as_str()), (Some("ann@x.example"), Some("Cy")));
    assert_eq!(hc["timestamp"], "2023-11-14T22:16:00Z", "the committer's date, not the author's 22:15:00");
    assert!(hc.get("added").is_none() && hc.get("modified").is_none(), "Actions' payload leaves them out");
    assert_eq!((v["pusher"]["name"].as_str(), v["sender"]["login"].as_str()), (Some("alice"), Some("alice")));
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let w = json!({ "key": "push", "body": v });
        std::fs::write(format!("{dir}/hook-push-actions.webhook.json"), serde_json::to_vec_pretty(&w).unwrap())
            .unwrap();
    }
}

#[test]
fn a_new_hook_is_pinged_once_and_an_edit_is_not() {
    let (mut c, from) = seeded();
    let pings = |c: &Chain| {
        c.log.iter().flat_map(|e| webhooks(&c.members[0].core, REPO, e)).filter(|w| w.event == "ping").count()
    };
    assert_eq!(pings(&c), 1);
    assert!(c.log[..from].iter().flat_map(|e| webhooks(&c.members[0].core, REPO, e)).any(|w| w.target.is_some()));
    let mut edited = record("https://ci.example/hook", None);
    edited.events = smallvec![EventKind("push".into())];
    c.commit(OWNER, IssueOp::PutHook { id: Some(HOOK), record: edited.clone() }).unwrap();
    assert_eq!(pings(&c), 1, "an edit does not ping");
    let before = c.log.len();
    c.commit(OWNER, IssueOp::PutHook { id: Some(HOOK), record: edited }).unwrap();
    assert_eq!(c.log.len(), before, "an identical edit commits nothing");
}

#[test]
fn the_request_is_signed_over_the_body_it_sends_and_the_rest_object_hides_the_secret() {
    let (c, _) = seeded();
    let (h, d) = (host(), Dir::new(OWNER));
    let r = c.repo(0, &h, &d);
    let id = HOOK;
    let row = hook_row(r.core, id).unwrap();
    let body = serde_json::to_vec(&ping(&r, id, &row)).unwrap();
    let chain = ChainId::Repo(REPO);
    let guid = delivery_guid(1, chain, Index(4), 0, id);
    let mut seq = Sequencer::new(AccountId(DbId(OWNER)), c.now);
    seq.record(
        chain,
        Index(4),
        vec![NewDelivery {
            guid,
            hook: id,
            entry: Some((chain, Index(4), 0)),
            event: "ping".into(),
            action: None,
            repo: Some(REPO),
            body: body.clone(),
        }],
        c.now,
    );
    let del: &HookDelivery = seq.delivery(&guid).unwrap();
    let secret = Ops.unwrap(row.record.secret.as_ref().unwrap()).unwrap();
    let req = request("ghes.example.com", del, &row.record, ("repository", REPO.0.0), Some(&secret));
    assert_eq!(req.body, body);
    let header = |n: &str| req.headers.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone());
    let mac = hmac_sha256(b"It's a Secret to Everybody", &req.body);
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(header("x-hub-signature-256"), Some(format!("sha256={hex}")));
    assert_eq!(header("x-github-delivery"), Some(guid.to_string()));
    assert_eq!(header("x-github-event").as_deref(), Some("ping"));
    assert_eq!(header("x-github-hook-id"), Some(HOOK.0.0.to_string()));
    let unsigned = request("ghes.example.com", del, &row.record, ("repository", REPO.0.0), None);
    assert!(unsigned.headers.iter().all(|(k, _)| !k.starts_with("x-hub-signature")));
    let text = serde_json::to_string(&hook_json(&r, id, &row)).unwrap() + &format!("{row:?}") + &format!("{req:?}");
    assert!(!text.contains("Secret to Everybody"), "no rendering or debug print shows the secret");
    assert_eq!(hook_json(&r, id, &row)["config"]["secret"], "********");
    // A form hook signs the bytes it sends, `payload=` and the escaped JSON.
    let form = HookRecord { content_type: HookContentType::Form, ..row.record.clone() };
    let req = request("ghes.example.com", del, &form, ("repository", REPO.0.0), Some(&secret));
    assert!(req.body.starts_with(b"payload=%7B"));
    let mac = hmac_sha256(b"It's a Secret to Everybody", &req.body);
    let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(req.headers.iter().find(|(k, _)| *k == "x-hub-signature-256").unwrap().1, format!("sha256={hex}"));
}

#[test]
fn the_rest_hook_object_matches_ghes_hook() {
    let (c, _) = seeded();
    let (h, d) = (host(), Dir::new(OWNER));
    let r = c.repo(0, &h, &d);
    let id = HOOK;
    let v = hook_json(&r, id, &hook_row(r.core, id).unwrap());
    assert_eq!(v["ping_url"], format!("https://ghes.example.com/api/v3/repos/o/r/hooks/{}/pings", HOOK.0.0));
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let path = "/repos/{owner}/{repo}/hooks/{hook_id}";
        let k = json!({ "method": "GET", "path": path, "status": 200, "body": v });
        std::fs::write(format!("{dir}/hook-get.rest.json"), serde_json::to_vec_pretty(&k).unwrap()).unwrap();
    }
}

fn keep_rest(case: &str, method: &str, path: &str, r: &graftyard_api::wire::Reply) {
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "method": method, "path": path, "status": r.status, "body": r.value() });
        std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }
}

#[test]
fn a_rest_body_becomes_a_record_and_a_patch_keeps_what_it_omits() {
    use graftyard_api::webhooks::hook_body;
    let body = json!({ "name": "web", "events": ["push", "pull_request"], "config": {
        "url": "https://ci.example/h", "content_type": "json", "secret": "s3", "insecure_ssl": "0" } });
    let (r, secret) = hook_body(None, &body).unwrap();
    assert_eq!(r.events.iter().map(|e| &*e.0).collect::<Vec<_>>(), ["push", "pull_request"]);
    assert_eq!((r.content_type, r.active, r.retry), (HookContentType::Json, true, false));
    assert_eq!(secret, Some(Some("s3".into())));
    // GHES's defaults: `push` only, `form`.
    let (d, s) = hook_body(None, &json!({ "config": { "url": "http://x.example/" } })).unwrap();
    assert_eq!((d.events[0].0.as_ref(), d.content_type, s), ("push", HookContentType::Form, None));
    let cur = HookRecord { secret: Some(Ops.wrap(b"k", [0; 12]).unwrap()), ..r.clone() };
    let (p, s) =
        hook_body(Some(&cur), &json!({ "add_events": ["issues"], "remove_events": ["push"], "active": false }))
            .unwrap();
    assert_eq!(p.events.iter().map(|e| &*e.0).collect::<Vec<_>>(), ["pull_request", "issues"]);
    assert_eq!((p.url.0.as_ref(), p.active, p.secret.is_some(), s), ("https://ci.example/h", false, true, None));
    for bad in [
        json!({ "config": {} }),
        json!({ "config": { "url": "ftp://x" } }),
        json!({ "config": { "url": "https://x" }, "events": ["no_such_event"] }),
        json!({ "config": { "url": "https://x" }, "events": [] }),
        json!({ "name": "email", "config": { "url": "https://x" } }),
    ] {
        assert_eq!(hook_body(None, &bad).unwrap_err().status, 422, "{bad}");
    }
    let (opt, _) = hook_body(None, &json!({ "config": { "url": "https://x.example", "retry": true } })).unwrap();
    assert!(opt.retry, "the retry opt-in");
}

#[test]
fn ping_deliveries_and_redelivery_through_the_sequencer() {
    use graftyard_api::webhooks::sequencer_route;
    let (c, _) = seeded();
    let (h, d) = (host(), Dir::new(OWNER));
    let r = c.repo(0, &h, &d);
    let row = hook_row(r.core, HOOK).unwrap();
    let mut seq = Sequencer::new(AccountId(DbId(OWNER)), c.now);
    let now = c.now;
    let cx = graftyard_api::webhooks::RouteCtx {
        repo: r.at.id,
        url: graftyard_api::webhooks::deliveries_url(&r, HOOK),
        ping: Some(serde_json::to_vec(&graftyard_api::webhooks::ping(&r, HOOK, &row)).unwrap()),
    };
    let r = &cx;
    let ping = sequencer_route(&mut seq, r, HOOK, &row, "POST", &["pings"], "", now, [7; 16]).unwrap();
    assert_eq!(ping.status, 204);
    keep_rest("hook-ping", "POST", "/repos/{owner}/{repo}/hooks/{hook_id}/pings", &ping);
    // Three attempts: the ping fails, a second ping succeeds, then the first is redelivered.
    let a = seq.start(now).remove(0);
    seq.report(a.attempt, Timestamp(now.0 + 120), Some(500), false, Some("boom"));
    sequencer_route(&mut seq, r, HOOK, &row, "POST", &["pings"], "", now, [8; 16]).unwrap();
    let b = seq.start(Timestamp(now.0 + 200)).remove(0);
    seq.report(b.attempt, Timestamp(now.0 + 260), Some(200), false, Some("ok"));
    let path = format!("{}", a.attempt);
    let red = sequencer_route(&mut seq, r, HOOK, &row, "POST", &["deliveries", &path, "attempts"], "", now, [0; 16]);
    let red = red.unwrap();
    assert_eq!(red.status, 202);
    keep_rest(
        "hook-redeliver",
        "POST",
        "/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}/attempts",
        &red,
    );
    let again = seq.start(Timestamp(now.0 + 300)).remove(0);
    assert!(again.redelivery && again.guid == a.guid, "a redelivery keeps its GUID");
    seq.report(again.attempt, Timestamp(now.0 + 330), Some(200), false, None);
    // The list, newest first, two to a page with a `Link` to the rest.
    let list = sequencer_route(&mut seq, r, HOOK, &row, "GET", &["deliveries"], "per_page=2", now, [0; 16]).unwrap();
    keep_rest("hook-deliveries", "GET", "/repos/{owner}/{repo}/hooks/{hook_id}/deliveries", &list);
    let items = list.value();
    let ids: Vec<u64> = items.as_array().unwrap().iter().map(|i| i["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, [again.attempt, b.attempt]);
    assert_eq!(items[0]["redelivery"], true);
    assert_eq!(items[0]["guid"], a.guid.to_string());
    let link = list.header("link").unwrap();
    let cursor = link.split("cursor=").nth(1).unwrap().split('>').next().unwrap();
    let q = format!("per_page=2&cursor={cursor}");
    let rest = sequencer_route(&mut seq, r, HOOK, &row, "GET", &["deliveries"], &q, now, [0; 16]).unwrap();
    let ids: Vec<u64> = rest.value().as_array().unwrap().iter().map(|i| i["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, [a.attempt]);
    assert!(rest.header("link").is_none());
    let one = format!("{}", a.attempt);
    let get = sequencer_route(&mut seq, r, HOOK, &row, "GET", &["deliveries", &one], "", now, [0; 16]).unwrap();
    keep_rest("hook-delivery", "GET", "/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}", &get);
    let v = get.value();
    assert_eq!((v["status_code"].as_u64(), v["status"].as_str()), (Some(500), Some("Invalid HTTP Response: 500")));
    assert_eq!(v["request"]["payload"]["hook_id"], HOOK.0.0);
    assert!(!serde_json::to_string(&v).unwrap().contains("Secret to Everybody"));
    // Another hook's delivery is not this hook's; past 3 days a redelivery is refused.
    let other = HookId(DbId(1));
    assert_eq!(
        sequencer_route(&mut seq, r, other, &row, "GET", &["deliveries", &one], "", now, [0; 16]).unwrap().status,
        404
    );
    let late = Timestamp(now.0 + graftyard_git::hooks::DELIVERY_KEEP_MS + 1_000);
    let exp = sequencer_route(&mut seq, r, HOOK, &row, "POST", &["deliveries", &one, "attempts"], "", late, [0; 16]);
    assert_eq!(exp.unwrap().status, 422);
}

#[test]
fn a_committed_put_hook_renders_ghes_hook_with_location() {
    use graftyard_api::hosted::{Render, What, render_ok};
    let (mut c, _) = seeded();
    let out = c.commit(OWNER, IssueOp::PutHook { id: None, record: record("https://two.example/h", None) }).unwrap();
    let (h, d) = (host(), Dir::new(OWNER));
    let r = c.repo(0, &h, &d);
    let dir = graftyard_api::hosted::HostedDir { home: Some(d.home()), ..Default::default() };
    let render = Render {
        host: "ghes.example.com".into(),
        isolation: false,
        dir,
        what: What::Hook { created: true },
        doc: None,
    };
    let reply = render_ok(&render, &ack(out), r.core, r.meta).reply();
    assert_eq!(reply.status, 201);
    assert!(reply.header("location").unwrap().starts_with("https://ghes.example.com/api/v3/repos/o/r/hooks/"));
    keep_rest("hook-create", "POST", "/repos/{owner}/{repo}/hooks", &reply);
    let refused = graftyard_api::hosted::render_err(&render, &graftyard_git::issue_ops::IssueErr::Exists).reply();
    assert_eq!(refused.status, 422);
    keep_rest("hook-create-taken", "POST", "/repos/{owner}/{repo}/hooks", &refused);
}

/// R24: an App's `workflow_job` body carries `installation`, and its request names the App, not a repository hook.
#[test]
fn an_apps_workflow_job_names_its_installation_and_the_app() {
    use graftyard_api::webhooks::with_installation;
    let all = rendered();
    let q = &all.iter().find(|(k, _)| k == "workflow-job-queued").unwrap().1;
    assert_eq!((q["workflow_job"]["status"].as_str(), q["workflow_job"]["runner_id"].as_i64()), (Some("queued"), None));
    assert_eq!(q["workflow_job"]["labels"], json!(["ubuntu-latest"]));
    assert_eq!(q["workflow_job"]["head_branch"], "f");
    let p = &all.iter().find(|(k, _)| k == "workflow-job-in-progress").unwrap().1;
    assert_eq!(
        (p["workflow_job"]["runner_id"].as_i64(), p["workflow_job"]["runner_name"].as_str()),
        (Some(77), Some("runner-77"))
    );
    let done = &all.iter().find(|(k, _)| k == "workflow-job-completed").unwrap().1;
    assert_eq!(done["workflow_job"]["conclusion"], "success");
    assert!(q.get("installation").is_none(), "a repository hook's body has no installation");
    let body = with_installation(q.clone(), graftyard_identity::InstallationId(DbId(7_190_002)));
    assert_eq!(body["installation"]["id"], 7_190_002);
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        let v = json!({ "key": "workflow-job-queued", "body": body });
        std::fs::write(format!("{dir}/app-workflow-job-queued.webhook.json"), serde_json::to_vec_pretty(&v).unwrap())
            .unwrap();
    }
    let app = graftyard_identity::AppId(DbId(7_100_009));
    let d = HookDelivery {
        guid: delivery_guid(1, ChainId::Repo(REPO), Index(9), 0, HookId::app(app)),
        hook: HookId::app(app),
        entry: Some((ChainId::Repo(REPO), Index(9), 0)),
        event: "workflow_job".into(),
        action: Some("queued".into()),
        repo: Some(REPO),
        body: serde_json::to_vec(&body).unwrap().into(),
        attempts: smallvec::SmallVec::new(),
        expires: Timestamp(0),
        done: false,
    };
    let r = request("ghes.test", &d, &record("https://runners.example/", None), ("integration", app.0.0), None);
    let header = |k: &str| r.headers.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
    assert_eq!(header("x-github-hook-id"), Some("7100009"));
    assert_eq!(header("x-github-hook-installation-target-type"), Some("integration"));
    assert_eq!(header("x-github-event"), Some("workflow_job"));
}

/// `app-hook-config`: GHES's `webhook-config` for an App, the secret masked; a `PATCH` body keeps what it omits,
/// clears the secret with `""`, and refuses `form`, `insecure_ssl` 1, and a URL with no host, since App deliveries are
/// JSON over verified TLS. The record's secret is the raft-codec bytes of a `Wrapped`, which `app_targets` reads back.
#[test]
fn an_apps_hook_config_renders_ghes_webhook_config_and_a_patch_keeps_what_it_omits() {
    use graftyard_api::webhooks::{app_hook_body, app_hook_config, app_secret_bytes};
    use graftyard_identity::{AppId, AppRecord, ClientId, PermissionSet, Slug, Url};
    let app = AppId(DbId(1_835_008));
    let w = Ops.wrap(b"runner-hook", [4; 12]).unwrap();
    let rec = AppRecord {
        id: app,
        slug: Slug::parse("graftyard-hosted-runners").unwrap(),
        name: "Graftyard hosted runners".into(),
        client: ClientId::for_app(app),
        keys: smallvec::SmallVec::new(),
        permissions: PermissionSet::default(),
        events: smallvec![EventKind("workflow_job".into())],
        webhook: Some(Url("https://graft-runners.example.workers.dev".into())),
        webhook_secret: Some(app_secret_bytes(&w)),
        client_secrets: smallvec::SmallVec::new(),
        callbacks: smallvec::SmallVec::new(),
        public: false,
    };
    let got: Wrapped = graftyard_raft::codec::from_bytes(rec.webhook_secret.as_deref().unwrap()).unwrap();
    assert_eq!(&*Ops.unwrap(&got).unwrap().0, b"runner-hook", "the record's bytes decode to the wrapped secret");
    let v = app_hook_config(&rec);
    assert_eq!(
        v,
        json!({ "content_type": "json", "insecure_ssl": "0", "secret": "********",
                "url": "https://graft-runners.example.workers.dev" })
    );
    assert!(!v.to_string().contains("runner-hook"));
    let none = AppRecord { webhook: None, webhook_secret: None, ..rec.clone() };
    assert_eq!(app_hook_config(&none), json!({ "content_type": "json", "insecure_ssl": "0" }));
    // A body that names only the secret keeps the URL; `""` clears the secret; a new URL replaces the old one.
    let (url, secret) = app_hook_body(&rec, &json!({ "secret": "s3" })).unwrap();
    assert_eq!((url, secret), (rec.webhook.clone(), Some(Some("s3".to_owned()))));
    assert_eq!(app_hook_body(&rec, &json!({ "secret": "" })).unwrap().1, Some(None));
    assert_eq!(app_hook_body(&rec, &json!({})).unwrap(), (rec.webhook.clone(), None));
    let (url, _) = app_hook_body(
        &none,
        &json!({ "url": "https://x.example/h", "content_type": "json",
        "insecure_ssl": "0" }),
    )
    .unwrap();
    assert_eq!(url, Some(Url("https://x.example/h".into())));
    for bad in [
        json!({ "content_type": "form" }),
        json!({ "insecure_ssl": "1" }),
        json!({ "insecure_ssl": 1 }),
        json!({ "url": "ftp://x" }),
        json!({ "url": "https:///x" }),
        json!({ "secret": 5 }),
    ] {
        assert_eq!(app_hook_body(&rec, &bad).unwrap_err().status, 422, "{bad}");
    }
    assert_eq!(app_hook_body(&rec, &Value::Null).unwrap_err().status, 400);
    if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
        for (case, method) in [("app-hook-config-get", "GET"), ("app-hook-config-patch", "PATCH")] {
            let k = json!({ "method": method, "path": "/app/hook/config", "status": 200, "body": v });
            std::fs::write(format!("{dir}/{case}.rest.json"), serde_json::to_vec_pretty(&k).unwrap()).unwrap();
        }
    }
}
