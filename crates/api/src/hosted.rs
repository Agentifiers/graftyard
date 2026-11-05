//! The hosted half of the handlers: what crosses between the edge and a repository member.
//!
//! - `TierRows`: one `fresh` read of a repository's two tiers at one index, which the edge loads into `MemStore`s and
//!   reads through `Repo`.
//! - `HostedDir`: the `Directory` the edge reads for a request (the owner's `Authz` fold, the logins it knows, and the
//!   signed-in account), serializable so the member that renders a write's reply reads the same one.
//! - `Render`: what the member renders from a committed issue command's `&Acked`. `Acked` cannot cross a binding
//!   (`design/DESIGN.md`, Shape: "the member renders the reply bytes and the edge frames them"), so a write's 2xx is
//!   built on the leader by `render_ok`, and the edge only adds the cross-cutting headers.

use std::collections::BTreeMap;

use graftyard_fold::{Key, MemStore, Read, Schema, Store, Stored, Val};
use graftyard_git::issue_ops::{IssueErr, IssueOut};
use graftyard_git::issues::IssueNumber;
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_identity::{AccountKind, AuthzFold, AuthzRead, RepoPermission, UserId};
use graftyard_raft::Acked;
use graftyard_types::{AccountId, DbId, Index, RepoId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::exec::Request;
use crate::view::{Compare, Directory, Repo, RepoRef};
use crate::wire::{Host, Reply};
use graftyard_wire::Body;

// ---- tier rows ----

/// The most rows one read returns [guess]. A repository with more is refused: a paged read would mix indexes, and
/// `Repo` reads one index.
pub const ROWS_MAX: usize = 50_000;

/// Both tiers of a repository at `index`, as the member's `{"q": {"rows": true}}` read answers. `meta` is `None` while
/// the member's metadata tier is still loading.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TierRows {
    pub index: u64,
    pub core: Vec<(String, Key, Vec<Val>)>,
    pub meta: Option<Vec<(String, Key, Vec<Val>)>>,
    /// False when the tiers hold more than `ROWS_MAX` rows; the rows are then cut short and must not be served.
    pub complete: bool,
}

impl TierRows {
    /// Every row of `schema`'s tables in `r`, at most `budget` in all. `complete` is false when more remain.
    pub fn dump(r: &dyn Read, schema: &Schema, budget: &mut usize) -> (Vec<(String, Key, Vec<Val>)>, bool) {
        let mut out = Vec::new();
        for def in schema.tables {
            let mut from: Key = Vec::new();
            loop {
                let want = (*budget).min(1_000) + 1;
                let batch = r.scan(def.name, &from, want);
                let full = batch.len() == want;
                for (k, cols) in batch {
                    if !from.is_empty() && k == from {
                        continue;
                    }
                    if *budget == 0 {
                        return (out, false);
                    }
                    *budget -= 1;
                    from = k.clone();
                    out.push((def.name.to_owned(), k, cols));
                }
                if !full {
                    break;
                }
            }
        }
        (out, true)
    }

    /// The rows as a store `Repo` reads.
    pub fn store(rows: &[(String, Key, Vec<Val>)]) -> MemStore {
        let mut s = MemStore::default();
        for (t, k, cols) in rows {
            s.put(t, k.clone(), Stored { written: Index(0), cols: cols.clone() });
        }
        s
    }
}

// ---- the directory ----

/// The accounts a request's replies name, as the edge read them: the owner's `Authz` fold for permissions and the
/// owner's kind, the logins it knows (the owner's, from the request path, and any the install names), and the
/// signed-in account.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedDir {
    pub home: Option<RepoRef>,
    pub authz: Option<AuthzFold>,
    pub logins: BTreeMap<i64, String>,
    pub viewer: Option<AccountId>,
}

impl Directory for HostedDir {
    fn login(&self, id: AccountId) -> Option<String> {
        self.logins.get(&id.0.0).cloned()
    }
    fn name(&self, _: AccountId) -> Option<String> {
        None
    }
    fn is_org(&self, id: AccountId) -> bool {
        self.authz.as_ref().is_some_and(|a| a.owner() == Some(id) && a.opened() == Some(AccountKind::Org))
    }
    fn by_login(&self, login: &str) -> Option<AccountId> {
        self.logins.iter().find(|(_, l)| l.eq_ignore_ascii_case(login)).map(|(id, _)| AccountId(DbId(*id)))
    }
    fn permission(&self, id: AccountId) -> Option<RepoPermission> {
        let (a, home) = (self.authz.as_ref()?, self.home.as_ref()?);
        let user = UserId(id);
        if a.is_admin(user) { Some(RepoPermission::Admin) } else { a.grant(home.id, user) }
    }
    fn repo(&self, id: RepoId) -> Option<RepoRef> {
        self.home.clone().filter(|h| h.id == id)
    }
    /// A fork's ref needs another repository's group; Milestone 2's repositories are roots, so none is read.
    fn ref_of(&self, _: RepoId, _: &RefName) -> Option<ObjectId> {
        None
    }
    fn team(&self, _: DbId) -> Option<(String, String, String)> {
        None
    }
    fn viewer(&self) -> Option<AccountId> {
        self.viewer
    }
    /// The git cache's compare is not wired, so a pull request's counts are zero [inference: gh prints them only in
    /// `pr view`'s header].
    fn compare(&self, _: ObjectId, _: ObjectId) -> Compare {
        Compare::default()
    }
}

// ---- rendering a write's reply on the member ----

/// Which reply a committed issue command renders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum What {
    /// A GraphQL mutation: its document and variables, its root field, and the issue it is about.
    Graphql {
        query: String,
        variables: Value,
        field: String,
        subject: Option<u32>,
    },
    ReleaseCreated,
    ReleaseUpdated,
    Deleted,
    LfsVerified,
    LfsLocked,
    /// The lock as it was before the command deleted it.
    LfsUnlocked {
        lock: Value,
    },
}

/// The render request an issue command carries to its leader.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Render {
    pub host: String,
    pub isolation: bool,
    pub dir: HostedDir,
    pub what: What,
}

/// A reply as it crosses the binding: its status, its handler headers, and its JSON body, if any.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

impl WireReply {
    pub fn of(r: &Reply) -> WireReply {
        WireReply {
            status: r.status,
            headers: r.headers.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            body: match &r.body {
                Body::Json(v) => Some(v.clone()),
                _ => None,
            },
        }
    }

    /// Back to a `Reply`. Only the header names a handler sets are carried.
    pub fn reply(&self) -> Reply {
        let mut r = match &self.body {
            Some(v) => Reply::json(self.status, v.clone()),
            None => Reply::empty(self.status),
        };
        for (k, v) in &self.headers {
            if let Some(name) = HANDLER_HEADERS.iter().find(|h| h.eq_ignore_ascii_case(k)) {
                r = r.with(name, v.clone());
            }
        }
        r
    }
}

const HANDLER_HEADERS: &[&str] =
    &["content-type", "content-disposition", "etag", "link", "location", "www-authenticate"];

fn graphql(v: Value) -> WireReply {
    WireReply::of(&Reply::json(200, v))
}

/// The reply to a committed issue command, from its `&Acked` and the leader's tiers after the entry applied.
pub fn render_ok(render: &Render, acked: &Acked<IssueOut>, core: &dyn Read, meta: &dyn Read) -> WireReply {
    let Some(at) = render.dir.home.clone() else { return WireReply::of(&Reply::error(500, "no repository", "rest")) };
    let Ok(host) = Host::new(&render.host, render.isolation) else {
        return WireReply::of(&Reply::error(500, "bad host", "rest"));
    };
    let r = Repo { host: &host, at, core, meta, dir: &render.dir };
    match &render.what {
        What::Graphql { query, variables, field, subject } => {
            match Request::parse(query, variables.as_object().cloned(), None) {
                Ok(req) => graphql(crate::graphql::render_payload(&req, field, subject.map(IssueNumber), acked, &r)),
                Err(e) => graphql(json!({ "errors": [e.json()] })),
            }
        }
        What::ReleaseCreated => WireReply::of(&crate::release::created(acked, &r)),
        What::ReleaseUpdated => WireReply::of(&crate::release::updated(acked, &r)),
        What::Deleted => WireReply::of(&crate::release::deleted(acked)),
        What::LfsVerified => WireReply::of(&crate::lfs::verified(acked)),
        What::LfsLocked => WireReply::of(&crate::lfs::locked(acked, &r)),
        What::LfsUnlocked { lock } => WireReply::of(&crate::lfs::unlocked(acked, lock.clone())),
    }
}

/// The reply to a refused issue command.
pub fn render_err(render: &Render, e: &IssueErr) -> WireReply {
    match &render.what {
        What::Graphql { field, .. } => graphql(crate::graphql::refused_field(field, e)),
        What::ReleaseCreated | What::ReleaseUpdated | What::Deleted => WireReply::of(&crate::release::refused(e)),
        What::LfsVerified => WireReply::of(&crate::lfs::not_verified()),
        What::LfsLocked | What::LfsUnlocked { .. } => WireReply::of(&crate::lfs::refused(e)),
    }
}
