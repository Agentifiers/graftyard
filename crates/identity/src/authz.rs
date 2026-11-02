//! An owner's `Authz` record with admin grants and `RepoName` entries (`design/SHARED.md`, "Event types",
//! `AuthzEvent`, and "Names").
//!
//! Milestone 1 builds `Opened`, `Member`, `Grant`, and `RepoName`. Teams, installations, policies, invitations, Apps,
//! `InstanceAuth`, and deletion come with Milestones 2 and 3.

use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, CredentialShard, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::AUTHZ_FORMAT;
use crate::credential::{
    CredentialRefusal, IdentityDep, Own, UserId, check_credential, credential_chains, encode_cred,
};
use crate::enc;
use crate::ids::{IdBlock, IdCursor};
use crate::names::{RepoName, RepoNameKey};
use crate::overlay::{Overlay, Table};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountKind {
    User,
    Org,
    Enterprise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrgRole {
    Member,
    Admin,
    BillingManager,
}

/// GitHub's five repository roles, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RepoPermission {
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

/// Bumped by the `Authz` entry that changes which names resolve to a repository: rename, transfer, alias overwrite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NameEpoch(pub u32);

/// `Reserved` is a transfer's commit point and counts as taken. `Deleted` keeps the name until `retain_until`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoEntry {
    Reserved { repo: RepoId, from: AccountId },
    Live { repo: RepoId, epoch: NameEpoch },
    Alias { repo: RepoId, epoch: NameEpoch },
    Deleted { repo: RepoId, retain_until: Timestamp },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzEvent {
    Opened {
        kind: AccountKind,
        enterprise: Option<AccountId>,
    },
    Member {
        user: UserId,
        role: Option<OrgRole>,
    },
    /// The design's grantee is `User` or `Team`; teams are later work.
    Grant {
        repo: RepoId,
        grantee: UserId,
        permission: Option<RepoPermission>,
    },
    RepoName {
        key: RepoNameKey,
        entry: Option<RepoEntry>,
    },
}

fn perm(p: RepoPermission) -> u8 {
    p as u8
}

impl Record for AuthzEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            AuthzEvent::Opened { kind, enterprise } => {
                e.u8(0);
                e.u8(*kind as u8);
                e.option(enterprise.as_ref(), |e, a| enc::account(e, *a));
            }
            AuthzEvent::Member { user, role } => {
                e.u8(1);
                enc::account(&mut e, user.0);
                e.option(role.as_ref(), |e, r| e.u8(*r as u8));
            }
            AuthzEvent::Grant { repo, grantee, permission } => {
                e.u8(3);
                enc::repo(&mut e, *repo);
                e.u8(0);
                enc::account(&mut e, grantee.0);
                e.option(permission.as_ref(), |e, p| e.u8(perm(*p)));
            }
            AuthzEvent::RepoName { key, entry } => {
                e.u8(4);
                e.str(key.as_str());
                e.option(entry.as_ref(), |e, x| match x {
                    RepoEntry::Reserved { repo, from } => {
                        e.u8(0);
                        enc::repo(e, *repo);
                        enc::account(e, *from);
                    }
                    RepoEntry::Live { repo, epoch } => {
                        e.u8(1);
                        enc::repo(e, *repo);
                        e.u32(epoch.0);
                    }
                    RepoEntry::Alias { repo, epoch } => {
                        e.u8(2);
                        enc::repo(e, *repo);
                        e.u32(epoch.0);
                    }
                    RepoEntry::Deleted { repo, retain_until } => {
                        e.u8(3);
                        enc::repo(e, *repo);
                        enc::at(e, *retain_until);
                    }
                });
            }
        }
    }
}

/// One owner's `Authz` fold. `owner` is the chain's account, which the host sets when it builds the fold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzFold {
    pub owner: Option<AccountId>,
    pub opened: Option<AccountKind>,
    pub members: BTreeMap<UserId, OrgRole>,
    pub grants: BTreeMap<(RepoId, UserId), RepoPermission>,
    pub names: BTreeMap<RepoNameKey, RepoEntry>,
    pub epoch: NameEpoch,
    pub ids: IdCursor,
}

impl AuthzFold {
    pub fn new(owner: AccountId) -> AuthzFold {
        AuthzFold { owner: Some(owner), ..AuthzFold::default() }
    }
}

pub trait AuthzRead {
    fn owner(&self) -> Option<AccountId>;
    fn opened(&self) -> Option<AccountKind>;
    fn role(&self, user: UserId) -> Option<OrgRole>;
    fn grant(&self, repo: RepoId, user: UserId) -> Option<RepoPermission>;
    fn name(&self, key: &RepoNameKey) -> Option<RepoEntry>;
    fn epoch(&self) -> NameEpoch;

    /// Whether `user` administers the owner: the user itself for a user account, an org `Admin` for an org.
    fn is_admin(&self, user: UserId) -> bool {
        match self.opened() {
            Some(AccountKind::User) => self.owner() == Some(user.0),
            Some(_) => self.role(user) == Some(OrgRole::Admin),
            None => false,
        }
    }

    /// The repository a key resolves to, and whether the key is its live name (an `Alias` answers 301).
    fn resolve(&self, key: &RepoNameKey) -> Option<(RepoId, bool)> {
        match self.name(key)? {
            RepoEntry::Live { repo, .. } => Some((repo, true)),
            RepoEntry::Alias { repo, .. } => Some((repo, false)),
            RepoEntry::Reserved { .. } | RepoEntry::Deleted { .. } => None,
        }
    }
}

impl AuthzRead for AuthzFold {
    fn owner(&self) -> Option<AccountId> {
        self.owner
    }
    fn opened(&self) -> Option<AccountKind> {
        self.opened
    }
    fn role(&self, user: UserId) -> Option<OrgRole> {
        self.members.get(&user).copied()
    }
    fn grant(&self, repo: RepoId, user: UserId) -> Option<RepoPermission> {
        self.grants.get(&(repo, user)).copied()
    }
    fn name(&self, key: &RepoNameKey) -> Option<RepoEntry> {
        self.names.get(key).copied()
    }
    fn epoch(&self) -> NameEpoch {
        self.epoch
    }
}

struct Tables<'t, M, G, N> {
    opened: &'t mut Option<AccountKind>,
    members: &'t mut M,
    grants: &'t mut G,
    names: &'t mut N,
    epoch: &'t mut NameEpoch,
    ids: &'t mut IdCursor,
}

trait Remove<K> {
    fn remove(&mut self, k: &K);
}

impl<K: Ord, V> Remove<K> for BTreeMap<K, V> {
    fn remove(&mut self, k: &K) {
        BTreeMap::remove(self, k);
    }
}

impl<K: Ord + Clone, V> Remove<K> for Overlay<'_, K, V> {
    fn remove(&mut self, k: &K) {
        self.delete(k.clone());
    }
}

fn write<M, G, N>(ev: &AuthzEvent, t: Tables<'_, M, G, N>)
where
    M: Table<UserId, OrgRole> + Remove<UserId>,
    G: Table<(RepoId, UserId), RepoPermission> + Remove<(RepoId, UserId)>,
    N: Table<RepoNameKey, RepoEntry> + Remove<RepoNameKey>,
{
    match ev {
        AuthzEvent::Opened { kind, .. } => *t.opened = Some(*kind),
        AuthzEvent::Member { user, role: Some(r) } => t.members.put(*user, *r),
        AuthzEvent::Member { user, role: None } => t.members.remove(user),
        AuthzEvent::Grant { repo, grantee, permission: Some(p) } => t.grants.put((*repo, *grantee), *p),
        AuthzEvent::Grant { repo, grantee, permission: None } => t.grants.remove(&(*repo, *grantee)),
        AuthzEvent::RepoName { key, entry } => {
            if let Some(RepoEntry::Live { epoch, .. } | RepoEntry::Alias { epoch, .. }) = entry {
                *t.epoch = (*t.epoch).max(*epoch);
            }
            if let Some(RepoEntry::Live { repo, .. }) = entry {
                t.ids.apply_assigned(repo.0);
            }
            match entry {
                Some(e) => t.names.put(key.clone(), *e),
                None => t.names.remove(key),
            }
        }
    }
}

pub struct AuthzScratch<'a> {
    fold: &'a AuthzFold,
    opened: Option<AccountKind>,
    members: Overlay<'a, UserId, OrgRole>,
    grants: Overlay<'a, (RepoId, UserId), RepoPermission>,
    names: Overlay<'a, RepoNameKey, RepoEntry>,
    epoch: NameEpoch,
    ids: IdCursor,
}

impl AuthzRead for AuthzScratch<'_> {
    fn owner(&self) -> Option<AccountId> {
        self.fold.owner
    }
    fn opened(&self) -> Option<AccountKind> {
        self.opened
    }
    fn role(&self, user: UserId) -> Option<OrgRole> {
        self.members.get(&user).copied()
    }
    fn grant(&self, repo: RepoId, user: UserId) -> Option<RepoPermission> {
        self.grants.get(&(repo, user)).copied()
    }
    fn name(&self, key: &RepoNameKey) -> Option<RepoEntry> {
        self.names.get(key).copied()
    }
    fn epoch(&self) -> NameEpoch {
        self.epoch
    }
}

/// Whether a new `Live` or `Reserved` entry for `repo` may take a key holding `prev`. `Reserved`, `Live`, and
/// `Deleted` hold the key; an `Alias` may be overwritten.
fn free_for(prev: Option<RepoEntry>, repo: RepoId) -> bool {
    match prev {
        None | Some(RepoEntry::Alias { .. }) => true,
        // A transfer's own reservation is what its `Live` replaces.
        Some(RepoEntry::Reserved { repo: r, .. }) => r == repo,
        Some(RepoEntry::Live { .. } | RepoEntry::Deleted { .. }) => false,
    }
}

impl Scratch for AuthzScratch<'_> {
    type Event = AuthzEvent;
    fn apply(&mut self, ev: &AuthzEvent) {
        write(
            ev,
            Tables {
                opened: &mut self.opened,
                members: &mut self.members,
                grants: &mut self.grants,
                names: &mut self.names,
                epoch: &mut self.epoch,
                ids: &mut self.ids,
            },
        );
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
    /// A follower refuses an entry that opens the chain twice, takes a held name for another repository, or
    /// resolves a name at an epoch that does not move forward.
    fn check(&self, ev: &AuthzEvent) -> Result<(), Box<str>> {
        match ev {
            AuthzEvent::Opened { .. } if self.opened.is_some() => Err("opened twice".into()),
            AuthzEvent::RepoName {
                key,
                entry: Some(RepoEntry::Live { epoch, .. } | RepoEntry::Alias { epoch, .. }),
            } if *epoch < self.epoch => Err(format!("{key:?} at epoch {epoch:?}, fold is at {:?}", self.epoch).into()),
            AuthzEvent::RepoName {
                key,
                entry: Some(RepoEntry::Live { repo, .. } | RepoEntry::Reserved { repo, .. }),
            } if !free_for(self.name(key), *repo) => Err(format!("{key:?} is held by {:?}", self.name(key)).into()),
            _ => Ok(()),
        }
    }
}

impl Fold for AuthzFold {
    type Event = AuthzEvent;
    type Scratch<'a> = AuthzScratch<'a>;
    fn scratch(&self) -> AuthzScratch<'_> {
        AuthzScratch {
            fold: self,
            opened: self.opened,
            members: Overlay::new(&self.members),
            grants: Overlay::new(&self.grants),
            names: Overlay::new(&self.names),
            epoch: self.epoch,
            ids: self.ids,
        }
    }
    fn apply(&mut self, ev: &AuthzEvent) {
        write(
            ev,
            Tables {
                opened: &mut self.opened,
                members: &mut self.members,
                grants: &mut self.grants,
                names: &mut self.names,
                epoch: &mut self.epoch,
                ids: &mut self.ids,
            },
        );
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzCmd {
    pub cred: CredentialShard,
    pub op: AuthzOp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzOp {
    /// The chain's first command, from the signup's login claim. Only `graft-ops` or the CLI submits it.
    Open { kind: AccountKind, enterprise: Option<AccountId> },
    /// Adds, changes, or (with `None`) removes an org member's role. `Admin` is the admin grant.
    SetMember { user: UserId, role: Option<OrgRole> },
    /// A user's direct permission on one of the owner's repositories.
    SetGrant { repo: RepoId, user: UserId, permission: Option<RepoPermission> },
    /// Creates a repository: its id is the next of this group's id block, and its name goes `Live`.
    CreateRepo { name: RepoName },
    /// Renames: `Live` at the new key, `Alias` at the old, both at a new epoch.
    RenameRepo { from: RepoNameKey, to: RepoName },
    /// Deletes: the name is held as `Deleted` until `retain_until`.
    DeleteRepo { key: RepoNameKey, retain_until: Timestamp },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzOut {
    Repo { repo: RepoId, epoch: NameEpoch },
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzErr {
    Unauthorized(CredentialRefusal),
    /// 403: the actor does not administer the owner (404 for a private repository is the renderer's call).
    Forbidden,
    NotOpened,
    AlreadyOpened,
    /// Membership needs an org.
    NotAnOrg,
    /// 422 `already_exists`.
    Taken,
    NoRepo,
    /// The new key is the old key.
    SameName,
    /// The group has no free id: lease a block from `Ids`, commit it here (`Member::id_block`), and re-send.
    NeedIdBlock,
}

impl Record for AuthzCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        encode_cred(&mut e, &self.cred);
        match &self.op {
            AuthzOp::Open { kind, enterprise } => {
                e.u8(0);
                e.u8(*kind as u8);
                e.option(enterprise.as_ref(), |e, a| enc::account(e, *a));
            }
            AuthzOp::SetMember { user, role } => {
                e.u8(1);
                enc::account(&mut e, user.0);
                e.option(role.as_ref(), |e, r| e.u8(*r as u8));
            }
            AuthzOp::SetGrant { repo, user, permission } => {
                e.u8(2);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, user.0);
                e.option(permission.as_ref(), |e, p| e.u8(perm(*p)));
            }
            AuthzOp::CreateRepo { name } => {
                e.u8(3);
                e.str(name.as_str());
            }
            AuthzOp::RenameRepo { from, to } => {
                e.u8(4);
                e.str(from.as_str());
                e.str(to.as_str());
            }
            AuthzOp::DeleteRepo { key, retain_until } => {
                e.u8(5);
                e.str(key.as_str());
                enc::at(&mut e, *retain_until);
            }
        }
    }
}

impl AuthzCmd {
    fn decide_op(&self, s: &AuthzScratch<'_>) -> Result<(SmallVec<[AuthzEvent; 4]>, AuthzOut), AuthzErr> {
        if let AuthzOp::Open { kind, enterprise } = &self.op {
            if self.cred.account.is_some() {
                return Err(AuthzErr::Forbidden);
            }
            if s.opened.is_some() {
                return Err(AuthzErr::AlreadyOpened);
            }
            return Ok((smallvec![AuthzEvent::Opened { kind: *kind, enterprise: *enterprise }], AuthzOut::Done));
        }
        if s.opened.is_none() {
            return Err(AuthzErr::NotOpened);
        }
        // `graft-ops` and the CLI name no account and act as a site admin.
        if let Some(a) = self.cred.account
            && !s.is_admin(UserId(a))
        {
            return Err(AuthzErr::Forbidden);
        }
        let next = NameEpoch(s.epoch.0 + 1);
        match &self.op {
            AuthzOp::Open { .. } => unreachable!("handled above"),
            AuthzOp::SetMember { user, role } => {
                if s.opened != Some(AccountKind::Org) {
                    return Err(AuthzErr::NotAnOrg);
                }
                Ok((smallvec![AuthzEvent::Member { user: *user, role: *role }], AuthzOut::Done))
            }
            AuthzOp::SetGrant { repo, user, permission } => Ok((
                smallvec![AuthzEvent::Grant { repo: *repo, grantee: *user, permission: *permission }],
                AuthzOut::Done,
            )),
            AuthzOp::CreateRepo { name } => {
                let key = name.key();
                if s.name(&key).is_some_and(|e| !matches!(e, RepoEntry::Alias { .. })) {
                    return Err(AuthzErr::Taken);
                }
                let repo = RepoId(s.ids.peek().ok_or(AuthzErr::NeedIdBlock)?);
                let entry = RepoEntry::Live { repo, epoch: next };
                Ok((smallvec![AuthzEvent::RepoName { key, entry: Some(entry) }], AuthzOut::Repo { repo, epoch: next }))
            }
            AuthzOp::RenameRepo { from, to } => {
                let Some(RepoEntry::Live { repo, .. }) = s.name(from) else { return Err(AuthzErr::NoRepo) };
                let key = to.key();
                if key == *from {
                    return Err(AuthzErr::SameName);
                }
                if !free_for(s.name(&key), repo) {
                    return Err(AuthzErr::Taken);
                }
                Ok((
                    smallvec![
                        AuthzEvent::RepoName { key, entry: Some(RepoEntry::Live { repo, epoch: next }) },
                        AuthzEvent::RepoName { key: from.clone(), entry: Some(RepoEntry::Alias { repo, epoch: next }) },
                    ],
                    AuthzOut::Repo { repo, epoch: next },
                ))
            }
            AuthzOp::DeleteRepo { key, retain_until } => {
                let Some(RepoEntry::Live { repo, .. }) = s.name(key) else { return Err(AuthzErr::NoRepo) };
                let entry = RepoEntry::Deleted { repo, retain_until: *retain_until };
                Ok((smallvec![AuthzEvent::RepoName { key: key.clone(), entry: Some(entry) }], AuthzOut::Done))
            }
        }
    }
}

impl Command for AuthzCmd {
    type Fold = AuthzFold;
    type Dep = IdentityDep;
    type Out = AuthzOut;
    type Err = AuthzErr;

    fn format(&self) -> Format {
        AUTHZ_FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, _fold: &AuthzFold) -> SmallVec<[ChainId; 3]> {
        credential_chains(&self.cred, None)
    }
    fn decide(
        &self,
        s: &AuthzScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, AuthzOut), AuthzErr> {
        check_credential(&self.cred, Own::Other, deps).map_err(AuthzErr::Unauthorized)?;
        self.decide_op(s)
    }
    fn replay(&self, events: &[AuthzEvent]) -> AuthzOut {
        match events.first() {
            Some(AuthzEvent::RepoName { entry: Some(RepoEntry::Live { repo, epoch }), .. }) => {
                AuthzOut::Repo { repo: *repo, epoch: *epoch }
            }
            _ => AuthzOut::Done,
        }
    }
}
