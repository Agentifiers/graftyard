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

use graftyard_types::{AccountId, ChainId, CredentialShard};

use crate::apps::{AppRecord, ClientId, Slug};
use crate::authz::{AuthzCmd, AuthzOp, AuthzRead};
use crate::names::{Login, LoginRow, NameKey, NamesCmd, NamesOp, NamesRead, SlugRow, account_shard, name_shard};

/// A command for one chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Names(ChainId, NamesCmd),
    Authz(ChainId, Box<AuthzCmd>),
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
