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

use graftyard_types::{AccountId, ChainId, CredentialShard, PlacementRecord, RepoId};

use crate::apps::{AppRecord, ClientId, Slug};
use crate::authz::{AuthzCmd, AuthzOp, AuthzRead, RepoEntry};
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
    /// The record of the repository group's latest committed config.
    pub group_at: &'a PlacementRecord,
    /// The owner the repository chain's `RepoSettings` names.
    pub settings_owner: AccountId,
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
    if !same_place(v.from_placement, v.to_placement) && !same_place(v.group_at, v.to_placement) {
        return Some(TransferStep::Move { to: v.to_placement.clone() });
    }
    if v.settings_owner != t.to {
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
