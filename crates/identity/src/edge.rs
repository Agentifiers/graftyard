//! The commands the edge sends after a commit point, in order (`build/BUILD-PLAN.md`, M3-WP3; `design/SHARED.md`,
//! "Multi-chain operations"). Each list starts after the commit point committed and holds only follow-ups that cannot
//! be refused while the commit point stands, so an edge that dies partway leaves the rest to the `Audit` finisher
//! (`crate::finish`), whose plans submit the same commands.
//!
//! - **Signup.** The commit point is `ClaimLogin` on the login's shard, which assigns the account id. Then the
//!   account's `AccountLogin` row on its `account_shard` (F50), its `Account` row, its `Placement` row copied from
//!   the install account's, and its `Identity` chain's `Created`.
//! - **Rename.** The commit point is the new login's `ClaimLogin` for the account. Then the old login's
//!   `ReleaseLogin`, then `AccountLogin` with the new login: the leader reads the login's shard, so it is written only
//!   while the account holds it, and a delayed write of the old login is refused.
//! - **New credential.** The commit point is its `Tokens` row. Then `AddCredential` on the account's `Identity` chain,
//!   which the leader refuses unless that row is live for the account.
//! - **Sign-in.** One command: `SignIn` on the account's `Identity` chain, after the edge verified the method.

use graftyard_types::{AccountId, ChainId, CredentialShard, PlacementRecord, Timestamp};

use crate::authz::AccountKind;
use crate::credential::WebSessionId;
use crate::finish::Step;
use crate::identity::{AccountCapture, CredentialRef, IdentityCmd, IdentityOp};
use crate::names::{Login, NameKey, NamesCmd, NamesOp, account_shard, name_shard};
use crate::org::SignInMethod;

fn on_names(chain: ChainId, cred: CredentialShard, op: NamesOp) -> Step {
    Step::Names(chain, NamesCmd { cred, op })
}

fn on_identity(account: AccountId, cred: CredentialShard, op: IdentityOp) -> Step {
    Step::Identity(ChainId::Identity(account), IdentityCmd { cred, op })
}

/// After a signup's `ClaimLogin` committed and assigned `account`. The new account holds no credential yet, so the
/// edge submits these as the finisher would, with no account. `default` is the install account's `Placement` record,
/// which a new owner copies (DESIGN, Shape, `PlacementRecord`: "a new owner copies the install account's record"), so
/// its `Authz` and `Identity` chains have a record before `Created`. A re-sent `InitPlacement` is refused
/// `RecordExists`, which a sender counts as done: the record it would write is already there.
pub fn after_signup(
    account: AccountId,
    login: &Login,
    kind: AccountKind,
    created: Timestamp,
    default: &PlacementRecord,
) -> Vec<Step> {
    let none = CredentialShard::NONE;
    let home = ChainId::Names(account_shard(account));
    let record = PlacementRecord { moving_from: None, ..default.clone() };
    let mut steps = vec![
        on_names(home, none, NamesOp::SetAccountLogin { account, login: login.clone() }),
        on_names(home, none, NamesOp::PutAccount { id: account, suspended: false, sessions_not_before: Timestamp(0) }),
        on_names(home, none, NamesOp::InitPlacement { owner: account, record }),
    ];
    if kind == AccountKind::User {
        let capture = AccountCapture { login: login.clone(), kind, created };
        steps.push(on_identity(account, none, IdentityOp::Create(capture)));
    }
    steps
}

/// A rename from `old` to `new`, as the account itself: the commit point first, then its follow-ups.
pub fn rename(account: AccountId, cred: CredentialShard, old: &Login, new: &Login) -> Vec<Step> {
    let claim = NamesOp::ClaimLogin { login: new.clone(), kind: AccountKind::User, account: Some(account) };
    vec![
        on_names(ChainId::Names(name_shard(&NameKey::Login(new.key()))), cred, claim),
        on_names(
            ChainId::Names(name_shard(&NameKey::Login(old.key()))),
            cred,
            NamesOp::ReleaseLogin { key: old.key(), account },
        ),
        on_names(
            ChainId::Names(account_shard(account)),
            cred,
            NamesOp::SetAccountLogin { account, login: new.clone() },
        ),
    ]
}

/// After a new credential's `Tokens` row committed.
pub fn after_credential(account: AccountId, cred: CredentialShard, credential: CredentialRef, title: &str) -> Step {
    on_identity(account, cred, IdentityOp::AddCredential { credential, title: title.into() })
}

/// A sign-in the edge verified by `method`. `install` is the install account, whose `Authz` chain holds
/// `InstanceAuth`.
pub fn sign_in(account: AccountId, id: WebSessionId, method: SignInMethod, install: AccountId) -> Step {
    on_identity(account, CredentialShard::NONE, IdentityOp::SignIn { id, method, install })
}
