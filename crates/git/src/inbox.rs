//! The `Notify` consumer's pure half and the `Inbox` (`design/DESIGN.md`, Shape, "Identity", `Inbox` and
//! `INBOX_WINDOW`; `design/SHARED.md`, `rest:activity` and "Known deviations from GitHub"; `build/BUILD-PLAN.md`,
//! M4-WP2).
//!
//! - **Notify.** `notices` turns one committed repository entry into the notifications it owes, by account, reading
//!   the repository's folds on a follower: the watcher index `Counts` wrote (`crate::watch`), and the thread's author,
//!   assignees, commenters, and mentions. The actor is never notified of its own action, an ignored repository
//!   notifies nobody, and an explicit thread unsubscription stops everything but a mention [inference: GitHub's
//!   documented watching rules].
//! - **Inbox.** One derived object per account (`Inbox { account, site }` in the site scripts). It keeps one row per
//!   thread, the newest notice wins, and `sources`, the last index it applied of each chain, so a redelivered or
//!   replayed notice changes nothing. Read and done state is never the Inbox's: it is the account's
//!   `UserRecord::Thread` on its `Identity` chain, and `view` merges it, so a rebuilt Inbox keeps it.
//! - **Snapshot and rebuild.** After each day it changed, the Inbox writes `Snapshot` (its rows and `sources`) to its
//!   site's bucket and one other site's, from its alarm at the next day's start. `rebuild_from` names what a rebuilt Inbox replays: each chain in `sources`
//!   from its index, and each repository in the account's `Watch` and `Subscription` records from `INBOX_WINDOW`
//!   before the snapshot. An event from any other chain after the snapshot is missing until that chain's next event.

use std::collections::BTreeMap;
use std::time::Duration;

use graftyard_fold::Read;
use graftyard_identity::{ThreadId, UserRows};
use graftyard_raft::{EntryBody, LogEntry};
use graftyard_types::{AccountId, ChainId, Index, RepoId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::issues::{CommentEvent, CommentId, CommentTarget, IssueChange, IssueKind, IssueNumber, ReviewEvent};
use crate::repo::RepoEvent;
use crate::tiers::{IssueRow, get_rec, int, n_key};
use crate::watch;

/// How far before its snapshot a rebuilt Inbox replays each watched or subscribed repository [guess].
pub const INBOX_WINDOW: Duration = Duration::from_secs(2 * 86_400);
pub const DAY_MS: u64 = 86_400_000;

/// GHES's notification `reason`s this build produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reason {
    Subscribed,
    Manual,
    Author,
    Comment,
    Assign,
    Mention,
    StateChange,
    ReviewRequested,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Subscribed => "subscribed",
            Reason::Manual => "manual",
            Reason::Author => "author",
            Reason::Comment => "comment",
            Reason::Assign => "assign",
            Reason::Mention => "mention",
            Reason::StateChange => "state_change",
            Reason::ReviewRequested => "review_requested",
        }
    }

    /// Whether the reason makes the account a participant, which `?participating=true` lists.
    pub fn participating(self) -> bool {
        !matches!(self, Reason::Subscribed)
    }
}

/// One notification an entry owes one account. `(chain, index, slot)` names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub thread: ThreadId,
    pub reason: Reason,
    pub at: Timestamp,
    pub index: Index,
    pub title: Box<str>,
    pub kind: IssueKind,
    pub latest_comment: Option<CommentId>,
}

/// What one entry did to a thread, before the readers are chosen.
struct Touch {
    n: IssueNumber,
    actor: Option<AccountId>,
    comment: Option<CommentId>,
    body: Option<Box<str>>,
    state_change: bool,
    review_requested: Vec<AccountId>,
}

fn touches(e: &LogEntry<RepoEvent>) -> Vec<Touch> {
    let EntryBody::Events(evs) = &e.body else { return Vec::new() };
    let mut out = Vec::new();
    let t = |n, actor| Touch { n, actor, comment: None, body: None, state_change: false, review_requested: Vec::new() };
    for ev in evs {
        match ev {
            RepoEvent::IssueOpened { n, capture }
            | RepoEvent::PrOpened { n, capture: crate::issues::PrCapture { issue: capture, .. }, .. } => {
                out.push(Touch { body: Some(capture.body.clone()), ..t(*n, capture.author) })
            }
            RepoEvent::Comment(CommentEvent::Created { id, on, author, body, .. }) => {
                let (CommentTarget::Issue(n) | CommentTarget::Review { n, .. }) = on else { continue };
                out.push(Touch { comment: Some(*id), body: Some(body.clone()), ..t(*n, *author) })
            }
            RepoEvent::IssueChanged { n, change: IssueChange::State { .. }, actor } => {
                out.push(Touch { state_change: true, ..t(*n, *actor) })
            }
            RepoEvent::Merged { n, by, .. } => out.push(Touch { state_change: true, ..t(*n, Some(by.0)) }),
            RepoEvent::Review(ReviewEvent::Submitted { n, reviewer, body, .. }) => {
                out.push(Touch { body: Some(body.clone()), ..t(*n, *reviewer) })
            }
            RepoEvent::Review(ReviewEvent::Requested { n, reviewers, removed: false, actor }) => {
                let users = reviewers
                    .iter()
                    .filter_map(|g| match g {
                        crate::issues::Grantee::User(u) => Some(u.0),
                        crate::issues::Grantee::Team(_) => None,
                    })
                    .collect();
                out.push(Touch { review_requested: users, ..t(*n, *actor) })
            }
            _ => {}
        }
    }
    out
}

/// `@login` mentions in a body, as GitHub finds them: an `@` not preceded by a word character, then a login.
pub fn mentions(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    for (i, c) in b.iter().enumerate() {
        if *c != b'@' || (i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'`')) {
            continue;
        }
        let end =
            b[i + 1..].iter().position(|c| !(c.is_ascii_alphanumeric() || *c == b'-')).map_or(b.len(), |p| i + 1 + p);
        if end > i + 1 {
            let l = &body[i + 1..end];
            if !out.contains(&l) {
                out.push(l);
            }
        }
    }
    out
}

/// Accounts that commented on thread `n`, from the metadata tier.
fn commenters(meta: &(impl Read + ?Sized), n: IssueNumber) -> Vec<AccountId> {
    let mut out = Vec::new();
    for (k, _) in meta.prefix("comment_by_issue", &[n_key(n)]) {
        let Some(graftyard_fold::Val::Int(id)) = k.get(1) else { continue };
        if let Some(row) = get_rec::<crate::tiers::CommentRow>(meta, "comment", &[int(*id)])
            && let Some(a) = row.author
            && !out.contains(&a)
        {
            out.push(a);
        }
    }
    out
}

fn assignees(meta: &(impl Read + ?Sized), n: IssueNumber) -> Vec<AccountId> {
    meta.prefix("issue_assignee", &[n_key(n)])
        .into_iter()
        .filter_map(|(k, _)| match k.get(1) {
            Some(graftyard_fold::Val::Int(a)) => Some(AccountId(graftyard_types::DbId(*a))),
            _ => None,
        })
        .collect()
}

/// Who may hear of thread `n`, read once per batch from the tier as it stands: on a busy issue the commenters are a
/// scan of every comment, which a batch read once for each entry that touched the issue (`decisions/DECISIONS.md`,
/// "`round-memo`").
struct Thread {
    row: IssueRow,
    assignees: Vec<AccountId>,
    commenters: Vec<AccountId>,
    subs: Vec<(AccountId, bool)>,
}

/// The notifications a batch of committed entries owes, each account's by entry, read from the repository's metadata
/// tier on a member that has applied them, as it stands now. `login` resolves a mention to an account (the `Names`
/// shards).
pub fn notices(
    repo: RepoId,
    meta: &(impl Read + ?Sized),
    es: &[LogEntry<RepoEvent>],
    login: &dyn Fn(&str) -> Option<AccountId>,
) -> BTreeMap<AccountId, Vec<(Index, Vec<Notice>)>> {
    let mut threads: BTreeMap<IssueNumber, Option<Thread>> = BTreeMap::new();
    let mut subscribers: Option<Vec<AccountId>> = None;
    let mut out: BTreeMap<AccountId, Vec<(Index, Vec<Notice>)>> = BTreeMap::new();
    for e in es {
        let mut by: BTreeMap<AccountId, Vec<Notice>> = BTreeMap::new();
        for t in touches(e) {
            let read = || {
                Some(Thread {
                    row: get_rec::<IssueRow>(meta, "issue", &[n_key(t.n)])?,
                    assignees: assignees(meta, t.n),
                    commenters: commenters(meta, t.n),
                    subs: watch::thread_subs(meta, t.n),
                })
            };
            let Some(th) = threads.entry(t.n).or_insert_with(read) else { continue };
            let subscribers = subscribers.get_or_insert_with(|| watch::subscribers(meta));
            let thread = ThreadId { repo, number: t.n.0 };
            // Strongest reason first: a reader keeps the first reason it is given.
            let mut readers: Vec<(AccountId, Reason)> = Vec::new();
            let mut add = |a: AccountId, r: Reason| {
                if !readers.iter().any(|(x, _)| *x == a) {
                    readers.push((a, r));
                }
            };
            let mentioned: Vec<AccountId> =
                t.body.as_deref().map(mentions).unwrap_or_default().into_iter().filter_map(login).collect();
            for a in &mentioned {
                add(*a, Reason::Mention);
            }
            for a in &t.review_requested {
                add(*a, Reason::ReviewRequested);
            }
            for a in &th.assignees {
                add(*a, Reason::Assign);
            }
            if let Some(a) = th.row.author {
                add(a, if t.state_change { Reason::StateChange } else { Reason::Author });
            }
            for a in &th.commenters {
                add(*a, if t.state_change { Reason::StateChange } else { Reason::Comment });
            }
            for (a, subscribed) in &th.subs {
                if *subscribed {
                    add(*a, Reason::Manual);
                }
            }
            for a in subscribers.iter() {
                add(*a, Reason::Subscribed);
            }
            for (a, reason) in readers {
                if Some(a) == t.actor {
                    continue;
                }
                let w = watch::watcher(meta, a);
                if w.is_some_and(|w| w.ignored) {
                    continue;
                }
                let unsubscribed = th.subs.iter().any(|(x, s)| *x == a && !s);
                if unsubscribed && reason != Reason::Mention {
                    continue;
                }
                let notice = Notice {
                    thread,
                    reason,
                    at: e.at,
                    index: e.index,
                    title: th.row.title.clone(),
                    kind: th.row.kind,
                    latest_comment: t.comment,
                };
                by.entry(a).or_default().push(notice);
            }
        }
        for (a, ns) in by {
            out.entry(a).or_default().push((e.index, ns));
        }
    }
    out
}

// ---- the Inbox ----

/// One thread's row: the newest notice's fields, and whether any notice made the account a participant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxRow {
    pub reason: Reason,
    pub updated: Timestamp,
    /// The chain index of the newest notice, which orders two notices of one time.
    pub index: Index,
    pub title: Box<str>,
    pub kind: IssueKind,
    pub latest_comment: Option<CommentId>,
    pub participating: bool,
}

/// What an `Inbox` object stores. Rows by thread and `sources`, the last index applied of each chain.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub threads: BTreeMap<ThreadId, InboxRow>,
    pub sources: BTreeMap<ChainId, Index>,
    /// Whether anything applied since the last snapshot.
    pub dirty: bool,
    /// The day (`at / DAY_MS`) the last snapshot was written on.
    pub snapshot_day: Option<u64>,
}

/// One thread as `GET /notifications` lists it: the row with the account's read and done state merged in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub thread: ThreadId,
    pub row: InboxRow,
    pub unread: bool,
    /// `last_read_at`: when the account last marked it read, if it did.
    pub last_read: Option<Timestamp>,
}

impl Inbox {
    /// Applies one chain entry's notices for this account. An entry at or below the chain's `sources` index changes
    /// nothing, and a thread keeps its newest notice, so replays and redeliveries are idempotent in any order.
    pub fn apply(&mut self, chain: ChainId, index: Index, notices: &[Notice]) -> bool {
        if self.sources.get(&chain).is_some_and(|i| *i >= index) {
            return false;
        }
        self.sources.insert(chain, index);
        self.dirty = true;
        for n in notices {
            let participating = n.reason.participating();
            match self.threads.get_mut(&n.thread) {
                Some(r) if (r.updated, r.index) >= (n.at, n.index) => r.participating |= participating,
                Some(r) => {
                    let p = r.participating || participating;
                    *r = row(n, p);
                }
                None => {
                    self.threads.insert(n.thread, row(n, participating));
                }
            }
        }
        true
    }

    /// Whether a snapshot is owed at `now`: the Inbox changed since its last snapshot, which was written on an
    /// earlier day. The object's alarm checks it at each day's start, so a day's snapshot holds that whole day.
    pub fn snapshot_due(&self, now: Timestamp) -> bool {
        self.dirty && self.snapshot_day.is_none_or(|d| d < now.0 / DAY_MS)
    }

    /// The snapshot to write, and marks it written.
    pub fn snapshot(&mut self, at: Timestamp) -> Snapshot {
        self.snapshot_day = Some(at.0 / DAY_MS);
        self.dirty = false;
        Snapshot { at, threads: self.threads.clone(), sources: self.sources.clone() }
    }

    /// GHES's list: newest first, with read and done from `user` (the account's `Identity` fold). A done thread is
    /// hidden until a notice newer than the done mark. `all` lists read threads too; `participating` only threads
    /// where the account participates.
    pub fn view(&self, user: &UserRows, all: bool, participating: bool, repo: Option<RepoId>) -> Vec<Listed> {
        let mut v: Vec<Listed> = self
            .threads
            .iter()
            .filter(|(t, r)| repo.is_none_or(|x| t.repo == x) && (!participating || r.participating))
            .filter_map(|(t, r)| {
                let mark = user.threads.get(t);
                let fresh = |m: &graftyard_identity::ThreadRow| r.updated > m.set.at;
                if mark.is_some_and(|m| m.done && !fresh(m)) {
                    return None;
                }
                let unread = mark.is_none_or(|m| !m.read || fresh(m));
                let last_read = mark.filter(|m| m.read).map(|m| m.set.at);
                (all || unread).then(|| Listed { thread: *t, row: r.clone(), unread, last_read })
            })
            .collect();
        v.sort_by_key(|l| std::cmp::Reverse((l.row.updated, l.row.index, l.thread)));
        v
    }
}

fn row(n: &Notice, participating: bool) -> InboxRow {
    InboxRow {
        reason: n.reason,
        updated: n.at,
        index: n.index,
        title: n.title.clone(),
        kind: n.kind,
        latest_comment: n.latest_comment,
        participating,
    }
}

/// The daily snapshot: rows and `sources`, as of `at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: Timestamp,
    pub threads: BTreeMap<ThreadId, InboxRow>,
    pub sources: BTreeMap<ChainId, Index>,
}

/// Where a rebuild replays a chain from: an index (a chain in `sources`), or a time (a watched or subscribed
/// repository, from INBOX_WINDOW before the snapshot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    After(Index),
    Since(Timestamp),
}

impl Snapshot {
    /// The Inbox the rebuild starts from.
    pub fn inbox(&self) -> Inbox {
        let day = Some(self.at.0 / DAY_MS);
        Inbox { threads: self.threads.clone(), sources: self.sources.clone(), dirty: false, snapshot_day: day }
    }

    /// DESIGN's rebuild rule: each chain in `sources` after its index, and each repository the account watches or
    /// subscribes to a thread of from INBOX_WINDOW before the snapshot. A chain in both replays after its index,
    /// which covers more than the window does only if the index is older.
    pub fn rebuild_from(&self, user: &UserRows) -> BTreeMap<ChainId, From> {
        let mut out: BTreeMap<ChainId, From> = self.sources.iter().map(|(c, i)| (*c, From::After(*i))).collect();
        let since = Timestamp(self.at.0.saturating_sub(INBOX_WINDOW.as_millis() as u64));
        let watched = user.watches.iter().filter(|(_, w)| w.subscribed && !w.ignored).map(|(r, _)| *r);
        let subscribed = user.subscriptions.iter().filter(|(_, (s, _))| *s).map(|(t, _)| t.repo);
        for r in watched.chain(subscribed) {
            out.entry(ChainId::Repo(r)).or_insert(From::Since(since));
        }
        out
    }
}

/// Whether a rebuild replays entry `e` of `chain` under `from`.
pub fn replays(from: From, e: &LogEntry<RepoEvent>) -> bool {
    match from {
        From::After(i) => e.index > i,
        From::Since(t) => e.at >= t,
    }
}
