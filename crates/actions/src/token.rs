//! A job's `GITHUB_TOKEN` (`design/SHARED.md`, "Actions", `GITHUB_TOKEN`; `decisions/DECISIONS.md`, "`gh-token`"):
//! an installation token of the install-owned Actions App (`graftyard_identity::stateless::ACTIONS_APP`) on the run's
//! owner, for the run's repository alone, with the permissions GHES 3.22 computes, minted at `acquirejob` and denied by
//! `jti` when the job ends. Pure: the hosts read the rows and commit the commands.
//!
//! - **Permissions** follow GHES: "initially set to the default setting for the enterprise, organization, or
//!   repository", then "adjusted based on any configuration within the workflow file, first at the workflow level and
//!   then at the job level", and for a pull request event other than `pull_request_target` from a fork, every write is
//!   read [doc: GHES 3.22, "Workflow syntax for GitHub Actions", "How permissions are calculated for a workflow job"].
//!   A workflow's `permissions:` replaces the default, so it may grant above it, as GHES does: "If you specify the
//!   access for any of these permissions, all of those that are not specified are set to `none`."
//! - **The `jti`** is derived from the job and its attempt (`job_jti`), so `RunOwner` denies a job's token from the
//!   shard's fold and entries alone (`denies`), and a second acquire of the same job gets the same `jti`. The acquire
//!   that gives a runner the token marks the job (`ActionsEvent::TokenIssued`, in the `Acquired` entry), so the deny
//!   follows the marked jobs whichever site leads when they end, and no unissued token costs a write. It fires on every
//!   way such a job ends: `Finished` (completed or failed), `Cancel` (a user's cancel, a concurrency group, or
//!   `RunOwner`'s abort when the repository is deleted), and `LeaseExpired` (the runner vanished).

use std::collections::BTreeMap;
use std::time::Duration;

use graftyard_identity::stateless::{
    ACTIONS_APP, ACTIONS_SCOPES, Grant, JOB_TOKEN_MAX, actions_installation, job_token_life,
};
use graftyard_identity::{Access, Jti, PermissionSet, RepoScope};
use graftyard_types::{AccountId, Timestamp, Ulid};

use crate::ids::JobId;
use crate::job::{JOB_LEASE, JobEvent, JobState};
use crate::shard::{ActionsEvent, ActionsFold, WorkflowRun};
use crate::workflow::template::{Node, T};

/// GHES's restricted default, the one new repositories get: "read access for the `contents` and `packages`
/// permissions" [doc: GHES 3.22, "Managing GitHub Actions settings for a repository", Workflow permissions]. Graftyard
/// has no per-repository or org setting yet, so every job without `permissions:` gets it, and `metadata` read, which
/// every installation token carries (`decisions/OPEN-QUESTIONS.md`, F93).
pub fn default_permissions() -> PermissionSet {
    set([("contents", Access::Read), ("packages", Access::Read), ("metadata", Access::Read)])
}

fn set<const N: usize>(p: [(&str, Access); N]) -> PermissionSet {
    PermissionSet(p.into_iter().map(|(k, a)| (Box::from(k), a)).collect())
}

/// Why a `permissions:` value is not one GHES takes. The schema refuses these before a run is made, so a run never
/// carries one; an expression is refused here, since the schema allows none.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("permissions: {0}")]
pub struct BadPermissions(pub String);

/// A workflow's or job's `permissions:` as the token's: `read-all`, `write-all`, `{}`, or a mapping of the GHES
/// scopes to `read`, `write`, or `none`, a scope left out being `none`. `metadata` read is always added. `id-token`
/// (OIDC) and the scopes GHES 3.22 does not list take no token permission [inference: the schema names them for
/// github.com; `ACTIONS_SCOPES` is the GHES table].
pub fn parse(n: &Node) -> Result<PermissionSet, BadPermissions> {
    let all = |a: Access| ACTIONS_SCOPES.iter().map(|s| (Box::from(*s), a)).collect::<BTreeMap<_, _>>();
    let mut out = match &n.t {
        T::Str(s) if s == "read-all" => all(Access::Read),
        T::Str(s) if s == "write-all" => all(Access::Write),
        T::Map(pairs) => {
            let mut m = BTreeMap::new();
            for (k, v) in pairs {
                let (Some(k), Some(v)) = (k.str(), v.str()) else {
                    return Err(BadPermissions("a scope or level is not a string".into()));
                };
                let level = match v {
                    "read" => Some(Access::Read),
                    "write" => Some(Access::Write),
                    "none" => None,
                    _ => return Err(BadPermissions(format!("{k}: {v}"))),
                };
                let scope = k.replace('-', "_");
                if let (Some(a), true) = (level, ACTIONS_SCOPES.contains(&scope.as_str())) {
                    m.insert(scope.into_boxed_str(), a);
                }
            }
            m
        }
        _ => return Err(BadPermissions("not read-all, write-all, or a mapping".into())),
    };
    out.insert("metadata".into(), Access::Read);
    Ok(PermissionSet(out))
}

/// The job's declared permissions: its own `permissions:`, else the workflow's, else none (the default applies at
/// `acquirejob`). A job's mapping replaces the workflow's whole, as GHES's "adjusted ... first at the workflow level
/// and then at the job level" and "all of those that are not specified are set to `none`" read together.
pub fn declared(workflow: Option<&Node>, job: Option<&Node>) -> Result<Option<PermissionSet>, BadPermissions> {
    job.or(workflow).map(parse).transpose()
}

/// The token's permissions: the declared set or the default, with every write read for a fork's pull request run,
/// which GHES does unless "Send write tokens to workflows from pull requests" is set, a setting Graftyard does not
/// have [doc: GHES 3.22, as the module docs].
pub fn job_permissions(declared: Option<&PermissionSet>, fork: bool) -> PermissionSet {
    let mut p = declared.cloned().unwrap_or_else(default_permissions);
    if fork {
        for a in p.0.values_mut() {
            *a = (*a).min(Access::Read);
        }
    }
    p
}

/// The job's `GITHUB_TOKEN` `jti`: a ULID whose 80 low bits are the job id and attempt and whose time part is 0, so
/// one (job, attempt) has one `jti` and no installation token minted with a random one can match it. A second
/// `acquirejob` of the same attempt mints the same `jti`, and `RunOwner` revokes it from the entries.
pub fn job_jti(job: JobId, attempt: u16) -> Jti {
    let mut r = [0u8; 10];
    r[..8].copy_from_slice(&(job.0.0 as u64).to_be_bytes());
    r[8..].copy_from_slice(&attempt.to_be_bytes());
    Jti(Ulid::new(Timestamp(0), r))
}

/// The job limit a token's lifetime covers: GHES's default `timeout-minutes`, 360 [doc: GHES 3.22, "Workflow syntax
/// for GitHub Actions", `jobs.<job_id>.timeout-minutes`]. A job that sets a longer one outlives its token
/// (`decisions/OPEN-QUESTIONS.md`, F93).
pub const JOB_LIMIT: Duration = Duration::from_secs(6 * 3600);

/// What `acquirejob` asks `graft-ops` to sign for `job` of `run` on `owner`: the owner's Actions installation, the run's
/// repository alone, the job's permissions, a lifetime of `JOB_LIMIT` plus one lease, and the job's own `jti`.
pub fn job_grant(run: &WorkflowRun, owner: AccountId, job: JobId, declared: Option<&PermissionSet>) -> Grant {
    Grant {
        app: ACTIONS_APP,
        installation: actions_installation(owner),
        repos: RepoScope::Selected(smallvec::smallvec![run.repo]),
        permissions: job_permissions(declared, run.fork),
        task: None,
        lifetime_ms: Some(job_token_life(JOB_LIMIT, JOB_LEASE).as_millis() as u64),
        jti: Some(job_jti(job, run.attempt)),
    }
}

/// The revocation's `exp` for a job that ended at `ended`: every token of the job was minted before it ended and lives
/// at most `JOB_TOKEN_MAX`, and a revocation row lives until its `exp`, so this one outlives each of them.
pub fn deny_until(ended: Timestamp) -> Timestamp {
    Timestamp(ended.0 + JOB_TOKEN_MAX.as_millis() as u64)
}

/// The job tokens the committed `entries` after `RunOwner`'s cursor end, each with its revocation's `exp`: every job
/// marked issued that an event completed, by `Finished`, `Cancel`, or `LeaseExpired`. As in `run_owner::steps`, an
/// event at version v completed its job when the fold holds the job at v+1, completed, since a completed job takes no
/// later event. `QueueExpired` and `NeedsFailed` complete only jobs no runner held.
pub fn denies<'a>(fold: &ActionsFold, entries: impl IntoIterator<Item = &'a ActionsEvent>) -> Vec<(Jti, Timestamp)> {
    let mut out = Vec::new();
    for ev in entries {
        let ActionsEvent::Job { job, version, event } = ev else { continue };
        if !matches!(event, JobEvent::Finished(_) | JobEvent::Cancel | JobEvent::LeaseExpired) {
            continue;
        }
        let Some(j) = fold.job(*job).filter(|_| fold.more.issued.contains(job)) else { continue };
        let (JobState::Completed { at, .. }, true) = (&j.state, j.version == version + 1) else { continue };
        let Some(run) = fold.run(j.run) else { continue };
        out.push((job_jti(*job, run.run.attempt), deny_until(*at)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(src: &str) -> Node {
        let p = crate::workflow::parse(
            "p.yml",
            &format!("on: push\npermissions: {src}\njobs:\n  a:\n    runs-on: x\n    steps: [{{run: a}}]\n"),
        );
        assert!(p.ok(), "{:?}", p.errors);
        p.workflow.permissions.unwrap()
    }

    fn level(p: &PermissionSet, k: &str) -> Option<Access> {
        p.0.get(k).copied()
    }

    /// GHES's rules: a mapping sets the scopes it names and drops the rest, `write-all` reaches above the restricted
    /// default, `{}` leaves only `metadata`, and a fork's run gets every write as read.
    #[test]
    fn a_mapping_replaces_the_default_and_a_fork_gets_reads() {
        let d = default_permissions();
        assert_eq!((level(&d, "contents"), level(&d, "issues")), (Some(Access::Read), None));
        let m = parse(&node("{issues: write, pull-requests: read, contents: none, id-token: write}")).unwrap();
        assert_eq!(level(&m, "issues"), Some(Access::Write), "above the restricted default");
        assert_eq!(level(&m, "pull_requests"), Some(Access::Read), "the token's name for pull-requests");
        assert_eq!(level(&m, "contents"), None, "none");
        assert_eq!(level(&m, "packages"), None, "a scope left out is none, though the default had it");
        assert_eq!(level(&m, "id_token"), None, "OIDC is no token permission");
        assert_eq!(level(&m, "metadata"), Some(Access::Read));
        let w = parse(&node("write-all")).unwrap();
        assert_eq!(w.0.len(), ACTIONS_SCOPES.len() + 1);
        assert!(ACTIONS_SCOPES.iter().all(|s| level(&w, s) == Some(Access::Write)));
        let r = parse(&node("read-all")).unwrap();
        assert!(ACTIONS_SCOPES.iter().all(|s| level(&r, s) == Some(Access::Read)));
        assert_eq!(parse(&node("{}")).unwrap(), set([("metadata", Access::Read)]));
        // The job's own replaces the workflow's whole.
        let job = node("{issues: write}");
        let both = declared(Some(&node("write-all")), Some(&job)).unwrap().unwrap();
        assert_eq!((level(&both, "issues"), level(&both, "contents")), (Some(Access::Write), None));
        assert_eq!(declared(None, None).unwrap(), None);
        // A fork's pull request run: every write is read, and no scope is added.
        let f = job_permissions(Some(&w), true);
        assert!(ACTIONS_SCOPES.iter().all(|s| level(&f, s) == Some(Access::Read)));
        assert_eq!(job_permissions(Some(&m), true).0.len(), m.0.len());
        assert_eq!(job_permissions(None, false), d);
        assert_eq!(job_permissions(Some(&w), false), w);
    }

    /// What `acquirejob` mints: the owner's Actions installation, the run's repository alone, 6 h 10 min, the job's own
    /// `jti` for its attempt, and a fork's writes as reads.
    #[test]
    fn a_jobs_grant_is_its_repository_its_permissions_and_its_jti() {
        use graftyard_types::{ChainId, DbId, Index, RepoId};
        let (owner, job) = (AccountId(DbId(70_001)), JobId(DbId(65_545)));
        let mut run = WorkflowRun {
            id: crate::RunId(DbId(9)),
            repo: RepoId(DbId(65_600)),
            workflow: "ci.yml".into(),
            event: "pull_request".into(),
            head_sha: [1; 20],
            head_ref: "refs/pull/1/merge".into(),
            trigger: (ChainId::Repo(RepoId(DbId(65_600))), Index(3)),
            attempt: 2,
            concurrency: None,
            fork: false,
        };
        let w = parse(&node("write-all")).unwrap();
        let g = job_grant(&run, owner, job, Some(&w));
        assert_eq!((g.app, g.installation), (ACTIONS_APP, actions_installation(owner)));
        assert_eq!(g.repos, RepoScope::Selected(smallvec::smallvec![run.repo]));
        assert_eq!(g.permissions, w);
        assert_eq!(g.lifetime_ms, Some((6 * 3600 + 600) * 1000));
        assert_eq!(g.jti, Some(job_jti(job, 2)));
        run.fork = true;
        let f = job_grant(&run, owner, job, Some(&w));
        assert!(f.permissions.0.values().all(|a| *a == Access::Read));
        assert_eq!(job_grant(&run, owner, job, None).permissions, default_permissions());
    }

    /// One (job, attempt) has one `jti`, another job or attempt another, and its text parses back as a `jti`.
    #[test]
    fn a_jobs_jti_is_its_own() {
        let j = JobId(graftyard_types::DbId(65_545));
        assert_eq!(job_jti(j, 1), job_jti(j, 1));
        assert_ne!(job_jti(j, 1), job_jti(j, 2));
        assert_ne!(job_jti(j, 1), job_jti(JobId(graftyard_types::DbId(65_546)), 1));
        let text = job_jti(j, 1).0.to_string();
        assert_eq!(text.parse::<Ulid>().map(Jti), Ok(job_jti(j, 1)));
    }
}
