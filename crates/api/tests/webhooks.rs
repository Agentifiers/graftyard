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
        refs: vec![RefUpdate { name: qref, expect: Some(oid(6)), new: None }],
    };
    for q in [enq, built, ejected] {
        let t = graftyard_git::tiers::tier_of(&RepoEvent::Queue(q.clone()));
        c.entry(EntryBody::Events(vec![RepoEvent::Queue(q)]), t);
    }
    c.commit(OWNER, IssueOp::Close { n: IssueNumber(2) }).unwrap();
    c.commit(OWNER, IssueOp::Change { n: IssueNumber(2), change: IssueChange::State { open: true, reason: None } })
        .unwrap();
    (c, from)
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
        ]
    );
    assert_eq!(counts["push"], 6, "main twice, f twice, tmp made and deleted, and never a queue ref");
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
    let mut seq = Sequencer::new(AccountId(DbId(OWNER)));
    seq.record(
        chain,
        Index(4),
        vec![NewDelivery {
            guid,
            hook: id,
            entry: Some((chain, Index(4), 0)),
            event: "ping",
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
