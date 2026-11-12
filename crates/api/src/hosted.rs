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
use graftyard_types::{AccountId, DbId, Index, RepoId, Timestamp};
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
/// The most row bytes one read builds before it stops as incomplete [guess, four times the 1.6 MB dumps of 500
/// agents]. Counting rows alone, a 4 GB metadata tier of 1,800-byte comments was scanned to `ROWS_MAX`, about 90 MB,
/// only to be refused, at about 302 s of CPU a read on its leader (`tests/am1/results/b0-30.md`, "Blocked on", 5).
pub const ROWS_BYTES_MAX: usize = 8 << 20;

/// What a `TierRows::dump` may still take, shared by both tiers' dumps of one read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub rows: usize,
    pub bytes: usize,
}

impl Budget {
    /// `ROWS_MAX` rows and `ROWS_BYTES_MAX` bytes: one hosted read's.
    pub const READ: Budget = Budget { rows: ROWS_MAX, bytes: ROWS_BYTES_MAX };

    /// `rows` rows, with no byte limit.
    pub fn rows(rows: usize) -> Budget {
        Budget { rows, bytes: usize::MAX }
    }
}

fn val_bytes(v: &Val) -> usize {
    match v {
        Val::Null => 1,
        Val::Int(_) => 8,
        Val::Text(t) => t.len(),
        Val::Blob(b) => b.len(),
    }
}

/// Both tiers of a repository at `index`, as the member's `{"q": {"rows": true}}` read answers. `meta` is `None` while
/// the member's metadata tier is still loading.
/// The pull request a merge's rows read (`TierRows::merge_rows`): by database id, as `mergePullRequest` names it, or
/// by number, as REST's `PUT /pulls/{n}/merge` does. Before LM-1 round 7 a REST merge read both whole tiers, about
/// 1 MB at journey 9, and its p99 was 4.4 s at 500 agents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeOf {
    Id(DbId),
    Number(IssueNumber),
}

impl MergeOf {
    /// The member read's `rows` value: `{"merge": id}` or `{"mergeN": n}`. A member built before `mergeN` answers it
    /// with both whole tiers.
    pub fn query(self) -> Value {
        match self {
            MergeOf::Id(id) => json!({ "merge": id.0 }),
            MergeOf::Number(n) => json!({ "mergeN": n.0 }),
        }
    }

    /// The pull request a member read's `rows` value names, if it is a merge's.
    pub fn of_query(rows: &Value) -> Option<MergeOf> {
        if let Some(id) = rows.get("merge").and_then(Value::as_i64) {
            return Some(MergeOf::Id(DbId(id)));
        }
        let n = rows.get("mergeN").and_then(Value::as_u64)?;
        Some(MergeOf::Number(IssueNumber(u32::try_from(n).ok()?)))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TierRows {
    pub index: u64,
    pub core: Vec<(String, Key, Vec<Val>)>,
    pub meta: Option<Vec<(String, Key, Vec<Val>)>>,
    /// False when the tiers hold more than `ROWS_MAX` rows; the rows are then cut short and must not be served.
    pub complete: bool,
}

/// Metadata tables no `Repo` view, directory, or planner reads, which `TierRows::dump` leaves out: each agent ref's
/// `PathSet` (the feed takes it from the entry's events) and each pushed commit's provenance. At 500 agents `path_set`
/// was 1.12 MB of a 1.6 MB whole-tier read (500 rows), and each REST, GraphQL, task, and lease request reads the tiers.
pub const UNSERVED: &[&str] = &["path_set", "provenance"];

impl TierRows {
    /// Every row of `schema`'s tables in `r` but `UNSERVED`'s, within `budget`'s rows and bytes in all. `complete` is
    /// false when more remain, and the scan stops there: a tier too large to serve costs one budget's work to find out.
    pub fn dump(r: &dyn Read, schema: &Schema, budget: &mut Budget) -> (Vec<(String, Key, Vec<Val>)>, bool) {
        let mut out = Vec::new();
        for def in schema.tables.iter().filter(|d| !UNSERVED.contains(&d.name)) {
            let mut from: Key = Vec::new();
            loop {
                let want = budget.rows.min(1_000) + 1;
                let batch = r.scan(def.name, &from, want);
                let full = batch.len() == want;
                for (k, cols) in batch {
                    if !from.is_empty() && k == from {
                        continue;
                    }
                    let bytes: usize = k.iter().chain(&cols).map(val_bytes).sum();
                    if budget.rows == 0 || budget.bytes < bytes {
                        return (out, false);
                    }
                    budget.rows -= 1;
                    budget.bytes -= bytes;
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

    /// The rows a merge plans from, for the pull request `of` names: its `issue_by_id` (by database id, as
    /// `mergePullRequest` names it; REST's `PUT /pulls/{n}/merge` names the number),
    /// `issue`, and `pull` rows, its `pull_head`, its base's `ref`, and `settings`, which is all `graphql::plan` and
    /// `graphql::merge_request` read, and the leader renders the reply from its own tiers. One `fresh` read of these
    /// instead of both whole tiers: the whole read of a 1,200-row repository was 570 KB and about 800 ms from LAX to a
    /// weur or apac leader, against 240 ms for a small read (`tests/m2/results/m2-wp6.md`, BM-2). `index` is 0 here;
    /// the member sets it. With the metadata tier still loading, `meta` is `None`, as `dump`'s caller leaves it.
    pub fn merge_rows(core: &dyn Read, meta: Option<&dyn Read>, of: MergeOf) -> TierRows {
        use graftyard_git::tiers::{get_rec, n_key, pull_head, ref_key};
        let row = |r: &dyn Read, t: &str, k: Key| r.get(t, &k).map(|cols| (t.to_owned(), k, cols));
        let mut c = TierRows::access_rows(core);
        let mut m = Vec::new();
        if let Some(meta) = meta {
            let n = match of {
                MergeOf::Id(id) => {
                    let by_id = vec![Val::Int(id.0)];
                    m.extend(row(meta, "issue_by_id", by_id.clone()));
                    get_rec::<u32>(meta, "issue_by_id", &by_id).map(IssueNumber)
                }
                MergeOf::Number(n) => Some(n),
            };
            if let Some(n) = n {
                m.extend(row(meta, "issue", vec![n_key(n)]));
                m.extend(row(meta, "pull", vec![n_key(n)]));
                c.extend(row(core, "pull_head", vec![n_key(n)]));
                if let Some(ph) = pull_head(core, n) {
                    c.extend(row(core, "ref", vec![ref_key(&ph.base)]));
                }
            }
        }
        TierRows { index: 0, core: c, meta: meta.map(|_| m), complete: true }
    }

    /// The core rows `graftyard_identity::authorize_read` reads through `graftyard_git::issue_ops::Access`: `settings`
    /// for the visibility and every `collaborator` row. The member answers them alone to `{"rows": {"access": true}}`,
    /// which a member built before it answers with every row, so the edge's read gate works on both.
    pub fn access_rows(core: &dyn Read) -> Vec<(String, Key, Vec<Val>)> {
        let mut c: Vec<_> =
            core.get("settings", &[]).map(|cols| ("settings".to_owned(), Vec::new(), cols)).into_iter().collect();
        c.extend(core.prefix("collaborator", &[]).into_iter().map(|(k, cols)| ("collaborator".to_owned(), k, cols)));
        c
    }

    /// The core rows an agent push reads (`linked` on the edge): the `AgentPolicy` row and every task. The member
    /// answers them alone to `{"rows": {"agent": true}}`, which a member built before it answers with every row.
    pub fn agent_rows(core: &dyn Read) -> Vec<(String, Key, Vec<Val>)> {
        let mut c: Vec<_> = core
            .get("agent_policy", &[])
            .map(|cols| ("agent_policy".to_owned(), Vec::new(), cols))
            .into_iter()
            .collect();
        c.extend(core.prefix("agent_task", &[]).into_iter().map(|(k, cols)| ("agent_task".to_owned(), k, cols)));
        c
    }

    /// The answer to `{"rows": true, "packed": true}`: the rows as one JSON string. As a `serde_json::Value` tree,
    /// each `Val::Blob` byte is its own node, and a 740 KB answer was a 13 MB tree, peaking at 24 MB per read while it
    /// was built and serialized. At 250 agents the members of a repository ran out of memory on `/read`
    /// (`tests/am1/results/lm1-rerun2.md`). The string is the same JSON, built once.
    pub fn packed(&self) -> Value {
        Value::String(serde_json::to_string(self).unwrap_or_default())
    }

    /// The rows a `rows` read's `value` holds, packed (`TierRows::packed`) or not: a member built before `packed`
    /// answers the object.
    pub fn from_value(v: &Value) -> Result<TierRows, serde_json::Error> {
        match v {
            Value::String(s) => serde_json::from_str(s),
            v => TierRows::deserialize(v),
        }
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

/// Every account a repository's tiers name, so the edge can read their logins before it renders (F50): authors,
/// assignees, reviewers, mergers, status creators, release authors, asset uploaders, and lock owners. Each comes from
/// the typed row `Repo` reads, so a reply names no account outside this set.
pub fn accounts(core: &dyn Read, meta: &dyn Read) -> std::collections::BTreeSet<AccountId> {
    use graftyard_git::issues::{PrState, PullHead, StatusUpdate};
    use graftyard_git::tiers::{AssetRow, CommentRow, IssueRow, LockRow, ReleaseRow, ReviewRow, Timed, get_rec};
    let mut out = std::collections::BTreeSet::new();
    let keys = |r: &dyn Read, table: &str| r.prefix(table, &[]).into_iter().map(|(k, _)| k).collect::<Vec<_>>();
    for k in keys(meta, "issue") {
        out.extend(get_rec::<IssueRow>(meta, "issue", &k).and_then(|i| i.author));
    }
    for k in keys(meta, "issue_assignee") {
        if let Some(Val::Int(a)) = k.get(1) {
            out.insert(AccountId(DbId(*a)));
        }
    }
    for k in keys(meta, "comment") {
        out.extend(get_rec::<CommentRow>(meta, "comment", &k).and_then(|c| c.author));
    }
    for k in keys(meta, "review") {
        out.extend(get_rec::<ReviewRow>(meta, "review", &k).and_then(|c| c.reviewer));
    }
    for k in keys(meta, "status") {
        out.extend(get_rec::<Timed<StatusUpdate>>(meta, "status", &k).map(|s| s.rec.creator));
    }
    for k in keys(meta, "release") {
        out.extend(get_rec::<ReleaseRow>(meta, "release", &k).and_then(|x| x.rec.author));
    }
    for k in keys(meta, "asset") {
        out.extend(get_rec::<AssetRow>(meta, "asset", &k).and_then(|a| a.uploader));
    }
    for k in keys(meta, "lfs_lock") {
        out.extend(get_rec::<LockRow>(meta, "lfs_lock", &k).map(|l| l.owner));
    }
    for k in keys(core, "pull_head") {
        if let Some(PrState::Merged { by, .. }) = get_rec::<PullHead>(core, "pull_head", &k).map(|h| h.state) {
            out.insert(by.0);
        }
    }
    out
}

/// The accounts a request's replies name, as the edge read them: the owner's `Authz` fold for permissions and the
/// owner's kind, the logins it knows (the owner's, from the request path, and each account's `AccountLogin` row on
/// its `account_shard`, F50), and the signed-in account.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostedDir {
    pub home: Option<RepoRef>,
    /// In the raft codec, as hex: the fold's maps have tuple and struct keys, which JSON cannot hold.
    #[serde(with = "authz_hex")]
    pub authz: Option<AuthzFold>,
    pub logins: BTreeMap<i64, String>,
    pub viewer: Option<AccountId>,
    /// The install's `Authz(Genesis.account)` fold, for `AgentApp` rows. The edge reads it only for a reply under the
    /// agent media type, and it never crosses the binding, so a `Render` is the same bytes with or without it.
    #[serde(skip)]
    pub install: Option<AuthzFold>,
}

mod authz_hex {
    use graftyard_identity::AuthzFold;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(a: &Option<AuthzFold>, s: S) -> Result<S::Ok, S::Error> {
        match a {
            Some(a) => {
                let b = graftyard_raft::codec::to_bytes(a);
                s.serialize_some(&b.iter().map(|x| format!("{x:02x}")).collect::<String>())
            }
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<AuthzFold>, D::Error> {
        let Some(h) = Option::<String>::deserialize(d)? else { return Ok(None) };
        let b: Option<Vec<u8>> =
            (0..h.len()).step_by(2).map(|i| u8::from_str_radix(h.get(i..i + 2)?, 16).ok()).collect();
        let b = b.ok_or_else(|| serde::de::Error::custom("authz is not hex"))?;
        graftyard_raft::codec::from_bytes(&b).map(Some).map_err(serde::de::Error::custom)
    }
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
    fn agent_app(&self, app: graftyard_identity::AppId) -> bool {
        self.install.as_ref().is_some_and(|a| a.agent_app(app).is_some())
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
    /// `PUT /repos/{o}/{r}/pulls/{n}/merge`.
    RestMerge,
    /// `POST /repos/{o}/{r}/statuses/{sha}`: the status's commit and id.
    StatusCreated {
        sha: ObjectId,
        id: i64,
    },
    /// `POST /repos/{o}/{r}/labels`.
    LabelCreated,
    /// `PUT /api/graftyard/v1/repos/{o}/{r}/agents/policy`.
    AgentPolicy,
    /// A task command under `/api/graftyard/v1/repos/{o}/{r}/tasks` (`crate::tasks`): 201 for a create, else 200. The
    /// caller's `contents: read` says whether the reply carries `prompt`.
    Task {
        created: bool,
        contents: bool,
    },
    /// `rest:repos` (`crate::repos`): a create's 201 with `Location`, or an update's 200, both `full-repository`.
    RepoSaved {
        created: bool,
    },
    /// gh's GraphQL `createRepository` (`crate::repos::create_payload`).
    RepoCreatedGraphql {
        query: String,
        variables: Value,
    },
    /// A lease command under `/api/graftyard/v1/repos/{o}/{r}/leases` (`crate::leases`): 201 for an acquire, 200 for
    /// a renew, 204 for a release.
    Lease {
        created: bool,
    },
    /// `POST`, `PUT`, `DELETE /repos/{o}/{r}/issues/{n}/labels[/{name}]` (`crate::rest::plan_issue_labels`): the
    /// issue's labels, or 204 when `empty` (a `DELETE` of every label).
    IssueLabels {
        n: u32,
        empty: bool,
    },
    /// `POST /repos/{o}/{r}/hooks` (201 with `Location`) and `PATCH .../hooks/{id}` (200): GHES's `hook` object
    /// (`crate::webhooks::hook_json`), secret masked. M4-WP1, format 32.16.
    Hook {
        created: bool,
    },
    /// POST /repos/{o}/{r}/pulls/{n}/comments.
    ReviewCommentCreated {
        n: u32,
    },
    /// POST /repos/{o}/{r}/check-runs. The 201 renders the id `decide` assigned, from the `&Acked`; `id` is the id an
    /// `lf4-rest-2` edge minted for its `RecordCheck`, which acks none, and 0 from a later edge.
    CheckRunCreated {
        id: i64,
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
        What::RestMerge => WireReply::of(&Reply::error(500, "a merge rendered as an issue command", "rest")),
        What::StatusCreated { sha, id } => WireReply::of(&crate::rest::status_created(acked, &r, *sha, *id)),
        What::LabelCreated => WireReply::of(&crate::rest::label_created(acked, &r)),
        What::AgentPolicy => {
            WireReply::of(&Reply::json(200, crate::agents::policy_json(&graftyard_git::tiers::agent_policy(core))))
        }
        What::Task { created, contents } => {
            // The state as the owner's `Authz` the edge read gives it, at the task's own update time: the command
            // just wrote it, so it is not idle.
            let Some(owner) = render.dir.authz.as_ref() else {
                return WireReply::of(&Reply::error(503, "the owner's Authz was not read", "rest"));
            };
            let at = match &acked.out {
                IssueOut::Task(id) => graftyard_git::task::task_core(core, id).map_or(Timestamp(0), |t| t.updated),
                _ => Timestamp(0),
            };
            let reader = crate::tasks::Reader { metadata: true, contents: *contents };
            WireReply::of(&crate::tasks::render_ok(&r, owner, at, acked, *created, reader))
        }
        What::RepoSaved { created } => WireReply::of(&crate::repos::saved(acked, &r, *created)),
        What::RepoCreatedGraphql { query, variables } => {
            match Request::parse(query, variables.as_object().cloned(), None) {
                Ok(req) => graphql(crate::repos::create_payload(acked, &req, &r)),
                Err(e) => graphql(json!({ "errors": [e.json()] })),
            }
        }
        What::Lease { created } => {
            let Some(owner) = render.dir.authz.as_ref() else {
                return WireReply::of(&Reply::error(503, "the owner's Authz was not read", "rest"));
            };
            // Rendered at the lease's grant or renewal time, which the command just wrote, so it reads live.
            let at = match &acked.out {
                IssueOut::Lease(id) => graftyard_git::lease::lease_row(core, *id).map_or(Timestamp(0), |row| {
                    if *created { row.lease.granted } else { Timestamp(row.lease.expires.0.saturating_sub(1)) }
                }),
                _ => Timestamp(0),
            };
            WireReply::of(&crate::leases::render_ok(&r, owner, at, acked, *created))
        }
        What::IssueLabels { n, empty } => WireReply::of(&crate::rest::issue_labels_ok(&r, *n, *empty)),
        What::Hook { created } => WireReply::of(&crate::webhooks::saved(acked, &r, *created)),
        What::ReviewCommentCreated { n } => WireReply::of(&crate::rest::review_comment_created(acked, &r, *n)),
        What::CheckRunCreated { id } => WireReply::of(&crate::rest::check_run_created(acked, &r, *id)),
    }
}

/// The reply to a refused issue command.
pub fn render_err(render: &Render, e: &IssueErr) -> WireReply {
    match &render.what {
        What::Graphql { field, .. } => graphql(crate::graphql::refused_field(field, e)),
        What::ReleaseCreated | What::ReleaseUpdated | What::Deleted => WireReply::of(&crate::release::refused(e)),
        What::LfsVerified => WireReply::of(&crate::lfs::not_verified()),
        What::LfsLocked | What::LfsUnlocked { .. } => WireReply::of(&crate::lfs::refused(e)),
        What::RestMerge | What::StatusCreated { .. } => WireReply::of(&crate::release::refused(e)),
        What::LabelCreated => WireReply::of(&crate::rest::label_refused(e)),
        What::AgentPolicy => WireReply::of(&crate::release::refused(e)),
        What::Task { .. } => WireReply::of(&crate::tasks::refused(e)),
        What::RepoSaved { .. } => WireReply::of(&crate::repos::refused(e)),
        What::RepoCreatedGraphql { .. } => graphql(crate::graphql::refused_field("createRepository", e)),
        What::Lease { .. } => WireReply::of(&crate::leases::refused(None, e)),
        What::IssueLabels { .. } => WireReply::of(&crate::rest::issue_labels_refused(e)),
        What::Hook { .. } => WireReply::of(&crate::webhooks::refused(e)),
        What::ReviewCommentCreated { .. } => WireReply::of(&crate::rest::write_refused(
            e,
            "rest/pulls/comments#create-a-review-comment-for-a-pull-request",
        )),
        What::CheckRunCreated { .. } => {
            WireReply::of(&crate::rest::write_refused(e, "rest/checks/runs#create-a-check-run"))
        }
    }
}

// ---- merges ----

/// The reply to a committed merge command, from its `&Acked` and the leader's core tier after the entry applied: the
/// GraphQL payload, or REST's `pull-request-merge-result`.
pub fn render_merge_ok(
    render: &Render,
    acked: &Acked<graftyard_git::merge::MergeCmdOut>,
    core: &dyn Read,
) -> WireReply {
    use graftyard_git::merge::{MergeCmdOut, MergeOut};
    let Some(at) = render.dir.home.clone() else { return WireReply::of(&Reply::error(500, "no repository", "rest")) };
    let Ok(host) = Host::new(&render.host, render.isolation) else {
        return WireReply::of(&Reply::error(500, "bad host", "rest"));
    };
    let meta = MemStore::default();
    let r = Repo { host: &host, at, core, meta: &meta, dir: &render.dir };
    match (&render.what, &acked.out) {
        (What::RestMerge, MergeCmdOut::Merged(o)) => WireReply::of(&Reply::json(
            200,
            json!({ "sha": o.merge_commit.hex(), "merged": true, "message": "Pull Request successfully merged" }),
        )),
        (What::Graphql { query, variables, field, subject }, out) => {
            match Request::parse(query, variables.as_object().cloned(), None) {
                Ok(req) => graphql(match out {
                    MergeCmdOut::Merged(o) => crate::graphql::render_merge(&req, &acked.map(|_| MergeOut { ..*o })),
                    _ => crate::graphql::render_queued(
                        &req,
                        field,
                        IssueNumber(subject.unwrap_or(0)),
                        &acked.map(|_| ()),
                        &r,
                    ),
                }),
                Err(e) => graphql(json!({ "errors": [e.json()] })),
            }
        }
        _ => WireReply::of(&Reply::error(500, "a merge reply of another kind", "rest")),
    }
}

/// The reply to a refused merge command: GraphQL's error with GitHub's text, or REST's 405, 409, 403, or 404.
pub fn render_merge_err(render: &Render, e: &graftyard_git::merge::MergeError) -> WireReply {
    match &render.what {
        What::Graphql { field, .. } => graphql(crate::graphql::refused_merge_field(field, e)),
        _ => WireReply::of(&crate::rest::refused_merge(e)),
    }
}
