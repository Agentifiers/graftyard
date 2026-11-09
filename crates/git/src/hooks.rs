//! Repository webhooks on the repository chain, and what the `Webhooks` consumer reads from it (`design/SHARED.md`,
//! "Event types" and "Event fan-out"; `design/DESIGN.md`, "Dominant access patterns", Webhook; `build/BUILD-PLAN.md`,
//! M4-WP1).
//!
//! - **Records.** `HookRecord` is SHARED's, core tier, written by `RepoEvent::Hook` (tag 39). `IssueOp::PutHook` and
//!   `DeleteHook` are the REST writes, admin only. The secret is `Wrapped` before it reaches the chain: the edge asks
//!   `graft-ops` to wrap it (`graftyard_identity::sealed::HookSecrets`).
//! - **The cursor.** `HookCursor { read, replay_from }` is the `Webhooks` consumer's position on this chain, core tier,
//!   written only by `RepoEvent::HookCursor` (tag 40) through `HookCursorCmd`. `read` passes an entry only once its
//!   `HookDelivery` rows are recorded in the owner's `HookSequencer` (`crate::sequencer`); `replay_from` is the oldest
//!   entry with a delivery not yet finished, so a sequencer lost with its site is rebuilt by replaying from it.
//! - **What an entry carries.** `webhooks` lists the webhooks one committed entry carries, in order. Each one's place
//!   in that list is its `slot`, so `(chain, index, slot)` names it, and with the hook and the install's restore
//!   generation it names its `X-GitHub-Delivery` GUID (`delivery_guid`). A rebuilt sequencer derives the same GUIDs.
//! - **Format 32.16** (`REPO_FORMAT_HOOKS`): the hook commands and the cursor command, and the two events. A member
//!   reads it only after a site day onto a release that lists it, and the edge and the consumer send none before
//!   `FormatsEnable` enabled it. No earlier command takes it, so every earlier entry keeps its bytes.

use graftyard_fold::{Read, Rows};
use graftyard_identity::EventKind;
use graftyard_identity::sealed::Wrapped;
use graftyard_raft::{EntryBody, Format, LogEntry, Record, TierSet};
use graftyard_types::{ChainId, DbId, Index, RepoId, RequestId, Timestamp};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

use crate::issues::{CheckRunUpdate, CheckStatus, CommentEvent, CommentTarget, IssueChange, IssueNumber, StatusUpdate};
use crate::queue::QueueEvent;
use crate::refs::{ObjectId, RefName};
use crate::repo::{RepoEvent, RepoScratch};
use crate::tiers::{get_rec, int, pull_head, pulls_from, put_rec};

/// The hook commands, the cursor command, and their events `Hook` (39) and `HookCursor` (40).
pub const REPO_FORMAT_HOOKS: Format = Format { family: 0x20, version: 16 };

/// Webhook attempts in flight at once per hook owner, compiled [guess] ("Failure modes", `event pipeline`,
/// bad-config: never config).
pub const HOOK_OWNER_CONCURRENCY: u16 = 50;
/// An attempt's deadline (`research/wire-census.md` #36: "The receiver must answer 2XX within 10 seconds").
pub const ATTEMPT_DEADLINE_MS: u64 = 10_000;
/// Manual redelivery and the delivery history last this long after the last attempt (`research/wire-census.md` #37).
pub const DELIVERY_KEEP_MS: u64 = 3 * 86_400_000;
/// A stretch of entries that carry no webhook is still covered by a cursor entry this often, as `PrSync`'s.
pub const CURSOR_STRIDE: u64 = 1_000;
/// A repository's hooks, as GitHub caps them [unverified for GHES 3.22: github.com documents 20 per event].
pub const HOOKS_PER_REPO_MAX: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HookId(pub DbId);

/// SHARED's `HookContentType`. Wire tags: add variants at the end only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookContentType {
    Json,
    Form,
}

/// SHARED's `HookRecord`. `url` is the edge-validated `http` or `https` URL; `events` the GHES 3.22 event names, or
/// `*`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRecord {
    pub url: graftyard_identity::Url,
    pub events: SmallVec<[EventKind; 8]>,
    pub content_type: HookContentType,
    pub active: bool,
    pub insecure_ssl: bool,
    pub secret: Option<Wrapped>,
    pub retry: bool,
}

impl HookRecord {
    /// Whether the hook takes `event` (a ping always reaches its own hook).
    pub fn takes(&self, event: &str) -> bool {
        self.active && self.events.iter().any(|e| &*e.0 == event || &*e.0 == "*")
    }
}

/// SHARED's `HookCursor`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCursor {
    pub read: Index,
    pub replay_from: Index,
}

/// Every event GHES 3.22 can deliver (`ghes-3.22.json`'s `x-webhooks`, 78 events, one per operationId prefix, with
/// dashes as underscores), plus the three `merge_group` and queue names the build plan asks for at T0 that GHES 3.22's
/// description lacks (`merge_group`; `pull_request`'s `enqueued` and `dequeued` are actions of an existing event).
pub const EVENTS: &[&str] = &[
    "branch_protection_configuration",
    "branch_protection_rule",
    "cache_sync",
    "check_run",
    "check_suite",
    "code_scanning_alert",
    "commit_comment",
    "create",
    "custom_property",
    "custom_property_values",
    "delete",
    "dependabot_alert",
    "deploy_key",
    "deployment",
    "deployment_protection_rule",
    "deployment_review",
    "deployment_status",
    "discussion",
    "discussion_comment",
    "dismissal_request_code_scanning",
    "dismissal_request_dependabot",
    "dismissal_request_secret_scanning",
    "enterprise",
    "exemption_request_push_ruleset",
    "exemption_request_secret_scanning",
    "fork",
    "github_app_authorization",
    "gollum",
    "installation",
    "installation_repositories",
    "installation_target",
    "issue_comment",
    "issue_dependencies",
    "issues",
    "label",
    "member",
    "membership",
    "meta",
    "milestone",
    "organization",
    "organization_custom_property",
    "organization_custom_property_values",
    "package",
    "page_build",
    "personal_access_token_request",
    "ping",
    "project",
    "project_card",
    "project_column",
    "projects_v2",
    "projects_v2_item",
    "public",
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
    "pull_request_review_thread",
    "push",
    "registry_package",
    "release",
    "repository",
    "repository_dispatch",
    "repository_ruleset",
    "repository_vulnerability_alert",
    "secret_scanning_alert",
    "secret_scanning_alert_location",
    "secret_scanning_scan",
    "security_advisory",
    "security_and_analysis",
    "sponsorship",
    "star",
    "status",
    "team",
    "team_add",
    "user",
    "watch",
    "workflow_dispatch",
    "workflow_job",
    "workflow_run",
    "merge_group",
];

/// SHARED's `SUBSCRIPTIONS`, for what this release deploys: the webhook events the `Webhooks` consumer renders from
/// repository entries, and the entries `PrSync` and `MergeQueue` read. `Audit` and `Search` are not deployed yet.
pub static SUBSCRIPTIONS: &[(&str, &[crate::issues::Consumer])] = {
    use crate::issues::Consumer::{MergeQueue, PrSync, Webhooks};
    &[
        ("push", &[Webhooks, PrSync, MergeQueue]),
        ("create", &[Webhooks]),
        ("delete", &[Webhooks]),
        ("pull_request", &[Webhooks, PrSync, MergeQueue]),
        ("issues", &[Webhooks]),
        ("issue_comment", &[Webhooks]),
        ("status", &[Webhooks, MergeQueue]),
        ("check_run", &[Webhooks, MergeQueue]),
        ("merge_group", &[Webhooks]),
        ("ping", &[Webhooks]),
    ]
};

// ---- the fold ----

/// The core `hook` row: the record, and the times of the entries that created and last changed it, which the REST
/// `hook` object renders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRow {
    pub record: HookRecord,
    pub created: Timestamp,
    pub updated: Timestamp,
}

pub fn hook_row(core: &(impl Read + ?Sized), id: HookId) -> Option<HookRow> {
    get_rec(core, "hook", &[int(id.0.0)])
}

/// The core `hook` row's record by id.
pub fn hook(core: &(impl Read + ?Sized), id: HookId) -> Option<HookRecord> {
    hook_row(core, id).map(|r| r.record)
}

/// Every hook of the repository, by id.
pub fn hooks(core: &(impl Read + ?Sized)) -> Vec<(HookId, HookRecord)> {
    core.prefix("hook", &[])
        .into_iter()
        .filter_map(|(k, _)| match k.first() {
            Some(graftyard_fold::Val::Int(i)) => Some((HookId(DbId(*i)), hook(core, HookId(DbId(*i)))?)),
            _ => None,
        })
        .collect()
}

/// The `Webhooks` consumer's cursor on this chain; zero before its first cursor entry.
pub fn hook_cursor(core: &(impl Read + ?Sized)) -> HookCursor {
    get_rec(core, "hook_cursor", &[]).unwrap_or_default()
}

pub(crate) fn write_core(w: &mut dyn Rows, ev: &RepoEvent, at: Timestamp) {
    match ev {
        RepoEvent::Hook { id, record: Some(r), .. } => {
            let created = hook_row(w, *id).map_or(at, |old| old.created);
            put_rec(w, "hook", vec![int(id.0.0)], &HookRow { record: r.clone(), created, updated: at })
        }
        RepoEvent::Hook { id, record: None, .. } => w.delete("hook", &[int(id.0.0)]),
        RepoEvent::HookCursor(c) => put_rec(w, "hook_cursor", Vec::new(), c),
        _ => {}
    }
}

// ---- the cursor command ----

/// The `Webhooks` consumer's cursor commit, `{"hookCursor": ..}`, format 32.16. It carries no account and no key, as
/// every consumer command. `decide` moves each half only forward, and keeps `replay_from` at most `read + 1`, so a
/// re-sent or late command changes nothing a newer one did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCursorCmd {
    pub cursor: HookCursor,
    pub request: RequestId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCursorOut {
    pub cursor: HookCursor,
    pub changed: bool,
}

impl Record for HookCursorCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        graftyard_fold::canon::write(out, self);
    }
}

impl HookCursorCmd {
    pub fn decide(&self, s: &RepoScratch<'_>) -> (SmallVec<[RepoEvent; 4]>, HookCursorOut) {
        let cur = hook_cursor(s.core());
        let read = cur.read.max(self.cursor.read);
        let replay_from = cur.replay_from.max(self.cursor.replay_from.min(Index(read.0 + 1)));
        let next = HookCursor { read, replay_from };
        if next == cur {
            return (SmallVec::new(), HookCursorOut { cursor: cur, changed: false });
        }
        (smallvec::smallvec![RepoEvent::HookCursor(next)], HookCursorOut { cursor: next, changed: true })
    }

    pub fn replay(&self, events: &[RepoEvent]) -> HookCursorOut {
        match events.iter().find_map(|e| if let RepoEvent::HookCursor(c) = e { Some(*c) } else { None }) {
            Some(c) => HookCursorOut { cursor: c, changed: true },
            None => HookCursorOut { cursor: self.cursor, changed: false },
        }
    }

    pub fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
}

// ---- what an entry carries ----

/// One webhook a committed entry carries: its event and action, and what its payload is about. `target` is set for a
/// ping, which goes only to the hook it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Webhook {
    pub event: &'static str,
    pub action: Option<&'static str>,
    pub subject: Subject,
    pub target: Option<HookId>,
}

/// What a webhook's payload is about, from the entry's own fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A `push`: one ref's move. `created` and `deleted` as GitHub's payload has them.
    Push {
        r#ref: RefName,
        before: ObjectId,
        after: ObjectId,
        pusher: Option<graftyard_types::AccountId>,
    },
    /// A `create` or `delete` of a branch or tag.
    Ref {
        r#ref: RefName,
        pusher: Option<graftyard_types::AccountId>,
    },
    Pull {
        n: IssueNumber,
        before: Option<ObjectId>,
        after: Option<ObjectId>,
        actor: Option<graftyard_types::AccountId>,
    },
    Issue {
        n: IssueNumber,
        actor: Option<graftyard_types::AccountId>,
    },
    Comment {
        n: IssueNumber,
        id: crate::issues::CommentId,
        actor: Option<graftyard_types::AccountId>,
    },
    Status(StatusUpdate),
    CheckRun(CheckRunUpdate),
    /// A queue entry's group (`merge_group`): the base, the queue ref, and its tips.
    MergeGroup {
        base: RefName,
        head_ref: RefName,
        head: ObjectId,
        base_sha: ObjectId,
        reason: Option<&'static str>,
    },
    /// The pull request a queue event names, with `reason` for `dequeued`.
    Queue {
        n: IssueNumber,
        reason: Option<&'static str>,
        actor: Option<graftyard_types::AccountId>,
    },
    Ping {
        hook: HookId,
    },
}

fn tag_or_branch(r: &RefName) -> bool {
    r.as_bytes().starts_with(b"refs/heads/") || r.as_bytes().starts_with(b"refs/tags/")
}

/// The webhooks entry `e` carries, in order; each one's position is its slot. `core` is the fold the consumer reads
/// it against (for which pull requests a branch heads), and `repo` the chain's repository. An entry that is not
/// `Events` carries none, and neither do the hook cursor, `PrSync`'s refs under `refs/pull/`, or queue refs.
pub fn webhooks(core: &dyn Read, repo: RepoId, e: &LogEntry<RepoEvent>) -> Vec<Webhook> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    let w = |event, action, subject| Webhook { event, action, subject, target: None };
    for ev in evs {
        match ev {
            RepoEvent::Refs { actor, updates, .. } => {
                for u in updates.iter().filter(|u| tag_or_branch(&u.name) && !is_queue_ref(&u.name)) {
                    let before = u.expect.unwrap_or(ObjectId::ZERO);
                    let after = u.new.unwrap_or(ObjectId::ZERO);
                    out.push(w("push", None, Subject::Push { r#ref: u.name.clone(), before, after, pusher: *actor }));
                    let r = Subject::Ref { r#ref: u.name.clone(), pusher: *actor };
                    match (u.expect, u.new) {
                        (None, Some(_)) => out.push(w("create", None, r)),
                        (Some(_), None) => out.push(w("delete", None, r)),
                        _ => {}
                    }
                    if let (Some(b), Some(a)) = (u.expect, u.new) {
                        for n in pulls_from(core, repo, &u.name) {
                            let s = Subject::Pull { n, before: Some(b), after: Some(a), actor: *actor };
                            out.push(w("pull_request", Some("synchronize"), s));
                        }
                    }
                }
            }
            RepoEvent::PrOpened { n, capture, .. } => {
                let s = Subject::Pull { n: *n, before: None, after: None, actor: capture.issue.author };
                out.push(w("pull_request", Some("opened"), s));
            }
            RepoEvent::PrClosed { n, .. } => out.push(w(
                "pull_request",
                Some("closed"),
                Subject::Pull { n: *n, before: None, after: None, actor: None },
            )),
            RepoEvent::Merged { n, by, .. } => {
                let s = Subject::Pull { n: *n, before: None, after: None, actor: Some(by.0) };
                out.push(w("pull_request", Some("closed"), s));
            }
            RepoEvent::IssueOpened { n, capture } => {
                out.push(w("issues", Some("opened"), Subject::Issue { n: *n, actor: capture.author }))
            }
            RepoEvent::IssueChanged { n, change, actor } => {
                let pr = pull_head(core, *n).is_some();
                let action = match change {
                    IssueChange::State { open: true, .. } => Some("reopened"),
                    IssueChange::State { open: false, .. } if !pr => Some("closed"),
                    IssueChange::Title(_) | IssueChange::Body(_) => Some("edited"),
                    IssueChange::Draft(false) if pr => Some("ready_for_review"),
                    IssueChange::Draft(true) if pr => Some("converted_to_draft"),
                    _ => None,
                };
                if let Some(a) = action {
                    out.push(if pr {
                        w("pull_request", Some(a), Subject::Pull { n: *n, before: None, after: None, actor: *actor })
                    } else {
                        w("issues", Some(a), Subject::Issue { n: *n, actor: *actor })
                    });
                }
            }
            RepoEvent::Comment(CommentEvent::Created { id, on: CommentTarget::Issue(n), author, .. }) => {
                out.push(w("issue_comment", Some("created"), Subject::Comment { n: *n, id: *id, actor: *author }))
            }
            RepoEvent::Status(s) => out.push(w("status", None, Subject::Status(s.clone()))),
            RepoEvent::CheckRun(c) => {
                let action = if c.version <= 1 {
                    Some("created")
                } else if c.status == CheckStatus::Completed {
                    Some("completed")
                } else {
                    None
                };
                if let Some(a) = action {
                    out.push(w("check_run", Some(a), Subject::CheckRun(c.clone())));
                }
            }
            RepoEvent::Queue(q) => queue_webhooks(q, &mut out),
            // A new hook's ping, to that hook only: SHARED's "queues a `ping` delivery when it reads a new `Hook` entry".
            RepoEvent::Hook { id, record: Some(r), created: true, .. } if r.active => out.push(Webhook {
                event: "ping",
                action: None,
                subject: Subject::Ping { hook: *id },
                target: Some(*id),
            }),
            _ => {}
        }
    }
    out
}

fn is_queue_ref(r: &RefName) -> bool {
    r.as_bytes().starts_with(b"refs/heads/gh-readonly-queue/")
}

fn queue_webhooks(q: &QueueEvent, out: &mut Vec<Webhook>) {
    let w = |event, action, subject| Webhook { event, action, subject, target: None };
    match q {
        QueueEvent::Enqueued { n, actor, .. } => {
            out.push(w("pull_request", Some("enqueued"), Subject::Queue { n: *n, reason: None, actor: Some(actor.0) }))
        }
        QueueEvent::GroupBuilt { base, entries, .. } => {
            for e in entries {
                if let Some(head_ref) = crate::queue::queue_ref(base, e) {
                    let s = Subject::MergeGroup {
                        base: base.clone(),
                        head_ref,
                        head: e.group_tip,
                        base_sha: e.parent_tip,
                        reason: None,
                    };
                    out.push(w("merge_group", Some("checks_requested"), s));
                }
            }
        }
        QueueEvent::Ejected { n, reason, refs } => {
            let why = match reason {
                crate::queue::EjectReason::Dequeued => "dequeued",
                _ => "invalidated",
            };
            for u in refs.iter().filter(|u| u.new.is_none()) {
                if let Some(head) = u.expect {
                    out.push(destroyed(u.name.clone(), head, why));
                }
            }
            // `pull_request.dequeued`'s reason is dotcom's upper-case enum, not `merge_group.destroyed`'s.
            let removal = match reason {
                crate::queue::EjectReason::Dequeued | crate::queue::EjectReason::Closed => "MANUAL",
                crate::queue::EjectReason::CheckFailed { .. } => "CI_FAILURE",
                crate::queue::EjectReason::CheckTimeout { .. } => "CI_TIMEOUT",
                crate::queue::EjectReason::Conflict => "MERGE_CONFLICT",
                crate::queue::EjectReason::Protection(_) => "BRANCH_PROTECTIONS",
                crate::queue::EjectReason::HeadMoved => "UNKNOWN_REMOVAL_REASON",
            };
            out.push(w("pull_request", Some("dequeued"), Subject::Queue { n: *n, reason: Some(removal), actor: None }));
        }
        QueueEvent::Landed { refs, .. } => {
            for u in refs.iter().filter(|u| u.new.is_none()) {
                if let Some(head) = u.expect {
                    out.push(destroyed(u.name.clone(), head, "merged"));
                }
            }
        }
    }
}

fn destroyed(head_ref: RefName, head: ObjectId, reason: &'static str) -> Webhook {
    let base = queue_base(&head_ref);
    Webhook {
        event: "merge_group",
        action: Some("destroyed"),
        subject: Subject::MergeGroup { base, head_ref, head, base_sha: ObjectId::ZERO, reason: Some(reason) },
        target: None,
    }
}

/// `refs/heads/gh-readonly-queue/{base}/pr-...`'s base, as `refs/heads/{base}`.
fn queue_base(r: &RefName) -> RefName {
    let s = String::from_utf8_lossy(r.as_bytes());
    let rest = s.strip_prefix("refs/heads/gh-readonly-queue/").unwrap_or(&s);
    let base = rest.rsplit_once("/pr-").map_or(rest, |(b, _)| b);
    RefName::parse(format!("refs/heads/{base}").as_bytes()).unwrap_or_else(|_| r.clone())
}

// ---- delivery identity ----

/// An `X-GitHub-Delivery` GUID: 16 bytes, printed 8-4-4-4-12.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Guid(pub [u8; 16]);

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Guid {
    pub fn parse(s: &str) -> Option<Guid> {
        let hex: String = s.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 || s.len() != 36 {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        (out.map(|_| ()).len() == 16 && Guid(out).to_string() == s).then_some(Guid(out))
    }

    /// A UUID version 8 (RFC 9562, custom) with the variant bits set, from 16 bytes.
    pub fn from_bytes(mut b: [u8; 16]) -> Guid {
        b[6] = (b[6] & 0x0f) | 0x80;
        b[8] = (b[8] & 0x3f) | 0x80;
        Guid(b)
    }
}

/// SHARED's delivery identity: from (restore generation, chain, index, slot, hook), so a redelivery after a crash or
/// a rebuild keeps its GUID and a restore, which changes the generation, never reuses one.
pub fn delivery_guid(generation: u64, chain: ChainId, index: Index, slot: u16, hook: HookId) -> Guid {
    let h = Sha256::new()
        .chain_update(b"graftyard/hook-delivery/1\0")
        .chain_update(generation.to_be_bytes())
        .chain_update(chain.to_string().as_bytes())
        .chain_update([0])
        .chain_update(index.0.to_be_bytes())
        .chain_update(slot.to_be_bytes())
        .chain_update(hook.0.0.to_be_bytes())
        .finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    Guid::from_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_are_stable_distinct_and_print_as_uuids() {
        let c = ChainId::Repo(RepoId(DbId(7)));
        let g = delivery_guid(1, c, Index(5), 0, HookId(DbId(3)));
        assert_eq!(g, delivery_guid(1, c, Index(5), 0, HookId(DbId(3))));
        for other in [
            delivery_guid(2, c, Index(5), 0, HookId(DbId(3))),
            delivery_guid(1, ChainId::Repo(RepoId(DbId(8))), Index(5), 0, HookId(DbId(3))),
            delivery_guid(1, c, Index(6), 0, HookId(DbId(3))),
            delivery_guid(1, c, Index(5), 1, HookId(DbId(3))),
            delivery_guid(1, c, Index(5), 0, HookId(DbId(4))),
        ] {
            assert_ne!(g, other);
        }
        let s = g.to_string();
        assert_eq!(s.len(), 36);
        assert_eq!(&s[14..15], "8");
        assert!(matches!(&s[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(Guid::parse(&s), Some(g));
        assert_eq!(Guid::parse(&s.to_uppercase()), None);
    }

    #[test]
    fn the_event_list_is_ghes_3_22s_78_and_merge_group() {
        assert_eq!(EVENTS.len(), 79);
        let mut v = EVENTS.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), 79);
        for (e, _) in SUBSCRIPTIONS {
            assert!(EVENTS.contains(e), "{e}");
        }
    }

    #[test]
    fn queue_base_reads_the_queue_refs_base() {
        let r = RefName::parse(b"refs/heads/gh-readonly-queue/release/1.0/pr-3-abc").unwrap();
        assert_eq!(queue_base(&r).as_bytes(), b"refs/heads/release/1.0");
    }
}
