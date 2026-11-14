//! The read side every handler takes: one repository's two fold tiers at one index, as a leader's `fresh` or a
//! signed stale read hands them out (`design/DESIGN.md`, Shape, "The read path"), plus the names of the accounts the
//! rows name. Handlers never see a table name; they read typed rows through `Repo`.

use graftyard_fold::{Read, Val};
use graftyard_git::issues::{
    AppId, BranchRule, CheckRunId, CheckRunUpdate, CheckSuiteUpdate, CommentId, CommentTarget, IssueKind, IssueNumber,
    PrState, Protection, PullHead, RepoSettings, StatusUpdate,
};
use graftyard_git::issues::{AssetId, LockId, RepoReleaseId};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::tiers::{AssetRow, LfsRow, LockRow, ReleaseRow};
use graftyard_git::tiers::{
    CommentRow, IssueRow, LabelRow, PullRow, RequiredRow, ReviewRow, Timed, VerdictRow, get_rec, int, n_key, order_key,
    pull_head, ref_key, sha_key,
};
use graftyard_identity::RepoPermission;
use graftyard_types::{AccountId, DbId, RepoId};

/// What a handler needs to know beyond the repository's own tiers: the accounts its rows name and the repositories of
/// its network, from the `Names`, `Identity`, and owner `Authz` folds the edge read for the request.
pub trait Directory {
    fn login(&self, id: AccountId) -> Option<String>;
    fn name(&self, id: AccountId) -> Option<String>;
    fn is_org(&self, id: AccountId) -> bool;
    fn by_login(&self, login: &str) -> Option<AccountId>;
    /// The account's permission on this repository, for `authorAssociation` and `author_association`.
    fn permission(&self, id: AccountId) -> Option<RepoPermission>;
    /// Another repository of the network, for a fork pull request's head.
    fn repo(&self, id: RepoId) -> Option<RepoRef>;
    /// A ref of another repository of the network, read fresh on its group, for a fork pull request's head.
    fn ref_of(&self, repo: RepoId, name: &RefName) -> Option<ObjectId>;
    /// A team's org login, name, and slug, for a requested team reviewer.
    fn team(&self, id: DbId) -> Option<(String, String, String)>;
    /// The signed-in account, if any.
    fn viewer(&self) -> Option<AccountId>;
    /// What the git cache computes between two commits: (commits, additions, deletions, changed files).
    fn compare(&self, base: ObjectId, head: ObjectId) -> Compare;
    /// Whether the install's `Authz(Genesis.account)` holds an `AgentApp` row for `app`. Only the trust view asks, and
    /// only for a client that opted in, so a directory that never read the install's rows answers false.
    fn agent_app(&self, _app: AppId) -> bool {
        false
    }
    /// The selection `key` names: `select`'s, over the tiers, unless this directory holds the one a member made over
    /// its whole fold (`crate::narrow`), because the rows the edge holds are only the selected ones.
    fn pick(&self, _key: &str, select: &mut dyn FnMut() -> Pick) -> Pick {
        select()
    }
}

/// A selection over a whole table, made where the whole fold is: the keys of the rows a reply renders, in order, how
/// many matched, and whether more follow the page (`crate::narrow`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pick {
    pub items: Vec<graftyard_fold::Key>,
    pub total: usize,
    pub more: bool,
}

impl Pick {
    /// A page of issue numbers, each keyed `[n]`.
    pub fn of_numbers(items: impl IntoIterator<Item = IssueNumber>, total: usize, more: bool) -> Pick {
        Pick { items: items.into_iter().map(|n| vec![n_key(n)]).collect(), total, more }
    }

    /// The items as issue numbers, for a pick of `[n]` keys.
    pub fn numbers(&self) -> Vec<IssueNumber> {
        self.items
            .iter()
            .filter_map(|k| match k.first() {
                Some(Val::Int(n)) => u32::try_from(*n).ok().map(IssueNumber),
                _ => None,
            })
            .collect()
    }
}

/// The git cache's diff summary of a pull request, from the merge base of its base tip and head to the head, as GHES
/// counts `commits`, `additions`, `deletions`, and `changed_files` (the shard's `/ui/compare`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Compare {
    pub commits: u32,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
}

/// The repository a request addresses, resolved by the edge from `o/r` (M1-WP1's lookup rule).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoRef {
    pub id: RepoId,
    pub owner: String,
    pub owner_id: AccountId,
    pub name: String,
}

/// One repository's tiers at one index, as one `fresh` read returned them.
pub struct Repo<'a> {
    pub host: &'a crate::wire::Host,
    pub at: RepoRef,
    pub core: &'a dyn Read,
    pub meta: &'a dyn Read,
    pub dir: &'a dyn Directory,
}

/// An issue or pull request as both tiers hold it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub n: IssueNumber,
    pub row: IssueRow,
    /// Present for a pull request: its metadata row and its core head.
    pub pull: Option<(PullRow, PullHead)>,
}

impl Item {
    pub fn is_pull(&self) -> bool {
        self.row.kind == IssueKind::PullRequest
    }

    pub fn pr_state(&self) -> Option<PrState> {
        self.pull.as_ref().map(|(_, h)| h.state)
    }
}

/// Which items a list wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}

impl StateFilter {
    /// The `open` flags of `issue_order` keys this filter takes.
    pub fn opens(self) -> &'static [bool] {
        match self {
            StateFilter::Open => &[true],
            StateFilter::Closed => &[false],
            StateFilter::All => &[true, false],
        }
    }
}

impl Repo<'_> {
    pub fn settings(&self) -> Option<RepoSettings> {
        get_rec(self.core, "settings", &[])
    }

    pub fn ref_value(&self, name: &RefName) -> Option<ObjectId> {
        get_rec(self.core, "ref", &[ref_key(name)])
    }

    /// The base tip and head a pull request's diff, compare, commits, and counts are read at: a closed or merged
    /// one's final head.
    pub fn pull_pair(&self, pull: &PullRow, head: &PullHead) -> (ObjectId, ObjectId) {
        let head_sha = match head.state {
            PrState::Open { .. } => head.head_sha,
            PrState::Closed { final_head, .. } | PrState::Merged { final_head, .. } => final_head,
        };
        (self.ref_value(&pull.base).unwrap_or(pull.base_sha), head_sha)
    }

    /// The git cache's counts for pull request `pull` (`Directory::compare`).
    pub fn counts(&self, pull: &PullRow, head: &PullHead) -> Compare {
        let (base, head) = self.pull_pair(pull, head);
        self.dir.compare(base, head)
    }

    pub fn item(&self, n: IssueNumber) -> Option<Item> {
        let row: IssueRow = get_rec(self.meta, "issue", &[n_key(n)])?;
        let pull = match row.kind {
            IssueKind::PullRequest => Some((get_rec(self.meta, "pull", &[n_key(n)])?, pull_head(self.core, n)?)),
            IssueKind::Issue => None,
        };
        Some(Item { n, row, pull })
    }

    /// Open issues and pull requests, as GitHub's `open_issues_count` counts them: a count over every open item, so a
    /// selection (`Directory::pick`).
    pub fn issue_count(&self) -> usize {
        let count = || {
            [IssueKind::Issue, IssueKind::PullRequest]
                .iter()
                .map(|k| self.meta.prefix("issue_order", &order_key(*k, true, IssueNumber(0))[..2]).len())
                .sum()
        };
        self.dir.pick("issue_count", &mut || Pick { total: count(), ..Pick::default() }).total
    }

    pub fn item_by_id(&self, id: DbId) -> Option<Item> {
        let n: u32 = get_rec(self.meta, "issue_by_id", &[int(id.0)])?;
        self.item(IssueNumber(n))
    }

    /// Items of `kind` in `state`, newest number first, from `issue_order`. `after` is the last number of the previous
    /// page; `keep` filters before `limit` counts. Returns the page and whether more match.
    pub fn list(
        &self,
        kind: IssueKind,
        state: StateFilter,
        after: Option<IssueNumber>,
        limit: usize,
        keep: &dyn Fn(&Item) -> bool,
    ) -> (Vec<Item>, bool) {
        let mut all: Vec<Item> = self
            .numbers(kind, state)
            .into_iter()
            .filter(|n| after.is_none_or(|a| *n < a))
            .filter_map(|n| self.item(n).filter(|i| keep(i)))
            .collect();
        let more = all.len() > limit;
        all.truncate(limit);
        (all, more)
    }

    /// The numbers of the items of `kind` in `state`, newest first, from `issue_order`'s keys alone: a few scans, and
    /// no item's rows. Every `issue_order` key has its item (`issue`, and for a pull request `pull` and `pull_head`,
    /// all written by the entry that opened it), so these are the numbers `list` keeps when `keep` keeps every item.
    pub fn numbers(&self, kind: IssueKind, state: StateFilter) -> Vec<IssueNumber> {
        let mut all: Vec<IssueNumber> = Vec::new();
        for open in state.opens() {
            let prefix = order_key(kind, *open, IssueNumber(0))[..2].to_vec();
            all.extend(self.meta.prefix("issue_order", &prefix).into_iter().filter_map(|(k, _)| match k.get(2) {
                Some(Val::Int(rev)) => Some(IssueNumber((-rev) as u32)),
                _ => None,
            }));
        }
        all.sort_unstable_by(|a, b| b.cmp(a));
        all
    }

    pub fn labels_of(&self, n: IssueNumber) -> Vec<(DbId, LabelRow)> {
        self.meta
            .prefix("issue_label", &[n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Int(id)) => Some((DbId(*id), get_rec(self.meta, "label", &[int(*id)])?)),
                _ => None,
            })
            .collect()
    }

    pub fn labels(&self) -> Vec<(DbId, LabelRow)> {
        let mut v: Vec<(DbId, LabelRow)> = self
            .meta
            .prefix("label", &[])
            .into_iter()
            .filter_map(|(k, _)| match k.first() {
                Some(Val::Int(id)) => Some((DbId(*id), get_rec(self.meta, "label", &k)?)),
                _ => None,
            })
            .collect();
        v.sort_by_key(|l| l.1.name.0.to_lowercase());
        v
    }

    pub fn assignees_of(&self, n: IssueNumber) -> Vec<AccountId> {
        self.meta
            .prefix("issue_assignee", &[n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Int(a)) => Some(AccountId(DbId(*a))),
                _ => None,
            })
            .collect()
    }

    /// Comments on issue or pull request `n`'s conversation, oldest first.
    pub fn comments_of(&self, n: IssueNumber) -> Vec<(CommentId, CommentRow)> {
        self.meta
            .prefix("comment_by_issue", &[n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Int(id)) => {
                    let row: CommentRow = get_rec(self.meta, "comment", &[int(*id)])?;
                    Some((CommentId(DbId(*id)), row))
                }
                _ => None,
            })
            .filter(|(_, c)| matches!(c.on, CommentTarget::Issue(_)))
            .collect()
    }

    pub fn reviews_of(&self, n: IssueNumber) -> Vec<(DbId, ReviewRow)> {
        self.meta
            .prefix("review_by_pull", &[n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Int(id)) => Some((DbId(*id), get_rec(self.meta, "review", &[int(*id)])?)),
                _ => None,
            })
            .collect()
    }

    /// Review comments on pull request `n`, oldest first, as GHES lists them (`sort=created`). Ids do not give that
    /// order: a thread's first comment takes a Core id and a reply a Metadata id.
    /// No index finds them, so which comments they are is a selection over every comment (`Directory::pick`).
    pub fn review_comments_of(&self, n: IssueNumber) -> Vec<(CommentId, CommentRow)> {
        let mut select = || {
            let mut rows: Vec<(i64, CommentRow)> = self
                .meta
                .prefix("comment", &[])
                .into_iter()
                .filter_map(|(k, _)| match k.first() {
                    Some(Val::Int(id)) => {
                        let row: CommentRow = get_rec(self.meta, "comment", &k)?;
                        matches!(row.on, CommentTarget::Review { n: on, .. } if on == n).then_some((*id, row))
                    }
                    _ => None,
                })
                .collect();
            rows.sort_by_key(|(id, row)| (row.created, *id));
            Pick { total: rows.len(), items: rows.into_iter().map(|(id, _)| vec![int(id)]).collect(), more: false }
        };
        let pick = self.dir.pick(&format!("review_comments/{}", n.0), &mut select);
        pick.items
            .iter()
            .filter_map(|k| match k.first() {
                Some(Val::Int(id)) => Some((CommentId(DbId(*id)), get_rec(self.meta, "comment", k)?)),
                _ => None,
            })
            .collect()
    }

    pub fn review_requests_of(&self, n: IssueNumber) -> Vec<String> {
        self.meta
            .prefix("review_request", &[n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Text(g)) => Some(g.to_string()),
                _ => None,
            })
            .collect()
    }

    /// Every check run on `sha`, with the entry time that last wrote it.
    pub fn check_runs(&self, sha: &ObjectId) -> Vec<Timed<CheckRunUpdate>> {
        self.meta
            .prefix("check_run", &[sha_key(sha)])
            .into_iter()
            .filter_map(|(k, _)| get_rec(self.meta, "check_run", &k))
            .collect()
    }

    /// The run with this id, whatever its commit: keyed by commit first, so found by a selection over every run
    /// (`Directory::pick`).
    pub fn check_run(&self, run: CheckRunId) -> Option<Timed<CheckRunUpdate>> {
        let mut select = || {
            let key = self.meta.prefix("check_run", &[]).into_iter().map(|(k, _)| k).find(|k| {
                get_rec::<Timed<CheckRunUpdate>>(self.meta, "check_run", k).is_some_and(|t| t.rec.run == run)
            });
            Pick { total: usize::from(key.is_some()), items: key.into_iter().collect(), more: false }
        };
        let pick = self.dir.pick(&format!("check_run/{}", run.0.0), &mut select);
        get_rec(self.meta, "check_run", pick.items.first()?)
    }

    pub fn check_suites(&self, sha: &ObjectId) -> Vec<Timed<CheckSuiteUpdate>> {
        self.meta
            .prefix("check_suite", &[sha_key(sha)])
            .into_iter()
            .filter_map(|(k, _)| get_rec(self.meta, "check_suite", &k))
            .collect()
    }

    /// Every status on `sha`, newest first, as `GET /commits/{ref}/statuses` lists them.
    pub fn statuses(&self, sha: &ObjectId, skip: usize, limit: usize) -> Vec<Timed<StatusUpdate>> {
        let prefix = [sha_key(sha)];
        let mut from = prefix.to_vec();
        let mut out = Vec::new();
        let mut seen = 0;
        loop {
            let batch = self.meta.scan("status", &from, 256);
            let full = batch.len() == 256;
            for (k, _) in batch {
                if !k.starts_with(&prefix) {
                    return out;
                }
                if k == from && seen > 0 {
                    continue;
                }
                from = k.clone();
                seen += 1;
                if seen <= skip {
                    continue;
                }
                if out.len() == limit {
                    return out;
                }
                if let Some(s) = get_rec(self.meta, "status", &k) {
                    out.push(s);
                }
            }
            if !full {
                return out;
            }
        }
    }

    /// The core required rows of `sha`: (name, source, conclusion).
    pub fn required(&self, sha: &ObjectId) -> Vec<(String, RequiredRow)> {
        self.core
            .prefix("required_check", &[sha_key(sha)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Text(n)) => Some((n.to_string(), get_rec(self.core, "required_check", &k)?)),
                _ => None,
            })
            .collect()
    }

    /// The protections of every rule over `base`.
    pub fn protections(&self, base: &RefName) -> Vec<Protection> {
        self.core
            .prefix("rule", &[])
            .into_iter()
            .filter_map(|(k, _)| get_rec::<BranchRule>(self.core, "rule", &k))
            .filter(|r| graftyard_git::issue_ops::branch_matches(&r.pattern, base))
            .filter_map(|r| r.protection)
            .collect()
    }

    /// Whether a rule over `base` requires the check or status `name` from `app` (`None` for a status).
    pub fn is_required(&self, base: &RefName, name: &str, app: Option<AppId>) -> bool {
        self.protections(base)
            .iter()
            .filter_map(|p| p.checks.as_ref())
            .any(|c| c.checks.iter().any(|(n, a)| &**n == name && (a.is_none() || *a == app)))
    }

    /// The core verdicts on pull request `n`: (reviewer, state, commit).
    pub fn verdicts(&self, n: IssueNumber) -> Vec<VerdictRow> {
        self.core
            .prefix("review_verdict", &[n_key(n)])
            .into_iter()
            .filter_map(|(k, _)| get_rec(self.core, "review_verdict", &k))
            .collect()
    }

    pub fn unresolved_threads(&self, n: IssueNumber) -> u32 {
        graftyard_git::tiers::unresolved_threads(self.core, n)
    }

    /// Every release, newest id first, as `releases(orderBy: CREATED_AT DESC)` lists them: ids grow with creation.
    pub fn releases(&self) -> Vec<(RepoReleaseId, ReleaseRow)> {
        self.meta
            .prefix("release", &[])
            .into_iter()
            .filter_map(|(k, _)| match k.first() {
                Some(Val::Int(neg)) => Some((RepoReleaseId(DbId(-neg)), get_rec(self.meta, "release", &k)?)),
                _ => None,
            })
            .collect()
    }

    pub fn release(&self, id: RepoReleaseId) -> Option<ReleaseRow> {
        graftyard_git::tiers::release_row(self.meta, id)
    }

    /// The release of `refs/tags/{tag}`.
    pub fn release_by_tag(&self, tag: &str) -> Option<(RepoReleaseId, ReleaseRow)> {
        let name = RefName::parse(format!("refs/tags/{tag}").as_bytes()).ok()?;
        let id = graftyard_git::tiers::release_by_tag(self.meta, &name)?;
        Some((id, self.release(id)?))
    }

    /// The latest release: of those published, neither draft nor prerelease, with `make_latest` set, the newest.
    pub fn latest_release(&self) -> Option<RepoReleaseId> {
        self.releases().into_iter().find(|(_, r)| !r.rec.draft && !r.rec.prerelease && r.rec.latest).map(|(i, _)| i)
    }

    /// A release's assets in upload order.
    pub fn assets_of(&self, release: RepoReleaseId) -> Vec<(AssetId, AssetRow)> {
        self.meta
            .prefix("asset_by_release", &[int(release.0.0)])
            .into_iter()
            .filter_map(|(k, _)| match k.get(1) {
                Some(Val::Int(id)) => Some((AssetId(DbId(*id)), get_rec(self.meta, "asset", &[int(*id)])?)),
                _ => None,
            })
            .collect()
    }

    pub fn asset(&self, id: AssetId) -> Option<AssetRow> {
        get_rec(self.meta, "asset", &[int(id.0.0)])
    }

    /// The LFS object a `verify` committed.
    pub fn lfs_object(&self, oid: &graftyard_types::Digest) -> Option<LfsRow> {
        get_rec(self.meta, "lfs_object", &[graftyard_git::tiers::lfs_key(oid)])
    }

    /// Every LFS lock, in id order.
    pub fn locks(&self) -> Vec<(LockId, LockRow)> {
        self.meta
            .prefix("lfs_lock", &[])
            .into_iter()
            .filter_map(|(k, _)| match k.first() {
                Some(Val::Int(id)) => Some((LockId(DbId(*id)), get_rec(self.meta, "lfs_lock", &k)?)),
                _ => None,
            })
            .collect()
    }
}
