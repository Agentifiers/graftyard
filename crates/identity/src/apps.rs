//! GitHub Apps and OAuth apps (`design/SHARED.md`, "Identity and tokens"; `build/BUILD-PLAN.md`, M3-WP1).
//!
//! The records live on the owner's `Authz` chain (`AuthzEvent::App`, `AuthzEvent::OAuthApp`), and `Names` holds only
//! the routes from an App id, App slug, client id, or installation id to the owner (`crate::names`).

use std::collections::BTreeMap;
use std::fmt;

use arrayvec::ArrayString;
use graftyard_raft::Enc;
use graftyard_types::{DbId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::enc;
use crate::names::NameError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AppId(pub DbId);

impl AppId {
    /// The App's bot account, which GHES names `{slug}[bot]` (`github-actions[bot]` for a `GITHUB_TOKEN`), as the
    /// creator of a commit status the App sets: the App's id negated, as `actions_installation` and `HookId::app`
    /// negate theirs. Every id `Ids` assigns is positive, so no account takes it, and its sign tells a renderer it is a
    /// bot (`decisions/DECISIONS.md`, "`app-bot`").
    pub fn bot(self) -> graftyard_types::AccountId {
        graftyard_types::AccountId(DbId(-self.0.0))
    }

    /// The App whose bot account `a` is (`bot`); `None` for every account `Ids` assigns.
    pub fn of_bot(a: graftyard_types::AccountId) -> Option<AppId> {
        (a.0.0 < 0).then(|| AppId(DbId(-a.0.0)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InstallationId(pub DbId);

/// An absolute URL as the client sent it. The edge validates it; the fold stores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Url(pub Box<str>);

/// An OAuth client id, 20 characters as GitHub's: 20 hex digits for an OAuth app, `Iv1.` and 16 hex digits or
/// `Iv23` and 16 more for a GitHub App [unverified: the alphabet; SHARED gives only the length]. This parses 1 to 20
/// ASCII letters, digits, or dots. Its serde form is the text, parsed again on decode.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClientId(ArrayString<20>);

impl ClientId {
    pub fn parse(s: &str) -> Result<ClientId, NameError> {
        let ok = !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.');
        match ArrayString::from(s) {
            Ok(a) if ok => Ok(ClientId(a)),
            _ => Err(NameError::Client),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A GitHub App's client id: `Iv1.` and its App id in 16 hex digits [inference: GitHub's are `Iv1.` and 16
    /// random hex digits; deriving it from the id makes it unique with no claim of its own]. An OAuth app's client
    /// id is 20 random hex digits, which never contain a dot, so the two forms never collide.
    pub fn for_app(app: AppId) -> ClientId {
        ClientId::parse(&format!("Iv1.{:016x}", app.0.0)).expect("20 characters")
    }

    /// Whether this is `for_app`'s form.
    pub fn is_app_form(&self) -> bool {
        self.0.starts_with("Iv1.") && self.0.len() == 20
    }
}

impl fmt::Debug for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClientId({:?})", self.as_str())
    }
}

impl From<ClientId> for String {
    fn from(c: ClientId) -> String {
        c.as_str().into()
    }
}

impl TryFrom<String> for ClientId {
    type Error = NameError;
    fn try_from(s: String) -> Result<ClientId, NameError> {
        ClientId::parse(&s)
    }
}

/// An App's slug, the key `NameKey::AppSlug` holds: 1 to 34 lowercase ASCII letters, digits, or single hyphens, not
/// starting or ending with a hyphen [unverified: GitHub limits App names to 34 characters and slugs them]. `parse`
/// lowercases. Its serde form is the text, parsed again on decode.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Slug(Box<str>);

impl Slug {
    pub fn parse(s: &str) -> Result<Slug, NameError> {
        let b = s.as_bytes();
        let ok = (1..=34).contains(&b.len())
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-'
            && !s.contains("--");
        if ok { Ok(Slug(s.to_ascii_lowercase().into())) } else { Err(NameError::Slug) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Slug({:?})", &*self.0)
    }
}

impl From<Slug> for String {
    fn from(s: Slug) -> String {
        s.0.into()
    }
}

impl TryFrom<String> for Slug {
    type Error = NameError;
    fn try_from(s: String) -> Result<Slug, NameError> {
        Slug::parse(&s).and_then(|l| if l.as_str() == s { Ok(l) } else { Err(NameError::Slug) })
    }
}

/// SHA-256 of a client secret. The secret is shown once and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SecretHash(pub [u8; 32]);

/// Two client secrets allow a rotation [guess].
pub const CLIENT_SECRETS_MAX: usize = 2;

/// An RS256 public key as DER `SubjectPublicKeyInfo` bytes [inference: SHARED names the type only].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RsaPublic(pub Box<[u8]>);

/// An App's RS256 public key. GitHub keeps the private half only with the App's owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppKey {
    pub id: DbId,
    pub public: RsaPublic,
    pub created: Timestamp,
}

/// One permission's level, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Access {
    Read,
    Write,
    Admin,
}

/// An App's or a token's permissions by GitHub's names (`contents`, `issues`, `metadata`, ...) [inference: SHARED
/// names `PermissionSet` without its shape].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionSet(pub BTreeMap<Box<str>, Access>);

impl PermissionSet {
    /// Whether every permission here is at most its level in `cap`: an installation's permissions never exceed its
    /// App's.
    pub fn within(&self, cap: &PermissionSet) -> bool {
        self.0.iter().all(|(k, a)| cap.0.get(k).is_some_and(|c| a <= c))
    }
}

/// A webhook event name, such as `push` or `pull_request` [inference: SHARED names the type only].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventKind(pub Box<str>);

/// Core tier of the owner's `Authz` chain. The edge reaches it through the `Names` route from the App id, slug, or
/// client id to the owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRecord {
    pub id: AppId,
    pub slug: Slug,
    pub name: Box<str>,
    pub client: ClientId,
    pub keys: SmallVec<[AppKey; 2]>,
    pub permissions: PermissionSet,
    pub events: SmallVec<[EventKind; 8]>,
    pub webhook: Option<Url>,
    /// Ciphertext, as org webhook secrets are.
    pub webhook_secret: Option<Box<[u8]>>,
    pub client_secrets: SmallVec<[SecretHash; 2]>,
    pub callbacks: SmallVec<[Url; 2]>,
    pub public: bool,
}

/// Core tier of the owner's `Authz` chain. `FirstPartyClient` ids have no record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthAppRecord {
    pub client: ClientId,
    pub name: Box<str>,
    pub callbacks: SmallVec<[Url; 1]>,
    pub client_secrets: SmallVec<[SecretHash; 2]>,
    pub device_flow: bool,
}

// ---- canonical encodings (`crates/raft`'s `Enc` rule) ----

pub(crate) fn app_id(e: &mut Enc, a: AppId) {
    enc::db(e, a.0);
}

pub(crate) fn client(e: &mut Enc, c: &ClientId) {
    e.str(c.as_str());
}

pub(crate) fn url(e: &mut Enc, u: &Url) {
    e.str(&u.0);
}

pub(crate) fn permissions(e: &mut Enc, p: &PermissionSet) {
    e.count(p.0.len());
    for (k, a) in &p.0 {
        e.str(k);
        e.u8(*a as u8);
    }
}

fn secrets(e: &mut Enc, s: &[SecretHash]) {
    e.count(s.len());
    for h in s {
        e.0.extend_from_slice(&h.0);
    }
}

fn urls(e: &mut Enc, us: &[Url]) {
    e.count(us.len());
    for u in us {
        url(e, u);
    }
}

pub(crate) fn app_record(e: &mut Enc, r: &AppRecord) {
    app_id(e, r.id);
    e.str(r.slug.as_str());
    e.str(&r.name);
    client(e, &r.client);
    e.count(r.keys.len());
    for k in &r.keys {
        enc::db(e, k.id);
        e.bytes(&k.public.0);
        enc::at(e, k.created);
    }
    permissions(e, &r.permissions);
    e.count(r.events.len());
    for k in &r.events {
        e.str(&k.0);
    }
    e.option(r.webhook.as_ref(), url);
    e.option(r.webhook_secret.as_ref(), |e, b| e.bytes(b));
    secrets(e, &r.client_secrets);
    urls(e, &r.callbacks);
    e.u8(u8::from(r.public));
}

pub(crate) fn oauth_app_record(e: &mut Enc, r: &OAuthAppRecord) {
    client(e, &r.client);
    e.str(&r.name);
    urls(e, &r.callbacks);
    secrets(e, &r.client_secrets);
    e.u8(u8::from(r.device_flow));
}
