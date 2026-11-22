//! Leases on paths and refs (`design/DESIGN.md` and `design/AGENT-LAYER.md`, Shape, the lease block and "Leases";
//! `build/BUILD-PLAN.md`, AM1-WP4), ported from the M0-WP12 prototype (`tests/m0/src/wp12/lease.ts`).
//!
//! A `Lease` is a core row on its repository's group, written by `LeaseAcquire`, `LeaseRenew`, and `LeaseRelease`
//! (format 32.14, `crate::repo::REPO_FORMAT_LEASES`). It is live while an entry's `at` is before `expires`, checked
//! on read and in `decide` (`holds`), so expiry needs no command, and `on_entry` drops rows that expired. A lease
//! whose holder fails the owner `Authz` read, or whose task reads final, counts as released: uninstalling an App and
//! cancelling or finishing a task release its leases with no sweep and no event. A stall moves `expires` only through
//! `LogEntry.extend` (`extend`).
//!
//! Exclusive leases are indexed by the literal prefix of each glob (`lease_exclusive`), so `exclusive_conflicts`,
//! which acquires and agent pushes under `AgentsExclusive` run, reads only rows near the paths it checks.

use std::time::Duration;

use graftyard_fold::{Read, Rows, Val};
use graftyard_identity::glob::{glob_overlap, literal_prefix};
use graftyard_identity::{
    Actor, AppId, AuthzRead, Credential, InstallationId, Principal, RepoPermission, RepoScope, ScopeSet, SessionId,
    UserId,
};
use graftyard_types::{RepoId, Timestamp, Ulid};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::agent::{PathGlob, RefPattern};
use crate::issues::{IssueNumber, PrState};
use crate::tiers::{get_rec, int, put_rec};

pub const LEASE_TTL: Duration = Duration::from_secs(600);
/// The TTL a holder may ask for.
pub const TTL_MIN: Duration = Duration::from_secs(60);
pub const TTL_MAX: Duration = Duration::from_secs(3_600);
pub const EXCLUSIVE_MAX: Duration = Duration::from_secs(4 * 3_600);
/// Live leases per holder per repository.
pub const LEASES_PER_HOLDER: u16 = 8;
/// Live leases per App per repository [guess].
pub const LEASES_PER_APP: u16 = 1_000;
/// Live plan-block leases per repository [guess].
pub const PLAN_LEASES_PER_REPO: u16 = 500;
/// Live leases per repository.
pub const LEASES_PER_REPO: u16 = 4_000;
/// A plan-block lease lives at most this long after the edit that wrote it, and only while its draft is open.
pub const PLAN_TTL: Duration = Duration::from_secs(24 * 3_600);
/// Globs per lease, refs per lease, and intent bytes.
pub const PATHS_PER_LEASE: usize = 32;
pub const REFS_PER_LEASE: usize = 2;
pub const INTENT_MAX: usize = 280;
/// The largest `graftyard-plan` block [guess].
pub const PLAN_BLOCK_MAX: usize = 4 << 10;

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

/// A ULID: the time of the entry that granted it, then the repository's lease sequence number, so every member
/// derives the same id with no randomness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LeaseId(pub Ulid);

impl LeaseId {
    pub fn text(self) -> String {
        self.0.to_string()
    }

    pub fn parse(s: &str) -> Option<LeaseId> {
        s.parse::<Ulid>().ok().map(LeaseId)
    }
}

/// SHARED's `Actor` cut to the ones that hold leases: a user, or an App installation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ActorId {
    User(UserId),
    App { app: AppId, installation: InstallationId },
}

impl ActorId {
    pub fn of(p: &Principal) -> Option<ActorId> {
        match p.actor {
            Actor::User(u) => Some(ActorId::User(u)),
            Actor::App { app, installation } => Some(ActorId::App { app, installation }),
            Actor::Anonymous => None,
        }
    }
}

/// `app` is the App behind a user-to-server token (or the App itself for an installation), and `session` the task
/// the holder works for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseHolder {
    pub actor: ActorId,
    pub app: Option<AppId>,
    pub session: Option<SessionId>,
}

impl LeaseHolder {
    /// The index key of the holder, (actor, App, session), which LEASES_PER_HOLDER counts.
    pub fn key(&self) -> String {
        let a = match self.actor {
            ActorId::User(u) => format!("u{}", u.0.0.0),
            ActorId::App { app, installation } => format!("a{}.{}", app.0.0, installation.0.0),
        };
        let app = self.app.map_or(String::new(), |x| x.0.0.to_string());
        let s = self.session.as_ref().map_or(String::new(), SessionId::text);
        format!("{a}|{app}|{s}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseScope {
    pub paths: SmallVec<[PathGlob; 8]>,
    pub refs: SmallVec<[RefPattern; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LeaseMode {
    Shared,
    Exclusive,
}

impl LeaseMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseMode::Shared => "shared",
            LeaseMode::Exclusive => "exclusive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: LeaseId,
    pub holder: LeaseHolder,
    pub scope: LeaseScope,
    pub mode: LeaseMode,
    /// At most INTENT_MAX bytes, untrusted, shown through the trust view.
    pub intent: Box<str>,
    pub granted: Timestamp,
    pub expires: Timestamp,
}

impl Lease {
    /// The mode it reads as at `at`: an Exclusive lease reads as Shared once EXCLUSIVE_MAX has passed since `granted`.
    pub fn mode_at(&self, at: Timestamp) -> LeaseMode {
        if self.mode == LeaseMode::Exclusive && at.0.saturating_sub(self.granted.0) < ms(EXCLUSIVE_MAX) {
            LeaseMode::Exclusive
        } else {
            LeaseMode::Shared
        }
    }

    /// Whether some path or ref of this lease overlaps one of `paths` or `refs`.
    pub fn overlaps(&self, paths: &[&str], refs: &[&str]) -> bool {
        self.scope.paths.iter().any(|g| paths.iter().any(|p| glob_overlap(g.as_str(), p)))
            || self.scope.refs.iter().any(|r| refs.iter().any(|q| glob_overlap(r.as_str(), q)))
    }
}

/// The core `lease` row: the lease, and for a plan-block lease the draft pull request whose body declared it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRow {
    pub lease: Lease,
    pub plan: Option<IssueNumber>,
}

// ---- rows ----

fn id_key(id: LeaseId) -> Val {
    Val::Blob(id.0.0.to_be_bytes().to_vec().into())
}

fn id_of(v: Option<&Val>) -> Option<LeaseId> {
    match v {
        Some(Val::Blob(b)) => <[u8; 16]>::try_from(&b[..]).ok().map(|a| LeaseId(Ulid(u128::from_be_bytes(a)))),
        _ => None,
    }
}

fn text(s: &str) -> Val {
    Val::Text(s.into())
}

pub fn lease_row(r: &(impl Read + ?Sized), id: LeaseId) -> Option<LeaseRow> {
    get_rec(r, "lease", &[id_key(id)])
}

/// Every lease row, in id order (grant order), decoded from the scan itself: a second `get` per row doubled the SQL
/// reads of every live-set rebuild, which the feed's ring makes once per applied entry (LM-1 round 7).
pub fn rows(r: &(impl Read + ?Sized)) -> Vec<LeaseRow> {
    r.prefix("lease", &[])
        .into_iter()
        .filter_map(|(k, v)| {
            id_of(k.first())?;
            match v.first() {
                Some(Val::Blob(b)) => Some(postcard::from_bytes(b).expect("a fold row this release wrote")),
                _ => None,
            }
        })
        .collect()
}

fn ids_in(r: &(impl Read + ?Sized), table: &str, prefix: &[Val], col: usize) -> Vec<LeaseId> {
    r.prefix(table, prefix).into_iter().filter_map(|(k, _)| id_of(k.get(col))).collect()
}

/// The rows of `session`'s leases.
pub fn session_rows(r: &(impl Read + ?Sized), s: &SessionId) -> Vec<LeaseRow> {
    ids_in(r, "lease_by_session", &[text(&s.text())], 1).into_iter().filter_map(|id| lease_row(r, id)).collect()
}

/// The next lease's id at `at`.
fn next_id(r: &(impl Read + ?Sized), at: Timestamp) -> LeaseId {
    let seq: u64 = get_rec(r, "lease_seq", &[]).unwrap_or(0);
    let mut low = [0u8; 10];
    low[2..].copy_from_slice(&(seq + 1).to_be_bytes());
    LeaseId(Ulid::new(at, low))
}

pub(crate) fn write_acquire(w: &mut dyn Rows, lease: &Lease, plan: Option<IssueNumber>) {
    let id = id_key(lease.id);
    let seq: u64 = get_rec(w, "lease_seq", &[]).unwrap_or(0);
    put_rec(w, "lease_seq", Vec::new(), &(seq + 1));
    put_rec(w, "lease", vec![id.clone()], &LeaseRow { lease: lease.clone(), plan });
    index(w, lease, plan, true);
}

/// Writes or deletes a lease's index rows.
fn index(w: &mut dyn Rows, lease: &Lease, plan: Option<IssueNumber>, put: bool) {
    let id = id_key(lease.id);
    let h = &lease.holder;
    let mut keys: Vec<(&str, Vec<Val>)> = vec![
        ("lease_by_holder", vec![text(&h.key()), id.clone()]),
        ("lease_expiry", vec![int(lease.expires.0 as i64), id.clone()]),
    ];
    if let Some(app) = h.app {
        keys.push(("lease_by_app", vec![int(app.0.0), id.clone()]));
    }
    if let Some(s) = &h.session {
        keys.push(("lease_by_session", vec![text(&s.text()), id.clone()]));
    }
    if let Some(n) = plan {
        keys.push(("lease_plan", vec![int(i64::from(n.0)), text(&h.key()), id.clone()]));
    }
    if lease.mode == LeaseMode::Exclusive {
        for g in &lease.scope.paths {
            keys.push(("lease_exclusive", vec![text(&literal_prefix(g.as_str())), id.clone()]));
        }
    }
    for (t, k) in keys {
        if put { w.put(t, k, Vec::new()) } else { w.delete(t, &k) }
    }
}

pub(crate) fn write_renew(w: &mut dyn Rows, id: LeaseId, expires: Timestamp) {
    let Some(mut row) = lease_row(w, id) else { return };
    w.delete("lease_expiry", &[int(row.lease.expires.0 as i64), id_key(id)]);
    row.lease.expires = expires;
    w.put("lease_expiry", vec![int(expires.0 as i64), id_key(id)], Vec::new());
    put_rec(w, "lease", vec![id_key(id)], &row);
}

pub(crate) fn write_release(w: &mut dyn Rows, id: LeaseId) {
    let Some(row) = lease_row(w, id) else { return };
    index(w, &row.lease, row.plan, false);
    w.delete("lease", &[id_key(id)]);
}

/// Every entry: drops the rows of leases that expired at or before `at`. No later entry can revive one, because an
/// `ExtendLeases` moves only leases whose `expires` is after its `since`, which is at least the `at` of the last
/// entry its leader holds.
pub(crate) fn prune(w: &mut dyn Rows, at: Timestamp) {
    loop {
        let batch = w.scan("lease_expiry", &[], 64);
        let due: Vec<LeaseId> = batch
            .iter()
            .take_while(|(k, _)| matches!(k.first(), Some(Val::Int(e)) if *e as u64 <= at.0))
            .filter_map(|(k, _)| id_of(k.get(1)))
            .collect();
        if due.is_empty() {
            return;
        }
        for id in due {
            match lease_row(w, id) {
                Some(_) => write_release(w, id),
                None => {
                    // An index row whose lease is gone: drop it alone.
                    let stale: Vec<_> =
                        batch.iter().filter(|(k, _)| id_of(k.get(1)) == Some(id)).map(|(k, _)| k.clone()).collect();
                    for k in stale {
                        w.delete("lease_expiry", &k);
                    }
                }
            }
        }
    }
}

/// `LogEntry.extend` (`ExtendLeases { since, by }`): every lease with `expires` after `since` moves by `by`.
pub(crate) fn extend(w: &mut dyn Rows, since: Timestamp, by: Duration) {
    let moved: Vec<(LeaseId, Timestamp)> = rows(w)
        .into_iter()
        .filter(|r| r.lease.expires > since)
        .map(|r| (r.lease.id, Timestamp(r.lease.expires.0 + ms(by))))
        .collect();
    for (id, e) in moved {
        write_renew(w, id, e);
    }
}

// ---- who holds what ----

/// The folds and time a lease check reads: the owner's `Authz`, the repository, and the entry's `at`.
#[derive(Clone, Copy)]
pub struct LeaseView<'a> {
    pub core: &'a dyn Read,
    pub owner: &'a dyn AuthzRead,
    pub repo: RepoId,
    pub at: Timestamp,
}

impl LeaseView<'_> {
    fn access(&self) -> crate::issue_ops::Access<'_> {
        let owner = self.owner.owner().unwrap_or(graftyard_types::AccountId(graftyard_types::DbId(0)));
        crate::issue_ops::Access { repo: self.repo, owner, core: self.core }
    }

    /// Whether `actor` may push here by the owner's `Authz` read and the repository's collaborator rows: a user with
    /// `Write`, or an installation that reaches the repository with `contents: write`, created no earlier than
    /// `since` (a reinstall after the lease was granted does not revive it).
    pub fn can_push(&self, actor: ActorId, since: Timestamp) -> bool {
        match actor {
            ActorId::User(u) => {
                let p = Principal {
                    actor: Actor::User(u),
                    scopes: ScopeSet(0),
                    credential: Credential::Stored([0; 32]),
                    sso_orgs: SmallVec::new(),
                };
                graftyard_identity::permission(&p, &[self.owner], &self.access()) >= Some(RepoPermission::Write)
            }
            ActorId::App { app, installation } => {
                let Some(state) = self.owner.installation(installation) else { return false };
                state.row.as_ref().is_some_and(|r| {
                    r.app == app
                        && since >= state.not_before
                        && match &r.repos {
                            RepoScope::All => true,
                            RepoScope::Selected(ids) => ids.contains(&self.repo),
                        }
                        && r.permissions.0.get("contents").is_some_and(|a| *a >= graftyard_identity::Access::Write)
                })
            }
        }
    }

    /// Whether the row's lease is held at `at`: not expired, its holder still passes the owner's `Authz` read with
    /// push access, its task (if it names one) does not read final, and a plan-block lease's draft is still open.
    pub fn holds(&self, r: &LeaseRow) -> bool {
        let l = &r.lease;
        if self.at >= l.expires || !self.can_push(l.holder.actor, l.granted) {
            return false;
        }
        if let Some(s) = &l.holder.session
            && let Some(t) = crate::task::task_core(self.core, s)
            && crate::task::effective(&t, self.owner, self.repo, self.at).is_final()
        {
            return false;
        }
        match r.plan {
            Some(n) => {
                crate::tiers::pull_head(self.core, n).is_some_and(|p| matches!(p.state, PrState::Open { draft: true }))
            }
            None => true,
        }
    }

    /// The live leases, in grant order.
    pub fn live(&self) -> Vec<LeaseRow> {
        rows(self.core).into_iter().filter(|r| self.holds(r)).collect()
    }

    fn live_in(&self, table: &str, prefix: &[Val], col: usize) -> Vec<LeaseRow> {
        ids_in(self.core, table, prefix, col)
            .into_iter()
            .filter_map(|id| lease_row(self.core, id))
            .filter(|r| self.holds(r))
            .collect()
    }

    pub fn holder_live(&self, h: &LeaseHolder) -> Vec<LeaseRow> {
        self.live_in("lease_by_holder", &[text(&h.key())], 1)
    }

    pub fn session_live(&self, s: &SessionId) -> Vec<LeaseRow> {
        self.live_in("lease_by_session", &[text(&s.text())], 1)
    }

    fn app_live(&self, app: AppId) -> usize {
        self.live_in("lease_by_app", &[int(app.0.0)], 1).len()
    }

    fn plan_live(&self) -> Vec<LeaseRow> {
        self.live_in("lease_plan", &[], 2)
    }

    /// Live Exclusive leases of another holder than `mine` that overlap one of `globs` and still read Exclusive. A
    /// lease is another holder's when its session differs from `mine`'s, or, when `mine` names no session, when its
    /// actor differs (`LeaseEnforce`). A glob ending in `/` (a `truncated` directory) overlaps every glob under it.
    pub fn exclusive_conflicts(&self, globs: &[&str], mine: &LeaseHolder) -> Vec<Lease> {
        let mut ids: Vec<LeaseId> = Vec::new();
        for g in globs {
            let lp = literal_prefix(g);
            if lp.is_empty() {
                ids.extend(ids_in(self.core, "lease_exclusive", &[], 1));
                continue;
            }
            // Leases whose literal prefix is an ancestor of, or equal to, this glob's, and those under it.
            let segs: Vec<&str> = lp.split('/').collect();
            for i in 1..=segs.len() {
                ids.extend(ids_in(self.core, "lease_exclusive", &[text(&segs[..i].join("/"))], 1));
            }
            // Those under it: one run of text keys from `under`, read a few rows at a time, so the read ends at the
            // run's end and not the table's (`usize::MAX` is SQLite's `LIMIT -1`, every row past `under`).
            let under = format!("{lp}/");
            // Each batch after the first starts at the last key read, which it reads again and skips.
            let mut from = vec![text(&under)];
            'under: loop {
                let batch = self.core.scan("lease_exclusive", &from, 8);
                let full = batch.len() == 8;
                for (k, _) in batch {
                    if k == from {
                        continue;
                    }
                    match k.first() {
                        Some(Val::Text(t)) if t.starts_with(&under) => ids.extend(id_of(k.get(1))),
                        _ => break 'under,
                    }
                    from = k;
                }
                if !full {
                    break;
                }
            }
        }
        ids.sort();
        ids.dedup();
        let other = |l: &Lease| match &mine.session {
            Some(s) => l.holder.session.as_ref() != Some(s),
            None => l.holder.actor != mine.actor,
        };
        ids.into_iter()
            .filter_map(|id| lease_row(self.core, id))
            .filter(|r| self.holds(r))
            .map(|r| r.lease)
            .filter(|l| l.mode_at(self.at) == LeaseMode::Exclusive && other(l))
            .filter(|l| l.scope.paths.iter().any(|p| globs.iter().any(|g| glob_overlap(p.as_str(), g))))
            .collect()
    }
}

// ---- the lease commands' `decide` (`IssueOp::AcquireLease`, `RenewLease`, `ReleaseLease`) ----

/// `POST .../leases`, as the edge parsed it. `session` is the `X-Graftyard-Agent-Session` header or the body's
/// `session`, a bare name the calling App scopes, or a full task id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewLease {
    pub paths: Vec<Box<str>>,
    pub refs: Vec<Box<str>>,
    pub mode: LeaseMode,
    pub ttl_seconds: Option<u32>,
    pub intent: Box<str>,
    pub session: Option<Box<str>>,
}

/// Why a lease command was refused. Each maps to the API reference's status (`graftyard_api::leases`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaseErr {
    /// 403: the caller has no push access.
    NoPush,
    /// 422: an App token names no session of its own.
    NoSession,
    /// 422 `invalid` on a field: a bad glob or ref pattern, too many, a TTL out of range, a long intent.
    Invalid(&'static str),
    /// 422: an `Agent` named a ref outside its task's prefix or its `confine`.
    RefScope(Box<str>),
    /// 422: an Exclusive lease without a live task of the holder.
    NoLiveTask,
    /// 422: an Exclusive glob without a literal first segment.
    Literal(Box<str>),
    /// 422: the task already holds a live Exclusive lease.
    OnePerTask(LeaseId),
    /// 422: LEASES_PER_HOLDER, LEASES_PER_APP, or LEASES_PER_REPO.
    Cap(&'static str),
    /// 409: another holder's live Exclusive leases overlap this Exclusive acquire.
    Conflict(Vec<Lease>),
    /// 404: no such lease, or it was released or expired.
    NotFound,
    /// 403: only the holder renews, and the holder or a repository admin releases.
    NotHolder,
}

/// What a lease command reads beside the view: the install's `AgentApp` rows, the caller's push and admin rights, and
/// the App behind a user-to-server token.
pub struct LeaseCtx<'a> {
    pub view: LeaseView<'a>,
    pub install: Option<&'a dyn AuthzRead>,
    pub push: bool,
    pub admin: bool,
    pub token_app: Option<AppId>,
}

type Decided = Result<(SmallVec<[crate::repo::RepoEvent; 4]>, crate::issue_ops::IssueOut), LeaseErr>;

/// The task `named` names for this caller, as the holder's `session`: an App's token claim, or a task of the same App
/// (its External task); a user-to-server token's task of its App; a plain user token's task that user requested.
pub fn holder_session(
    core: &dyn Read,
    p: &Principal,
    token_app: Option<AppId>,
    named: Option<&str>,
) -> Option<SessionId> {
    match p.actor {
        Actor::App { app, .. } => {
            if let Some(c) = crate::task::claim(p) {
                return Some(c.clone());
            }
            let s = SessionId::parse(named?, Some(app))?;
            crate::task::task_core(core, &s).filter(|t| t.app == app).map(|_| s)
        }
        Actor::User(u) => {
            let s = SessionId::parse(named?, token_app)?;
            let t = crate::task::task_core(core, &s)?;
            let mine = match token_app {
                Some(a) => t.app == a,
                None => t.requester == Some(u),
            };
            mine.then_some(s)
        }
        Actor::Anonymous => None,
    }
}

/// Whether ref pattern `r` stays inside `scope`: a literal ref must match it, and a pattern's literal prefix with any
/// segment after it must.
fn inside(scope: &RefPattern, r: &RefPattern) -> bool {
    if !r.as_str().contains('*') {
        return scope.matches(r.as_str().as_bytes());
    }
    scope.matches(format!("{}/x", literal_prefix(r.as_str())).as_bytes())
}

fn ttl(seconds: Option<u32>) -> Result<Duration, LeaseErr> {
    let t = seconds.map_or(LEASE_TTL, |s| Duration::from_secs(u64::from(s)));
    if t < TTL_MIN || t > TTL_MAX {
        return Err(LeaseErr::Invalid("ttl_seconds"));
    }
    Ok(t)
}

/// Parses a request's globs and patterns into a scope.
pub fn scope_of(paths: &[Box<str>], refs: &[Box<str>]) -> Result<LeaseScope, LeaseErr> {
    if paths.is_empty() || paths.len() > PATHS_PER_LEASE {
        return Err(LeaseErr::Invalid("paths"));
    }
    if refs.len() > REFS_PER_LEASE {
        return Err(LeaseErr::Invalid("refs"));
    }
    let paths = paths.iter().map(|p| PathGlob::parse(p)).collect::<Result<_, _>>();
    let refs = refs.iter().map(|r| RefPattern::parse(r)).collect::<Result<_, _>>();
    Ok(LeaseScope {
        paths: paths.map_err(|_| LeaseErr::Invalid("paths"))?,
        refs: refs.map_err(|_| LeaseErr::Invalid("refs"))?,
    })
}

/// `decide` for the three lease commands, on the core tier with the entry's earlier events on top.
pub fn decide(op: &crate::issue_ops::IssueOp<impl Clone>, p: &Principal, ctx: &LeaseCtx<'_>) -> Decided {
    use crate::issue_ops::{IssueOp, IssueOut};
    use crate::repo::RepoEvent;
    let v = &ctx.view;
    let actor = ActorId::of(p).ok_or(LeaseErr::NoPush)?;
    match op {
        IssueOp::AcquireLease(n) => {
            if !ctx.push {
                return Err(LeaseErr::NoPush);
            }
            let scope = scope_of(&n.paths, &n.refs)?;
            if n.intent.len() > INTENT_MAX {
                return Err(LeaseErr::Invalid("intent"));
            }
            let ttl = ttl(n.ttl_seconds)?;
            let session = holder_session(v.core, p, ctx.token_app, n.session.as_deref());
            let app = match p.actor {
                Actor::App { app, .. } => Some(app),
                _ => ctx.token_app,
            };
            if matches!(p.actor, Actor::App { .. }) && session.is_none() {
                return Err(LeaseErr::NoSession);
            }
            let holder = LeaseHolder { actor, app, session };
            // An `Agent` may name refs only inside its task's prefix or its `confine`.
            if let crate::agent::ActorKind::Agent(mut a) = crate::agent::actor_kind(p, ctx.install) {
                a.task = holder.session.clone().filter(|s| matches!(s, SessionId::Run(_)));
                let prefix = crate::agent::allowed_prefix(ctx.install, &a);
                if let Some(r) = scope.refs.iter().find(|r| prefix.as_ref().is_none_or(|s| !inside(s, r))) {
                    return Err(LeaseErr::RefScope(r.as_str().into()));
                }
            }
            if n.mode == LeaseMode::Exclusive {
                let live = holder
                    .session
                    .as_ref()
                    .and_then(|s| crate::task::task_core(v.core, s))
                    .is_some_and(|t| crate::task::effective(&t, v.owner, v.repo, v.at).is_live());
                if !live {
                    return Err(LeaseErr::NoLiveTask);
                }
                if let Some(g) = scope.paths.iter().find(|g| {
                    let first = g.as_str().split('/').next().unwrap_or("");
                    first.is_empty() || first.contains('*')
                }) {
                    return Err(LeaseErr::Literal(g.as_str().into()));
                }
                let s = holder.session.as_ref().expect("checked live");
                if let Some(own) = v.session_live(s).into_iter().find(|r| r.lease.mode_at(v.at) == LeaseMode::Exclusive)
                {
                    return Err(LeaseErr::OnePerTask(own.lease.id));
                }
            }
            if v.holder_live(&holder).len() >= usize::from(LEASES_PER_HOLDER) {
                return Err(LeaseErr::Cap("LEASES_PER_HOLDER"));
            }
            if app.is_some_and(|a| v.app_live(a) >= usize::from(LEASES_PER_APP)) {
                return Err(LeaseErr::Cap("LEASES_PER_APP"));
            }
            if v.live().len() >= usize::from(LEASES_PER_REPO) {
                return Err(LeaseErr::Cap("LEASES_PER_REPO"));
            }
            if n.mode == LeaseMode::Exclusive {
                let globs: Vec<&str> = scope.paths.iter().map(PathGlob::as_str).collect();
                let c = v.exclusive_conflicts(&globs, &holder);
                if !c.is_empty() {
                    return Err(LeaseErr::Conflict(c));
                }
            }
            let lease = Lease {
                id: next_id(v.core, v.at),
                holder,
                scope,
                mode: n.mode,
                intent: n.intent.clone(),
                granted: v.at,
                expires: Timestamp(v.at.0 + ms(ttl)),
            };
            let id = lease.id;
            Ok((smallvec::smallvec![RepoEvent::LeaseAcquire { lease, plan: None }], IssueOut::Lease(id)))
        }
        IssueOp::RenewLease { id, ttl_seconds } => {
            let r = lease_row(v.core, *id).filter(|r| v.holds(r)).ok_or(LeaseErr::NotFound)?;
            if r.lease.holder.actor != actor {
                return Err(LeaseErr::NotHolder);
            }
            let expires = Timestamp(v.at.0 + ms(ttl(*ttl_seconds)?));
            Ok((smallvec::smallvec![RepoEvent::LeaseRenew { id: *id, expires }], IssueOut::Lease(*id)))
        }
        IssueOp::ReleaseLease { id } => {
            let r = lease_row(v.core, *id).filter(|r| v.holds(r)).ok_or(LeaseErr::NotFound)?;
            if r.lease.holder.actor != actor && !ctx.admin {
                return Err(LeaseErr::NotHolder);
            }
            let ev = RepoEvent::LeaseRelease { ids: smallvec::smallvec![*id], by: actor };
            Ok((smallvec::smallvec![ev], IssueOut::Done))
        }
        _ => Err(LeaseErr::Invalid("not a lease command")),
    }
}

// ---- `graftyard-plan` blocks (`design/AGENT-LAYER.md`, "API reference") ----

/// A parsed block: what its Shared lease covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanBlock {
    pub scope: LeaseScope,
    pub intent: Box<str>,
}

/// The first fenced block whose info string is exactly `graftyard-plan`, as CommonMark fences it (three or more
/// backticks or tildes, closed by a fence of the same character at least as long, or by the end of the body), parsed
/// by the grammar: `key: value` lines, blank and `#` lines skipped, `paths` required with 1 to 32 comma-separated
/// globs, `refs` at most 2, `intent` at most 280 bytes, unknown keys ignored. `None` when the body has no such block;
/// `Some(None)` when its block is invalid: a repeated key, a line that is not `key: value`, a bad glob, or over
/// PLAN_BLOCK_MAX. Later blocks are never read.
pub fn find_plan(body: &str) -> Option<Option<PlanBlock>> {
    let lines: Vec<&str> = body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let fence = |l: &str| -> Option<(char, usize, String)> {
        let t = l.trim_start_matches(' ');
        if l.len() - t.len() > 3 {
            return None;
        }
        let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let n = t.chars().take_while(|x| *x == c).count();
        (n >= 3).then(|| (c, n, t[n..].trim().to_owned()))
    };
    let mut i = 0;
    while i < lines.len() {
        let Some((c, n, info)) = fence(lines[i]) else {
            i += 1;
            continue;
        };
        let end = (i + 1..lines.len())
            .find(|j| fence(lines[*j]).is_some_and(|(c2, n2, rest)| c2 == c && n2 >= n && rest.is_empty()))
            .unwrap_or(lines.len());
        if info == "graftyard-plan" {
            return Some(plan_lines(&lines[i + 1..end]));
        }
        i = end + 1;
    }
    None
}

/// `find_plan`'s valid block, if the body has one.
pub fn parse_plan(body: &str) -> Option<PlanBlock> {
    find_plan(body).flatten()
}

fn plan_lines(lines: &[&str]) -> Option<PlanBlock> {
    if lines.iter().map(|l| l.len() + 1).sum::<usize>() > PLAN_BLOCK_MAX {
        return None;
    }
    let mut seen: Vec<&str> = Vec::new();
    let (mut paths, mut refs, mut intent) = (None, None, None);
    for l in lines {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (k, v) = l.split_once(':')?;
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || seen.contains(&k) {
            return None;
        }
        seen.push(k);
        let list = || v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(Box::from).collect::<Vec<Box<str>>>();
        match k {
            "paths" => paths = Some(list()),
            "refs" => refs = Some(list()),
            "intent" => intent = Some(v),
            _ => {}
        }
    }
    let scope = scope_of(&paths?, &refs.unwrap_or_default()).ok()?;
    let intent = intent.unwrap_or("");
    (intent.len() <= INTENT_MAX).then(|| PlanBlock { scope, intent: intent.into() })
}

/// What a draft pull request's body write does to its editor's plan lease, in the write's own command: a valid block
/// replaces the editor's live plan lease on `n` with a new Shared one, a body with no block releases it, and an invalid
/// block changes nothing. The editor needs push access, and a new lease must fit LEASES_PER_HOLDER and
/// PLAN_LEASES_PER_REPO; otherwise the body is still written and no lease is.
pub fn plan_events(
    v: &LeaseView<'_>,
    p: &Principal,
    token_app: Option<AppId>,
    n: IssueNumber,
    body: &str,
) -> SmallVec<[crate::repo::RepoEvent; 2]> {
    use crate::repo::RepoEvent;
    let mut out = SmallVec::new();
    let Some(actor) = ActorId::of(p) else { return out };
    if !v.can_push(actor, v.at) {
        return out;
    }
    let app = match p.actor {
        Actor::App { app, .. } => Some(app),
        _ => token_app,
    };
    let holder = LeaseHolder { actor, app, session: crate::task::claim(p).cloned() };
    let block = match find_plan(body) {
        // An invalid block makes no lease and changes nothing else.
        Some(None) => return out,
        Some(Some(b)) => Some(b),
        None => None,
    };
    let old: Vec<LeaseId> = ids_in(v.core, "lease_plan", &[int(i64::from(n.0)), text(&holder.key())], 2)
        .into_iter()
        .filter(|id| lease_row(v.core, *id).is_some_and(|r| v.holds(&r)))
        .collect();
    if !old.is_empty() {
        out.push(RepoEvent::LeaseRelease { ids: old.iter().copied().collect(), by: actor });
    }
    let Some(b) = block else { return out };
    let held = v.holder_live(&holder).iter().filter(|r| !old.contains(&r.lease.id)).count();
    let plans = v.plan_live().iter().filter(|r| !old.contains(&r.lease.id)).count();
    if held >= usize::from(LEASES_PER_HOLDER) || plans >= usize::from(PLAN_LEASES_PER_REPO) {
        return out;
    }
    let lease = Lease {
        id: next_id(v.core, v.at),
        holder,
        scope: b.scope,
        mode: LeaseMode::Shared,
        intent: b.intent,
        granted: v.at,
        expires: Timestamp(v.at.0 + ms(PLAN_TTL)),
    };
    out.push(RepoEvent::LeaseAcquire { lease, plan: Some(n) });
    out
}

/// Whether the edge sends an issue command as 32.14 once that format is enabled, because its entry may write lease
/// events: a lease command; a draft pull request opened with a `graftyard-plan` block; a body edit of a pull request
/// that has a plan lease or now names a block; and a task command on a task with lease rows (its cancel or finish
/// releases them). Every other command keeps its format and its bytes.
pub fn marks<B>(op: &crate::issue_ops::IssueOp<B>, core: &(impl Read + ?Sized)) -> bool {
    use crate::issue_ops::IssueOp;
    match op {
        IssueOp::AcquireLease(_) | IssueOp::RenewLease { .. } | IssueOp::ReleaseLease { .. } => true,
        IssueOp::OpenPr { draft: true, body, .. } => body.contains("graftyard-plan"),
        IssueOp::Change { n, change: crate::issues::IssueChange::Body(b) } => {
            b.contains("graftyard-plan") || !ids_in(core, "lease_plan", &[int(i64::from(n.0))], 2).is_empty()
        }
        IssueOp::UpdateTask { id, .. } | IssueOp::CancelTask { id } => {
            !ids_in(core, "lease_by_session", &[text(&id.text())], 1).is_empty()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(inner: &str) -> String {
        format!("Intro\n\n```graftyard-plan\n{inner}\n```\n\nMore")
    }

    #[test]
    fn plan_blocks_follow_the_grammar() {
        let b =
            parse_plan(&block("paths: pkg/auth/**, docs/auth.md\n# a comment\n\nintent: Rework\nother: x")).unwrap();
        assert_eq!(b.scope.paths.iter().map(|p| p.as_str()).collect::<Vec<_>>(), ["pkg/auth/**", "docs/auth.md"]);
        assert_eq!(&*b.intent, "Rework");
        assert!(parse_plan(&block("paths: a\npaths: b")).is_none(), "a repeated key");
        assert!(parse_plan(&block("paths: /abs")).is_none(), "a bad glob");
        assert!(parse_plan(&block("intent: x")).is_none(), "paths is required");
        assert!(parse_plan(&block("paths: a\nrefs: refs/a, refs/b, refs/c")).is_none(), "at most 2 refs");
        let many = (0..33).map(|i| format!("p{i}")).collect::<Vec<_>>().join(",");
        assert!(parse_plan(&block(&format!("paths: {many}"))).is_none(), "at most 32 paths");
        assert!(parse_plan(&block(&format!("paths: a\nintent: {}", "x".repeat(281)))).is_none());
        let big = format!("paths: a\n{}", "# pad\n".repeat(800));
        assert!(parse_plan(&block(&big)).is_none(), "over 4 KiB");
        // Only the first `graftyard-plan` block counts, and an info string must be exactly the name.
        let two = format!("{}\n{}", block("paths: a\npaths: b"), block("paths: c"));
        assert!(parse_plan(&two).is_none());
        assert!(parse_plan("```graftyard-plan extra\npaths: a\n```").is_none());
        assert!(parse_plan("~~~~graftyard-plan\npaths: a\n~~~~").is_some());
        assert!(parse_plan("```\npaths: a\n```").is_none());
    }

    #[test]
    fn exclusive_reads_shared_after_exclusive_max() {
        let l = Lease {
            id: LeaseId(Ulid(1)),
            holder: LeaseHolder {
                actor: ActorId::User(UserId(graftyard_types::AccountId(graftyard_types::DbId(1)))),
                app: None,
                session: None,
            },
            scope: scope_of(&["a/**".into()], &[]).unwrap(),
            mode: LeaseMode::Exclusive,
            intent: "".into(),
            granted: Timestamp(0),
            expires: Timestamp(u64::MAX),
        };
        assert_eq!(l.mode_at(Timestamp(ms(EXCLUSIVE_MAX) - 1)), LeaseMode::Exclusive);
        assert_eq!(l.mode_at(Timestamp(ms(EXCLUSIVE_MAX))), LeaseMode::Shared);
    }
}
