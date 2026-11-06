//! What an org's `Authz` chain holds beyond memberships and grants (`design/SHARED.md`, "Event types" and "Identity
//! and tokens"; `build/BUILD-PLAN.md`, M3-WP3): teams, the org policy, invitations, and, on the install's own chain,
//! `InstanceAuth`. Invitations are also the repository chain's (an outside collaborator's), so their types live here.

use std::collections::BTreeSet;
use std::time::Duration;

use graftyard_raft::Enc;
use graftyard_types::{AccountId, DbId, Digest};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::apps::{self, Url};
use crate::authz::{OrgRole, RepoPermission};
use crate::credential::UserId;
use crate::enc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TeamId(pub DbId);

/// A team and its members. `Created` takes the next id of the org's block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TeamEvent {
    Created { team: TeamId, name: Box<str> },
    Member { team: TeamId, user: UserId, on: bool },
    Deleted { team: TeamId },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRow {
    pub name: Box<str>,
    pub members: BTreeSet<UserId>,
}

/// The org policy rows `authorize` reads [inference: SHARED lists base permission and SSO enforcement among the org
/// policies and names `OrgPolicy` without its fields; rulesets, residency, custom roles, webhooks, and secrets are
/// later packages']. Every `Policy` entry carries the whole record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgPolicy {
    /// What every member may do on every org repository.
    pub base_permission: Option<RepoPermission>,
    /// Access through membership needs the org in the principal's `sso_orgs`.
    pub sso_enforced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InvitationId(pub DbId);

/// A pending invitation lapses at `expires`, 7 days after `Sent` [unverified for GitHub's repository invitations].
pub const INVITATION_TTL: Duration = Duration::from_secs(7 * 86_400);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Invitee {
    User(UserId),
    /// Org invitations only: the user who verifies the email accepts.
    Email(Digest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvitationGrant {
    Repo(RepoPermission),
    Org { role: OrgRole, teams: SmallVec<[TeamId; 2]> },
}

/// SHARED's `InvitationEvent`. `inviter` is SHARED's `Actor`, cut to the account that sent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvitationEvent {
    Sent {
        id: InvitationId,
        invitee: Invitee,
        grant: InvitationGrant,
        inviter: Option<AccountId>,
        expires: graftyard_types::Timestamp,
    },
    Accepted {
        id: InvitationId,
    },
    Declined {
        id: InvitationId,
    },
    Cancelled {
        id: InvitationId,
        actor: Option<AccountId>,
    },
}

/// A pending invitation: `Sent` until it is accepted, declined, cancelled, or lapses at `expires`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitationRow {
    pub invitee: Invitee,
    pub grant: InvitationGrant,
    pub inviter: Option<AccountId>,
    pub expires: graftyard_types::Timestamp,
}

/// Install scope only (SHARED, "Instance sign-in in installs"). Secrets inside the configs are ciphertext.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceAuth {
    #[default]
    Builtin,
    Saml(SamlConfig),
    Oidc(OidcConfig),
    Ldap(LdapConfig),
}

/// [inference: SHARED names the config types only.]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamlConfig {
    pub sso_url: Url,
    pub issuer: Box<str>,
    /// The IdP's signing certificate, DER.
    pub certificate: Box<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer: Url,
    pub client: Box<str>,
    /// Ciphertext of the client secret.
    pub secret: Box<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LdapConfig {
    pub host: Box<str>,
    pub base: Box<str>,
}

/// How a sign-in proves who the user is. It must be the one the install's `InstanceAuth` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignInMethod {
    Password,
    Saml,
    Oidc,
    Ldap,
    /// `graftyard admin bootstrap`'s one-time sign-in link, which works whatever `InstanceAuth` says, so the first
    /// site admin can configure it.
    Link,
}

impl InstanceAuth {
    pub fn method(&self) -> SignInMethod {
        match self {
            InstanceAuth::Builtin => SignInMethod::Password,
            InstanceAuth::Saml(_) => SignInMethod::Saml,
            InstanceAuth::Oidc(_) => SignInMethod::Oidc,
            InstanceAuth::Ldap(_) => SignInMethod::Ldap,
        }
    }

    /// Whether a sign-in by `m` is the one this install takes.
    pub fn admits(&self, m: SignInMethod) -> bool {
        m == SignInMethod::Link || m == self.method()
    }
}

/// The accounts `Genesis` and the CLI create sit in cell block 0, which `Ids` never leases (`IdsFold::new`), so no
/// user, org, or enterprise has one: only such an account's `Authz` chain is the install's own [inference: the CLI's
/// install account is `DbId::pack(0, 0, 1)`; `Genesis.account` is whatever it names].
pub fn is_install_account(a: AccountId) -> bool {
    a.0.block() == 0
}

// ---- canonical encodings ----

pub(crate) fn team_event(e: &mut Enc, t: &TeamEvent) {
    match t {
        TeamEvent::Created { team, name } => {
            e.u8(0);
            enc::db(e, team.0);
            e.str(name);
        }
        TeamEvent::Member { team, user, on } => {
            e.u8(1);
            enc::db(e, team.0);
            enc::account(e, user.0);
            e.u8(u8::from(*on));
        }
        TeamEvent::Deleted { team } => {
            e.u8(2);
            enc::db(e, team.0);
        }
    }
}

pub(crate) fn policy(e: &mut Enc, p: &OrgPolicy) {
    e.option(p.base_permission.as_ref(), |e, b| e.u8(*b as u8));
    e.u8(u8::from(p.sso_enforced));
}

fn invitee(e: &mut Enc, i: &Invitee) {
    match i {
        Invitee::User(u) => {
            e.u8(0);
            enc::account(e, u.0);
        }
        Invitee::Email(d) => {
            e.u8(1);
            e.0.extend_from_slice(&d.0);
        }
    }
}

fn grant(e: &mut Enc, g: &InvitationGrant) {
    match g {
        InvitationGrant::Repo(p) => {
            e.u8(0);
            e.u8(*p as u8);
        }
        InvitationGrant::Org { role, teams } => {
            e.u8(1);
            e.u8(*role as u8);
            e.count(teams.len());
            for t in teams {
                enc::db(e, t.0);
            }
        }
    }
}

pub fn invitation_event(e: &mut Enc, i: &InvitationEvent) {
    let id = |e: &mut Enc, id: &InvitationId| enc::db(e, id.0);
    match i {
        InvitationEvent::Sent { id: x, invitee: who, grant: g, inviter, expires } => {
            e.u8(0);
            id(e, x);
            invitee(e, who);
            grant(e, g);
            e.option(inviter.as_ref(), |e, a| enc::account(e, *a));
            enc::at(e, *expires);
        }
        InvitationEvent::Accepted { id: x } => {
            e.u8(1);
            id(e, x);
        }
        InvitationEvent::Declined { id: x } => {
            e.u8(2);
            id(e, x);
        }
        InvitationEvent::Cancelled { id: x, actor } => {
            e.u8(3);
            id(e, x);
            e.option(actor.as_ref(), |e, a| enc::account(e, *a));
        }
    }
}

pub(crate) fn instance_auth(e: &mut Enc, a: &InstanceAuth) {
    match a {
        InstanceAuth::Builtin => e.u8(0),
        InstanceAuth::Saml(c) => {
            e.u8(1);
            apps::url(e, &c.sso_url);
            e.str(&c.issuer);
            e.bytes(&c.certificate);
        }
        InstanceAuth::Oidc(c) => {
            e.u8(2);
            apps::url(e, &c.issuer);
            e.str(&c.client);
            e.bytes(&c.secret);
        }
        InstanceAuth::Ldap(c) => {
            e.u8(3);
            e.str(&c.host);
            e.str(&c.base);
        }
    }
}
