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
//! M4-WP2 adds DESIGN's `Record(UserRecord)` (stars, watches, follows, thread subscriptions, and thread state) and the
//! `Counts` consumer's cursor, format 0x14.3, in a trailing part of the fold (`UserRows`).

use std::borrow::Cow;
use std::collections::BTreeMap;

use graftyard_raft::{Command, Enc, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{
    AccountId, ChainId, CredentialKey, CredentialShard, Digest, Index, RepoId, Timestamp, TokenShard,
};
use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::apps::Url;
use crate::authz::{AccountKind, AuthzRead};
use crate::compat::tail_serde;
use crate::credential::{
    CredentialRefusal, IdentityDep, OrgId, Own, WebSessionId, check_credential, credential_chains, encode_cred,
};
use crate::enc;
use crate::names::{Login, NamesRead, account_shard};
use crate::org::{SignInMethod, is_install_account};
use crate::tokens::{
    Fingerprint, GrantId, TokenKind, TokensFold, TokensRead, WriteScope, fingerprint_shard, shard_of_hash,
};

/// The `Identity` chain's format. Family 0x14: the identity families are 0x10 to 0x13 (`crate::lib`).
pub const IDENTITY_FORMAT: Format = Format { family: 0x14, version: 1 };
/// `AddKey`: an SSH key listed with its public key text, which `GET /user/keys` renders (BF-35). A release that reads
/// only 0x14.1 cannot apply `KeyText`, so the edge sends it only once `FormatsEnable` enabled 0x14.2.
pub const IDENTITY_FORMAT_V2: Format = Format { family: 0x14, version: 2 };
/// M4-WP2: `SetRecord` with its `Record(UserRecord)` and the `Counts` consumer's `CountsCursor`. A release that reads
/// only 0x14.2 cannot apply them, so the edge sends them only once `FormatsEnable` enabled 0x14.3.
pub const IDENTITY_FORMAT_V3: Format = Format { family: 0x14, version: 3 };
/// AM2-WP1: the account's `McpGrant` rows (`PutMcpGrant`, `McpGeneration`, `RevokeMcpGrant`, and their
/// `IdentityEvent::McpGrant`). Sent only once `FormatsEnable` enabled it.
pub const IDENTITY_FORMAT_V4: Format = Format { family: 0x14, version: 4 };

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

/// A password hash or second-factor verifier: encoded exactly as the value it wraps, and never printed by `Debug`, so
/// a logged command, event, fold, or error cannot leak it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret<T>(pub T);

impl<T> From<T> for Secret<T> {
    fn from(v: T) -> Secret<T> {
        Secret(v)
    }
}

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// SHARED's `CredentialEvent` [inference: SHARED names it without variants]. `Password` and `SecondFactor` hold
/// hashes: an Argon2id or PBKDF2 string for the password and a verifier per factor [inference]; the edge checks them,
/// and `decide` sees only whether one exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialEvent {
    Added { credential: CredentialRef, title: Box<str> },
    Removed { credential: CredentialRef },
    Password { hash: Secret<Box<str>> },
    SecondFactor { id: u32, verifier: Option<Secret<Box<[u8]>>> },
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

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityEvent {
    Created(AccountCapture),
    Credential(CredentialEvent),
    Sso(SsoEvent),
    Audit(AuditEvent),
    /// Not in SHARED's list, which puts sessions on this chain but names no event for them [inference].
    Session(SessionEvent),
    /// Format 0x14.2: the public key text of a listed SSH key, which its fingerprint is the SHA-256 of (the key blob,
    /// decoded from base64, as OpenSSH's `SHA256:` fingerprint hashes it). Not in SHARED's list [inference: `GET
    /// /user/keys` renders a key's text, which the `Tokens` row does not hold].
    KeyText {
        fingerprint: Fingerprint,
        key: Box<str>,
    },
    /// Format 0x14.3: DESIGN's `Record(UserRecord)`, a star, watch, follow, thread subscription, or thread state the
    /// account set. The fold keeps each one with its entry's time and index (`UserRows`).
    Record(UserRecord),
    /// Format 0x14.3: the `Counts` consumer's cursor on this chain, moved only forward, after the watcher rows its
    /// batch owed committed on their repositories [inference: SHARED puts a cursor on the chain a consumer reads].
    CountsCursor(Index),
    /// Format 0x14.4: AGENT-LAYER's `McpGrant { grant, revoked }`, a grant's whole row each time it changes.
    McpGrant {
        grant: McpGrant,
        revoked: bool,
    },
    /// Format 0x14.4: a scoped `ghu_` that `token/scoped` minted under a live grant, by its hash, kept until its
    /// `expires` so a revocation of the grant can revoke it on its `Tokens` shard [inference: A7 step 7 deletes the
    /// grant's upstream tokens, and only this row knows them].
    McpScoped {
        grant: GrantId,
        token: [u8; 32],
        /// The unscoped `ghu_` the scoped one narrowed. Revoking it revokes its paired `ghr_` too, so a revoked grant
        /// leaves the client no refresh.
        unscoped: [u8; 32],
        expires: Timestamp,
    },
}

/// `design/AGENT-LAYER.md`, Shape, "graft-mcp": one approved MCP client in the user's `Identity` group. `id` is drawn
/// by the edge and unique within the account; its wire form names the account too (`graftyard_api::mcp`), so the
/// generation check, which carries no user token, finds the group. `write` is the scope the user consented to, which
/// `token/scoped` checks `graftyard_write` against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpGrant {
    pub id: GrantId,
    pub client: Url,
    pub redirect_hosts: SmallVec<[Box<str>; 2]>,
    pub generation: u32,
    pub write: WriteScope,
}

/// A grant as the fold keeps it. A revoked row stays, so its id is never reused and a late refresh finds it revoked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRow {
    pub grant: McpGrant,
    /// The entry time of its `PutMcpGrant`.
    pub created: Timestamp,
    pub revoked: bool,
    /// The scoped tokens minted under it that had not lapsed at the last `McpScoped`, with their `expires`.
    pub tokens: BTreeMap<[u8; 32], Timestamp>,
    /// The unscoped `ghu_` the last scoped token narrowed. A refresh spends the one before it, whose pair is then
    /// revoked, so only the last one can still refresh.
    pub upstream: Option<[u8; 32]>,
}

impl GrantRow {
    pub fn live(&self) -> bool {
        !self.revoked
    }
}

/// Format 0x14.4's rows: the account's MCP grants by id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRows {
    pub grants: BTreeMap<GrantId, GrantRow>,
}

/// A notification thread: one issue or pull request of one repository, as GitHub's thread is [inference: DESIGN names
/// `ThreadId` without fields]. Its REST id is `thread_id`'s decimal form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ThreadId {
    pub repo: RepoId,
    pub number: u32,
}

impl ThreadId {
    /// The REST `id` (a string of digits): the repository id times 2^24 plus the number, so a thread's id is stable and
    /// distinct across repositories while numbers stay below 2^24 [inference].
    pub fn rest_id(self) -> u64 {
        ((self.repo.0.0 as u64) << 24) | u64::from(self.number & 0x00ff_ffff)
    }

    pub fn from_rest_id(id: u64) -> ThreadId {
        ThreadId { repo: RepoId(graftyard_types::DbId((id >> 24) as i64)), number: (id & 0x00ff_ffff) as u32 }
    }
}

/// DESIGN's `UserRecord`. These gate nothing; the `Counts` consumer copies stars, watches, and thread subscriptions
/// into each repository's watcher index, and `Inbox` reads `Thread` for read and done state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserRecord {
    Star { repo: RepoId, on: bool },
    Watch { repo: RepoId, subscribed: bool, ignored: bool },
    Follow { user: AccountId, on: bool },
    Subscription { thread: ThreadId, subscribed: bool },
    Thread { thread: ThreadId, read: bool, done: bool },
}

impl UserRecord {
    /// The repository whose watcher index the record changes, for the `Counts` consumer.
    pub fn repo(&self) -> Option<RepoId> {
        match self {
            UserRecord::Star { repo, .. } | UserRecord::Watch { repo, .. } => Some(*repo),
            UserRecord::Subscription { thread, .. } => Some(thread.repo),
            UserRecord::Follow { .. } | UserRecord::Thread { .. } => None,
        }
    }
}

/// When a record was set: the entry's time and index on the account's chain. `index` is the watcher rows' `src`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordAt {
    pub at: Timestamp,
    pub index: Index,
}

impl Default for RecordAt {
    fn default() -> RecordAt {
        RecordAt { at: Timestamp(0), index: Index(0) }
    }
}

/// A repository the account watches or ignores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchRow {
    pub subscribed: bool,
    pub ignored: bool,
    pub set: RecordAt,
}

/// A thread's state as the account set it: `read` and `done` as of `set.at`, which is GHES's `last_read_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRow {
    pub read: bool,
    pub done: bool,
    pub set: RecordAt,
}

/// Format 0x14.3's rows: the account's `UserRecord`s, last write wins, and the `Counts` cursor. Then 0x14.4's
/// `McpRows` as a trailing part of its own, so a fold with no grant keeps 0x14.3's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserRows {
    pub stars: BTreeMap<RepoId, RecordAt>,
    pub watches: BTreeMap<RepoId, WatchRow>,
    pub follows: BTreeMap<AccountId, RecordAt>,
    pub subscriptions: BTreeMap<ThreadId, (bool, RecordAt)>,
    pub threads: BTreeMap<ThreadId, ThreadRow>,
    pub counts_cursor: Index,
    pub more: McpRows,
}

tail_serde!(UserRows {
    stars: BTreeMap<RepoId, RecordAt>,
    watches: BTreeMap<RepoId, WatchRow>,
    follows: BTreeMap<AccountId, RecordAt>,
    subscriptions: BTreeMap<ThreadId, (bool, RecordAt)>,
    threads: BTreeMap<ThreadId, ThreadRow>,
    counts_cursor: Index,
} more: McpRows);

impl UserRows {
    /// Whether `r` would change nothing: a repeat is a no-op, so `decide` commits no entry for it.
    fn holds(&self, r: &UserRecord) -> bool {
        match *r {
            UserRecord::Star { repo, on } => self.stars.contains_key(&repo) == on,
            UserRecord::Watch { repo, subscribed, ignored } => match self.watches.get(&repo) {
                Some(w) => (w.subscribed, w.ignored) == (subscribed, ignored),
                None => !subscribed && !ignored,
            },
            UserRecord::Follow { user, on } => self.follows.contains_key(&user) == on,
            UserRecord::Subscription { thread, subscribed } => {
                self.subscriptions.get(&thread).is_some_and(|(s, _)| *s == subscribed)
            }
            UserRecord::Thread { thread, read, done } => {
                self.threads.get(&thread).is_some_and(|t| (t.read, t.done) == (read, done))
            }
        }
    }

    fn write(&mut self, r: &UserRecord, set: RecordAt) {
        match *r {
            UserRecord::Star { repo, on: true } => {
                self.stars.entry(repo).or_insert(set);
            }
            UserRecord::Star { repo, on: false } => {
                self.stars.remove(&repo);
            }
            UserRecord::Watch { repo, subscribed, ignored } => {
                if subscribed || ignored {
                    self.watches.insert(repo, WatchRow { subscribed, ignored, set });
                } else {
                    self.watches.remove(&repo);
                }
            }
            UserRecord::Follow { user, on: true } => {
                self.follows.entry(user).or_insert(set);
            }
            UserRecord::Follow { user, on: false } => {
                self.follows.remove(&user);
            }
            UserRecord::Subscription { thread, subscribed } => {
                self.subscriptions.insert(thread, (subscribed, set));
            }
            UserRecord::Thread { thread, read, done } => {
                self.threads.insert(thread, ThreadRow { read, done, set });
            }
        }
    }
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
            IdentityEvent::KeyText { fingerprint, key } => {
                e.u8(6);
                e.0.extend_from_slice(&fingerprint.0);
                e.str(key);
            }
            IdentityEvent::Record(r) => {
                e.u8(4);
                user_record(&mut e, r);
            }
            IdentityEvent::CountsCursor(i) => {
                e.u8(7);
                e.u64(i.0);
            }
            IdentityEvent::McpGrant { grant, revoked } => {
                e.u8(8);
                mcp_grant(&mut e, grant);
                e.u8(u8::from(*revoked));
            }
            IdentityEvent::McpScoped { grant, token, unscoped, expires } => {
                e.u8(9);
                enc::db(&mut e, grant.0);
                e.0.extend_from_slice(token);
                e.0.extend_from_slice(unscoped);
                enc::at(&mut e, *expires);
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

fn mcp_grant(e: &mut Enc, g: &McpGrant) {
    enc::db(e, g.id.0);
    e.str(&g.client.0);
    e.count(g.redirect_hosts.len());
    for h in &g.redirect_hosts {
        e.str(h);
    }
    e.u32(g.generation);
    e.u8(g.write as u8);
}

fn thread(e: &mut Enc, t: &ThreadId) {
    enc::repo(e, t.repo);
    e.u32(t.number);
}

fn user_record(e: &mut Enc, r: &UserRecord) {
    match r {
        UserRecord::Star { repo, on } => {
            e.u8(0);
            enc::repo(e, *repo);
            e.u8(u8::from(*on));
        }
        UserRecord::Watch { repo, subscribed, ignored } => {
            e.u8(1);
            enc::repo(e, *repo);
            e.u8(u8::from(*subscribed));
            e.u8(u8::from(*ignored));
        }
        UserRecord::Follow { user, on } => {
            e.u8(2);
            enc::account(e, *user);
            e.u8(u8::from(*on));
        }
        UserRecord::Subscription { thread: t, subscribed } => {
            e.u8(3);
            thread(e, t);
            e.u8(u8::from(*subscribed));
        }
        UserRecord::Thread { thread: t, read, done } => {
            e.u8(4);
            thread(e, t);
            e.u8(u8::from(*read));
            e.u8(u8::from(*done));
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
            e.str(&hash.0);
        }
        CredentialEvent::SecondFactor { id, verifier } => {
            e.u8(3);
            e.u32(*id);
            e.option(verifier.as_ref(), |e, v| e.bytes(&v.0));
        }
    }
}

/// One account's `Identity` fold. Its serde form is M3-WP3's fields, then `more` only when it holds a row
/// (`crate::compat`), so a fold with no key text keeps 0x14.1's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityFold {
    pub account: Option<AccountId>,
    pub created: Option<AccountCapture>,
    /// What `GET /user/keys` and `gh ssh-key list` list, with each credential's title.
    pub credentials: BTreeMap<CredentialRef, Box<str>>,
    pub password: Option<Secret<Box<str>>>,
    pub second_factors: BTreeMap<u32, Secret<Box<[u8]>>>,
    pub sessions: BTreeMap<WebSessionId, SignInMethod>,
    pub sso: BTreeMap<OrgId, Digest>,
    pub audit: Vec<AuditEvent>,
    pub more: IdentityMore,
}

/// Format 0x14.2's rows: each listed SSH key's public key text, by fingerprint. Then 0x14.3's `UserRows` as a
/// trailing part of its own, so a fold with no record keeps 0x14.2's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityMore {
    pub key_text: BTreeMap<Fingerprint, Box<str>>,
    pub more: UserRows,
}

tail_serde!(IdentityMore {
    key_text: BTreeMap<Fingerprint, Box<str>>,
} more: UserRows);

tail_serde!(IdentityFold {
    account: Option<AccountId>,
    created: Option<AccountCapture>,
    credentials: BTreeMap<CredentialRef, Box<str>>,
    password: Option<Secret<Box<str>>>,
    second_factors: BTreeMap<u32, Secret<Box<[u8]>>>,
    sessions: BTreeMap<WebSessionId, SignInMethod>,
    sso: BTreeMap<OrgId, Digest>,
    audit: Vec<AuditEvent>,
} more: IdentityMore);

impl IdentityFold {
    pub fn new(account: AccountId) -> IdentityFold {
        IdentityFold { account: Some(account), ..IdentityFold::default() }
    }

    /// The account's `UserRecord`s and the `Counts` cursor (format 0x14.3).
    pub fn user(&self) -> &UserRows {
        &self.more.more
    }

    /// The account's MCP grants (format 0x14.4).
    pub fn grants(&self) -> &BTreeMap<GrantId, GrantRow> {
        &self.more.more.more.grants
    }

    /// A live grant of this account.
    pub fn live_grant(&self, id: GrantId) -> Option<&McpGrant> {
        self.grants().get(&id).filter(|r| r.live()).map(|r| &r.grant)
    }

    fn write(&mut self, ev: &IdentityEvent, set: RecordAt) {
        match ev {
            IdentityEvent::McpGrant { grant, revoked } => {
                let rows = &mut self.more.more.more.grants;
                match rows.get_mut(&grant.id) {
                    Some(r) => {
                        r.grant = grant.clone();
                        r.revoked = *revoked;
                    }
                    None => {
                        let row = GrantRow {
                            grant: grant.clone(),
                            created: set.at,
                            revoked: *revoked,
                            tokens: BTreeMap::new(),
                            upstream: None,
                        };
                        rows.insert(grant.id, row);
                    }
                }
            }
            IdentityEvent::McpScoped { grant, token, unscoped, expires } => {
                if let Some(r) = self.more.more.more.grants.get_mut(grant) {
                    r.tokens.retain(|_, e| *e > set.at);
                    r.tokens.insert(*token, *expires);
                    r.upstream = Some(*unscoped);
                }
            }
            IdentityEvent::Record(r) => self.more.more.write(r, set),
            // `decide` emits it only forward.
            IdentityEvent::CountsCursor(i) => self.more.more.counts_cursor = *i,
            IdentityEvent::Created(c) => self.created = Some(c.clone()),
            IdentityEvent::Credential(CredentialEvent::Added { credential, title }) => {
                self.credentials.insert(*credential, title.clone());
            }
            IdentityEvent::Credential(CredentialEvent::Removed { credential }) => {
                self.credentials.remove(credential);
                if let CredentialRef::SshKey(fp) = credential {
                    self.more.key_text.remove(fp);
                }
            }
            IdentityEvent::KeyText { fingerprint, key } => {
                self.more.key_text.insert(*fingerprint, key.clone());
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
        self.fold.to_mut().write(ev, RecordAt::default());
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
            // A grant is created at generation 0, moves only one generation forward, and stays revoked.
            IdentityEvent::McpGrant { grant, revoked } => match f.grants().get(&grant.id) {
                None if grant.generation == 0 && !revoked => Ok(()),
                None => Err(format!("{:?} created revoked or past generation 0", grant.id).into()),
                Some(r) if r.revoked => Err(format!("{:?} changed after its revocation", grant.id).into()),
                Some(r) if r.grant.client != grant.client || r.grant.write != grant.write => {
                    Err(format!("{:?} changed its client or write scope", grant.id).into())
                }
                Some(r) if grant.generation == r.grant.generation + 1 && !revoked => Ok(()),
                Some(r) if grant.generation == r.grant.generation && *revoked => Ok(()),
                Some(_) => Err(format!("{:?} skipped a generation", grant.id).into()),
            },
            IdentityEvent::McpScoped { grant, .. } if !f.grants().get(grant).is_some_and(GrantRow::live) => {
                Err(format!("a scoped token under {grant:?}, which is not live").into())
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
        self.write(ev, RecordAt::default());
    }
    /// Each event with its entry's time and index, which a `UserRecord` row keeps.
    fn apply_entry(&mut self, e: &graftyard_raft::LogEntry<IdentityEvent>) {
        if let graftyard_raft::EntryBody::Events(evs) = &e.body {
            for ev in evs {
                self.write(ev, RecordAt { at: e.at, index: e.index });
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityCmd {
    pub cred: CredentialShard,
    pub op: IdentityOp,
}

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
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
        hash: Secret<Box<str>>,
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
    // ---- format 0x14.2 ----
    /// `POST /user/keys`, after the key's `AddSshKey` committed on its `fingerprint_shard`: `AddCredential` for the key
    /// with its public key text. The leader refuses text whose blob's SHA-256 is not the fingerprint, so the list
    /// never shows a key under another key's row. A repeat is a no-op.
    AddKey {
        fingerprint: Fingerprint,
        key: Box<str>,
        title: Box<str>,
    },
    // ---- format 0x14.3 ----
    /// A star, watch, follow, thread subscription, or thread state, by the account itself. A repeat is a no-op. The
    /// edge checks that the viewer can read the repository before it sends one [inference: GHES answers 404 for a
    /// repository the viewer cannot see].
    SetRecord(UserRecord),
    /// The `Counts` consumer's cursor, moved only forward. Only the operator (a consumer) sends it.
    CountsCursor {
        through: Index,
    },
    // ---- format 0x14.4 ----
    /// `POST /api/graftyard/v1/mcp/grants`: a new grant at generation 0, by the account itself under the `Graftyard
    /// MCP` user token the edge checked. The leader gives it the account's next grant id, so the edge draws none.
    PutMcpGrant {
        client: Url,
        redirect_hosts: SmallVec<[Box<str>; 2]>,
        write: WriteScope,
    },
    /// `POST .../mcp/grants/{id}/generation`, sent by the edge as the operator after the client credentials checked.
    /// The grant's current generation moves it one forward; any other generation revokes the grant, in the same
    /// entry, and answers `Grant { revoked: true }`. A revoked grant is `GrantRevoked`.
    McpGeneration {
        grant: GrantId,
        generation: u32,
    },
    /// `DELETE .../mcp/grants/{id}`, by the account itself, or the operator. A revoked grant stays revoked.
    RevokeMcpGrant {
        grant: GrantId,
    },
    /// `token/scoped` from `Graftyard MCP` with `graftyard_grant` and `graftyard_write`, by the account under the
    /// unscoped user token it narrows (the credential's key), before the scoped token's `Tokens` commit. Refused unless
    /// the grant is this account's and live and `write` is within the grant's, so the check binds at commit.
    McpScoped {
        grant: GrantId,
        write: WriteScope,
        token: [u8; 32],
        expires: Timestamp,
    },
}

/// What a committed `IdentityCmd` answers: a grant's row after the entry for the 0x14.4 ops, `Done` for the rest.
/// After a revocation the edge reads the row's `tokens` and `upstream` fresh and revokes them on their `Tokens`
/// shards (`graftyard_api::mcp`), so a replayed request and a repeat revocation revoke them too. Wire tags: add
/// variants at the end only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityOut {
    Done,
    /// `created` is the row's `PutMcpGrant` time, `None` for the entry that creates it (`decide` sees no clock).
    Grant {
        grant: McpGrant,
        revoked: bool,
        created: Option<Timestamp>,
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
    /// The key text is not an OpenSSH public key whose blob hashes to the fingerprint.
    KeyMismatch,
    /// No grant of this account has that id.
    NoGrant,
    /// The grant is revoked: a refresh under it is refused, and nothing revives it.
    GrantRevoked,
    /// A new grant with no redirect host, or a client that is not an https URL with a path.
    BadGrant,
    /// `graftyard_write` above the grant's write scope.
    WriteAbove,
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
                e.str(&hash.0);
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
            IdentityOp::AddKey { fingerprint, key, title } => {
                e.u8(7);
                e.0.extend_from_slice(&fingerprint.0);
                e.str(key);
                e.str(title);
            }
            IdentityOp::SetRecord(r) => {
                e.u8(8);
                user_record(&mut e, r);
            }
            IdentityOp::CountsCursor { through } => {
                e.u8(9);
                e.u64(through.0);
            }
            IdentityOp::PutMcpGrant { client, redirect_hosts, write } => {
                e.u8(10);
                e.str(&client.0);
                e.count(redirect_hosts.len());
                for h in redirect_hosts {
                    e.str(h);
                }
                e.u8(*write as u8);
            }
            IdentityOp::McpGeneration { grant, generation } => {
                e.u8(11);
                enc::db(&mut e, grant.0);
                e.u32(*generation);
            }
            IdentityOp::RevokeMcpGrant { grant } => {
                e.u8(12);
                enc::db(&mut e, grant.0);
            }
            IdentityOp::McpScoped { grant, write, token, expires } => {
                e.u8(13);
                enc::db(&mut e, grant.0);
                e.u8(*write as u8);
                e.0.extend_from_slice(token);
                enc::at(&mut e, *expires);
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
            (IdentityOp::AddKey { fingerprint, .. }, _) => smallvec![ChainId::Tokens(fingerprint_shard(fingerprint))],
            (IdentityOp::SignIn { install, .. }, Some(a)) => {
                smallvec![ChainId::Authz(*install), ChainId::Names(account_shard(a))]
            }
            _ => SmallVec::new(),
        }
    }
}

/// An OpenSSH public key's fingerprint: the SHA-256 of its blob, the second field decoded from base64, for a key
/// whose type GitHub accepts (`ssh-rsa`, `ssh-dss`, `ssh-ed25519`, `ecdsa-sha2-nistp256/384/521`, and the `sk-`
/// forms) and whose blob names that type. The text is the type and the blob; a comment is dropped by the caller.
pub fn key_fingerprint(text: &str) -> Option<Fingerprint> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let mut parts = text.split_ascii_whitespace();
    let (ty, b64) = (parts.next()?, parts.next()?);
    const TYPES: [&str; 8] = [
        "ssh-rsa",
        "ssh-dss",
        "ssh-ed25519",
        "ecdsa-sha2-nistp256",
        "ecdsa-sha2-nistp384",
        "ecdsa-sha2-nistp521",
        "sk-ssh-ed25519@openssh.com",
        "sk-ecdsa-sha2-nistp256@openssh.com",
    ];
    if !TYPES.contains(&ty) {
        return None;
    }
    let blob = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let n = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    if blob.get(4..4 + n)? != ty.as_bytes() {
        return None;
    }
    Some(Fingerprint(sha2::Sha256::digest(&blob).into()))
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
    type Out = IdentityOut;
    type Err = IdentityErr;

    fn format(&self) -> Format {
        match self.op {
            IdentityOp::AddKey { .. } => IDENTITY_FORMAT_V2,
            IdentityOp::SetRecord(_) | IdentityOp::CountsCursor { .. } => IDENTITY_FORMAT_V3,
            IdentityOp::PutMcpGrant { .. }
            | IdentityOp::McpGeneration { .. }
            | IdentityOp::RevokeMcpGrant { .. }
            | IdentityOp::McpScoped { .. } => IDENTITY_FORMAT_V4,
            _ => IDENTITY_FORMAT,
        }
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
    ) -> Result<(SmallVec<[IdentityEvent; 4]>, IdentityOut), IdentityErr> {
        check_credential(&self.cred, Own::Other, deps).map_err(IdentityErr::Unauthorized)?;
        let f = s.fold();
        let account = f.account.ok_or(IdentityErr::NotCreated)?;
        // The account itself, or the operator and finishers, who name neither an account nor a key.
        if self.cred.account.is_some_and(|a| a != account) || (self.cred.account.is_none() && !self.cred.is_operator())
        {
            return Err(IdentityErr::Forbidden);
        }
        let none = || Ok((SmallVec::new(), IdentityOut::Done));
        let one = |ev| Ok((smallvec![ev], IdentityOut::Done));
        let row = |id: &GrantId| f.grants().get(id).ok_or(IdentityErr::NoGrant);
        let grant_ev = |grant: McpGrant, revoked: bool| {
            let created = f.grants().get(&grant.id).map(|r| r.created);
            let out = IdentityOut::Grant { grant: grant.clone(), revoked, created };
            Ok((smallvec![IdentityEvent::McpGrant { grant, revoked }], out))
        };
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
            IdentityOp::AddKey { fingerprint, key, title } => {
                if key_fingerprint(key) != Some(*fingerprint) {
                    return Err(IdentityErr::KeyMismatch);
                }
                let credential = CredentialRef::SshKey(*fingerprint);
                if !live_for(tokens_dep(deps, &credential)?, &credential, account) {
                    return Err(IdentityErr::NotCommitted);
                }
                let mut evs = SmallVec::new();
                if !f.credentials.contains_key(&credential) {
                    evs.push(IdentityEvent::Credential(CredentialEvent::Added { credential, title: title.clone() }));
                }
                if f.more.key_text.get(fingerprint) != Some(key) {
                    evs.push(IdentityEvent::KeyText { fingerprint: *fingerprint, key: key.clone() });
                }
                Ok((evs, IdentityOut::Done))
            }
            IdentityOp::SetRecord(r) => {
                if self.cred.account != Some(account) {
                    return Err(IdentityErr::Forbidden);
                }
                if f.user().holds(r) {
                    return none();
                }
                one(IdentityEvent::Record(*r))
            }
            IdentityOp::CountsCursor { through } => {
                if !self.cred.is_operator() {
                    return Err(IdentityErr::Forbidden);
                }
                if *through <= f.user().counts_cursor {
                    return none();
                }
                one(IdentityEvent::CountsCursor(*through))
            }
            IdentityOp::PutMcpGrant { client, redirect_hosts, write } => {
                if self.cred.account != Some(account) {
                    return Err(IdentityErr::Forbidden);
                }
                let https = client.0.strip_prefix("https://").is_some_and(|r| r.contains('/'));
                if redirect_hosts.is_empty() || !https {
                    return Err(IdentityErr::BadGrant);
                }
                let next = f.grants().keys().next_back().map_or(1, |g| g.0.0 + 1);
                let grant = McpGrant {
                    id: GrantId(graftyard_types::DbId(next)),
                    client: client.clone(),
                    redirect_hosts: redirect_hosts.clone(),
                    generation: 0,
                    write: *write,
                };
                grant_ev(grant, false)
            }
            IdentityOp::McpGeneration { grant, generation } => {
                if !self.cred.is_operator() {
                    return Err(IdentityErr::Forbidden);
                }
                let r = row(grant)?;
                if r.revoked {
                    return Err(IdentityErr::GrantRevoked);
                }
                // An older generation is a replayed `McpRefresh`: it revokes the grant (A7 step 6). A newer one
                // cannot come from a refresh this grant issued, and is refused the same way.
                if *generation != r.grant.generation {
                    return grant_ev(r.grant.clone(), true);
                }
                let mut next = r.grant.clone();
                next.generation += 1;
                grant_ev(next, false)
            }
            IdentityOp::RevokeMcpGrant { grant } => {
                let r = row(grant)?;
                if r.revoked {
                    let out = IdentityOut::Grant { grant: r.grant.clone(), revoked: true, created: Some(r.created) };
                    return Ok((SmallVec::new(), out));
                }
                grant_ev(r.grant.clone(), true)
            }
            IdentityOp::McpScoped { grant, write, token, expires } => {
                if self.cred.account != Some(account) {
                    return Err(IdentityErr::Forbidden);
                }
                let r = row(grant)?;
                if r.revoked {
                    return Err(IdentityErr::GrantRevoked);
                }
                if *write > r.grant.write {
                    return Err(IdentityErr::WriteAbove);
                }
                let Some(CredentialKey::Token { hash: unscoped, .. }) = self.cred.key else {
                    return Err(IdentityErr::Forbidden);
                };
                one(IdentityEvent::McpScoped { grant: *grant, token: *token, unscoped, expires: *expires })
            }
        }
    }
    fn replay(&self, events: &[IdentityEvent]) -> IdentityOut {
        match events.first() {
            Some(IdentityEvent::McpGrant { grant, revoked }) => {
                IdentityOut::Grant { grant: grant.clone(), revoked: *revoked, created: None }
            }
            _ => IdentityOut::Done,
        }
    }
}
