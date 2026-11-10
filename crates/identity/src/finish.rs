//! What the `Audit` consumer submits to finish a multi-chain operation nobody finished within a minute
//! (`design/SHARED.md`, "Multi-chain operations", Finisher; `design/DESIGN.md`, Shape, "Identity").
//!
//! Each plan is pure: it reads the folds the consumer read, and returns the commands still owed, in order, each with
//! no credential, so a credential revoked in between cannot strand the operation. Every command is idempotent: one a
//! racing edge already committed decides to no events. The consumer submits them one at a time and plans again after
//! each, so a plan never rests on a write that has not committed.
//!
//! - **App registration.** The commit point is the slug's `AppSlug` claim on `Names`, which assigns the App id. Then
//!   `AuthzOp::PutApp` on the owner's `Authz` chain, then `RegisterApp` and `RegisterClient` on `Names`. The abort
//!   path is `CloseApp` and `ReleaseSlug`, taken only when the claim has no record to finish from: the edge that
//!   claimed it died before it wrote the record, and the record's contents were only in that request.
//! - **Login row (F50).** The commit point is `LoginClaimed`; the account's `AccountLogin` row follows.
//! - **Credential (M3-WP3).** The commit point is the credential's `Tokens` entry; the edge appends the `Identity`
//!   side right after it, and the finisher lists a committed credential the `Identity` fold does not, or drops one
//!   whose row was revoked or removed.
//! - **Transfer (M3-WP3).** The commit point is the destination's `Reserved` name (`transfer`).
//! - **Enterprise change.** The commit point is the org's `Enterprise` event; each repository the org names `Live`
//!   then gets the transfer's own `Settings` with the new enterprise (`enterprise_change`).
//! - **Repository create (F59).** The commit point is `CreateRepoWith` on the owner's `Authz` chain (Authz format
//!   0x12.11): the name's `Live` and, in the same entry, `RepoCreate` with the settings asked for. Then `RepoOwner` on
//!   `Names`, then the chain's first settings and `Created` (`RepoStep::Init`, repository format 32.17). The abort path
//!   is `AbortCreate`, which releases the name, taken only once `CREATE_ABORT_AFTER` passed since the commit point and
//!   the chain still holds no `Created` (`create`). The README commit of `auto_init` is not redone: it needs the git
//!   cache's `validate`, which no finisher calls, so a create the finisher completed has no commit.
//! - **Repository delete (F59).** The commit point is the name's `Deleted` on the owner's `Authz` chain; the chain's
//!   `Deleted` tombstone follows (`RepoStep::Delete`, 32.17). It has no abort: once the name is `Deleted` the
//!   repository is gone for every route, and only the tombstone is owed (`delete`).

use graftyard_types::{AccountId, ChainId, CredentialShard, PlacementRecord, RepoId, SiteId, Timestamp};

use crate::apps::{AppRecord, ClientId, Slug};
use crate::authz::{AuthzCmd, AuthzFold, AuthzOp, AuthzRead, RepoEntry};
use crate::identity::{CredentialRef, IdentityCmd, IdentityFold, IdentityOp};
use crate::names::{
    Login, LoginRow, NameKey, NamesCmd, NamesOp, NamesRead, RepoNameKey, SlugRow, account_shard, name_shard,
};
use crate::tokens::TokensRead;

/// A command for one chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Names(ChainId, NamesCmd),
    Authz(ChainId, Box<AuthzCmd>),
    Identity(ChainId, IdentityCmd),
    /// Only the `Audit` consumer's cursor (`crate::audit::cursor`): no finisher plan owes a `Tokens` command.
    Tokens(ChainId, Box<crate::tokens::TokensCmd>),
    /// A step on the repository chain, which this crate cannot name as a command: the site builds it.
    Repo(RepoId, RepoStep),
}

/// A transfer's or an enterprise change's step on the repository chain (`TransferStep`'s two that are not identity
/// commands).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoStep {
    /// Move the repository's group to `to`'s sites and home (`crate::group_move`).
    Move { to: PlacementRecord },
    /// The transfer's own `Settings` (repository format 32.10) with the new owner and its enterprise.
    Settings { owner: AccountId, enterprise: Option<AccountId> },
    /// A create's first settings and `Created` (repository format 32.17): `init` is the JSON of the `RepoSettings` the
    /// create asked for, with `owner` and `enterprise` as the owner's `Authz` names them now. The site creates the
    /// group under `owner` first if it has none.
    Init { owner: AccountId, enterprise: Option<AccountId>, init: Box<str> },
    /// A delete's tombstone on the chain (32.17), with the name's `retain_until`.
    Delete { retain_until: Timestamp },
}

/// How long the finisher keeps trying a create's first settings before it aborts the create and releases its name
/// [guess: ten times `FINISH_AFTER`, far past any edge request's life, so no in-band `InitRepo` is still coming].
pub const CREATE_ABORT_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

/// Where a repository chain stands, as the finisher reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoLife {
    /// No group answers for the chain, or no `RepoOwner` row places it: nothing was written to it.
    NoGroup,
    /// The group exists and holds neither `Created` nor `Deleted`.
    Open,
    Created,
    Deleted,
}

/// A repository create whose commit point committed on `owner`'s chain at `at`, with the settings asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Create<'a> {
    pub owner: AccountId,
    pub repo: RepoId,
    pub init: &'a str,
    pub at: Timestamp,
}

/// The next step of a create: `RepoOwner`, then the chain's first settings, or, once `CREATE_ABORT_AFTER` passed with
/// the chain still not created, the abort, which releases the repository's `Live` name wherever a rename put it.
/// `authz` is the owner's fold, `repo_owner` the repository's `RepoOwner` row, and `life` the chain's state, `None`
/// until it is read (`Err(())` then asks for it). `None` when nothing is owed: the chain was created or deleted, the
/// owner names the repository `Live` nowhere (aborted, deleted, or transferred away), or another owner's `RepoOwner`
/// row holds it.
#[allow(clippy::result_unit_err)]
pub fn create(
    c: &Create<'_>,
    authz: &AuthzFold,
    repo_owner: Option<AccountId>,
    life: Option<RepoLife>,
    now: Timestamp,
) -> Result<Option<Step>, ()> {
    let live = authz.names.iter().find_map(|(k, e)| match e {
        RepoEntry::Live { repo, .. } if *repo == c.repo => Some(k),
        _ => None,
    });
    let Some(key) = live else { return Ok(None) };
    match repo_owner {
        None => return Ok(Some(names(NameKey::Repo(c.repo), NamesOp::RepoOwner { repo: c.repo, owner: c.owner }))),
        Some(o) if o != c.owner => return Ok(None),
        Some(_) => {}
    }
    match life.ok_or(())? {
        RepoLife::Created | RepoLife::Deleted => Ok(None),
        RepoLife::NoGroup | RepoLife::Open => {
            if now.0 >= c.at.0.saturating_add(CREATE_ABORT_AFTER.as_millis() as u64) {
                let op = AuthzOp::AbortCreate { key: key.clone(), repo: c.repo };
                return Ok(Some(Step::Authz(
                    ChainId::Authz(c.owner),
                    Box::new(AuthzCmd { cred: CredentialShard::NONE, op }),
                )));
            }
            let init = RepoStep::Init { owner: c.owner, enterprise: authz.enterprise(), init: c.init.into() };
            Ok(Some(Step::Repo(c.repo, init)))
        }
    }
}

/// The next step of a delete whose name turned `Deleted` until `retain_until` on `owner`'s chain: the chain's
/// tombstone, while the owner's fold still holds the name so, the `RepoOwner` row names the owner, and the chain has
/// a group that holds no `Deleted`. `Err(())` asks for the chain's state, which is read only once the row names the
/// owner: the site finds the chain's members through it, and a chain with no row was never written. Nothing is owed
/// once `retain_until` passed, when the chain would refuse the tombstone anyway.
#[allow(clippy::result_unit_err)]
pub fn delete(
    owner: AccountId,
    repo: RepoId,
    retain_until: Timestamp,
    authz: &AuthzFold,
    repo_owner: Option<AccountId>,
    life: Option<RepoLife>,
    now: Timestamp,
) -> Result<Option<Step>, ()> {
    if authz.deleted(repo) != Some(retain_until) || now >= retain_until || repo_owner != Some(owner) {
        return Ok(None);
    }
    Ok(match life.ok_or(())? {
        RepoLife::NoGroup | RepoLife::Deleted => None,
        RepoLife::Open | RepoLife::Created => Some(Step::Repo(repo, RepoStep::Delete { retain_until })),
    })
}

fn names(key: NameKey, op: NamesOp) -> Step {
    Step::Names(ChainId::Names(name_shard(&key)), NamesCmd { cred: CredentialShard::NONE, op })
}

/// The next step of an App registration whose slug claim committed: `slug` holds `claim`, `authz` is the owner's
/// `Authz` fold, and `app_route` and `client_route` are the `Names` shards of the App id and its client id. `record`
/// is the registration's record if the finisher has it (the `Audit` consumer finds it in no chain, so it passes
/// `None` for a claim whose record never committed). `None` when nothing is owed.
pub fn app_registration(
    slug: &Slug,
    claim: SlugRow,
    authz: &dyn AuthzRead,
    app_route: &dyn NamesRead,
    client_route: &dyn NamesRead,
    record: Option<&AppRecord>,
) -> Option<Step> {
    let SlugRow { app, owner } = claim;
    let on_authz = |op| Step::Authz(ChainId::Authz(owner), Box::new(AuthzCmd { cred: CredentialShard::NONE, op }));
    if authz.app_closed(app) {
        return Some(names(NameKey::AppSlug(slug.clone()), NamesOp::ReleaseSlug { slug: slug.clone(), app, owner }));
    }
    let Some(have) = authz.app(app) else {
        return Some(match record {
            Some(r) if r.id == app && r.slug == *slug => on_authz(AuthzOp::PutApp { record: Box::new(r.clone()) }),
            _ => on_authz(AuthzOp::CloseApp { app }),
        });
    };
    if app_route.app_owner(app).is_none() {
        return Some(names(NameKey::App(app), NamesOp::RegisterApp { app, owner }));
    }
    let client = ClientId::for_app(app);
    debug_assert_eq!(have.client, client);
    if client_route.client(&client).is_none() {
        return Some(names(NameKey::Client(client), NamesOp::RegisterClient { client, owner, app: Some(app) }));
    }
    None
}

/// The account's `AccountLogin` row is owed when its login's shard says the account holds `login` and its
/// `account_shard` does not name it yet.
pub fn account_login(
    account: AccountId,
    login: &Login,
    login_shard: &dyn NamesRead,
    account_shard_fold: &dyn NamesRead,
) -> Option<Step> {
    let holds = matches!(login_shard.login(&login.key()), Some(LoginRow::Claimed { account: a, .. }) if a == account);
    if !holds || account_shard_fold.account_login(account) == Some(login) {
        return None;
    }
    Some(Step::Names(
        ChainId::Names(account_shard(account)),
        NamesCmd { cred: CredentialShard::NONE, op: NamesOp::SetAccountLogin { account, login: login.clone() } },
    ))
}

/// The `Identity` side of a credential the `Tokens` shard holds for `account`: listed when its row is live and the
/// list lacks it, dropped when the row is revoked or gone and the list has it. `title` is what the edge was given; the
/// finisher, which has none, passes the credential's kind.
pub fn credential(
    account: AccountId,
    credential: CredentialRef,
    title: &str,
    tokens: &dyn TokensRead,
    identity: &IdentityFold,
) -> Option<Step> {
    let live = match &credential {
        CredentialRef::Token { hash, kind } => {
            tokens.token(hash).is_some_and(|r| r.account == account && !r.revoked && r.kind == *kind)
        }
        CredentialRef::SshKey(f) => tokens.ssh_key(f).is_some_and(|r| r.account == account),
    };
    let listed = identity.credentials.contains_key(&credential);
    let op = match (live, listed) {
        (true, false) => IdentityOp::AddCredential { credential, title: title.into() },
        (false, true) => IdentityOp::RemoveCredential { credential },
        _ => return None,
    };
    Some(Step::Identity(ChainId::Identity(account), IdentityCmd { cred: CredentialShard::NONE, op }))
}

/// A repository transfer whose commit point, the destination's `Reserved` name, may have committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub repo: RepoId,
    pub from: AccountId,
    pub from_key: RepoNameKey,
    pub to: AccountId,
    pub to_key: RepoNameKey,
}

/// What the folds the finisher read say about a transfer.
pub struct TransferView<'a> {
    pub from_authz: &'a dyn AuthzRead,
    pub to_authz: &'a dyn AuthzRead,
    pub from_placement: &'a PlacementRecord,
    pub to_placement: &'a PlacementRecord,
    /// The voter sites and home of the repository group's latest committed config.
    pub group: (&'a [SiteId], SiteId),
    /// The owner and enterprise the repository chain's `RepoSettings` names.
    pub settings_owner: AccountId,
    pub settings_enterprise: Option<AccountId>,
    /// The repository's `RepoOwner` row on `Names`.
    pub repo_owner: Option<AccountId>,
}

/// A transfer's next step that is not a command on an identity chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferStep {
    Step(Step),
    /// Move the repository's group, and its network's for a root repository, to `to` through `Joint` (M1-WP7's owner
    /// move, per group).
    Move {
        to: PlacementRecord,
    },
    /// Record `Settings` on the repository chain with the new owner and its enterprise, the other fields unchanged.
    Settings {
        owner: AccountId,
        enterprise: Option<AccountId>,
    },
}

/// Whether two owners' records place groups on the same members.
fn same_place(a: &PlacementRecord, b: &PlacementRecord) -> bool {
    (a.name, &a.sites, a.home) == (b.name, &b.sites, b.home)
}

/// Whether a group with these voter sites and home is where `r` places it.
pub fn group_at(group: (&[SiteId], SiteId), r: &PlacementRecord) -> bool {
    let (voters, home) = group;
    voters.len() == r.sites.len() && voters.iter().all(|s| r.sites.contains(s)) && home == r.home
}

/// The next step of a transfer whose `Reserved` name committed, in SHARED's order ("Multi-chain operations",
/// Transfer): the group's move when the placements differ, `Settings`, the destination's `Live` name, the source's
/// `Alias`, then `RepoOwner`. The move goes first, so a route through the new owner's record (its `Live` name, or
/// `RepoOwner` for `/repositories/{id}`) never names a placement the group is not in. `None` when nothing is owed, or
/// when no reservation committed, in which case the transfer never happened.
pub fn transfer(t: &Transfer, v: &TransferView<'_>) -> Option<TransferStep> {
    let reserved = match v.to_authz.name(&t.to_key) {
        Some(RepoEntry::Reserved { repo, .. }) if repo == t.repo => true,
        Some(RepoEntry::Live { repo, .. } | RepoEntry::Alias { repo, .. }) if repo == t.repo => false,
        _ => return None,
    };
    let on_authz = |owner: AccountId, op| {
        TransferStep::Step(Step::Authz(ChainId::Authz(owner), Box::new(AuthzCmd { cred: CredentialShard::NONE, op })))
    };
    if !same_place(v.from_placement, v.to_placement) && !group_at(v.group, v.to_placement) {
        return Some(TransferStep::Move { to: v.to_placement.clone() });
    }
    if v.settings_owner != t.to || v.settings_enterprise != v.to_authz.enterprise() {
        return Some(TransferStep::Settings { owner: t.to, enterprise: v.to_authz.enterprise() });
    }
    if reserved {
        return Some(on_authz(t.to, AuthzOp::CompleteTransfer { key: t.to_key.clone(), repo: t.repo }));
    }
    if matches!(v.from_authz.name(&t.from_key), Some(RepoEntry::Live { repo, .. }) if repo == t.repo) {
        return Some(on_authz(t.from, AuthzOp::LeaveAlias { key: t.from_key.clone(), repo: t.repo }));
    }
    if v.repo_owner != Some(t.to) {
        return Some(TransferStep::Step(names(
            NameKey::Repo(t.repo),
            NamesOp::RepoOwner { repo: t.repo, owner: t.to },
        )));
    }
    None
}

/// The next step of an org's enterprise change: the first repository the org's `Authz` fold names `Live` whose
/// `Settings` names the org but another enterprise. `repos` is each such repository's settings owner and enterprise,
/// as its chain holds them; a repository missing from it is read first (`Err`).
pub fn enterprise_change(
    org: AccountId,
    authz: &dyn AuthzRead,
    live: &[RepoId],
    repos: &dyn Fn(RepoId) -> Option<(AccountId, Option<AccountId>)>,
) -> Result<Option<Step>, RepoId> {
    let want = authz.enterprise();
    for r in live {
        let (owner, enterprise) = repos(*r).ok_or(*r)?;
        if owner == org && enterprise != want {
            return Ok(Some(Step::Repo(*r, RepoStep::Settings { owner: org, enterprise: want })));
        }
    }
    Ok(None)
}
