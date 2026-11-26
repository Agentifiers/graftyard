//! External tasks and the task rows the read endpoints serve (`design/DESIGN.md`, Shape, "Agent load and agent
//! safety"; `design/AGENT-LAYER.md`, Shape, "Agent-layer entries" and "Tasks and the activity feed";
//! `build/BUILD-PLAN.md`, AM1-WP3).
//!
//! An `AgentTask` is a row on its repository's group, written by `TaskCreated` and `TaskUpdated` (format 32.12,
//! `crate::repo::REPO_FORMAT_TASKS`). Its core part is what pushes and minting read: kind, stored state, prefix,
//! requester, App, installation, and when it last changed. The rest is metadata.
//!
//! Two states are derived, never swept. A task whose `installation` the owner's `Authz` read no longer finds for its
//! App reads as `Cancelled`, and one with no update for `TASK_IDLE` reads as `TimedOut` (`effective`). `decide` and
//! every read apply the same rule, and the task's next command records the derived state.

use std::time::Duration;

use graftyard_fold::{Read, Rows, Val};
use graftyard_identity::{
    Actor, AgentKind, AppId, AuthzRead, Credential, InstallationId, Principal, RepoScope, SessionId, UserId,
};
use graftyard_types::{Index, RepoId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::agent::RefPattern;
use crate::issues::IssueNumber;
use crate::tiers::{get_rec, put_rec};

/// Open tasks per requester, or per App for tasks without one [guess].
pub const TASKS_OPEN_PER_REQUESTER: u16 = 100;
pub const TASKS_OPEN_PER_REPO: u16 = 2_000; // [guess]
pub const TASK_IDLE: Duration = Duration::from_secs(86_400); // [guess]
/// A prompt is at most 64 KiB, stored on the row.
pub const PROMPT_MAX: usize = 64 << 10;
/// A summary's cap. The design names none; the prompt's [guess].
pub const SUMMARY_MAX: usize = 64 << 10;
/// The host-owned prefix a Graftyard-run task's refs live under: `refs/heads/graftyard/<uuid>/`.
pub const RUN_PREFIX: &str = "refs/heads/graftyard/";
/// The slug of the first-party App that pushes for Graftyard-run tasks. Its token may carry another App's task.
pub const WORKSPACE_SLUG: &str = "graftyard-workspace";
/// The slug of the controller's App, the `app` of every Graftyard-run task.
pub const AGENT_SLUG: &str = "graftyard-agent";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskKind {
    GraftyardRun,
    External,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::GraftyardRun => "graftyard_run",
            TaskKind::External => "external",
        }
    }
}

/// GitHub's agent task states. Completed, Failed, TimedOut, and Cancelled are final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskApiState {
    Queued,
    InProgress,
    Completed,
    Failed,
    Idle,
    WaitingForUser,
    TimedOut,
    Cancelled,
}

impl TaskApiState {
    pub const ALL: [TaskApiState; 8] = [
        TaskApiState::Queued,
        TaskApiState::InProgress,
        TaskApiState::Completed,
        TaskApiState::Failed,
        TaskApiState::Idle,
        TaskApiState::WaitingForUser,
        TaskApiState::TimedOut,
        TaskApiState::Cancelled,
    ];

    pub fn is_final(self) -> bool {
        matches!(
            self,
            TaskApiState::Completed | TaskApiState::Failed | TaskApiState::TimedOut | TaskApiState::Cancelled
        )
    }

    /// Live for pushes and minting: not final, and not waiting for its user, which is how a paused task reads.
    pub fn is_live(self) -> bool {
        !self.is_final() && self != TaskApiState::WaitingForUser
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskApiState::Queued => "queued",
            TaskApiState::InProgress => "in_progress",
            TaskApiState::Completed => "completed",
            TaskApiState::Failed => "failed",
            TaskApiState::Idle => "idle",
            TaskApiState::WaitingForUser => "waiting_for_user",
            TaskApiState::TimedOut => "timed_out",
            TaskApiState::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<TaskApiState> {
        TaskApiState::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// Who sent a task command: SHARED's `Actor` cut to the ones that may (a user, or an App's installation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskActor {
    User(UserId),
    App { app: AppId, installation: InstallationId },
}

impl TaskActor {
    pub fn of(p: &Principal) -> Option<TaskActor> {
        match p.actor {
            Actor::User(u) => Some(TaskActor::User(u)),
            Actor::App { app, installation } => Some(TaskActor::App { app, installation }),
            Actor::Anonymous => None,
        }
    }
}

/// The task API's row. `app` is the controller's App for a `GraftyardRun` task and the creating App for an `External`
/// one, and `creator` is whoever sent `POST`. `prefix` is derived from `session` for a `GraftyardRun` task and none for
/// an `External` one. `snapshot` is the create index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTask {
    pub session: SessionId,
    pub kind: TaskKind,
    pub state: TaskApiState,
    pub prefix: Option<RefPattern>,
    pub requester: Option<UserId>,
    pub app: AppId,
    pub installation: InstallationId,
    pub creator: TaskActor,
    pub issue: Option<IssueNumber>,
    pub snapshot: Index,
    pub prompt: Option<Box<str>>,
    pub pr: Option<IssueNumber>,
    pub summary: Box<str>,
}

/// The core `agent_task` row: what pushes, minting, and the caps read. `installation` and `updated` are here because
/// `decide` derives `Cancelled` and `TimedOut` from them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCore {
    pub kind: TaskKind,
    pub state: TaskApiState,
    pub prefix: Option<RefPattern>,
    pub requester: Option<UserId>,
    pub app: AppId,
    pub installation: InstallationId,
    /// The time of the entry that created the task: an installation whose `not_before` is later no longer covers it,
    /// as it no longer covers a token minted before then.
    pub created: Timestamp,
    /// The time of the entry that last wrote the task.
    pub updated: Timestamp,
}

/// The metadata `agent_task_meta` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMeta {
    pub creator: TaskActor,
    pub issue: Option<IssueNumber>,
    pub snapshot: Index,
    pub prompt: Option<Box<str>>,
    pub pr: Option<IssueNumber>,
    pub summary: Box<str>,
    pub created: Timestamp,
}

/// The key of a task's rows: its string form.
pub fn task_key(s: &SessionId) -> Vec<Val> {
    vec![Val::Text(s.text().into())]
}

pub fn task_core(r: &(impl Read + ?Sized), s: &SessionId) -> Option<TaskCore> {
    get_rec(r, "agent_task", &task_key(s))
}

pub fn task_meta(r: &(impl Read + ?Sized), s: &SessionId) -> Option<TaskMeta> {
    get_rec(r, "agent_task_meta", &task_key(s))
}

/// Every task id on the repository, in key order from `after` (exclusive), at most `limit`.
pub fn task_ids(r: &(impl Read + ?Sized), after: Option<&SessionId>, limit: usize) -> Vec<SessionId> {
    let from = after.map(task_key).unwrap_or_default();
    let mut out = Vec::new();
    let mut cursor = from;
    loop {
        let batch = r.scan("agent_task", &cursor, 256);
        let full = batch.len() == 256;
        for (k, _) in &batch {
            if Some(k) == after.map(task_key).as_ref() {
                continue;
            }
            if let Some(Val::Text(t)) = k.first()
                && let Some(id) = SessionId::parse(t, None)
            {
                out.push(id);
                if out.len() == limit {
                    return out;
                }
            }
        }
        match batch.last() {
            Some((k, _)) if full => {
                if *k == cursor {
                    return out;
                }
                cursor = k.clone();
            }
            _ => return out,
        }
    }
}

/// The tasks whose stored state is not final, which the caps count after `effective`.
fn open_ids(r: &(impl Read + ?Sized)) -> Vec<SessionId> {
    r.prefix("agent_task_open", &[])
        .into_iter()
        .filter_map(|(k, _)| match k.first() {
            Some(Val::Text(t)) => SessionId::parse(t, None),
            _ => None,
        })
        .collect()
}

/// Whether the owner's `Authz` read still finds the task's installation, for its App, reaching `repo`, and not reset
/// since the task was created: the check an installation token's `app_permission` makes, with the task's create time
/// in place of the token's `iat`. Uninstalling or suspending the App, narrowing it, or taking `repo` out of its
/// selection fails it, as it fails the App's tokens and releases its leases, and a reinstall does not revive it.
pub fn installed(t: &TaskCore, owner: &dyn AuthzRead, repo: RepoId) -> bool {
    let Some(state) = owner.installation(t.installation) else { return false };
    state.row.as_ref().is_some_and(|r| {
        r.app == t.app
            && t.created >= state.not_before
            && match &r.repos {
                RepoScope::All => true,
                RepoScope::Selected(ids) => ids.contains(&repo),
            }
    })
}

/// The state a task of `repo` reads as at `at`: its stored state when final; otherwise `Cancelled` when its
/// installation no longer reaches `repo`, `TimedOut` after TASK_IDLE with no update, and its stored state else.
pub fn effective(t: &TaskCore, owner: &dyn AuthzRead, repo: RepoId, at: Timestamp) -> TaskApiState {
    if t.state.is_final() {
        t.state
    } else if !installed(t, owner, repo) {
        TaskApiState::Cancelled
    } else if at.0.saturating_sub(t.updated.0) >= TASK_IDLE.as_millis() as u64 {
        TaskApiState::TimedOut
    } else {
        t.state
    }
}

/// The open tasks that count against a new task's caps: (for this requester, or this App when there is none; in the
/// repository).
pub fn open_counts(
    core: &(impl Read + ?Sized),
    owner: &dyn AuthzRead,
    repo: RepoId,
    at: Timestamp,
    requester: Option<UserId>,
    app: AppId,
) -> (usize, usize) {
    let (mut mine, mut all) = (0, 0);
    for id in open_ids(core) {
        let Some(t) = task_core(core, &id) else { continue };
        if effective(&t, owner, repo, at).is_final() {
            continue;
        }
        all += 1;
        let same = match requester {
            Some(u) => t.requester == Some(u),
            None => t.requester.is_none() && t.app == app,
        };
        mine += usize::from(same);
    }
    (mine, all)
}

/// A Graftyard-run task's stored `prefix`, `refs/heads/graftyard/<uuid>/**`, as the design writes it.
pub fn run_prefix(s: &SessionId) -> Option<RefPattern> {
    match s {
        SessionId::Run(_) => RefPattern::parse(&format!("{RUN_PREFIX}{}/**", s.text())).ok(),
        SessionId::External { .. } => None,
    }
}

/// The pattern a Graftyard-run task's refs must match: `refs/heads/graftyard/<uuid>/*/**`. In the glob dialect `**`
/// matches zero segments, so the stored `.../**` alone would also match `refs/heads/graftyard/<uuid>` itself, and the
/// design says a ref needs at least one segment after the id.
pub fn run_refs(s: &SessionId) -> Option<RefPattern> {
    match s {
        SessionId::Run(_) => RefPattern::parse(&format!("{RUN_PREFIX}{}/*/**", s.text())).ok(),
        SessionId::External { .. } => None,
    }
}

/// Whether `app` is the install's `graftyard-workspace`: a Graftyard-run `AgentApp` the install registered under
/// that slug.
pub fn is_workspace(install: Option<&dyn AuthzRead>, app: AppId) -> bool {
    install.is_some_and(|i| {
        i.agent_app(app).is_some_and(|a| a.kind == AgentKind::GraftyardRun)
            && i.app(app).is_some_and(|r| r.slug.as_str() == WORKSPACE_SLUG)
    })
}

/// The installation token's `task` claim, if any.
pub fn claim(p: &Principal) -> Option<&SessionId> {
    match &p.credential {
        Credential::Stateless(c) => c.task.as_ref(),
        _ => None,
    }
}

/// The live Graftyard-run task a token's claim binds `app` to on this repository: the task exists here, is
/// `GraftyardRun`, reads live, and belongs to `app` or `app` is the install's `graftyard-workspace`.
pub fn live_run_task(
    core: &(impl Read + ?Sized),
    owner: &dyn AuthzRead,
    install: Option<&dyn AuthzRead>,
    repo: RepoId,
    at: Timestamp,
    app: AppId,
    session: &SessionId,
) -> Option<TaskCore> {
    let t = task_core(core, session)?;
    let mine = t.app == app || is_workspace(install, app);
    (t.kind == TaskKind::GraftyardRun && mine && effective(&t, owner, repo, at).is_live()).then_some(t)
}

/// Of the sessions a push's provenance names, those `decide` may link for an App pusher: tasks of that App on this
/// repository, live or final, and the task its token's claim names.
pub fn own_sessions(
    core: &(impl Read + ?Sized),
    app: AppId,
    claimed: Option<&SessionId>,
    named: impl Iterator<Item = SessionId>,
) -> Vec<SessionId> {
    let mut out: Vec<SessionId> = Vec::new();
    for id in named {
        let mine = Some(&id) == claimed || task_core(core, &id).is_some_and(|t| t.app == app);
        if mine && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Why a token request's `graftyard_task` was refused: GitHub's 422.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TaskTokenError {
    #[error("graftyard_task needs repositories that name the task's repository")]
    NeedsRepositories,
    #[error("graftyard_task names no task of this repository")]
    NoTask,
    #[error("graftyard_task names a task that is not a live graftyard_run task")]
    NotLive,
    #[error("graftyard_task names a task of another App")]
    OtherApp,
    #[error("a token with graftyard_task covers only the task's repository")]
    OtherRepository,
}

/// The minting check for `graftyard_task` (`design/SHARED.md`, "Identity and tokens"), on the task's repository's
/// fold and its owner's `Authz`, both read fresh: the token names repositories, every one is `repo` (workspace forks
/// are AM3-WP3's), and the task is a live Graftyard-run task of the requesting App or the App is the install's
/// `graftyard-workspace`. Returns the claim to sign.
#[allow(clippy::too_many_arguments)]
pub fn token_task(
    core: &(impl Read + ?Sized),
    owner: &dyn AuthzRead,
    install: Option<&dyn AuthzRead>,
    at: Timestamp,
    app: AppId,
    repo: RepoId,
    repos: &RepoScope,
    session: &SessionId,
) -> Result<SessionId, TaskTokenError> {
    let RepoScope::Selected(ids) = repos else { return Err(TaskTokenError::NeedsRepositories) };
    if ids.is_empty() {
        return Err(TaskTokenError::NeedsRepositories);
    }
    if ids.iter().any(|r| *r != repo) {
        return Err(TaskTokenError::OtherRepository);
    }
    let t = task_core(core, session).ok_or(TaskTokenError::NoTask)?;
    if t.kind != TaskKind::GraftyardRun || !effective(&t, owner, repo, at).is_live() {
        return Err(TaskTokenError::NotLive);
    }
    if t.app != app && !is_workspace(install, app) {
        return Err(TaskTokenError::OtherApp);
    }
    Ok(session.clone())
}

// ---- the task commands' `decide` (`IssueOp::CreateTask`, `UpdateTask`, `CancelTask`) ----

/// What both creates check once the creator passed: the session is new, the summary and issue are valid, and neither
/// open-task cap is reached.
#[allow(clippy::too_many_arguments)]
fn admit(
    core: &dyn Read,
    ctx: &TaskCtx<'_>,
    owner: &dyn AuthzRead,
    session: &SessionId,
    requester: Option<UserId>,
    app: AppId,
    summary: &str,
    issue: Option<IssueNumber>,
) -> Result<(), TaskErr> {
    if task_core(core, session).is_some() {
        return Err(TaskErr::SessionExists);
    }
    if summary.len() > SUMMARY_MAX {
        return Err(TaskErr::Invalid("summary"));
    }
    if let Some(i) = issue
        && (i.0 == 0 || i.0 > get_rec::<u32>(core, "counter", &[]).unwrap_or(0))
    {
        return Err(TaskErr::Invalid("issue_number"));
    }
    let (mine, all) = open_counts(core, owner, ctx.repo, ctx.at, requester, app);
    if all >= usize::from(TASKS_OPEN_PER_REPO) {
        return Err(TaskErr::Cap { repo: true });
    }
    if mine >= usize::from(TASKS_OPEN_PER_REQUESTER) {
        return Err(TaskErr::Cap { repo: false });
    }
    Ok(())
}

/// `POST .../tasks` with `kind: external`: what the edge parsed from the body. `session` is the bare name or
/// `<app_id>.<name>`, which `decide` scopes to the calling App; `requester` is the login the edge resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub session: Box<str>,
    pub requester: Option<UserId>,
    pub issue: Option<IssueNumber>,
    pub summary: Box<str>,
}

/// `POST .../tasks` without `kind` (AM3-WP3, format 32.28): a Graftyard-run task on an issue, a prompt, or both. The
/// host mints its session from the request (`run_session`), so the body names none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewRun {
    pub issue: Option<IssueNumber>,
    pub prompt: Option<Box<str>>,
    pub summary: Box<str>,
}

/// A Graftyard-run task's id, minted by the host from the create's request id: the request's 128 bits as a UUID of
/// version 8 (RFC 9562's custom layout). The same request always mints the same id, and a duplicate of it replays the
/// stored outcome before `decide` runs.
pub fn run_session(request: graftyard_types::RequestId) -> SessionId {
    let u = request.0.0;
    let u = (u & !(0xf << 76)) | (0x8 << 76);
    SessionId::Run((u & !(0x3 << 62)) | (0x2 << 62))
}

/// The install's `graftyard-agent` and its live installation on the repository's owner, when `graftyard-workspace` is
/// installed there too: the App and installation a Graftyard-run task records, from the install's `AgentApp` rows and
/// the owner's installations. `None` when either App is not registered as `GraftyardRun`, or not installed on `repo`.
pub fn run_apps(
    install: &graftyard_identity::AuthzFold,
    owner: &graftyard_identity::AuthzFold,
    repo: RepoId,
) -> Option<(AppId, InstallationId)> {
    let run_app = |slug: &str| {
        install.more.more.agent_apps.values().find_map(|r| {
            (r.kind == AgentKind::GraftyardRun && install.app(r.app).is_some_and(|a| a.slug.as_str() == slug))
                .then_some(r.app)
        })
    };
    let installed = |app: AppId| {
        owner.more.installations.iter().find_map(|(id, s)| {
            s.row
                .as_ref()
                .is_some_and(|r| {
                    r.app == app
                        && match &r.repos {
                            RepoScope::All => true,
                            RepoScope::Selected(ids) => ids.contains(&repo),
                        }
                })
                .then_some(*id)
        })
    };
    installed(run_app(WORKSPACE_SLUG)?)?;
    let agent = run_app(AGENT_SLUG)?;
    Some((agent, installed(agent)?))
}

/// Why a task command was refused. Each maps to the API reference's status (`graftyard_api::tasks`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskErr {
    /// 403: this credential may not create an external task (not an `AgentApp`'s installation token).
    NotAgentApp,
    /// 403: not the task's App (`PATCH`), or not its requester, a repository admin, or its App (cancel).
    NotYours,
    /// 404.
    NoTask,
    /// 422 `invalid` on `session`.
    BadSession,
    /// 422 `already_exists` on `session`.
    SessionExists,
    /// 422: the requester's (or App's) open tasks, or the repository's, are at their cap.
    Cap { repo: bool },
    /// 422 `invalid` on a field.
    Invalid(&'static str),
    /// 422: `PATCH` may never set `cancelled` or `timed_out`.
    HostState,
    /// 409: the task is final (or, for a Graftyard-run task, paused), with the state it reads as.
    Final(TaskApiState),
    // ---- AM3-WP3, a Graftyard-run create ----
    /// 403: not a human user credential that may write the repository's contents (an installation token, an
    /// `AgentApp`'s user-to-server token, a reader, or a token without `contents: write`).
    NotWriter,
    /// 422: the org's `OrgPolicy.agents_enabled` is off, its default.
    AgentsOff,
    /// 422: the org's `agents_workspace` is `Fork`, which is not built yet (in-repo mode only, AM3-WP3's minimum).
    ForkMode,
    /// 422: the install's `graftyard-agent` or `graftyard-workspace` is not registered as `GraftyardRun`, or not
    /// installed on the repository.
    NoRunApps,
}

/// The entry's time and the folds a task command reads: the owner's `Authz`, for permissions and derived states, and
/// the install's, for `AgentApp` rows.
pub struct TaskCtx<'a> {
    pub repo: RepoId,
    pub at: Timestamp,
    pub applied: Index,
    pub owner: Option<&'a dyn AuthzRead>,
    pub install: Option<&'a dyn AuthzRead>,
    /// The caller's permission on the repository, as `issue_ops::permission` or `authorize_app` gives it.
    pub perm: Option<graftyard_identity::RepoPermission>,
    /// Format 32.14: the command may write `LeaseRelease`, so a command that makes the task final releases its leases
    /// in the same command (AM1-WP4). Before it they count as released anyway, because `holds` reads the task final.
    pub leases: bool,
    /// The command's request id, which a Graftyard-run create mints its session from (`run_session`).
    pub request: graftyard_types::RequestId,
    /// A user's credential may write the repository's contents within its own limits, and is not an `AgentApp`'s
    /// user-to-server token: what a Graftyard-run create needs beyond `perm`.
    pub human_writer: bool,
    /// `run_apps`, read for a Graftyard-run create only.
    pub run: Option<(AppId, InstallationId)>,
    /// The creator's committed writes in the last `DEMOTE_WINDOW` at this entry, from the group's replicated window
    /// (`graftyard_raft::Scratch::set_actor_writes`): at DEMOTE_PER_MIN the actor is demoted, and a Graftyard-run
    /// create needs a human. Task commands are not counted in it.
    pub writes: u32,
}

type Decided = Result<(smallvec::SmallVec<[crate::repo::RepoEvent; 4]>, crate::issue_ops::IssueOut), TaskErr>;

/// The task's leases that a command making it final releases: every lease row naming its session, held or not.
fn release(ctx: &TaskCtx<'_>, core: &dyn Read, id: &SessionId, p: &Principal) -> Option<crate::repo::RepoEvent> {
    if !ctx.leases {
        return None;
    }
    let ids: smallvec::SmallVec<[crate::lease::LeaseId; 4]> =
        crate::lease::session_rows(core, id).into_iter().map(|r| r.lease.id).collect();
    let by = crate::lease::ActorId::of(p)?;
    (!ids.is_empty()).then_some(crate::repo::RepoEvent::LeaseRelease { ids, by })
}

fn updated(session: &SessionId, state: TaskApiState, actor: TaskActor) -> crate::repo::RepoEvent {
    crate::repo::RepoEvent::TaskUpdated { session: session.clone(), state: Some(state), pr: None, summary: None, actor }
}

/// A command on a task that reads final only by derivation (its installation gone, or TASK_IDLE passed) records that
/// state and changes nothing else; one already stored final changes nothing.
fn derived_final(
    ctx: &TaskCtx<'_>,
    core: &dyn Read,
    p: &Principal,
    id: &SessionId,
    t: &TaskCore,
    actor: TaskActor,
) -> Option<Decided> {
    let owner = ctx.owner?;
    let now = effective(t, owner, ctx.repo, ctx.at);
    if t.state.is_final() {
        return Some(Err(TaskErr::Final(t.state)));
    }
    if now.is_final() {
        let out = crate::issue_ops::IssueOut::TaskFinal(id.clone(), now);
        let mut evs = smallvec::smallvec![updated(id, now, actor)];
        evs.extend(release(ctx, core, id, p));
        return Some(Ok((evs, out)));
    }
    None
}

/// `decide` for the three task commands, on the core and metadata tiers with the entry's earlier events on top.
pub fn decide(
    op: &crate::issue_ops::IssueOp<impl Clone>,
    p: &Principal,
    core: &dyn Read,
    ctx: &TaskCtx<'_>,
) -> Decided {
    use crate::issue_ops::{IssueOp, IssueOut};
    use crate::repo::RepoEvent;
    use graftyard_identity::RepoPermission;
    let actor = TaskActor::of(p).ok_or(TaskErr::NotYours)?;
    let read = ctx.perm >= Some(RepoPermission::Read);
    match op {
        IssueOp::CreateTask(n) => {
            // Only an `AgentApp`'s installation token creates an external task (Graftyard-run creation is the
            // controller's, AM3-WP3).
            let TaskActor::App { app, installation } = actor else { return Err(TaskErr::NotAgentApp) };
            if crate::agent::agent_app(ctx.install, app).is_none() {
                return Err(TaskErr::NotAgentApp);
            }
            let session = match SessionId::parse(&n.session, Some(app)) {
                Some(s @ SessionId::External { app: a, .. }) if a == app => s,
                _ => return Err(TaskErr::BadSession),
            };
            let owner = ctx.owner.ok_or(TaskErr::NotYours)?;
            admit(core, ctx, owner, &session, n.requester, app, &n.summary, n.issue)?;
            let t = AgentTask {
                session: session.clone(),
                kind: TaskKind::External,
                state: TaskApiState::Queued,
                prefix: None,
                requester: n.requester,
                app,
                installation,
                creator: actor,
                issue: n.issue,
                snapshot: ctx.applied,
                prompt: None,
                pr: None,
                summary: n.summary.clone(),
            };
            Ok((smallvec::smallvec![RepoEvent::TaskCreated(t)], IssueOut::Task(session)))
        }
        IssueOp::CreateRun(n) => {
            // A human writer's create (`design/AGENT-LAYER.md`, Shape, "Tasks and the activity feed"). The controller's
            // exception for an issue assigned to its bot, and fork mode, are AM3-WP3's rest.
            let TaskActor::User(u) = actor else { return Err(TaskErr::NotWriter) };
            if !ctx.human_writer || ctx.perm < Some(RepoPermission::Write) {
                return Err(TaskErr::NotWriter);
            }
            // A demoted actor, past DEMOTE_PER_MIN at the create's own entry, is not a human for this rule.
            if ctx.writes >= graftyard_raft::admit::DEMOTE_PER_MIN {
                return Err(TaskErr::NotWriter);
            }
            let owner = ctx.owner.ok_or(TaskErr::NotYours)?;
            let policy = owner.policy();
            if !policy.agents_enabled {
                return Err(TaskErr::AgentsOff);
            }
            if policy.agents_workspace != graftyard_identity::WorkspaceMode::InRepo {
                return Err(TaskErr::ForkMode);
            }
            let (app, installation) = ctx.run.ok_or(TaskErr::NoRunApps)?;
            if n.issue.is_none() && n.prompt.is_none() {
                return Err(TaskErr::Invalid("prompt"));
            }
            if n.prompt.as_ref().is_some_and(|p| p.len() > PROMPT_MAX) {
                return Err(TaskErr::Invalid("prompt"));
            }
            let session = run_session(ctx.request);
            admit(core, ctx, owner, &session, Some(u), app, &n.summary, n.issue)?;
            let t = AgentTask {
                session: session.clone(),
                kind: TaskKind::GraftyardRun,
                state: TaskApiState::Queued,
                prefix: run_prefix(&session),
                requester: Some(u),
                app,
                installation,
                creator: actor,
                issue: n.issue,
                snapshot: ctx.applied,
                prompt: n.prompt.clone(),
                pr: None,
                summary: n.summary.clone(),
            };
            Ok((smallvec::smallvec![RepoEvent::TaskCreated(t)], IssueOut::Task(session)))
        }
        IssueOp::UpdateTask { id, state, pr, summary } => {
            let t = task_core(core, id).filter(|_| read).ok_or(TaskErr::NoTask)?;
            if !matches!(actor, TaskActor::App { app, .. } if app == t.app) {
                return Err(TaskErr::NotYours);
            }
            if matches!(state, Some(TaskApiState::Cancelled | TaskApiState::TimedOut)) {
                return Err(TaskErr::HostState);
            }
            if let Some(d) = derived_final(ctx, core, p, id, &t, actor) {
                return d;
            }
            // A Graftyard-run task the host paused may only be left paused or finished by its App.
            let to = state.unwrap_or(t.state);
            if t.kind == TaskKind::GraftyardRun
                && t.state == TaskApiState::WaitingForUser
                && !(to == TaskApiState::WaitingForUser || to.is_final())
            {
                return Err(TaskErr::Final(t.state));
            }
            if t.kind == TaskKind::GraftyardRun && *state == Some(TaskApiState::Idle) {
                return Err(TaskErr::Invalid("state"));
            }
            if pr.is_some_and(|n| crate::tiers::pull_head(core, n).is_none()) {
                return Err(TaskErr::Invalid("pr_number"));
            }
            if summary.as_ref().is_some_and(|x| x.len() > SUMMARY_MAX) {
                return Err(TaskErr::Invalid("summary"));
            }
            // An empty `PATCH` still records that the task is alive, against TASK_IDLE.
            let ev =
                RepoEvent::TaskUpdated { session: id.clone(), state: *state, pr: *pr, summary: summary.clone(), actor };
            let mut evs = smallvec::smallvec![ev];
            // Finishing a task releases its leases in the same command.
            if to.is_final() {
                evs.extend(release(ctx, core, id, p));
            }
            Ok((evs, IssueOut::Task(id.clone())))
        }
        IssueOp::CancelTask { id } => {
            let t = task_core(core, id).filter(|_| read).ok_or(TaskErr::NoTask)?;
            let may = match actor {
                TaskActor::App { app, .. } => app == t.app,
                TaskActor::User(u) => t.requester == Some(u) || ctx.perm >= Some(RepoPermission::Admin),
            };
            if !may {
                return Err(TaskErr::NotYours);
            }
            if let Some(d) = derived_final(ctx, core, p, id, &t, actor) {
                return d;
            }
            // Cancelling a task releases its leases in the same command.
            let mut evs = smallvec::smallvec![updated(id, TaskApiState::Cancelled, actor)];
            evs.extend(release(ctx, core, id, p));
            Ok((evs, IssueOut::Task(id.clone())))
        }
        _ => Err(TaskErr::Invalid("not a task command")),
    }
}

/// The output for a duplicate task command, from the `TaskUpdated` its first commit stored: a derived final state
/// recorded by a `PATCH` or a cancel that did not ask for it answers as that state.
pub fn replay<B>(
    op: &crate::issue_ops::IssueOp<B>,
    session: &SessionId,
    state: Option<TaskApiState>,
) -> crate::issue_ops::IssueOut {
    use crate::issue_ops::{IssueOp, IssueOut};
    match (op, state) {
        (IssueOp::UpdateTask { state: asked, .. }, Some(s)) if *asked != Some(s) && s.is_final() => {
            IssueOut::TaskFinal(session.clone(), s)
        }
        (IssueOp::CancelTask { .. }, Some(s)) if s != TaskApiState::Cancelled => {
            IssueOut::TaskFinal(session.clone(), s)
        }
        _ => IssueOut::Task(session.clone()),
    }
}

/// The sessions of `app`'s tasks on the repository, for the edge's `crate::agent::link`.
pub fn app_sessions(core: &(impl Read + ?Sized), app: AppId) -> Vec<SessionId> {
    task_ids(core, None, usize::MAX)
        .into_iter()
        .filter(|id| task_core(core, id).is_some_and(|t| t.app == app))
        .collect()
}

// ---- the rows `TaskCreated` and `TaskUpdated` write (`crate::tiers`) ----

pub(crate) fn write_created_core(w: &mut dyn Rows, t: &AgentTask, at: Timestamp) {
    let core = TaskCore {
        kind: t.kind,
        state: t.state,
        prefix: t.prefix.clone(),
        requester: t.requester,
        app: t.app,
        installation: t.installation,
        created: at,
        updated: at,
    };
    put_rec(w, "agent_task", task_key(&t.session), &core);
    if !t.state.is_final() {
        w.put("agent_task_open", task_key(&t.session), Vec::new());
    }
}

pub(crate) fn write_created_meta(w: &mut dyn Rows, t: &AgentTask, at: Timestamp) {
    let meta = TaskMeta {
        creator: t.creator,
        issue: t.issue,
        snapshot: t.snapshot,
        prompt: t.prompt.clone(),
        pr: t.pr,
        summary: t.summary.clone(),
        created: at,
    };
    put_rec(w, "agent_task_meta", task_key(&t.session), &meta);
}

pub(crate) fn write_updated_core(w: &mut dyn Rows, s: &SessionId, state: Option<TaskApiState>, at: Timestamp) {
    let Some(mut t) = task_core(w, s) else { return };
    if let Some(st) = state {
        t.state = st;
    }
    t.updated = at;
    put_rec(w, "agent_task", task_key(s), &t);
    if t.state.is_final() {
        w.delete("agent_task_open", &task_key(s));
    }
}

pub(crate) fn write_updated_meta(w: &mut dyn Rows, s: &SessionId, pr: Option<IssueNumber>, summary: Option<&str>) {
    let Some(mut m) = task_meta(w, s) else { return };
    if let Some(n) = pr {
        m.pr = Some(n);
    }
    if let Some(x) = summary {
        m.summary = x.into();
    }
    put_rec(w, "agent_task_meta", task_key(s), &m);
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_identity::{AuthzEvent, AuthzFold, InstallationRow, PermissionSet};
    use graftyard_raft::Fold as _;
    use graftyard_types::{AccountId, DbId};

    fn row(state: TaskApiState, updated: u64) -> TaskCore {
        TaskCore {
            kind: TaskKind::External,
            state,
            prefix: None,
            requester: None,
            app: AppId(DbId(3)),
            installation: InstallationId(DbId(903)),
            created: Timestamp(updated),
            updated: Timestamp(updated),
        }
    }

    fn owner(installed: bool) -> AuthzFold {
        let mut a = AuthzFold::new(AccountId(DbId(100)));
        let r = InstallationRow { app: AppId(DbId(3)), repos: RepoScope::All, permissions: PermissionSet::default() };
        a.apply(&AuthzEvent::Installation {
            id: InstallationId(DbId(903)),
            row: installed.then_some(r),
            not_before: Timestamp(0),
        });
        a
    }

    fn effective_t(t: &TaskCore, owner: &AuthzFold, at: Timestamp) -> TaskApiState {
        effective(t, owner, RepoId(DbId(7)), at)
    }

    #[test]
    fn derived_states_come_from_the_installation_and_the_idle_time() {
        let day = TASK_IDLE.as_millis() as u64;
        let (on, off) = (owner(true), owner(false));
        assert_eq!(effective_t(&row(TaskApiState::InProgress, 0), &on, Timestamp(day - 1)), TaskApiState::InProgress);
        assert_eq!(effective_t(&row(TaskApiState::InProgress, 0), &on, Timestamp(day)), TaskApiState::TimedOut);
        assert_eq!(effective_t(&row(TaskApiState::InProgress, 0), &off, Timestamp(1)), TaskApiState::Cancelled);
        // A final state stays what it was, whatever the installation or the clock.
        assert_eq!(effective_t(&row(TaskApiState::Completed, 0), &off, Timestamp(day * 9)), TaskApiState::Completed);
        assert!(TaskApiState::Queued.is_live() && !TaskApiState::WaitingForUser.is_live());
        for s in TaskApiState::ALL {
            assert_eq!(TaskApiState::parse(s.as_str()), Some(s));
        }
    }

    #[test]
    fn a_run_prefix_needs_a_segment_after_the_id() {
        let s = SessionId::parse("0193a1b2-c3d4-8e5f-8a9b-0c1d2e3f4a5b", None).unwrap();
        let t = s.text();
        let p = run_refs(&s).unwrap();
        assert_eq!(run_prefix(&s).unwrap().as_str(), format!("refs/heads/graftyard/{t}/**"));
        assert!(p.matches(format!("refs/heads/graftyard/{t}/x").as_bytes()));
        assert!(p.matches(format!("refs/heads/graftyard/{t}/x/y").as_bytes()));
        assert!(!p.matches(format!("refs/heads/graftyard/{t}").as_bytes()));
        assert!(!p.matches(b"refs/heads/graftyard/other/x"));
        assert!(run_prefix(&SessionId::parse("3.fix", None).unwrap()).is_none());
    }
}
