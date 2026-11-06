//! An owner's `Authz` record with admin grants and `RepoName` entries (`design/SHARED.md`, "Event types",
//! `AuthzEvent`, and "Names").
//!
//! Milestone 1 builds `Opened`, `Member`, `Grant`, and `RepoName`. Milestone 3 adds the owner's App and OAuth app
//! registrations, `App` and `OAuthApp` (M3-WP1), and M3-WP3 teams, team grants, the org policy, org invitations,
//! `InstanceAuth`, an org's enterprise change, and the transfer steps on the two owners' chains. Installations
//! (M3-WP4) and account deletion are not built.

use std::borrow::Cow;
use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, CredentialShard, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::apps::{self, AppId, AppRecord, ClientId, OAuthAppRecord};
use crate::compat::tail_serde;
use crate::credential::{
    CredentialRefusal, IdentityDep, Own, UserId, check_credential, credential_chains, encode_cred,
};
use crate::enc;
use crate::ids::{IdBlock, IdCursor};
use crate::names::{ClientRoute, NameKey, NamesRead, RepoName, RepoNameKey, SlugRow, name_shard};
use crate::org::{
    self, INVITATION_TTL, InstanceAuth, InvitationEvent, InvitationGrant, InvitationId, InvitationRow, Invitee,
    OrgPolicy, TeamEvent, TeamId, TeamRow, is_install_account,
};
use crate::overlay::{Overlay, SetOverlay, Table};
use crate::{AUTHZ_FORMAT, AUTHZ_FORMAT_V2};

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
    /// An App this account owns (SHARED, "Identity and tokens"). `None` deletes the registration.
    App {
        app: AppId,
        record: Option<Box<AppRecord>>,
    },
    /// An OAuth app this account owns. `None` deletes the registration.
    OAuthApp {
        client: ClientId,
        record: Option<OAuthAppRecord>,
    },
    // ---- M3-WP3, format 2 ----
    Team(TeamEvent),
    /// SHARED's `Grant` with a `Team` grantee: the same tag as `Grant`, grantee tag 1.
    TeamGrant {
        repo: RepoId,
        team: TeamId,
        permission: Option<RepoPermission>,
    },
    Policy(OrgPolicy),
    /// An invitation to join the org. Accepting it appends `Member`, and `Team` entries for its teams, in the same
    /// entry.
    Invitation(InvitationEvent),
    /// Only on the install's own `Authz` chain. Written by a site admin and read fresh by every sign-in.
    InstanceAuth(InstanceAuth),
    /// An org's enterprise change: its commit point (SHARED, "Multi-chain operations", Enterprise change). Not in
    /// SHARED's `AuthzEvent`, which names the entry but no event for it [inference].
    Enterprise {
        enterprise: Option<AccountId>,
    },
}

/// One op's events and output, or its refusal.
type Decided = Result<(SmallVec<[AuthzEvent; 4]>, AuthzOut), AuthzErr>;

fn org_only(s: &AuthzScratch<'_>) -> Result<(), AuthzErr> {
    if s.opened() == Some(AccountKind::Org) { Ok(()) } else { Err(AuthzErr::NotAnOrg) }
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
            // Tags follow SHARED's `AuthzEvent` order: `Team` 2, `Installation` 5, `Policy` 6, `Invitation` 7, then
            // `App` 8 and `OAuthApp` 9; `InstanceAuth`, `Deleted`, and `Undeleted` keep 10 to 12.
            AuthzEvent::App { app, record } => {
                e.u8(8);
                apps::app_id(&mut e, *app);
                e.option(record.as_deref(), apps::app_record);
            }
            AuthzEvent::OAuthApp { client, record } => {
                e.u8(9);
                apps::client(&mut e, client);
                e.option(record.as_ref(), apps::oauth_app_record);
            }
            AuthzEvent::Team(t) => {
                e.u8(2);
                org::team_event(&mut e, t);
            }
            AuthzEvent::TeamGrant { repo, team, permission } => {
                e.u8(3);
                enc::repo(&mut e, *repo);
                e.u8(1);
                enc::db(&mut e, team.0);
                e.option(permission.as_ref(), |e, p| e.u8(perm(*p)));
            }
            AuthzEvent::Policy(p) => {
                e.u8(6);
                org::policy(&mut e, p);
            }
            AuthzEvent::Invitation(i) => {
                e.u8(7);
                org::invitation_event(&mut e, i);
            }
            // `Deleted` 11 and `Undeleted` 12 stay free; `Installation` is 5.
            AuthzEvent::InstanceAuth(a) => {
                e.u8(10);
                org::instance_auth(&mut e, a);
            }
            AuthzEvent::Enterprise { enterprise } => {
                e.u8(13);
                e.option(enterprise.as_ref(), |e, a| enc::account(e, *a));
            }
        }
    }
}

/// Milestone 3's rows of an `Authz` chain: the fold's trailing part (`crate::compat`). `closed` holds the Apps whose
/// registration was deleted or aborted, so their slug may be released and their id is never registered again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzMore {
    pub apps: BTreeMap<AppId, AppRecord>,
    pub closed: std::collections::BTreeSet<AppId>,
    pub oauth_apps: BTreeMap<ClientId, OAuthAppRecord>,
    pub org: OrgRows,
}

/// M3-WP3's rows: teams, team grants, the policy, invitations, `InstanceAuth`, and the enterprise. A scratch view
/// copies them on its entry's first write to them (`Cow`): they are small next to names and grants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgRows {
    pub teams: BTreeMap<TeamId, TeamRow>,
    pub team_grants: BTreeMap<(RepoId, TeamId), RepoPermission>,
    pub policy: OrgPolicy,
    pub invitations: BTreeMap<InvitationId, InvitationRow>,
    pub instance_auth: Option<InstanceAuth>,
    pub enterprise: Option<AccountId>,
}

impl OrgRows {
    fn write(&mut self, ev: &AuthzEvent) {
        match ev {
            AuthzEvent::Opened { enterprise, .. } | AuthzEvent::Enterprise { enterprise } => {
                self.enterprise = *enterprise
            }
            AuthzEvent::Member { user, role: None } => {
                for t in self.teams.values_mut() {
                    t.members.remove(user);
                }
            }
            AuthzEvent::Team(TeamEvent::Created { team, name }) => {
                self.teams.insert(*team, TeamRow { name: name.clone(), members: Default::default() });
            }
            AuthzEvent::Team(TeamEvent::Member { team, user, on }) => {
                if let Some(t) = self.teams.get_mut(team) {
                    if *on {
                        t.members.insert(*user);
                    } else {
                        t.members.remove(user);
                    }
                }
            }
            AuthzEvent::Team(TeamEvent::Deleted { team }) => {
                self.teams.remove(team);
                self.team_grants.retain(|(_, t), _| t != team);
            }
            AuthzEvent::TeamGrant { repo, team, permission: Some(p) } => {
                self.team_grants.insert((*repo, *team), *p);
            }
            AuthzEvent::TeamGrant { repo, team, permission: None } => {
                self.team_grants.remove(&(*repo, *team));
            }
            AuthzEvent::Policy(p) => self.policy = *p,
            AuthzEvent::Invitation(InvitationEvent::Sent { id, invitee, grant, inviter, expires }) => {
                let row =
                    InvitationRow { invitee: *invitee, grant: grant.clone(), inviter: *inviter, expires: *expires };
                self.invitations.insert(*id, row);
            }
            AuthzEvent::Invitation(
                InvitationEvent::Accepted { id }
                | InvitationEvent::Declined { id }
                | InvitationEvent::Cancelled { id, .. },
            ) => {
                self.invitations.remove(id);
            }
            AuthzEvent::InstanceAuth(a) => self.instance_auth = Some(a.clone()),
            _ => {}
        }
    }

    /// The pending invitation, unless it lapsed by `now`.
    pub fn invitation(&self, id: InvitationId, now: Timestamp) -> Option<&InvitationRow> {
        self.invitations.get(&id).filter(|r| r.expires > now)
    }

    fn team_permission(&self, repo: RepoId, user: UserId) -> Option<RepoPermission> {
        self.team_grants
            .iter()
            .filter(|((r, t), _)| *r == repo && self.teams.get(t).is_some_and(|t| t.members.contains(&user)))
            .map(|(_, p)| *p)
            .max()
    }
}

/// One owner's `Authz` fold. `owner` is the chain's account, which the host sets when it builds the fold. Its serde
/// form is Milestone 1's fields, then `more` only when it holds a row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthzFold {
    pub owner: Option<AccountId>,
    pub opened: Option<AccountKind>,
    pub members: BTreeMap<UserId, OrgRole>,
    pub grants: BTreeMap<(RepoId, UserId), RepoPermission>,
    pub names: BTreeMap<RepoNameKey, RepoEntry>,
    pub epoch: NameEpoch,
    pub ids: IdCursor,
    pub more: AuthzMore,
}

tail_serde!(AuthzFold {
    owner: Option<AccountId>,
    opened: Option<AccountKind>,
    members: BTreeMap<UserId, OrgRole>,
    grants: BTreeMap<(RepoId, UserId), RepoPermission>,
    names: BTreeMap<RepoNameKey, RepoEntry>,
    epoch: NameEpoch,
    ids: IdCursor,
} more: AuthzMore);

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
    fn app(&self, app: AppId) -> Option<&AppRecord>;
    /// Whether the App's registration was deleted or aborted here.
    fn app_closed(&self, app: AppId) -> bool;
    fn oauth_app(&self, client: &ClientId) -> Option<&OAuthAppRecord>;
    /// Teams, the policy, invitations, `InstanceAuth`, and the enterprise.
    fn org(&self) -> &OrgRows;
    /// The enterprise an org belongs to, whose own `Authz` chain is a third dependency.
    fn enterprise(&self) -> Option<AccountId> {
        self.org().enterprise
    }
    /// The org policy's base permission for every member.
    fn base_permission(&self) -> Option<RepoPermission> {
        self.org().policy.base_permission
    }
    /// Whether the org policy enforces SSO: access through membership needs the org in the principal's `sso_orgs`.
    fn sso_enforced(&self) -> bool {
        self.org().policy.sso_enforced
    }
    /// The highest permission a team the user is in holds on the repository.
    fn team_permission(&self, repo: RepoId, user: UserId) -> Option<RepoPermission> {
        self.org().team_permission(repo, user)
    }
    /// The install's sign-in choice, on the install's own chain: `Builtin` until a site admin writes another.
    fn instance_auth(&self) -> InstanceAuth {
        self.org().instance_auth.clone().unwrap_or_default()
    }

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
    fn app(&self, app: AppId) -> Option<&AppRecord> {
        self.more.apps.get(&app)
    }
    fn app_closed(&self, app: AppId) -> bool {
        self.more.closed.contains(&app)
    }
    fn oauth_app(&self, client: &ClientId) -> Option<&OAuthAppRecord> {
        self.more.oauth_apps.get(client)
    }
    fn org(&self) -> &OrgRows {
        &self.more.org
    }
}

struct Tables<'t, M, G, N, A, C, O> {
    opened: &'t mut Option<AccountKind>,
    members: &'t mut M,
    grants: &'t mut G,
    names: &'t mut N,
    epoch: &'t mut NameEpoch,
    ids: &'t mut IdCursor,
    apps: &'t mut A,
    closed: &'t mut C,
    oauth_apps: &'t mut O,
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

/// Whether `OrgRows::write` changes anything for the event, so a scratch view copies the rows only then.
fn touches_org(ev: &AuthzEvent) -> bool {
    matches!(
        ev,
        AuthzEvent::Opened { .. }
            | AuthzEvent::Member { role: None, .. }
            | AuthzEvent::Team(_)
            | AuthzEvent::TeamGrant { .. }
            | AuthzEvent::Policy(_)
            | AuthzEvent::Invitation(_)
            | AuthzEvent::InstanceAuth(_)
            | AuthzEvent::Enterprise { .. }
    )
}

fn write<M, G, N, A, C, O>(ev: &AuthzEvent, t: Tables<'_, M, G, N, A, C, O>)
where
    M: Table<UserId, OrgRole> + Remove<UserId>,
    G: Table<(RepoId, UserId), RepoPermission> + Remove<(RepoId, UserId)>,
    N: Table<RepoNameKey, RepoEntry> + Remove<RepoNameKey>,
    A: Table<AppId, AppRecord>,
    C: Table<AppId, ()>,
    O: Table<ClientId, OAuthAppRecord>,
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
        AuthzEvent::App { app, record: Some(r) } => t.apps.put(*app, (**r).clone()),
        AuthzEvent::App { app, record: None } => {
            t.apps.del(app);
            t.closed.put(*app, ());
        }
        AuthzEvent::OAuthApp { client, record: Some(r) } => t.oauth_apps.put(*client, r.clone()),
        AuthzEvent::OAuthApp { client, record: None } => t.oauth_apps.del(client),
        AuthzEvent::Team(TeamEvent::Created { team, .. }) => t.ids.apply_assigned(team.0),
        AuthzEvent::Invitation(InvitationEvent::Sent { id, .. }) => t.ids.apply_assigned(id.0),
        // `OrgRows::write`'s.
        AuthzEvent::Team(_)
        | AuthzEvent::TeamGrant { .. }
        | AuthzEvent::Policy(_)
        | AuthzEvent::Invitation(_)
        | AuthzEvent::InstanceAuth(_)
        | AuthzEvent::Enterprise { .. } => {}
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
    apps: Overlay<'a, AppId, AppRecord>,
    closed: SetOverlay<'a, AppId>,
    oauth_apps: Overlay<'a, ClientId, OAuthAppRecord>,
    org: Cow<'a, OrgRows>,
    /// The entry's time, which an invitation's `expires` is set from and compared with.
    at: Timestamp,
}

impl AuthzScratch<'_> {
    pub fn at(&self) -> Timestamp {
        self.at
    }
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
    fn app(&self, app: AppId) -> Option<&AppRecord> {
        self.apps.get(&app)
    }
    fn app_closed(&self, app: AppId) -> bool {
        self.closed.contains(&app)
    }
    fn oauth_app(&self, client: &ClientId) -> Option<&OAuthAppRecord> {
        self.oauth_apps.get(client)
    }
    fn org(&self) -> &OrgRows {
        &self.org
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
                apps: &mut self.apps,
                closed: &mut self.closed,
                oauth_apps: &mut self.oauth_apps,
            },
        );
        if touches_org(ev) {
            self.org.to_mut().write(ev);
        }
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
    /// A follower refuses an entry that opens the chain twice, takes a held name for another repository, resolves a
    /// name at an epoch that does not move forward, files a record under another key, or registers a closed App.
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
            AuthzEvent::App { app, record: Some(r) } if r.id != *app => Err(format!("{app:?} holds {:?}", r.id).into()),
            AuthzEvent::App { app, record: Some(_) } if self.closed.contains(app) => {
                Err(format!("{app:?} was closed and is never registered again").into())
            }
            AuthzEvent::OAuthApp { client, record: Some(r) } if r.client != *client => {
                Err(format!("{client:?} holds {:?}", r.client).into())
            }
            AuthzEvent::Invitation(
                InvitationEvent::Accepted { id }
                | InvitationEvent::Declined { id }
                | InvitationEvent::Cancelled { id, .. },
            ) if self.org.invitation(*id, self.at).is_none() => {
                Err(format!("{id:?} is not pending at {:?}", self.at).into())
            }
            AuthzEvent::Team(TeamEvent::Member { team, on: true, user }) if self.role(*user).is_none() => {
                Err(format!("{user:?} joins {team:?} without being a member").into())
            }
            AuthzEvent::Team(TeamEvent::Member { team, .. }) | AuthzEvent::TeamGrant { team, .. }
                if !self.org.teams.contains_key(team) =>
            {
                Err(format!("{team:?} does not exist").into())
            }
            AuthzEvent::InstanceAuth(_) if !self.fold.owner.is_some_and(is_install_account) => {
                Err("InstanceAuth off the install's own chain".into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for AuthzFold {
    type Event = AuthzEvent;
    type Scratch<'a> = AuthzScratch<'a>;
    fn scratch(&self) -> AuthzScratch<'_> {
        self.scratch_at(Timestamp(0), TierSet::CORE)
    }
    fn scratch_at(&self, at: Timestamp, _tiers: TierSet) -> AuthzScratch<'_> {
        AuthzScratch {
            fold: self,
            opened: self.opened,
            members: Overlay::new(&self.members),
            grants: Overlay::new(&self.grants),
            names: Overlay::new(&self.names),
            epoch: self.epoch,
            ids: self.ids,
            apps: Overlay::new(&self.more.apps),
            closed: SetOverlay::new(&self.more.closed),
            oauth_apps: Overlay::new(&self.more.oauth_apps),
            org: Cow::Borrowed(&self.more.org),
            at,
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
                apps: &mut self.more.apps,
                closed: &mut self.more.closed,
                oauth_apps: &mut self.more.oauth_apps,
            },
        );
        self.more.org.write(ev);
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
    Open {
        kind: AccountKind,
        enterprise: Option<AccountId>,
    },
    /// Adds, changes, or (with `None`) removes an org member's role. `Admin` is the admin grant.
    SetMember {
        user: UserId,
        role: Option<OrgRole>,
    },
    /// A user's direct permission on one of the owner's repositories.
    SetGrant {
        repo: RepoId,
        user: UserId,
        permission: Option<RepoPermission>,
    },
    /// Creates a repository: its id is the next of this group's id block, and its name goes `Live`.
    CreateRepo {
        name: RepoName,
    },
    /// Renames: `Live` at the new key, `Alias` at the old, both at a new epoch.
    RenameRepo {
        from: RepoNameKey,
        to: RepoName,
    },
    /// Deletes: the name is held as `Deleted` until `retain_until`.
    DeleteRepo {
        key: RepoNameKey,
        retain_until: Timestamp,
    },
    // ---- Milestone 3, format 2 ----
    /// Writes an App's record, after its slug's `ClaimSlug` committed on `Names` with this id (the commit point).
    /// Re-sending the same record is a no-op, so the edge and the `Audit` finisher may both submit it.
    PutApp {
        record: Box<AppRecord>,
    },
    /// Deletes an App's registration, or aborts one that never wrote its record. Either way the id is closed: its
    /// slug may then be released (`NamesOp::ReleaseSlug`), and the id is never registered again.
    CloseApp {
        app: AppId,
    },
    /// Writes an OAuth app's record, after its client id's `RegisterClient` committed on `Names`.
    PutOAuthApp {
        record: OAuthAppRecord,
    },
    DeleteOAuthApp {
        client: ClientId,
    },
    // ---- M3-WP3, format 2 ----
    /// A team takes the next id of the group's block.
    CreateTeam {
        name: Box<str>,
    },
    /// Only an org member joins a team.
    SetTeamMember {
        team: TeamId,
        user: UserId,
        on: bool,
    },
    DeleteTeam {
        team: TeamId,
    },
    SetTeamGrant {
        repo: RepoId,
        team: TeamId,
        permission: Option<RepoPermission>,
    },
    SetPolicy(OrgPolicy),
    /// An org invitation: the leader sets `expires` from the entry's time. A user already a member is refused.
    Invite {
        invitee: Invitee,
        role: OrgRole,
        teams: SmallVec<[TeamId; 2]>,
    },
    /// The invitee accepts: `Accepted`, `Member`, and each team's `Member` in one entry. An email invitation is
    /// accepted by the account that holds the email on `Names`, which the leader reads as a dependency.
    Accept {
        id: InvitationId,
    },
    Decline {
        id: InvitationId,
    },
    /// An org admin withdraws a pending invitation.
    Cancel {
        id: InvitationId,
    },
    /// The install's sign-in choice. Only on the install's own chain, and only from one of its admins (a site
    /// admin): never from the operator, a finisher, or another account.
    SetInstanceAuth(InstanceAuth),
    /// An org's enterprise change: the commit point. Only the operator submits it; a Workflow then appends
    /// `Settings` with the new enterprise to each of the org's repositories.
    SetEnterprise {
        enterprise: Option<AccountId>,
    },
    /// A transfer's commit point on the destination owner's chain: the name is `Reserved` for `repo`, which counts as
    /// taken. The transferring user administers both owners; the leader reads the source owner's chain.
    ReserveTransfer {
        name: RepoName,
        repo: RepoId,
        from: AccountId,
    },
    /// The destination's `Live` name, at a new epoch, after the repository's `Settings` names the new owner. The
    /// finisher submits it, and nothing refuses it but a missing reservation.
    CompleteTransfer {
        key: RepoNameKey,
        repo: RepoId,
    },
    /// The source owner's old name becomes an `Alias` once the destination's name is `Live`, so old links redirect.
    LeaveAlias {
        key: RepoNameKey,
        repo: RepoId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthzOut {
    Repo { repo: RepoId, epoch: NameEpoch },
    Done,
    Team(TeamId),
    Invitation(InvitationId),
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
    /// The record does not fit: a client id that is not the App's, a key or secret over its cap, or an
    /// installation's permissions over the App's.
    BadRecord,
    /// The App's registration was closed, so its id is never registered again.
    Closed,
    /// The registration's claim on `Names` (the App's slug, the OAuth app's client id) has not committed for this
    /// owner: claim it first.
    NotClaimed,
    NoApp,
    NoTeam,
    /// A team member, or the invitee of an org invitation, must not already be (or must be) a member.
    NotAMember,
    AlreadyMember,
    /// Missing, lapsed, accepted, declined, or cancelled.
    NoInvitation,
    /// `InstanceAuth` lives only on the install's own chain.
    NotInstall,
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
            AuthzOp::PutApp { record } => {
                e.u8(6);
                apps::app_record(&mut e, record);
            }
            AuthzOp::CloseApp { app } => {
                e.u8(7);
                apps::app_id(&mut e, *app);
            }
            AuthzOp::PutOAuthApp { record } => {
                e.u8(8);
                apps::oauth_app_record(&mut e, record);
            }
            AuthzOp::DeleteOAuthApp { client } => {
                e.u8(9);
                apps::client(&mut e, client);
            }
            AuthzOp::CreateTeam { name } => {
                e.u8(10);
                e.str(name);
            }
            AuthzOp::SetTeamMember { team, user, on } => {
                e.u8(11);
                enc::db(&mut e, team.0);
                enc::account(&mut e, user.0);
                e.u8(u8::from(*on));
            }
            AuthzOp::DeleteTeam { team } => {
                e.u8(12);
                enc::db(&mut e, team.0);
            }
            AuthzOp::SetTeamGrant { repo, team, permission } => {
                e.u8(13);
                enc::repo(&mut e, *repo);
                enc::db(&mut e, team.0);
                e.option(permission.as_ref(), |e, p| e.u8(perm(*p)));
            }
            AuthzOp::SetPolicy(p) => {
                e.u8(14);
                org::policy(&mut e, p);
            }
            AuthzOp::Invite { invitee, role, teams } => {
                e.u8(15);
                let ev = InvitationEvent::Sent {
                    id: InvitationId(graftyard_types::DbId(0)),
                    invitee: *invitee,
                    grant: InvitationGrant::Org { role: *role, teams: teams.clone() },
                    inviter: None,
                    expires: Timestamp(0),
                };
                org::invitation_event(&mut e, &ev);
            }
            AuthzOp::Accept { id } | AuthzOp::Decline { id } | AuthzOp::Cancel { id } => {
                e.u8(match self.op {
                    AuthzOp::Accept { .. } => 16,
                    AuthzOp::Decline { .. } => 17,
                    _ => 18,
                });
                enc::db(&mut e, id.0);
            }
            AuthzOp::SetInstanceAuth(a) => {
                e.u8(19);
                org::instance_auth(&mut e, a);
            }
            AuthzOp::SetEnterprise { enterprise } => {
                e.u8(20);
                e.option(enterprise.as_ref(), |e, a| enc::account(e, *a));
            }
            AuthzOp::ReserveTransfer { name, repo, from } => {
                e.u8(21);
                e.str(name.as_str());
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *from);
            }
            AuthzOp::CompleteTransfer { key, repo } | AuthzOp::LeaveAlias { key, repo } => {
                e.u8(if matches!(self.op, AuthzOp::CompleteTransfer { .. }) { 22 } else { 23 });
                e.str(key.as_str());
                enc::repo(&mut e, *repo);
            }
        }
    }
}

impl AuthzCmd {
    /// The `Names` shard of the claim a registration's record follows: the App's slug, or the OAuth app's client id.
    fn claim_chain(&self) -> Option<ChainId> {
        let key = match &self.op {
            AuthzOp::PutApp { record } => NameKey::AppSlug(record.slug.clone()),
            AuthzOp::PutOAuthApp { record } => NameKey::Client(record.client),
            _ => return None,
        };
        Some(ChainId::Names(name_shard(&key)))
    }

    /// A record commits only after its claim: the slug's `AppSlug` names this App id and owner, or the client id's
    /// route names this owner and no App. Read once per entry through the shard's `fresh`, so the commit point is
    /// the claim, and a record for a slug someone else claimed is refused.
    fn check_claim(&self, s: &AuthzScratch<'_>, deps: &[(ChainId, IdentityDep)]) -> Result<(), AuthzErr> {
        let Some(chain) = self.claim_chain() else { return Ok(()) };
        let names = match deps.iter().find(|(c, _)| *c == chain) {
            Some((_, IdentityDep::Names(f))) => f,
            _ => return Err(AuthzErr::Unauthorized(CredentialRefusal::NotRead(chain))),
        };
        let owner = s.owner().ok_or(AuthzErr::NotOpened)?;
        let claimed = match &self.op {
            AuthzOp::PutApp { record } => names.slug(&record.slug) == Some(SlugRow { app: record.id, owner }),
            AuthzOp::PutOAuthApp { record } => names.client(&record.client) == Some(ClientRoute { owner, app: None }),
            _ => true,
        };
        if claimed { Ok(()) } else { Err(AuthzErr::NotClaimed) }
    }

    /// The other chains this op reads beyond the credential: the claim a registration follows, the `Names` shard of an
    /// email invitation's email (named by the fold's pending row), and a transfer's source owner.
    fn extra_deps(&self, fold: &AuthzFold) -> SmallVec<[ChainId; 2]> {
        let mut d = SmallVec::new();
        d.extend(self.claim_chain());
        match &self.op {
            AuthzOp::Accept { id } => {
                if let Some(InvitationRow { invitee: Invitee::Email(email), .. }) = fold.more.org.invitations.get(id) {
                    d.push(ChainId::Names(name_shard(&NameKey::Email(*email))));
                }
            }
            AuthzOp::ReserveTransfer { from, .. } if self.cred.account.is_some() => d.push(ChainId::Authz(*from)),
            _ => {}
        }
        d
    }

    /// Ops a member may submit for itself: answering its own invitation. Everything else needs an admin of the owner,
    /// or no account (the operator, the CLI, a finisher), and `SetInstanceAuth` needs an admin and refuses the rest.
    fn decide_m3(&self, s: &AuthzScratch<'_>, deps: &[(ChainId, IdentityDep)]) -> Option<Decided> {
        let actor = self.cred.account.map(UserId);
        let one = |ev: AuthzEvent, out: AuthzOut| Ok((smallvec![ev], out));
        let pending = |id: InvitationId| s.org.invitation(id, s.at).cloned().ok_or(AuthzErr::NoInvitation);
        let r = match &self.op {
            AuthzOp::Accept { id } => (|| {
                let row = pending(*id)?;
                let user = match row.invitee {
                    Invitee::User(u) => u,
                    Invitee::Email(email) => {
                        let chain = ChainId::Names(name_shard(&NameKey::Email(email)));
                        let names = match deps.iter().find(|(c, _)| *c == chain) {
                            Some((_, IdentityDep::Names(f))) => f,
                            _ => return Err(AuthzErr::Unauthorized(CredentialRefusal::NotRead(chain))),
                        };
                        UserId(names.email(&email).ok_or(AuthzErr::Forbidden)?)
                    }
                };
                if actor != Some(user) {
                    return Err(AuthzErr::Forbidden);
                }
                if s.role(user).is_some() {
                    return Err(AuthzErr::AlreadyMember);
                }
                let InvitationGrant::Org { role, teams } = row.grant else { return Err(AuthzErr::NoInvitation) };
                let mut evs: SmallVec<[AuthzEvent; 4]> = smallvec![
                    AuthzEvent::Invitation(InvitationEvent::Accepted { id: *id }),
                    AuthzEvent::Member { user, role: Some(role) },
                ];
                for t in teams.iter().filter(|t| s.org.teams.contains_key(t)) {
                    evs.push(AuthzEvent::Team(TeamEvent::Member { team: *t, user, on: true }));
                }
                Ok((evs, AuthzOut::Done))
            })(),
            AuthzOp::Decline { id } => (|| {
                let row = pending(*id)?;
                match row.invitee {
                    Invitee::User(u) if actor == Some(u) => {}
                    // An email invitee declines by the account that holds the email; an admin cancels instead.
                    _ => return Err(AuthzErr::Forbidden),
                }
                one(AuthzEvent::Invitation(InvitationEvent::Declined { id: *id }), AuthzOut::Done)
            })(),
            AuthzOp::SetInstanceAuth(a) => (|| {
                let owner = s.owner().ok_or(AuthzErr::NotOpened)?;
                if !is_install_account(owner) {
                    return Err(AuthzErr::NotInstall);
                }
                // A site admin: an admin of the install's own org. The operator and finishers name no account.
                if !actor.is_some_and(|u| s.is_admin(u)) {
                    return Err(AuthzErr::Forbidden);
                }
                one(AuthzEvent::InstanceAuth(a.clone()), AuthzOut::Done)
            })(),
            _ => return None,
        };
        Some(r)
    }

    fn decide_op(&self, s: &AuthzScratch<'_>) -> Result<(SmallVec<[AuthzEvent; 4]>, AuthzOut), AuthzErr> {
        if let AuthzOp::Open { kind, enterprise } = &self.op {
            if !self.cred.is_operator() {
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
        // `graft-ops` and the CLI name no account and act as a site admin; an installation token is neither.
        let admin = match self.cred.account {
            Some(a) => s.is_admin(UserId(a)),
            None => self.cred.is_operator(),
        };
        if !admin {
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
            AuthzOp::PutApp { record } => {
                let fits = record.client == ClientId::for_app(record.id)
                    && record.keys.len() <= 2
                    && record.client_secrets.len() <= apps::CLIENT_SECRETS_MAX;
                if !fits {
                    return Err(AuthzErr::BadRecord);
                }
                if s.app_closed(record.id) {
                    return Err(AuthzErr::Closed);
                }
                if s.app(record.id) == Some(&**record) {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                let ev = AuthzEvent::App { app: record.id, record: Some(record.clone()) };
                Ok((smallvec![ev], AuthzOut::Done))
            }
            AuthzOp::CloseApp { app } => {
                if s.app_closed(*app) {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                Ok((smallvec![AuthzEvent::App { app: *app, record: None }], AuthzOut::Done))
            }
            AuthzOp::PutOAuthApp { record } => {
                if record.client.is_app_form() || record.client_secrets.len() > apps::CLIENT_SECRETS_MAX {
                    return Err(AuthzErr::BadRecord);
                }
                if s.oauth_app(&record.client) == Some(record) {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                let ev = AuthzEvent::OAuthApp { client: record.client, record: Some(record.clone()) };
                Ok((smallvec![ev], AuthzOut::Done))
            }
            AuthzOp::DeleteOAuthApp { client } => {
                if s.oauth_app(client).is_none() {
                    return Err(AuthzErr::NoApp);
                }
                Ok((smallvec![AuthzEvent::OAuthApp { client: *client, record: None }], AuthzOut::Done))
            }
            AuthzOp::Accept { .. } | AuthzOp::Decline { .. } | AuthzOp::SetInstanceAuth(_) => {
                unreachable!("decide_m3's")
            }
            AuthzOp::CreateTeam { name } => {
                org_only(s)?;
                let team = TeamId(s.ids.peek().ok_or(AuthzErr::NeedIdBlock)?);
                Ok((smallvec![AuthzEvent::Team(TeamEvent::Created { team, name: name.clone() })], AuthzOut::Team(team)))
            }
            AuthzOp::SetTeamMember { team, user, on } => {
                org_only(s)?;
                if !s.org.teams.contains_key(team) {
                    return Err(AuthzErr::NoTeam);
                }
                if *on && s.role(*user).is_none() {
                    return Err(AuthzErr::NotAMember);
                }
                Ok((
                    smallvec![AuthzEvent::Team(TeamEvent::Member { team: *team, user: *user, on: *on })],
                    AuthzOut::Done,
                ))
            }
            AuthzOp::DeleteTeam { team } => {
                if !s.org.teams.contains_key(team) {
                    return Err(AuthzErr::NoTeam);
                }
                Ok((smallvec![AuthzEvent::Team(TeamEvent::Deleted { team: *team })], AuthzOut::Done))
            }
            AuthzOp::SetTeamGrant { repo, team, permission } => {
                if !s.org.teams.contains_key(team) {
                    return Err(AuthzErr::NoTeam);
                }
                let ev = AuthzEvent::TeamGrant { repo: *repo, team: *team, permission: *permission };
                Ok((smallvec![ev], AuthzOut::Done))
            }
            AuthzOp::SetPolicy(p) => {
                org_only(s)?;
                Ok((smallvec![AuthzEvent::Policy(*p)], AuthzOut::Done))
            }
            AuthzOp::Invite { invitee, role, teams } => {
                org_only(s)?;
                if let Invitee::User(u) = invitee
                    && s.role(*u).is_some()
                {
                    return Err(AuthzErr::AlreadyMember);
                }
                if teams.iter().any(|t| !s.org.teams.contains_key(t)) {
                    return Err(AuthzErr::NoTeam);
                }
                let id = InvitationId(s.ids.peek().ok_or(AuthzErr::NeedIdBlock)?);
                let ev = InvitationEvent::Sent {
                    id,
                    invitee: *invitee,
                    grant: InvitationGrant::Org { role: *role, teams: teams.clone() },
                    inviter: self.cred.account,
                    expires: Timestamp(s.at.0.saturating_add(INVITATION_TTL.as_millis() as u64)),
                };
                Ok((smallvec![AuthzEvent::Invitation(ev)], AuthzOut::Invitation(id)))
            }
            AuthzOp::Cancel { id } => {
                s.org.invitation(*id, s.at).ok_or(AuthzErr::NoInvitation)?;
                let ev = InvitationEvent::Cancelled { id: *id, actor: self.cred.account };
                Ok((smallvec![AuthzEvent::Invitation(ev)], AuthzOut::Done))
            }
            AuthzOp::SetEnterprise { enterprise } => {
                org_only(s)?;
                if !self.cred.is_operator() {
                    return Err(AuthzErr::Forbidden);
                }
                if s.enterprise() == *enterprise {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                Ok((smallvec![AuthzEvent::Enterprise { enterprise: *enterprise }], AuthzOut::Done))
            }
            AuthzOp::ReserveTransfer { name, repo, from } => {
                let key = name.key();
                match s.name(&key) {
                    Some(RepoEntry::Reserved { repo: r, .. }) if r == *repo => {
                        return Ok((SmallVec::new(), AuthzOut::Done));
                    }
                    e if !free_for(e, *repo) => return Err(AuthzErr::Taken),
                    _ => {}
                }
                let entry = RepoEntry::Reserved { repo: *repo, from: *from };
                Ok((smallvec![AuthzEvent::RepoName { key, entry: Some(entry) }], AuthzOut::Done))
            }
            AuthzOp::CompleteTransfer { key, repo } => match s.name(key) {
                Some(RepoEntry::Live { repo: r, epoch }) if r == *repo => {
                    Ok((SmallVec::new(), AuthzOut::Repo { repo: r, epoch }))
                }
                Some(RepoEntry::Reserved { repo: r, .. }) if r == *repo => {
                    let entry = RepoEntry::Live { repo: *repo, epoch: next };
                    Ok((
                        smallvec![AuthzEvent::RepoName { key: key.clone(), entry: Some(entry) }],
                        AuthzOut::Repo { repo: *repo, epoch: next },
                    ))
                }
                _ => Err(AuthzErr::NoRepo),
            },
            AuthzOp::LeaveAlias { key, repo } => match s.name(key) {
                Some(RepoEntry::Alias { repo: r, .. }) if r == *repo => Ok((SmallVec::new(), AuthzOut::Done)),
                Some(RepoEntry::Live { repo: r, .. }) if r == *repo => {
                    let entry = RepoEntry::Alias { repo: *repo, epoch: next };
                    Ok((smallvec![AuthzEvent::RepoName { key: key.clone(), entry: Some(entry) }], AuthzOut::Done))
                }
                _ => Err(AuthzErr::NoRepo),
            },
        }
    }
}

impl Command for AuthzCmd {
    type Fold = AuthzFold;
    type Dep = IdentityDep;
    type Out = AuthzOut;
    type Err = AuthzErr;

    /// Milestone 1's ops keep format 1, so their entries are what an older release reads; the rest are format 2.
    fn format(&self) -> Format {
        match self.op {
            AuthzOp::PutApp { .. }
            | AuthzOp::CloseApp { .. }
            | AuthzOp::PutOAuthApp { .. }
            | AuthzOp::DeleteOAuthApp { .. } => AUTHZ_FORMAT_V2,
            AuthzOp::Open { .. }
            | AuthzOp::SetMember { .. }
            | AuthzOp::SetGrant { .. }
            | AuthzOp::CreateRepo { .. }
            | AuthzOp::RenameRepo { .. }
            | AuthzOp::DeleteRepo { .. } => AUTHZ_FORMAT,
            _ => AUTHZ_FORMAT_V2,
        }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, fold: &AuthzFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.cred, None);
        for c in self.extra_deps(fold) {
            if !d.contains(&c) {
                d.push(c);
            }
        }
        d
    }
    fn decide(
        &self,
        s: &AuthzScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, AuthzOut), AuthzErr> {
        check_credential(&self.cred, Own::Other, deps).map_err(AuthzErr::Unauthorized)?;
        self.check_claim(s, deps)?;
        if let Some(r) = self.decide_m3(s, deps) {
            return r;
        }
        if let (AuthzOp::ReserveTransfer { from, .. }, Some(a)) = (&self.op, self.cred.account) {
            let chain = ChainId::Authz(*from);
            match deps.iter().find(|(c, _)| *c == chain) {
                Some((_, IdentityDep::Authz(f))) if f.owner() == Some(*from) && f.is_admin(UserId(a)) => {}
                Some(_) => return Err(AuthzErr::Forbidden),
                None => return Err(AuthzErr::Unauthorized(CredentialRefusal::NotRead(chain))),
            }
        }
        self.decide_op(s)
    }
    fn replay(&self, events: &[AuthzEvent]) -> AuthzOut {
        match events.first() {
            Some(AuthzEvent::RepoName { entry: Some(RepoEntry::Live { repo, epoch }), .. }) => {
                AuthzOut::Repo { repo: *repo, epoch: *epoch }
            }
            Some(AuthzEvent::Team(TeamEvent::Created { team, .. })) => AuthzOut::Team(*team),
            Some(AuthzEvent::Invitation(InvitationEvent::Sent { id, .. })) => AuthzOut::Invitation(*id),
            _ => AuthzOut::Done,
        }
    }
}
