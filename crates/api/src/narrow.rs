//! Narrow API reads (`design/DESIGN.md`, "Follower read"): a REST `GET` or a GraphQL query over a repository answered
//! from only the rows its reply renders, not from both whole tiers.
//!
//! The member that serves the read runs the same renderer the edge runs, over its whole fold, through a `Recorder`
//! that keeps every row the renderer reads. Where a renderer selects from a whole table (a list's filter, sort, and
//! page; a count; a lookup by a non-key column), it asks `Directory::pick`: the member runs the selection over its fold
//! without recording it and keeps the result, a `Pick`. The member answers the recorded rows, the access rows the
//! edge's read gate reads, and the picks. The edge renders the same route over those rows with the picks in its
//! `HostedDir`, so it reads exactly the rows the member read, and the reply is the bytes a whole-tier read renders.
//!
//! A route that reads the directory to choose rows (`by_login`, for a GraphQL `filterBy` or a search qualifier) or a
//! mutation is answered `{"whole": true}`: the edge reads the whole tiers for it as before. `Overlaps` and `Provenance`
//! read tables whole tiers leave out (`hosted::UNSERVED`), so the edge answers them 503 instead.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use graftyard_fold::{Key, Read, Val};
use graftyard_git::issues::AppId;
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_identity::RepoPermission;
use graftyard_types::{AccountId, DbId, Index, RepoId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::hosted::TierRows;
use crate::view::{Compare, Directory, Pick, Repo, RepoRef};
use crate::wire::{Host, Page, Reply};

/// What the edge asks a member to answer narrowly: the repository as the request names it, and the route.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub at: RepoRef,
    pub route: Route,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    /// A `GET /repos/{o}/{r}/...` that `rest_get` serves: the path's segments after `{o}/{r}`, the query string, the
    /// REST path under `/api/v3` for `trust::layer`, and whether the client opted in to the trust view.
    Rest { tail: Vec<String>, query: String, path: String, agent: bool },
    /// A GraphQL query document, its variables, and its operation name.
    Graphql { query: String, variables: Option<Map<String, Value>>, op: Option<String> },
    /// `GET /api/graftyard/v1/repos/{o}/{r}/overlaps` (`crate::overlaps`). Its rows are never in a whole-tier read
    /// (`crate::hosted::UNSERVED`), so a member that answers it `whole` or `error` cannot serve it.
    Overlaps(crate::overlaps::OverlapQuery),
    /// `GET /api/graftyard/v1/repos/{o}/{r}/provenance` for the commit blame named, as 40 hex digits.
    Provenance { commit: String },
}

/// Whether `rest_get` serves `GET` of this tail.
pub fn rest_serves(tail: &[&str]) -> bool {
    let num = |s: &str| s.parse::<i64>().is_ok();
    matches!(
        tail,
        [] | ["issues"]
            | ["pulls", _]
            | ["issues", _, "comments"]
            | ["pulls", _, "reviews"]
            | ["pulls", _, "comments"]
            | ["commits", _, "check-runs"]
            | ["commits", _, "statuses"]
            | ["check-runs", _]
            | ["check-runs", _, "annotations"]
            | ["commits", _]
            | ["releases", "tags", _]
            | ["tarball" | "zipball", _]
    ) || matches!(tail, ["releases", n] if num(n))
}

/// The reply of a repository `GET` both the member (recording) and the edge (rendering) run, for the tails
/// `rest_serves` names but `commits/{ref}`, whose reply the edge renders from the git object; `None` for any other. `index` is the tiers' index, which `GET /repos/{o}/{r}`'s ETag names.
pub fn rest_get(r: &Repo<'_>, tail: &[&str], query: &str, index: Index) -> Option<Reply> {
    use crate::{release, rest};
    use graftyard_git::blob::ArchiveFormat;
    Some(match tail {
        [] => crate::repos::get(r, index),
        ["issues"] => rest::list_issues(r, query),
        ["pulls", n] => rest::get_pull(r, n),
        ["issues", n, "comments"] => rest::list_issue_comments(r, n, query),
        ["pulls", n, "reviews"] => rest::list_reviews(r, n, query),
        ["pulls", n, "comments"] => rest::list_review_comments(r, n, query),
        ["commits", reference, "check-runs"] => rest::list_check_runs_for_ref(r, reference, query),
        ["commits", reference, "statuses"] => rest::list_commit_statuses(r, reference, Page::parse(query)),
        ["check-runs", id] => rest::get_check_run(r, id),
        ["check-runs", id, "annotations"] => rest::list_check_run_annotations(r, id, query),
        ["releases", "tags", tag] => release::get_by_tag(r, tag),
        ["releases", n] => release::get(r, n.parse().ok()?),
        [kind @ ("tarball" | "zipball"), reference] => {
            let f = if *kind == "tarball" { ArchiveFormat::TarGz } else { ArchiveFormat::Zip };
            release::archive_redirect(r, f, reference)
        }
        _ => return None,
    })
}

/// A tier read that keeps every row it returns, except while a selection runs (`paused`).
struct Recorder<'a> {
    inner: &'a dyn Read,
    paused: &'a Cell<bool>,
    rows: RefCell<BTreeMap<(String, Key), Vec<Val>>>,
}

impl<'a> Recorder<'a> {
    fn new(inner: &'a dyn Read, paused: &'a Cell<bool>) -> Self {
        Recorder { inner, paused, rows: RefCell::new(BTreeMap::new()) }
    }

    fn keep(&self, table: &str, rows: &[(Key, Vec<Val>)]) {
        if self.paused.get() {
            return;
        }
        let mut m = self.rows.borrow_mut();
        for (k, v) in rows {
            m.insert((table.to_owned(), k.clone()), v.clone());
        }
    }

    fn into_rows(self) -> Vec<(String, Key, Vec<Val>)> {
        self.rows.into_inner().into_iter().map(|((t, k), v)| (t, k, v)).collect()
    }
}

impl Read for Recorder<'_> {
    fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>> {
        let v = self.inner.get(table, key)?;
        self.keep(table, &[(key.to_vec(), v.clone())]);
        Some(v)
    }
    /// Keeps the rows that share `from`'s first column: a renderer that scans (`Repo::statuses`) stops at the first
    /// row past its prefix, and over the kept rows the edge's scan ends where the member's did.
    fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Vec<Val>)> {
        let out = self.inner.scan(table, from, limit);
        let n = out.iter().take_while(|(k, _)| from.first().is_none_or(|f| k.first() == Some(f))).count();
        self.keep(table, &out[..n]);
        out
    }
    /// Only the rows under `prefix`: the default would record each batch's rows past it, which the edge never reads.
    fn prefix(&self, table: &str, prefix: &[Val]) -> Vec<(Key, Vec<Val>)> {
        let out = self.inner.prefix(table, prefix);
        self.keep(table, &out);
        out
    }
}

/// The member's directory while it records: it knows no account, and a selection by login (`by_login`) makes the read
/// whole. Every other answer leaves which rows a renderer reads unchanged.
struct Recording<'a> {
    home: RepoRef,
    paused: &'a Cell<bool>,
    picks: RefCell<Vec<(String, Pick)>>,
    whole: Cell<bool>,
    /// Every account whose login the reply asked for.
    asked: RefCell<std::collections::BTreeSet<AccountId>>,
}

impl Directory for Recording<'_> {
    fn login(&self, id: AccountId) -> Option<String> {
        self.asked.borrow_mut().insert(id);
        None
    }
    fn name(&self, _: AccountId) -> Option<String> {
        None
    }
    fn is_org(&self, _: AccountId) -> bool {
        false
    }
    fn by_login(&self, _: &str) -> Option<AccountId> {
        self.whole.set(true);
        None
    }
    fn permission(&self, _: AccountId) -> Option<RepoPermission> {
        None
    }
    fn repo(&self, id: RepoId) -> Option<RepoRef> {
        (self.home.id == id).then(|| self.home.clone())
    }
    fn ref_of(&self, _: RepoId, _: &RefName) -> Option<ObjectId> {
        None
    }
    fn team(&self, _: DbId) -> Option<(String, String, String)> {
        None
    }
    fn viewer(&self) -> Option<AccountId> {
        None
    }
    fn compare(&self, _: ObjectId, _: ObjectId) -> Compare {
        Compare::default()
    }
    fn agent_app(&self, _: AppId) -> bool {
        false
    }
    fn pick(&self, key: &str, select: &mut dyn FnMut() -> Pick) -> Pick {
        let was = self.paused.replace(true);
        let p = select();
        self.paused.set(was);
        let mut picks = self.picks.borrow_mut();
        if !picks.iter().any(|(k, _)| k == key) {
            picks.push((key.to_owned(), p.clone()));
        }
        p
    }
}

/// The host a member renders under while it records: URLs carry it, and no row read depends on it.
const RECORD_HOST: &str = "graftyard.invalid";

/// The pick of the accounts whose logins the edge reads (`LOGINS`).
pub const LOGINS: &str = "logins";
/// The pick of the lowest name shards of the accounts the whole tiers name (`LOGIN_SHARDS`).
pub const LOGIN_SHARDS: &str = "login_shards";
/// How many name shards the edge reads logins from at most (the edge's `LOGIN_SHARDS_MAX`), and so how many of the
/// lowest the member sends: with the viewer's shard added, the lowest this many of the union are among them.
pub const LOGIN_SHARDS_MAX: usize = 24;

/// A member's answer to `{"rows": {"api": spec}}` over its tiers at `index`: `TierRows` with the rows the route's reply
/// reads, the core access rows, and the picks; `meta` is `None` while the metadata tier loads. `{"whole": true}` for
/// a route that needs both whole tiers, and `{"error": ..}` for a spec this release cannot read.
///
/// `named` is every account the whole tiers name (`hosted::accounts`), which the member may cache per state: a whole-tier read had the edge read the logins of all of them on its lowest LOGIN_SHARDS_MAX name shards, so
/// the answer carries, as picks, which of the accounts the reply asked for are among them (`LOGINS`) and those lowest
/// shards (`LOGIN_SHARDS`), and the edge knows exactly the logins a whole-tier read knew.
pub fn answer(
    core: &dyn Read,
    meta: Option<&dyn Read>,
    index: u64,
    spec: &Value,
    named: &dyn Fn() -> std::collections::BTreeSet<AccountId>,
) -> Value {
    let Ok(spec) = Spec::deserialize(spec) else { return json!({ "error": "not a narrow read spec" }) };
    let mut access = TierRows::access_rows(core);
    let Some(meta) = meta else {
        return to_value(TierRows { index, core: access, meta: None, complete: true, picks: Some(Vec::new()) });
    };
    let Ok(host) = Host::new(RECORD_HOST, false) else { return json!({ "error": "host" }) };
    let paused = Cell::new(false);
    let (c, m) = (Recorder::new(core, &paused), Recorder::new(meta, &paused));
    let dir = Recording {
        home: spec.at.clone(),
        paused: &paused,
        picks: RefCell::new(Vec::new()),
        whole: Cell::new(false),
        asked: RefCell::new(Default::default()),
    };
    let r = Repo { host: &host, at: spec.at.clone(), core: &c, meta: &m, dir: &dir };
    match &spec.route {
        Route::Rest { tail, query, path, agent } => {
            let tail: Vec<&str> = tail.iter().map(String::as_str).collect();
            if let ["commits", reference] = tail.as_slice() {
                // The reply is the git object's; the rows give only which commit (`rest::get_commit`).
                crate::rest::resolve(&r, reference);
            } else {
                let Some(reply) = rest_get(&r, &tail, query, Index(index)) else { return json!({ "whole": true }) };
                if *agent {
                    crate::trust::layer(Some(crate::trust::AGENT_MEDIA_TYPE), path, &r, reply);
                }
            }
        }
        Route::Graphql { query, variables, op } => {
            let Ok(req) = crate::exec::Request::parse(query, variables.clone(), op.clone()) else {
                return json!({ "whole": true });
            };
            if req.is_mutation().unwrap_or(true) {
                return json!({ "whole": true });
            }
            crate::graphql::query(&req, Some(&r));
        }
        Route::Overlaps(q) => {
            // No owner `Authz` here: every overlapping lease's rows, which the edge then judges held or not.
            crate::overlaps::overlaps_json(&r, None, graftyard_types::Timestamp(0), q);
        }
        Route::Provenance { commit } => {
            let Some(c) = ObjectId::parse_hex(commit.as_bytes()) else { return json!({ "error": "not a commit" }) };
            crate::overlaps::provenance_json(&r, c, None);
        }
    }
    if dir.whole.get() {
        return json!({ "whole": true });
    }
    let mut picks = dir.picks.into_inner();
    picks.extend(login_picks(named(), spec.at.owner_id, &dir.asked.into_inner()));
    let mut core_rows = c.into_rows();
    access.retain(|(t, k, _)| !core_rows.iter().any(|(t2, k2, _)| t2 == t && k2 == k));
    core_rows.extend(access);
    to_value(TierRows { index, core: core_rows, meta: Some(m.into_rows()), complete: true, picks: Some(picks) })
}

/// The picks that tell the edge which logins a whole-tier read knew (`LOGINS`, `LOGIN_SHARDS`): of the accounts a reply
/// asks for, those the whole tiers name (`named`, `hosted::accounts`), and the lowest LOGIN_SHARDS_MAX name shards of
/// every named account but the repository's `owner`, whose login the edge holds from the request path. A narrow API
/// read and a command read (`hosted::CommandSpec`) both send them.
pub fn login_picks(
    mut named: std::collections::BTreeSet<AccountId>,
    owner: AccountId,
    asked: &std::collections::BTreeSet<AccountId>,
) -> [(String, Pick); 2] {
    named.remove(&owner);
    let mut shards: Vec<i64> = named.iter().map(|a| i64::from(graftyard_identity::account_shard(*a).0)).collect();
    shards.sort_unstable();
    shards.dedup();
    shards.truncate(LOGIN_SHARDS_MAX);
    let ids = |v: Vec<i64>| -> Vec<Key> { v.into_iter().map(|i| vec![Val::Int(i)]).collect() };
    let known: Vec<i64> = asked.intersection(&named).map(|a| a.0.0).collect();
    [
        (LOGINS.into(), Pick { total: known.len(), items: ids(known), more: false }),
        (LOGIN_SHARDS.into(), Pick { total: shards.len(), items: ids(shards), more: false }),
    ]
}

fn to_value(t: TierRows) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

/// The picks an edge renders with (`HostedDir::picks`): a member's, by key. A key the member did not pick is answered
/// by running the selection over the rows the edge holds, and `missed` says so, because those rows are only the ones
/// the member recorded: the edge then reads the whole tiers instead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picks {
    pub held: Option<Vec<(String, Pick)>>,
    pub missed: Cell<bool>,
}

impl Picks {
    pub fn of(held: Option<Vec<(String, Pick)>>) -> Picks {
        Picks { held, missed: Cell::new(false) }
    }

    pub fn get(&self, key: &str, select: &mut dyn FnMut() -> Pick) -> Pick {
        let Some(held) = &self.held else { return select() };
        match held.iter().find(|(k, _)| k == key) {
            Some((_, p)) => p.clone(),
            None => {
                self.missed.set(true);
                select()
            }
        }
    }

    /// Whether a render over a narrow answer asked for a selection the member did not make.
    pub fn missed(&self) -> bool {
        self.missed.get()
    }

    /// For a narrow read, the accounts whose logins the edge reads: those the member found the reply asks for and the
    /// whole tiers name (`LOGINS`), and `viewer`, on the lowest LOGIN_SHARDS_MAX name shards of the member's lowest
    /// (`LOGIN_SHARDS`) and the viewer's, as a whole-tier read's edge cut them. `None` for a whole-tier read.
    pub fn login_accounts(&self, viewer: Option<AccountId>, owner: AccountId) -> Option<Vec<AccountId>> {
        let held = self.held.as_ref()?;
        let ints = |key: &str| -> Vec<i64> {
            held.iter()
                .find(|(k, _)| k == key)
                .map(|(_, p)| {
                    p.items
                        .iter()
                        .filter_map(|k| match k.first() {
                            Some(Val::Int(i)) => Some(*i),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut named: std::collections::BTreeSet<AccountId> =
            ints(LOGINS).into_iter().map(|i| AccountId(DbId(i))).collect();
        named.extend(viewer);
        named.remove(&owner);
        let mut shards: Vec<i64> = ints(LOGIN_SHARDS);
        shards.extend(viewer.filter(|v| *v != owner).map(|v| i64::from(graftyard_identity::account_shard(v).0)));
        shards.sort_unstable();
        shards.dedup();
        shards.truncate(LOGIN_SHARDS_MAX);
        Some(
            named
                .into_iter()
                .filter(|a| shards.contains(&i64::from(graftyard_identity::account_shard(*a).0)))
                .collect(),
        )
    }
}
