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
use graftyard_types::{AccountId, ChainId, CredentialShard, Digest, Index, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::agents::{self, AgentApp, AgentKind, WorkspaceMode};
use crate::apps::{self, AppId, AppRecord, ClientId, InstallationId, OAuthAppRecord, PermissionSet};
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
use crate::sealed::{ActionsName, SealKey, SecretRecord, SecretScope, VariableRecord};
use crate::tokens::RepoScope;
use crate::{
    AUTHZ_FORMAT, AUTHZ_FORMAT_V2, AUTHZ_FORMAT_V3, AUTHZ_FORMAT_V4, AUTHZ_FORMAT_V11, AUTHZ_FORMAT_V12,
    AUTHZ_FORMAT_V13, AUTHZ_FORMAT_V14, AUTHZ_FORMAT_V15,
};

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

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
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
    // ---- M3-WP4, format 2 ----
    /// SHARED's `Installation { id, state: InstallState, not_before }`. SHARED does not define `InstallState`, so the
    /// state is the row itself: `Some` while installed, `None` once uninstalled [inference]. Either way `not_before`
    /// is the time before which no stateless token of the installation is honored, so an uninstall or a narrowing
    /// fails every token minted before it at once.
    Installation {
        id: InstallationId,
        row: Option<InstallationRow>,
        not_before: Timestamp,
    },
    // ---- AM1-WP2, format 3 ----
    /// DESIGN's `AuthzEvent::AgentApp(AgentApp)`, written only on the install's own chain. `row: None` removes the
    /// flag (`DELETE /agents/apps/{id}`) [inference: the design names the event with a row and no removal form].
    AgentApp {
        app: AppId,
        row: Option<AgentApp>,
    },
    /// `OrgPolicy.agents_enabled` and `agents_workspace`, a whole record each time. An event of its own, so a `Policy`
    /// entry's bytes stay M3-WP3's [inference: DESIGN adds the fields to `OrgPolicy` and names no event].
    OrgAgents {
        enabled: bool,
        workspace: WorkspaceMode,
    },
    // ---- M3's close, format 4 ----
    /// The `Audit` consumer finished, or found nothing owed for, every entry of this chain through `through`
    /// (`crate::audit`): a transfer's `Reserved` name and an enterprise change commit here. Not in the design's
    /// `AuthzEvent`, as `NamesEvent::AuditCursor`.
    AuditCursor {
        through: Index,
    },
    // ---- F59, format 11 ----
    /// A repository create's commit point carries what its finisher needs (`crate::audit`, `Owed::Create`): in the
    /// same entry as the name's `Live`, the repository and the settings the create asked for, as the JSON of
    /// `graftyard_git`'s `RepoSettings`, which this crate never reads. The fold keeps nothing of it: only the log
    /// does, which the consumer scans. Not in the design's `AuthzEvent` [inference: F59 names no event].
    RepoCreate {
        key: RepoNameKey,
        repo: RepoId,
        init: Box<str>,
    },
    // ---- M4-WP3, format 12 ----
    /// SHARED's `SealKey`, one of the owner's: minted, retired (`retired` set), or dropped (`key: None`) once no record
    /// names it.
    SealKey {
        key_id: Box<str>,
        key: Option<SealKey>,
    },
    /// An org secret: its scope (`SecretScope::Org`) and the sealed bytes the client sent. `row: None` deletes it.
    OrgSecret {
        name: ActionsName,
        row: Option<(SecretScope, SecretRecord)>,
    },
    /// An org variable. `row: None` deletes it.
    OrgVariable {
        name: ActionsName,
        row: Option<(SecretScope, VariableRecord)>,
    },
    // ---- BF-76, format 15 ----
    /// The org's Actions substitution policy, the whole record each time (`design/SHARED.md`, "Toolkit hostname
    /// heuristics and SHA pins"; `decisions/DECISIONS.md`, "`pin-map`").
    ActionsSubstitution(ActionsSubstitution),
}

/// BF-76's org policy: `tags` substitutes `actions/<name>@<tag>` by its `graftyard-actions` fork, and `pin_map` is the
/// SHA-256 of the one pin map an org admin approved, whose pins substitute SHA pins. Off and none until set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionsSubstitution {
    pub tags: bool,
    pub pin_map: Option<Digest>,
}

/// An App's installation on this owner: its App, the repositories it reaches, and its permissions, which never
/// exceed the App's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationRow {
    pub app: AppId,
    pub repos: RepoScope,
    pub permissions: PermissionSet,
}

/// The Actions App's installation row on every owner it is installed on: every repository, and each `GITHUB_TOKEN`
/// scope GHES 3.22 names at write plus `metadata` read, the cap a workflow's `permissions:` reaches with `write-all`
/// [doc: GHES 3.22, "Workflow syntax for GitHub Actions", `permissions`]. A job's token narrows it to one repository
/// and the job's permissions (`graftyard_actions::token`).
pub fn actions_row() -> InstallationRow {
    let mut p: BTreeMap<Box<str>, apps::Access> =
        crate::stateless::ACTIONS_SCOPES.iter().map(|s| (Box::from(*s), apps::Access::Write)).collect();
    p.insert("metadata".into(), apps::Access::Read);
    InstallationRow { app: crate::stateless::ACTIONS_APP, repos: RepoScope::All, permissions: PermissionSet(p) }
}

/// An installation's current state on its owner's chain, as `authorize` reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationState {
    pub row: Option<InstallationRow>,
    pub not_before: Timestamp,
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
            AuthzEvent::Installation { id, row, not_before } => {
                e.u8(5);
                enc::db(&mut e, id.0);
                e.option(row.as_ref(), installation_row);
                enc::at(&mut e, *not_before);
            }
            AuthzEvent::AgentApp { app, row } => {
                e.u8(14);
                apps::app_id(&mut e, *app);
                e.option(row.as_ref(), agents::agent_app);
            }
            AuthzEvent::OrgAgents { enabled, workspace } => {
                e.u8(15);
                e.u8(u8::from(*enabled));
                e.u8(*workspace as u8);
            }
            AuthzEvent::AuditCursor { through } => {
                e.u8(16);
                e.u64(through.0);
            }
            AuthzEvent::RepoCreate { key, repo, init } => {
                e.u8(17);
                e.str(key.as_str());
                enc::repo(&mut e, *repo);
                e.str(init);
            }
            // M4-WP3: by the canonical rule of `graftyard_fold::canon`, as the repository's newer events are.
            AuthzEvent::SealKey { key_id, key } => {
                e.u8(18);
                graftyard_fold::canon::write(e.0, &(key_id, key));
            }
            AuthzEvent::OrgSecret { name, row } => {
                e.u8(19);
                graftyard_fold::canon::write(e.0, &(name, row));
            }
            AuthzEvent::OrgVariable { name, row } => {
                e.u8(20);
                graftyard_fold::canon::write(e.0, &(name, row));
            }
            AuthzEvent::ActionsSubstitution(p) => {
                e.u8(21);
                graftyard_fold::canon::write(e.0, p);
            }
        }
    }
}

/// Whether every repository `a` reaches, `b` reaches too.
fn scope_within(a: &RepoScope, b: &RepoScope) -> bool {
    match (a, b) {
        (_, RepoScope::All) => true,
        (RepoScope::All, RepoScope::Selected(_)) => false,
        (RepoScope::Selected(a), RepoScope::Selected(b)) => a.iter().all(|r| b.contains(r)),
    }
}

fn installation_row(e: &mut Enc, r: &InstallationRow) {
    apps::app_id(e, r.app);
    match &r.repos {
        RepoScope::All => e.u8(0),
        RepoScope::Selected(rs) => {
            e.u8(1);
            e.count(rs.len());
            for r in rs {
                enc::repo(e, *r);
            }
        }
    }
    apps::permissions(e, &r.permissions);
}

/// Milestone 3's rows of an `Authz` chain: the fold's trailing part (`crate::compat`). `closed` holds the Apps whose
/// registration was deleted or aborted, so their slug may be released and their id is never registered again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthzMore {
    pub apps: BTreeMap<AppId, AppRecord>,
    pub closed: std::collections::BTreeSet<AppId>,
    pub oauth_apps: BTreeMap<ClientId, OAuthAppRecord>,
    pub org: OrgRows,
    /// M3-WP4's: every installation this owner has had, removed ones kept for their `not_before`.
    pub installations: BTreeMap<InstallationId, InstallationState>,
    /// AM1-WP2's rows, a trailing part of their own, so a fold with none keeps M3's bytes.
    pub more: AgentRows,
}

tail_serde!(AuthzMore {
    apps: BTreeMap<AppId, AppRecord>,
    closed: std::collections::BTreeSet<AppId>,
    oauth_apps: BTreeMap<ClientId, OAuthAppRecord>,
    org: OrgRows,
    installations: BTreeMap<InstallationId, InstallationState>,
} more: AgentRows);

/// AM1-WP2's rows: the install's flagged Apps (on its own chain only) and an org's agent settings, which `OrgPolicy`
/// carries in memory and this part stores, so `OrgPolicy`'s stored shape stays M3-WP3's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentRows {
    pub agent_apps: BTreeMap<AppId, AgentApp>,
    pub agents_enabled: bool,
    pub agents_workspace: WorkspaceMode,
    /// The `Audit` consumer's cursor (`AuthzEvent::AuditCursor`, format 0x12.4), then M4-WP3's Actions rows: a
    /// trailing part of its own, so a fold whose cursor never moved and that holds no Actions row keeps AM1-WP2's bytes.
    pub more: CursorRows,
}

tail_serde!(AgentRows {
    agent_apps: BTreeMap<AppId, AgentApp>,
    agents_enabled: bool,
    agents_workspace: WorkspaceMode,
} more: CursorRows);

/// The `Audit` cursor, then M4-WP3's rows as a trailing part, so a fold with none keeps its cursor's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CursorRows {
    pub cursor: Index,
    pub more: ActionsRows,
}

impl serde::Serialize for CursorRows {
    /// A fold with no Actions row writes the cursor alone, as the `Index` it was before M4-WP3.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        if self.more == ActionsRows::default() {
            return self.cursor.serialize(s);
        }
        let mut t = s.serialize_tuple(3)?;
        t.serialize_element(&self.cursor)?;
        t.serialize_element(&1u8)?;
        t.serialize_element(&self.more)?;
        t.end()
    }
}

impl<'de> serde::Deserialize<'de> for CursorRows {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = CursorRows;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("CursorRows")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<CursorRows, A::Error> {
                let cursor: Index = seq.next_element()?.ok_or_else(|| serde::de::Error::missing_field("cursor"))?;
                let more = match seq.next_element::<u8>() {
                    Ok(Some(1)) => seq.next_element()?.ok_or_else(|| serde::de::Error::missing_field("more"))?,
                    Ok(Some(m)) => {
                        return Err(serde::de::Error::invalid_value(
                            serde::de::Unexpected::Unsigned(u64::from(m)),
                            &"1, the marker of a fold's trailing part",
                        ));
                    }
                    Ok(None) | Err(_) => ActionsRows::default(),
                };
                Ok(CursorRows { cursor, more })
            }
        }
        d.deserialize_tuple(3, V)
    }
}

/// M4-WP3's rows on the owner's chain (`design/SHARED.md`, "Actions", Secrets and variables): the owner's `SealKey`s
/// by `key_id`, and its org secrets and variables by name, each with the scope that says which repositories read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionsRows {
    pub seal_keys: BTreeMap<Box<str>, SealKey>,
    pub secrets: BTreeMap<ActionsName, (SecretScope, SecretRecord)>,
    pub variables: BTreeMap<ActionsName, (SecretScope, VariableRecord)>,
    /// BF-76's rows (format 0x12.15), a trailing part so a fold that never set them keeps M4-WP3's bytes.
    pub more: SubstitutionRows,
}

tail_serde!(ActionsRows {
    seal_keys: BTreeMap<Box<str>, SealKey>,
    secrets: BTreeMap<ActionsName, (SecretScope, SecretRecord)>,
    variables: BTreeMap<ActionsName, (SecretScope, VariableRecord)>,
} more: SubstitutionRows);

/// The org's substitution policy (`AuthzEvent::ActionsSubstitution`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutionRows {
    pub substitution: ActionsSubstitution,
}

impl ActionsRows {
    fn write(&mut self, ev: &AuthzEvent) {
        match ev {
            AuthzEvent::SealKey { key_id, key: Some(k) } => {
                self.seal_keys.insert(key_id.clone(), k.clone());
            }
            AuthzEvent::SealKey { key_id, key: None } => {
                self.seal_keys.remove(key_id);
            }
            AuthzEvent::OrgSecret { name, row: Some(r) } => {
                self.secrets.insert(name.clone(), r.clone());
            }
            AuthzEvent::OrgSecret { name, row: None } => {
                self.secrets.remove(name);
            }
            AuthzEvent::OrgVariable { name, row: Some(r) } => {
                self.variables.insert(name.clone(), r.clone());
            }
            AuthzEvent::OrgVariable { name, row: None } => {
                self.variables.remove(name);
            }
            AuthzEvent::ActionsSubstitution(p) => self.more.substitution = *p,
            _ => {}
        }
    }

    /// The key `GET .../secrets/public-key` returns: the newest unretired.
    pub fn current_key(&self) -> Option<&SealKey> {
        self.seal_keys.values().filter(|k| k.retired.is_none()).max_by_key(|k| (k.created, k.key_id.clone()))
    }
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
            // The agent fields are `OrgAgents`'s, never a `Policy` entry's (`OrgPolicy`'s note).
            AuthzEvent::Policy(p) => {
                self.policy = OrgPolicy { agents_enabled: false, agents_workspace: WorkspaceMode::Fork, ..*p }
            }
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

    /// Every entry through this index is one the `Audit` consumer finished or found nothing owed for.
    pub fn audit_cursor(&self) -> Index {
        self.more.more.more.cursor
    }

    /// The owner's Actions rows: `SealKey`s and org secrets and variables.
    pub fn actions(&self) -> &ActionsRows {
        &self.more.more.more.more
    }

    /// The name `/repositories/{id}` renders a repository under, for the owner its `RepoOwner` row names: its `Live`
    /// name, else its `Alias`. A transfer commits the old owner's `Alias` before `RepoOwner` moves to the new one
    /// (`finish::transfer`), so between the two the row's owner holds only the `Alias`, which still names the
    /// repository; without it the id route answered 404 for that window (BF-30, `tests/m3/results/m3-finish.md`).
    pub fn id_name(&self, repo: RepoId) -> Option<&RepoNameKey> {
        let find = |live: bool| {
            self.names.iter().find_map(|(k, e)| match e {
                RepoEntry::Live { repo: r, .. } if live && *r == repo => Some(k),
                RepoEntry::Alias { repo: r, .. } if !live && *r == repo => Some(k),
                _ => None,
            })
        };
        find(true).or_else(|| find(false))
    }

    /// Whether this owner, the one the repository's `RepoOwner` row names, still holds it under a name: `Live`, the
    /// `Alias` a transfer leaves before `RepoOwner` moves, or a `Reserved` name. Not holding it is not enough to call
    /// it gone (`repo_gone`): a transfer's `Alias` can be taken by a new create before `RepoOwner` moves.
    pub fn holds(&self, repo: RepoId) -> bool {
        self.names.values().any(|e| match e {
            RepoEntry::Live { repo: r, .. }
            | RepoEntry::Alias { repo: r, .. }
            | RepoEntry::Reserved { repo: r, .. } => *r == repo,
            RepoEntry::Deleted { .. } => false,
        })
    }

    /// `repo-gone` (`decisions/DECISIONS.md`): whether a cursor consumer whose write to `repo` failed ends that work
    /// instead of retrying it. It needs two proofs:
    /// - **No group.** The lookup answered no group (`lookup_no_group`, `graftyard_lookup::is_no_group`), and every
    ///   site's registry answered that it holds no row for the chain (`rows`, `graftyard_types::no_row_anywhere`).
    ///   The lookup's answer is relative to the record it used: under an owner's record whose placement the group
    ///   has left, or with a registry read that timed out, it can say no group for a live group. The rows cannot:
    ///   every voter writes one on its own site before it stores anything, and only a purge deletes one.
    /// - **No name.** This owner, the one its `RepoOwner` row names, holds it under no name.
    ///
    /// A repository in a transfer can fail the name proof while live, once a new create takes its `Alias` and before
    /// `RepoOwner` moves, and the lookup's no group too when the group already moved to the new owner's placement, but
    /// its voters' rows stay; one that is only down fails both. A deleted repository whose group still answers is the
    /// group's own `Gone`, not this.
    pub fn repo_gone(&self, repo: RepoId, lookup_no_group: bool, rows: &[graftyard_types::RowRead]) -> bool {
        lookup_no_group && graftyard_types::no_row_anywhere(rows) && !self.holds(repo)
    }

    /// The `retain_until` of a name this owner holds `Deleted` for the repository (F59's delete finisher).
    pub fn deleted(&self, repo: RepoId) -> Option<Timestamp> {
        self.names.values().find_map(|e| match e {
            RepoEntry::Deleted { repo: r, retain_until } if *r == repo => Some(*retain_until),
            _ => None,
        })
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
    /// An installation on this owner, live or removed.
    fn installation(&self, id: InstallationId) -> Option<&InstallationState>;
    /// The App's `AgentApp` row: only the install's own chain holds any.
    fn agent_app(&self, app: AppId) -> Option<&AgentApp>;
    /// The whole org policy, with its agent fields.
    fn policy(&self) -> OrgPolicy;
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
    fn installation(&self, id: InstallationId) -> Option<&InstallationState> {
        self.more.installations.get(&id)
    }
    fn agent_app(&self, app: AppId) -> Option<&AgentApp> {
        self.more.more.agent_apps.get(&app)
    }
    fn policy(&self) -> OrgPolicy {
        OrgPolicy {
            agents_enabled: self.more.more.agents_enabled,
            agents_workspace: self.more.more.agents_workspace,
            ..self.more.org.policy
        }
    }
}

struct Tables<'t, M, G, N, A, C, O, I, X> {
    opened: &'t mut Option<AccountKind>,
    members: &'t mut M,
    grants: &'t mut G,
    names: &'t mut N,
    epoch: &'t mut NameEpoch,
    ids: &'t mut IdCursor,
    apps: &'t mut A,
    closed: &'t mut C,
    oauth_apps: &'t mut O,
    installations: &'t mut I,
    agent_apps: &'t mut X,
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

fn write<M, G, N, A, C, O, I, X>(ev: &AuthzEvent, t: Tables<'_, M, G, N, A, C, O, I, X>)
where
    I: Table<InstallationId, InstallationState>,
    X: Table<AppId, AgentApp>,
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
        AuthzEvent::Installation { id, row, not_before } => {
            t.installations.put(*id, InstallationState { row: row.clone(), not_before: *not_before })
        }
        AuthzEvent::AgentApp { app, row: Some(r) } => t.agent_apps.put(*app, r.clone()),
        AuthzEvent::AgentApp { app, row: None } => t.agent_apps.del(app),
        AuthzEvent::OrgAgents { .. } | AuthzEvent::AuditCursor { .. } | AuthzEvent::RepoCreate { .. } => {}
        // `ActionsRows::write`'s.
        AuthzEvent::SealKey { .. }
        | AuthzEvent::OrgSecret { .. }
        | AuthzEvent::OrgVariable { .. }
        | AuthzEvent::ActionsSubstitution(_) => {}
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
    installations: Overlay<'a, InstallationId, InstallationState>,
    agent_apps: Overlay<'a, AppId, AgentApp>,
    /// `OrgPolicy.agents_enabled` and `agents_workspace`, which `AgentRows` stores.
    agents: (bool, WorkspaceMode),
    org: Cow<'a, OrgRows>,
    /// M4-WP3's rows, copied on the entry's first write to them.
    actions: Cow<'a, ActionsRows>,
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
    fn installation(&self, id: InstallationId) -> Option<&InstallationState> {
        self.installations.get(&id)
    }
    fn agent_app(&self, app: AppId) -> Option<&AgentApp> {
        self.agent_apps.get(&app)
    }
    fn policy(&self) -> OrgPolicy {
        OrgPolicy { agents_enabled: self.agents.0, agents_workspace: self.agents.1, ..self.org.policy }
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
                installations: &mut self.installations,
                agent_apps: &mut self.agent_apps,
            },
        );
        if touches_org(ev) {
            self.org.to_mut().write(ev);
        }
        if matches!(
            ev,
            AuthzEvent::SealKey { .. }
                | AuthzEvent::OrgSecret { .. }
                | AuthzEvent::OrgVariable { .. }
                | AuthzEvent::ActionsSubstitution(_)
        ) {
            self.actions.to_mut().write(ev);
        }
        if let AuthzEvent::OrgAgents { enabled, workspace } = ev {
            self.agents = (*enabled, *workspace);
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
            AuthzEvent::RepoCreate { key, repo, .. } if !matches!(self.name(key), Some(RepoEntry::Live { repo: r, .. }) if r == *repo) => {
                Err(format!("{key:?} is not live for {repo:?}").into())
            }
            AuthzEvent::SealKey { key_id, key: Some(k) } if k.key_id != *key_id => {
                Err(format!("{key_id} holds {}", k.key_id).into())
            }
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
            AuthzEvent::AgentApp { .. } if self.fold.owner != Some(agents::install_account()) => {
                Err("AgentApp off Authz(Genesis.account)".into())
            }
            AuthzEvent::AgentApp { app, row: Some(r) } if r.app != *app => {
                Err(format!("{app:?} holds {:?}", r.app).into())
            }
            AuthzEvent::Installation { id, row, not_before } => {
                let prev = self.installation(*id);
                if prev.is_some_and(|p| *not_before < p.not_before) {
                    return Err(format!("{id:?}'s not_before moves back").into());
                }
                match row {
                    Some(r) if prev.and_then(|p| p.row.as_ref()).is_some_and(|p| p.app != r.app) => {
                        Err(format!("{id:?} changes its App").into())
                    }
                    _ => Ok(()),
                }
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
            installations: Overlay::new(&self.more.installations),
            agent_apps: Overlay::new(&self.more.more.agent_apps),
            agents: (self.more.more.agents_enabled, self.more.more.agents_workspace),
            org: Cow::Borrowed(&self.more.org),
            actions: Cow::Borrowed(&self.more.more.more.more),
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
                installations: &mut self.more.installations,
                agent_apps: &mut self.more.more.agent_apps,
            },
        );
        self.more.org.write(ev);
        self.more.more.more.more.write(ev);
        if let AuthzEvent::OrgAgents { enabled, workspace } = ev {
            self.more.more.agents_enabled = *enabled;
            self.more.more.agents_workspace = *workspace;
        }
        // Never back: two cursor commands in one entry leave the later of the two.
        if let AuthzEvent::AuditCursor { through } = ev {
            self.more.more.more.cursor = self.more.more.more.cursor.max(*through);
        }
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

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
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
    // ---- M3-WP4, format 2 ----
    /// Installs an App on this owner, or changes an installation's repositories or permissions, after the
    /// installation id's `RegisterInstallation` committed on `Names` for this App and owner (the commit point). The
    /// leader reads that route and the App owner's `Authz` chain, `app_owner`, where the App's record must be, and
    /// refuses permissions beyond the record's. A narrowing (a repository or a permission dropped or lowered) moves
    /// `not_before` to the entry's time, so tokens minted before it fail at once.
    Install {
        id: InstallationId,
        row: InstallationRow,
        app_owner: AccountId,
    },
    /// Uninstalls: the row goes, and `not_before` moves to the entry's time.
    Uninstall {
        id: InstallationId,
    },
    // ---- AM1-WP2, format 3 ----
    /// `PUT /agents/apps/{app_id}`: a site admin flags an App, only on `Authz(Genesis.account)`. A `GraftyardRun` or
    /// `Mcp` row needs an App the install owns, one whose record is on this chain.
    PutAgentApp(AgentApp),
    /// `DELETE /agents/apps/{app_id}`. Removing a row that is not there is a no-op.
    DeleteAgentApp {
        app: AppId,
    },
    /// `PATCH /orgs/{org}/agents`: the org policy's agent fields, read and written in one decision so the rest of the
    /// policy is never overwritten from a stale read.
    SetOrgAgents {
        enabled: Option<bool>,
        workspace: Option<WorkspaceMode>,
    },
    // ---- M3's close, format 4 ----
    /// The `Audit` consumer's cursor on this chain. Only the consumer sends it, with no account and no key.
    AuditCursor {
        through: Index,
    },
    // ---- F59, format 11 ----
    /// `CreateRepo` with the settings the create asked for, `init` (at most `CREATE_INIT_MAX` bytes), so the `Audit`
    /// consumer can finish a create whose edge died: the entry also records `AuthzEvent::RepoCreate`.
    CreateRepoWith {
        name: RepoName,
        init: Box<str>,
    },
    /// A create's abort: its name, still `Live` for `repo`, is released, and the id is never used again. Only the
    /// `Audit` consumer sends it, with no account and no key, once the create's `InitRepo` could not commit within
    /// `crate::audit::CREATE_ABORT_AFTER`. A key that no longer names `repo` as `Live` commits nothing.
    AbortCreate {
        key: RepoNameKey,
        repo: RepoId,
    },
    // ---- M4-WP3, format 12 ----
    /// A new `SealKey` that `graft-ops` generated and wrapped (`POST /actions/sealkey`): the first public-key read's,
    /// or a rotation's. Refused for a `key_id` the owner already has. An owner admin, or `graft-ops`.
    PutSealKey {
        key: SealKey,
    },
    /// Retires a key, which then seals no new secret; its records are resealed and it is dropped once none names it.
    /// `drop: true` drops a retired key and is refused while a record of this chain names it.
    RetireSealKey {
        key_id: Box<str>,
        drop: bool,
    },
    /// `PUT /orgs/{org}/actions/secrets/{name}`. Org only.
    PutOrgSecret {
        name: ActionsName,
        scope: SecretScope,
        sealed: Box<[u8]>,
        key_id: Box<str>,
    },
    DeleteOrgSecret {
        name: ActionsName,
    },
    /// `POST /orgs/{org}/actions/variables` (`create: true`) and `PATCH .../variables/{name}`.
    PutOrgVariable {
        name: ActionsName,
        scope: SecretScope,
        value: Box<str>,
        create: bool,
    },
    DeleteOrgVariable {
        name: ActionsName,
    },
    // ---- `app-hook-config`, format 13 ----
    /// `PATCH /app/hook/config`: the App's `webhook` and its wrapped `webhook_secret` (the raft-codec bytes of a
    /// `Wrapped`) as the request leaves them. `decide` sets these two fields on the record the fold holds, so a hook
    /// change never undoes a key or permission change. An owner admin, or the edge after the App's JWT verified.
    SetAppHook {
        app: AppId,
        webhook: Option<crate::apps::Url>,
        secret: Option<Box<[u8]>>,
    },
    // ---- BF-76, format 15 ----
    /// `PUT /api/graftyard/v1/orgs/{org}/actions/substitution`: the org's policy, each field kept when `None`, read and
    /// written in one decision. An org admin's.
    SetActionsSubstitution {
        tags: Option<bool>,
        /// `Some(None)` withdraws the approval.
        pin_map: Option<Option<Digest>>,
    },
}

/// The largest `init` a `CreateRepoWith` carries [guess: a `RepoSettings` with a description and twenty topics is
/// under 2 KB].
pub const CREATE_INIT_MAX: usize = 8 * 1024;

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
    NoInstallation,
    /// 422: a `GraftyardRun` or `Mcp` `AgentApp` for an App the install does not own.
    NotOwnApp,
    /// 404: an org secret or variable that does not exist (M4-WP3).
    NoRecord,
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
            AuthzOp::Install { id, row, app_owner } => {
                e.u8(24);
                enc::db(&mut e, id.0);
                installation_row(&mut e, row);
                enc::account(&mut e, *app_owner);
            }
            AuthzOp::Uninstall { id } => {
                e.u8(25);
                enc::db(&mut e, id.0);
            }
            AuthzOp::PutAgentApp(a) => {
                e.u8(26);
                agents::agent_app(&mut e, a);
            }
            AuthzOp::DeleteAgentApp { app } => {
                e.u8(27);
                apps::app_id(&mut e, *app);
            }
            AuthzOp::SetOrgAgents { enabled, workspace } => {
                e.u8(28);
                e.option(enabled.as_ref(), |e, b| e.u8(u8::from(*b)));
                e.option(workspace.as_ref(), |e, w| e.u8(*w as u8));
            }
            AuthzOp::AuditCursor { through } => {
                e.u8(29);
                e.u64(through.0);
            }
            AuthzOp::CreateRepoWith { name, init } => {
                e.u8(30);
                e.str(name.as_str());
                e.str(init);
            }
            AuthzOp::AbortCreate { key, repo } => {
                e.u8(31);
                e.str(key.as_str());
                enc::repo(&mut e, *repo);
            }
            AuthzOp::PutSealKey { key } => {
                e.u8(32);
                graftyard_fold::canon::write(e.0, key);
            }
            AuthzOp::RetireSealKey { key_id, drop } => {
                e.u8(33);
                graftyard_fold::canon::write(e.0, &(key_id, drop));
            }
            AuthzOp::PutOrgSecret { name, scope, sealed, key_id } => {
                e.u8(34);
                graftyard_fold::canon::write(e.0, &(name, scope, sealed, key_id));
            }
            AuthzOp::DeleteOrgSecret { name } => {
                e.u8(35);
                graftyard_fold::canon::write(e.0, name);
            }
            AuthzOp::PutOrgVariable { name, scope, value, create } => {
                e.u8(36);
                graftyard_fold::canon::write(e.0, &(name, scope, value, create));
            }
            AuthzOp::DeleteOrgVariable { name } => {
                e.u8(37);
                graftyard_fold::canon::write(e.0, name);
            }
            AuthzOp::SetAppHook { app, webhook, secret } => {
                e.u8(38);
                graftyard_fold::canon::write(e.0, &(app, webhook, secret));
            }
            AuthzOp::SetActionsSubstitution { tags, pin_map } => {
                e.u8(39);
                graftyard_fold::canon::write(e.0, &(tags, pin_map));
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
            // The Actions App's installation follows no route and reads no record (`check_install`).
            AuthzOp::Install { row, .. } if row.app == crate::stateless::ACTIONS_APP => {}
            AuthzOp::Install { id, app_owner, .. } => {
                d.push(ChainId::Names(name_shard(&NameKey::Installation(*id))));
                if fold.owner != Some(*app_owner) {
                    d.push(ChainId::Authz(*app_owner));
                }
            }
            _ => {}
        }
        d
    }

    /// An installation follows its commit point: the id's `Names` route names this App and owner. Its permissions
    /// are within the App's record, on `app_owner`'s chain (this one, or a dependency read once per entry).
    fn check_install(&self, s: &AuthzScratch<'_>, deps: &[(ChainId, IdentityDep)]) -> Result<(), AuthzErr> {
        let AuthzOp::Install { id, row, app_owner } = &self.op else { return Ok(()) };
        let owner = s.owner().ok_or(AuthzErr::NotOpened)?;
        // The Actions App: only its own installation on this owner, with its one row. It has no route on `Names`, so
        // no token request reaches it, and no record, so its permissions are `actions_row`'s. Every other App's
        // installation id is one `Ids` assigned, which is positive, so neither can take the other's.
        if row.app == crate::stateless::ACTIONS_APP || crate::stateless::is_actions_installation(*id) {
            let ok = *id == crate::stateless::actions_installation(owner) && *row == actions_row();
            return if ok { Ok(()) } else { Err(AuthzErr::BadRecord) };
        }
        let read = |chain: ChainId| {
            deps.iter()
                .find(|(c, _)| *c == chain)
                .map(|(_, d)| d)
                .ok_or(AuthzErr::Unauthorized(CredentialRefusal::NotRead(chain)))
        };
        let route = ChainId::Names(name_shard(&NameKey::Installation(*id)));
        let IdentityDep::Names(names) = read(route)? else { return Err(AuthzErr::NotClaimed) };
        if names.installation(*id) != Some((row.app, owner)) {
            return Err(AuthzErr::NotClaimed);
        }
        let record = if *app_owner == owner {
            s.app(row.app).cloned()
        } else {
            match read(ChainId::Authz(*app_owner))? {
                IdentityDep::Authz(f) if f.owner() == Some(*app_owner) => f.app(row.app).cloned(),
                _ => None,
            }
        };
        let record = record.ok_or(AuthzErr::NoApp)?;
        if !row.permissions.within(&record.permissions) {
            return Err(AuthzErr::BadRecord);
        }
        Ok(())
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
            AuthzOp::Open { .. } | AuthzOp::AuditCursor { .. } => unreachable!("handled above"),
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
            AuthzOp::CreateRepoWith { name, init } => {
                if init.len() > CREATE_INIT_MAX {
                    return Err(AuthzErr::BadRecord);
                }
                let key = name.key();
                if s.name(&key).is_some_and(|e| !matches!(e, RepoEntry::Alias { .. })) {
                    return Err(AuthzErr::Taken);
                }
                let repo = RepoId(s.ids.peek().ok_or(AuthzErr::NeedIdBlock)?);
                let entry = RepoEntry::Live { repo, epoch: next };
                Ok((
                    smallvec![
                        AuthzEvent::RepoName { key: key.clone(), entry: Some(entry) },
                        AuthzEvent::RepoCreate { key, repo, init: init.clone() },
                    ],
                    AuthzOut::Repo { repo, epoch: next },
                ))
            }
            AuthzOp::AbortCreate { key, repo } => {
                if !self.cred.is_operator() {
                    return Err(AuthzErr::Forbidden);
                }
                match s.name(key) {
                    Some(RepoEntry::Live { repo: r, .. }) if r == *repo => {
                        Ok((smallvec![AuthzEvent::RepoName { key: key.clone(), entry: None }], AuthzOut::Done))
                    }
                    _ => Ok((SmallVec::new(), AuthzOut::Done)),
                }
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
            AuthzOp::Install { id, row, .. } => {
                let prev =
                    s.installation(*id).cloned().unwrap_or(InstallationState { row: None, not_before: Timestamp(0) });
                if prev.row.as_ref() == Some(row) {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                let narrowed = prev
                    .row
                    .as_ref()
                    .is_some_and(|p| !scope_within(&p.repos, &row.repos) || !p.permissions.within(&row.permissions));
                let not_before = if narrowed { s.at.max(prev.not_before) } else { prev.not_before };
                let ev = AuthzEvent::Installation { id: *id, row: Some(row.clone()), not_before };
                Ok((smallvec![ev], AuthzOut::Done))
            }
            AuthzOp::PutAgentApp(a) => {
                if s.owner() != Some(agents::install_account()) {
                    return Err(AuthzErr::NotInstall);
                }
                let own = matches!(a.kind, AgentKind::GraftyardRun | AgentKind::Mcp);
                if own && s.app(a.app).is_none() {
                    return Err(AuthzErr::NotOwnApp);
                }
                if s.agent_app(a.app) == Some(a) {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                Ok((smallvec![AuthzEvent::AgentApp { app: a.app, row: Some(a.clone()) }], AuthzOut::Done))
            }
            AuthzOp::DeleteAgentApp { app } => {
                if s.owner() != Some(agents::install_account()) {
                    return Err(AuthzErr::NotInstall);
                }
                if s.agent_app(*app).is_none() {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                Ok((smallvec![AuthzEvent::AgentApp { app: *app, row: None }], AuthzOut::Done))
            }
            AuthzOp::SetOrgAgents { enabled, workspace } => {
                org_only(s)?;
                let was = s.policy();
                let mut p = was;
                p.agents_enabled = enabled.unwrap_or(p.agents_enabled);
                p.agents_workspace = workspace.unwrap_or(p.agents_workspace);
                if p == was {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                let ev = AuthzEvent::OrgAgents { enabled: p.agents_enabled, workspace: p.agents_workspace };
                Ok((smallvec![ev], AuthzOut::Done))
            }
            // Uninstalling the Actions App would fail every job's token on the owner; GHES offers no such uninstall.
            AuthzOp::Uninstall { id } if crate::stateless::is_actions_installation(*id) => Err(AuthzErr::Forbidden),
            AuthzOp::Uninstall { id } => match s.installation(*id) {
                None => Err(AuthzErr::NoInstallation),
                Some(InstallationState { row: None, .. }) => Ok((SmallVec::new(), AuthzOut::Done)),
                Some(prev) => {
                    let ev = AuthzEvent::Installation { id: *id, row: None, not_before: s.at.max(prev.not_before) };
                    Ok((smallvec![ev], AuthzOut::Done))
                }
            },
            AuthzOp::PutSealKey { .. }
            | AuthzOp::RetireSealKey { .. }
            | AuthzOp::PutOrgSecret { .. }
            | AuthzOp::DeleteOrgSecret { .. }
            | AuthzOp::PutOrgVariable { .. }
            | AuthzOp::DeleteOrgVariable { .. } => self.decide_actions(s),
            AuthzOp::SetActionsSubstitution { tags, pin_map } => {
                org_only(s)?;
                let was = s.actions.more.substitution;
                let now =
                    ActionsSubstitution { tags: tags.unwrap_or(was.tags), pin_map: pin_map.unwrap_or(was.pin_map) };
                if now == was {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                Ok((smallvec![AuthzEvent::ActionsSubstitution(now)], AuthzOut::Done))
            }
            AuthzOp::SetAppHook { app, webhook, secret } => {
                let Some(have) = s.app(*app) else { return Err(AuthzErr::NoApp) };
                let record = AppRecord { webhook: webhook.clone(), webhook_secret: secret.clone(), ..have.clone() };
                if record == *have {
                    return Ok((SmallVec::new(), AuthzOut::Done));
                }
                Ok((smallvec![AuthzEvent::App { app: *app, record: Some(Box::new(record)) }], AuthzOut::Done))
            }
        }
    }

    /// M4-WP3's ops, after `decide_op`'s admin check. A key's id is unique; a retired key seals nothing new; a key is
    /// dropped only once no org secret names it (the repository chains' records are resealed by the rotation job,
    /// which drops the key last). Org rows need an org and a sealed box under one of the owner's unretired keys.
    fn decide_actions(&self, s: &AuthzScratch<'_>) -> Decided {
        use crate::sealed::SecretVisibility;
        let a = &*s.actions;
        let none = || Ok((SmallVec::new(), AuthzOut::Done));
        let one = |ev: AuthzEvent| Ok((smallvec![ev], AuthzOut::Done));
        let org_scope = |sc: &SecretScope| match sc {
            SecretScope::Org { visibility, repos } if *visibility == SecretVisibility::Selected || repos.is_empty() => {
                Ok(())
            }
            _ => Err(AuthzErr::BadRecord),
        };
        match &self.op {
            AuthzOp::PutSealKey { key } => match a.seal_keys.get(&key.key_id) {
                Some(k) if k == key => none(),
                // The wrapping key's rotation: the same key, its private half wrapped again under another slot.
                Some(k) if SealKey { private: key.private.clone(), ..k.clone() } == *key => {
                    one(AuthzEvent::SealKey { key_id: key.key_id.clone(), key: Some(key.clone()) })
                }
                Some(_) => Err(AuthzErr::BadRecord),
                None if key.retired.is_some() || key.key_id.is_empty() => Err(AuthzErr::BadRecord),
                None => one(AuthzEvent::SealKey { key_id: key.key_id.clone(), key: Some(key.clone()) }),
            },
            AuthzOp::RetireSealKey { key_id, drop } => {
                let Some(k) = a.seal_keys.get(key_id) else { return none() };
                if *drop {
                    if k.retired.is_none() || a.secrets.values().any(|(_, r)| r.key_id == *key_id) {
                        return Err(AuthzErr::BadRecord);
                    }
                    return one(AuthzEvent::SealKey { key_id: key_id.clone(), key: None });
                }
                if k.retired.is_some() {
                    return none();
                }
                let key = SealKey { retired: Some(s.at), ..k.clone() };
                one(AuthzEvent::SealKey { key_id: key_id.clone(), key: Some(key) })
            }
            AuthzOp::PutOrgSecret { name, scope, sealed, key_id } => {
                org_only(s)?;
                org_scope(scope)?;
                // A sealed box is the 32-byte ephemeral key, the 16-byte tag, and at most 48 KB of value.
                let unretired = a.seal_keys.get(key_id).is_some_and(|k| k.retired.is_none());
                if !unretired || sealed.len() <= 48 || sealed.len() > 48 + 48 * 1024 {
                    return Err(AuthzErr::BadRecord);
                }
                let record = SecretRecord { sealed: sealed.clone(), key_id: key_id.clone(), updated: s.at };
                one(AuthzEvent::OrgSecret { name: name.clone(), row: Some((scope.clone(), record)) })
            }
            AuthzOp::DeleteOrgSecret { name } => {
                org_only(s)?;
                if !a.secrets.contains_key(name) {
                    return Err(AuthzErr::NoRecord);
                }
                one(AuthzEvent::OrgSecret { name: name.clone(), row: None })
            }
            AuthzOp::PutOrgVariable { name, scope, value, create } => {
                org_only(s)?;
                org_scope(scope)?;
                if value.len() > 48 * 1024 {
                    return Err(AuthzErr::BadRecord);
                }
                match (create, a.variables.get(name)) {
                    (true, Some(_)) => return Err(AuthzErr::Taken),
                    (false, None) => return Err(AuthzErr::NoRecord),
                    (false, Some((sc, r))) if sc == scope && r.value == *value => return none(),
                    _ => {}
                }
                let record = VariableRecord { value: value.clone(), updated: s.at };
                one(AuthzEvent::OrgVariable { name: name.clone(), row: Some((scope.clone(), record)) })
            }
            AuthzOp::DeleteOrgVariable { name } => {
                org_only(s)?;
                if !a.variables.contains_key(name) {
                    return Err(AuthzErr::NoRecord);
                }
                one(AuthzEvent::OrgVariable { name: name.clone(), row: None })
            }
            _ => unreachable!("only M4-WP3's ops"),
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
            AuthzOp::AuditCursor { .. } => AUTHZ_FORMAT_V4,
            AuthzOp::CreateRepoWith { .. } | AuthzOp::AbortCreate { .. } => AUTHZ_FORMAT_V11,
            AuthzOp::PutSealKey { .. }
            | AuthzOp::RetireSealKey { .. }
            | AuthzOp::PutOrgSecret { .. }
            | AuthzOp::DeleteOrgSecret { .. }
            | AuthzOp::PutOrgVariable { .. }
            | AuthzOp::DeleteOrgVariable { .. } => AUTHZ_FORMAT_V12,
            AuthzOp::SetAppHook { .. } => AUTHZ_FORMAT_V13,
            AuthzOp::SetActionsSubstitution { .. } => AUTHZ_FORMAT_V15,
            AuthzOp::Install { ref row, .. } if row.app == crate::stateless::ACTIONS_APP => AUTHZ_FORMAT_V14,
            AuthzOp::PutAgentApp(_) | AuthzOp::DeleteAgentApp { .. } | AuthzOp::SetOrgAgents { .. } => AUTHZ_FORMAT_V3,
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
        if let AuthzOp::AuditCursor { through } = &self.op {
            if !self.cred.is_operator() {
                return Err(AuthzErr::Forbidden);
            }
            if *through <= s.fold.audit_cursor() {
                return Ok((SmallVec::new(), AuthzOut::Done));
            }
            return Ok((smallvec![AuthzEvent::AuditCursor { through: *through }], AuthzOut::Done));
        }
        self.check_claim(s, deps)?;
        self.check_install(s, deps)?;
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
