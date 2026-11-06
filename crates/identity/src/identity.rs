//! An account's `Identity` chain (`design/SHARED.md`, "Event types", `IdentityEvent`, and "Identity and tokens"; M3-WP3).
//!
//! It holds the account record, password hashes, second factors, web sessions, SSO authorizations, the identity audit
//! log, and the list of the account's credentials. It gates nothing: "Sign-in churn never touches what gates
//! requests". A credential's commit point is its `Tokens` entry, and the edge appends `Credential` here right after
//! that commit, with `Audit` finishing only what nobody finished (`crate::finish::credential`). It holds no client
//! secret.
//!
//! Sign-in reads two folds besides this chain's: the install's own `Authz` chain, whose `InstanceAuth` names the one
//! method this install takes, and the account's `Account` row, so a suspended account cannot start a session.
//! `UserRecord` (stars, watches, follows) is DESIGN's `Record` variant and is later work.

use std::borrow::Cow;
use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, CredentialShard, Digest, Timestamp, TokenShard};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::authz::{AccountKind, AuthzRead};
use crate::credential::{
    CredentialRefusal, IdentityDep, OrgId, Own, WebSessionId, check_credential, credential_chains, encode_cred,
};
use crate::enc;
use crate::names::{Login, NamesRead, account_shard};
use crate::org::{SignInMethod, is_install_account};
use crate::tokens::{Fingerprint, TokenKind, TokensFold, TokensRead, fingerprint_shard, shard_of_hash};

/// The `Identity` chain's format. Family 0x14: the identity families are 0x10 to 0x13 (`crate::lib`).
pub const IDENTITY_FORMAT: Format = Format { family: 0x14, version: 1 };

/// The audit log keeps this many entries in the fold; the archive keeps them all [guess].
pub const AUDIT_KEEP: usize = 1_000;

/// SHARED's `AccountCapture`: what the signup's login claim carried, recorded on the account's own chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCapture {
    pub login: Login,
    pub kind: AccountKind,
    pub created: Timestamp,
}

/// A credential in the account's list, by what identifies its row on `Tokens`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CredentialRef {
    Token { hash: [u8; 32], kind: TokenKind },
    SshKey(Fingerprint),
}

/// SHARED's `CredentialEvent` [inference: SHARED names it without variants]. `Password` and `SecondFactor` hold
/// hashes: an Argon2id or PBKDF2 string for the password and a verifier per factor [inference]; the edge checks them,
/// and `decide` sees only whether one exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialEvent {
    Added { credential: CredentialRef, title: Box<str> },
    Removed { credential: CredentialRef },
    Password { hash: Box<str> },
    SecondFactor { id: u32, verifier: Option<Box<[u8]>> },
}

/// Web sessions: started by a sign-in, ended by sign-out. Sign-out everywhere is the `Account` row's
/// `sessions_not_before` on `Names`, which every write reads; ending them here only keeps the list honest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionEvent {
    Started { id: WebSessionId, method: SignInMethod },
    Ended { id: WebSessionId },
}

/// SHARED's `SsoEvent`: the org's IdP vouched for this account [inference: SHARED names it without variants].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SsoEvent {
    Linked { org: OrgId, subject: Digest },
    Unlinked { org: OrgId },
}

/// SHARED's `AuditEvent`: one line of the identity audit log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub action: Box<str>,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityEvent {
    Created(AccountCapture),
    Credential(CredentialEvent),
    Sso(SsoEvent),
    Audit(AuditEvent),
    /// Not in SHARED's list, which puts sessions on this chain but names no event for them [inference].
    Session(SessionEvent),
}

impl Record for IdentityEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            // Tags follow SHARED's order; `Record` (DESIGN's `UserRecord`) keeps 4.
            IdentityEvent::Created(c) => {
                e.u8(0);
                e.str(c.login.as_str());
                e.u8(c.kind as u8);
                enc::at(&mut e, c.created);
            }
            IdentityEvent::Credential(c) => {
                e.u8(1);
                credential_event(&mut e, c);
            }
            IdentityEvent::Sso(s) => {
                e.u8(2);
                match s {
                    SsoEvent::Linked { org, subject } => {
                        e.u8(0);
                        enc::account(&mut e, org.0);
                        e.0.extend_from_slice(&subject.0);
                    }
                    SsoEvent::Unlinked { org } => {
                        e.u8(1);
                        enc::account(&mut e, org.0);
                    }
                }
            }
            IdentityEvent::Audit(a) => {
                e.u8(3);
                e.str(&a.action);
                enc::at(&mut e, a.at);
            }
            IdentityEvent::Session(s) => {
                e.u8(5);
                match s {
                    SessionEvent::Started { id, method } => {
                        e.u8(0);
                        e.u128(id.0.0);
                        e.u8(*method as u8);
                    }
                    SessionEvent::Ended { id } => {
                        e.u8(1);
                        e.u128(id.0.0);
                    }
                }
            }
        }
    }
}

fn credential_ref(e: &mut Enc, c: &CredentialRef) {
    match c {
        CredentialRef::Token { hash, kind } => {
            e.u8(0);
            e.0.extend_from_slice(hash);
            e.u8(*kind as u8);
        }
        CredentialRef::SshKey(f) => {
            e.u8(1);
            e.0.extend_from_slice(&f.0);
        }
    }
}

fn credential_event(e: &mut Enc, c: &CredentialEvent) {
    match c {
        CredentialEvent::Added { credential, title } => {
            e.u8(0);
            credential_ref(e, credential);
            e.str(title);
        }
        CredentialEvent::Removed { credential } => {
            e.u8(1);
            credential_ref(e, credential);
        }
        CredentialEvent::Password { hash } => {
            e.u8(2);
            e.str(hash);
        }
        CredentialEvent::SecondFactor { id, verifier } => {
            e.u8(3);
            e.u32(*id);
            e.option(verifier.as_ref(), |e, v| e.bytes(v));
        }
    }
}

/// One account's `Identity` fold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityFold {
    pub account: Option<AccountId>,
    pub created: Option<AccountCapture>,
    /// What `GET /user/keys` and `gh ssh-key list` list, with each credential's title.
    pub credentials: BTreeMap<CredentialRef, Box<str>>,
    pub password: Option<Box<str>>,
    pub second_factors: BTreeMap<u32, Box<[u8]>>,
    pub sessions: BTreeMap<WebSessionId, SignInMethod>,
    pub sso: BTreeMap<OrgId, Digest>,
    pub audit: Vec<AuditEvent>,
}

impl IdentityFold {
    pub fn new(account: AccountId) -> IdentityFold {
        IdentityFold { account: Some(account), ..IdentityFold::default() }
    }

    fn write(&mut self, ev: &IdentityEvent) {
        match ev {
            IdentityEvent::Created(c) => self.created = Some(c.clone()),
            IdentityEvent::Credential(CredentialEvent::Added { credential, title }) => {
                self.credentials.insert(*credential, title.clone());
            }
            IdentityEvent::Credential(CredentialEvent::Removed { credential }) => {
                self.credentials.remove(credential);
            }
            IdentityEvent::Credential(CredentialEvent::Password { hash }) => self.password = Some(hash.clone()),
            IdentityEvent::Credential(CredentialEvent::SecondFactor { id, verifier: Some(v) }) => {
                self.second_factors.insert(*id, v.clone());
            }
            IdentityEvent::Credential(CredentialEvent::SecondFactor { id, verifier: None }) => {
                self.second_factors.remove(id);
            }
            IdentityEvent::Session(SessionEvent::Started { id, method }) => {
                self.sessions.insert(*id, *method);
            }
            IdentityEvent::Session(SessionEvent::Ended { id }) => {
                self.sessions.remove(id);
            }
            IdentityEvent::Sso(SsoEvent::Linked { org, subject }) => {
                self.sso.insert(*org, *subject);
            }
            IdentityEvent::Sso(SsoEvent::Unlinked { org }) => {
                self.sso.remove(org);
            }
            IdentityEvent::Audit(a) => {
                self.audit.push(a.clone());
                if self.audit.len() > AUDIT_KEEP {
                    self.audit.remove(0);
                }
            }
        }
    }
}

/// The fold with an entry's earlier events on top. An account's rows are few, so the view copies the fold on its
/// entry's first write.
pub struct IdentityScratch<'a> {
    fold: Cow<'a, IdentityFold>,
}

impl IdentityScratch<'_> {
    pub fn fold(&self) -> &IdentityFold {
        &self.fold
    }
}

impl Scratch for IdentityScratch<'_> {
    type Event = IdentityEvent;
    fn apply(&mut self, ev: &IdentityEvent) {
        self.fold.to_mut().write(ev);
    }
    /// A follower refuses a second `Created`, a credential listed twice, and a session started twice.
    fn check(&self, ev: &IdentityEvent) -> Result<(), Box<str>> {
        let f = &self.fold;
        match ev {
            IdentityEvent::Created(_) if f.created.is_some() => Err("created twice".into()),
            IdentityEvent::Credential(CredentialEvent::Added { credential, .. })
                if f.credentials.contains_key(credential) =>
            {
                Err(format!("{credential:?} listed twice").into())
            }
            IdentityEvent::Session(SessionEvent::Started { id, .. }) if f.sessions.contains_key(id) => {
                Err(format!("{id:?} started twice").into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for IdentityFold {
    type Event = IdentityEvent;
    type Scratch<'a> = IdentityScratch<'a>;
    fn scratch(&self) -> IdentityScratch<'_> {
        IdentityScratch { fold: Cow::Borrowed(self) }
    }
    fn apply(&mut self, ev: &IdentityEvent) {
        self.write(ev);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityCmd {
    pub cred: CredentialShard,
    pub op: IdentityOp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityOp {
    /// The chain's first entry, after the signup's login claim committed. Only the operator or a finisher submits it.
    Create(AccountCapture),
    /// The edge's write right after a new credential's `Tokens` commit, or the `Audit` finisher's. The leader reads the
    /// credential's `Tokens` shard and refuses unless the row there names this account and is live, so the list never
    /// shows a credential that did not commit. A repeat is a no-op.
    AddCredential {
        credential: CredentialRef,
        title: Box<str>,
    },
    /// After the row's revocation or removal on `Tokens`, which the leader reads the same way.
    RemoveCredential {
        credential: CredentialRef,
    },
    SetPassword {
        hash: Box<str>,
    },
    /// A sign-in the edge verified (password and second factor here, or the IdP's assertion, or the bootstrap link's
    /// code use). The leader reads the install's `Authz` chain and refuses a method other than its `InstanceAuth`, and
    /// the account's `Account` row and refuses a suspended account.
    SignIn {
        id: WebSessionId,
        method: SignInMethod,
        install: AccountId,
    },
    SignOut {
        id: WebSessionId,
    },
    LinkSso {
        org: OrgId,
        subject: Digest,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityErr {
    Unauthorized(CredentialRefusal),
    Forbidden,
    NotCreated,
    AlreadyCreated,
    /// The credential's `Tokens` row does not name this account, or is revoked or removed.
    NotCommitted,
    /// The install's `InstanceAuth` takes another method.
    WrongMethod,
    Suspended,
    /// `install` does not name the install's own chain.
    NotInstall,
}

impl Record for IdentityCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        encode_cred(&mut e, &self.cred);
        match &self.op {
            IdentityOp::Create(c) => IdentityEvent::Created(c.clone()).encode(e.0),
            IdentityOp::AddCredential { credential, title } => {
                e.u8(1);
                credential_ref(&mut e, credential);
                e.str(title);
            }
            IdentityOp::RemoveCredential { credential } => {
                e.u8(2);
                credential_ref(&mut e, credential);
            }
            IdentityOp::SetPassword { hash } => {
                e.u8(3);
                e.str(hash);
            }
            IdentityOp::SignIn { id, method, install } => {
                e.u8(4);
                e.u128(id.0.0);
                e.u8(*method as u8);
                enc::account(&mut e, *install);
            }
            IdentityOp::SignOut { id } => {
                e.u8(5);
                e.u128(id.0.0);
            }
            IdentityOp::LinkSso { org, subject } => {
                e.u8(6);
                enc::account(&mut e, org.0);
                e.0.extend_from_slice(&subject.0);
            }
        }
    }
}

impl CredentialRef {
    pub fn shard(&self) -> TokenShard {
        match self {
            CredentialRef::Token { hash, .. } => shard_of_hash(hash),
            CredentialRef::SshKey(f) => fingerprint_shard(f),
        }
    }
}

impl IdentityCmd {
    /// The chains the leader reads beyond the credential: a credential's `Tokens` shard, and for a sign-in the
    /// install's `Authz` chain and the account's `Account` row.
    fn extra_deps(&self, account: Option<AccountId>) -> SmallVec<[ChainId; 2]> {
        match (&self.op, account) {
            (IdentityOp::AddCredential { credential, .. } | IdentityOp::RemoveCredential { credential }, _) => {
                smallvec![ChainId::Tokens(credential.shard())]
            }
            (IdentityOp::SignIn { install, .. }, Some(a)) => {
                smallvec![ChainId::Authz(*install), ChainId::Names(account_shard(a))]
            }
            _ => SmallVec::new(),
        }
    }
}

fn dep(deps: &[(ChainId, IdentityDep)], c: ChainId) -> Result<&IdentityDep, IdentityErr> {
    deps.iter().find(|(d, _)| *d == c).map(|(_, d)| d).ok_or(IdentityErr::Unauthorized(CredentialRefusal::NotRead(c)))
}

fn tokens_dep<'d>(deps: &'d [(ChainId, IdentityDep)], c: &CredentialRef) -> Result<&'d TokensFold, IdentityErr> {
    match dep(deps, ChainId::Tokens(c.shard()))? {
        IdentityDep::Tokens(t) => Ok(t),
        _ => Err(IdentityErr::NotCommitted),
    }
}

/// Whether the credential's `Tokens` row is live and this account's.
fn live_for(t: &TokensFold, c: &CredentialRef, account: AccountId) -> bool {
    match c {
        CredentialRef::Token { hash, kind } => {
            t.token(hash).is_some_and(|r| r.account == account && !r.revoked && r.kind == *kind)
        }
        CredentialRef::SshKey(fp) => t.ssh_key(fp).is_some_and(|r| r.account == account),
    }
}

impl Command for IdentityCmd {
    type Fold = IdentityFold;
    type Dep = IdentityDep;
    type Out = ();
    type Err = IdentityErr;

    fn format(&self) -> Format {
        IDENTITY_FORMAT
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, fold: &IdentityFold) -> SmallVec<[ChainId; 3]> {
        let mut d = credential_chains(&self.cred, None);
        for c in self.extra_deps(fold.account) {
            if !d.contains(&c) {
                d.push(c);
            }
        }
        d
    }
    fn decide(
        &self,
        s: &IdentityScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[IdentityEvent; 4]>, ()), IdentityErr> {
        check_credential(&self.cred, Own::Other, deps).map_err(IdentityErr::Unauthorized)?;
        let f = s.fold();
        let account = f.account.ok_or(IdentityErr::NotCreated)?;
        // The account itself, or the operator and finishers, who name neither an account nor a key.
        if self.cred.account.is_some_and(|a| a != account) || (self.cred.account.is_none() && !self.cred.is_operator())
        {
            return Err(IdentityErr::Forbidden);
        }
        let none = || Ok((SmallVec::new(), ()));
        let one = |ev| Ok((smallvec![ev], ()));
        match &self.op {
            IdentityOp::Create(c) => {
                if !self.cred.is_operator() {
                    return Err(IdentityErr::Forbidden);
                }
                match &f.created {
                    Some(have) if have == c => none(),
                    Some(_) => Err(IdentityErr::AlreadyCreated),
                    None => one(IdentityEvent::Created(c.clone())),
                }
            }
            _ if f.created.is_none() => Err(IdentityErr::NotCreated),
            IdentityOp::AddCredential { credential, title } => {
                if !live_for(tokens_dep(deps, credential)?, credential, account) {
                    return Err(IdentityErr::NotCommitted);
                }
                if f.credentials.contains_key(credential) {
                    return none();
                }
                one(IdentityEvent::Credential(CredentialEvent::Added { credential: *credential, title: title.clone() }))
            }
            IdentityOp::RemoveCredential { credential } => {
                // Listed until its row is revoked or removed on `Tokens`, so the list never drops a live credential.
                if live_for(tokens_dep(deps, credential)?, credential, account) {
                    return Err(IdentityErr::Forbidden);
                }
                if !f.credentials.contains_key(credential) {
                    return none();
                }
                one(IdentityEvent::Credential(CredentialEvent::Removed { credential: *credential }))
            }
            IdentityOp::SetPassword { hash } => {
                one(IdentityEvent::Credential(CredentialEvent::Password { hash: hash.clone() }))
            }
            IdentityOp::SignIn { id, method, install } => {
                if !is_install_account(*install) {
                    return Err(IdentityErr::NotInstall);
                }
                let IdentityDep::Authz(a) = dep(deps, ChainId::Authz(*install))? else {
                    return Err(IdentityErr::NotInstall);
                };
                if a.owner() != Some(*install) {
                    return Err(IdentityErr::NotInstall);
                }
                if !a.instance_auth().admits(*method) {
                    return Err(IdentityErr::WrongMethod);
                }
                let IdentityDep::Names(n) = dep(deps, ChainId::Names(account_shard(account)))? else {
                    return Err(IdentityErr::Suspended);
                };
                if !n.account(account).is_some_and(|r| !r.suspended) {
                    return Err(IdentityErr::Suspended);
                }
                if f.sessions.contains_key(id) {
                    return none();
                }
                one(IdentityEvent::Session(SessionEvent::Started { id: *id, method: *method }))
            }
            IdentityOp::SignOut { id } => {
                if !f.sessions.contains_key(id) {
                    return none();
                }
                one(IdentityEvent::Session(SessionEvent::Ended { id: *id }))
            }
            IdentityOp::LinkSso { org, subject } => {
                one(IdentityEvent::Sso(SsoEvent::Linked { org: *org, subject: *subject }))
            }
        }
    }
    fn replay(&self, _events: &[IdentityEvent]) {}
}
