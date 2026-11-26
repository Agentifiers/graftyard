//! `Names` groups (`design/DESIGN.md`, Shape, "Identity" and "Sites and placement"; `design/SHARED.md`, "Names").
//!
//! 1,024 shards on `Tri`. A login claim is unique on `name_shard(Login(key))`, an account's `Account` row, owner
//! `Placement` record, and login (`AccountLogin`) are on `account_shard(id)`, and a repository's `RepoOwner` row is on
//! `name_shard(Repo(id))`. Milestone 3 adds the routes from an App slug, App id, client id, or installation id to the
//! owner, shortcodes, emails, and retired repository names, each unique on its key's shard (M3-WP1).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use arrayvec::ArrayString;
use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{
    AccountId, ChainId, CredentialShard, Digest, Index, NameShard, NetworkId, Placement, PlacementError, PlacementName,
    PlacementRecord, RepoId, SiteId, Timestamp,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use smallvec::{SmallVec, smallvec};

use crate::apps::{self, AppId, ClientId, InstallationId, Slug};
use crate::authz::{AccountKind, AuthzRead};
use crate::compat::tail_serde;
use crate::credential::{CredentialRefusal, IdentityDep, Own, check_credential, credential_chains, encode_cred};
use crate::enc;
use crate::ids::{IdBlock, IdCursor};
use crate::overlay::{Overlay, SetOverlay, Table};
use crate::{NAMES_FORMAT, NAMES_FORMAT_V2, NAMES_FORMAT_V3, NAMES_FORMAT_V4};

pub const NAME_SHARDS: u16 = 1024;

/// Every first path segment the host serves, so no login can shadow a route. SHARED's list ends in `/* .. */`: the
/// rest is to be generated from the router, which does not exist yet.
pub const RESERVED: &[&str] = &[
    "api",
    "login",
    "logout",
    "session",
    "settings",
    "orgs",
    "organizations",
    "users",
    "repositories",
    "notifications",
    "search",
    "status",
    "site",
    "raw",
    "gist",
    "codeload",
    "uploads",
    "_services",
    "_t",
    "_codeload",
    "_registry",
];

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("a login is 1 to 39 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen")]
    Login,
    #[error("a repository name is 1 to 100 of [A-Za-z0-9._-], not \".\" or \"..\"")]
    Repo,
    #[error("an App slug is 1 to 34 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen")]
    Slug,
    #[error("a client id is 1 to 20 ASCII letters, digits, or dots")]
    Client,
    #[error("a shortcode is 1 to 8 ASCII lowercase letters or digits")]
    Shortcode,
}

/// An enterprise's managed-user suffix: 1 to 8 ASCII lowercase letters or digits [unverified: SHARED gives only the
/// length]. Its serde form is the text, parsed again on decode.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Shortcode(ArrayString<8>);

impl Shortcode {
    pub fn parse(s: &str) -> Result<Shortcode, NameError> {
        let ok = !s.is_empty() && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        match ArrayString::from(s) {
            Ok(a) if ok => Ok(Shortcode(a)),
            _ => Err(NameError::Shortcode),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Shortcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Shortcode({:?})", self.as_str())
    }
}

impl From<Shortcode> for String {
    fn from(c: Shortcode) -> String {
        c.as_str().into()
    }
}

impl TryFrom<String> for Shortcode {
    type Error = NameError;
    fn try_from(s: String) -> Result<Shortcode, NameError> {
        Shortcode::parse(&s)
    }
}

/// 1..=39 ASCII letters, digits, or single hyphens, not starting or ending with a hyphen. Keeps the spelling the
/// owner chose; its `key` is the only form compared. Its serde form is the text, parsed again on decode.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Login(Box<str>);

/// ASCII lowercase of a `Login`. Logins are ASCII, so lowercasing is exact.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LoginKey(Box<str>);

impl Login {
    pub fn parse(s: &str) -> Result<Login, NameError> {
        let b = s.as_bytes();
        let ok = (1..=39).contains(&b.len())
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-'
            && !s.contains("--");
        if ok { Ok(Login(s.into())) } else { Err(NameError::Login) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> LoginKey {
        LoginKey(self.0.to_ascii_lowercase().into())
    }
}

impl LoginKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the key is a first path segment the host serves.
    pub fn is_reserved(&self) -> bool {
        RESERVED.contains(&&*self.0)
    }
}

/// 1..=100 of [A-Za-z0-9._-], not "." or "..", with one trailing ".git" stripped first.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoName(Box<str>);

/// The compared form of a `RepoName`: ASCII lowercase, as for logins [inference: SHARED does not say how the key is
/// formed; GitHub repository names are case-insensitive].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoNameKey(Box<str>);

impl RepoName {
    pub fn parse(s: &str) -> Result<RepoName, NameError> {
        let s = s.strip_suffix(".git").unwrap_or(s);
        let ok = (1..=100).contains(&s.len())
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            && s != "."
            && s != "..";
        if ok { Ok(RepoName(s.into())) } else { Err(NameError::Repo) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key(&self) -> RepoNameKey {
        RepoNameKey(self.0.to_ascii_lowercase().into())
    }
}

impl RepoNameKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! debug_as_str {
    ($($t:ident),*) => {$(
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:?})", stringify!($t), &*self.0)
            }
        }
    )*};
}
debug_as_str!(Login, LoginKey, RepoName, RepoNameKey);

/// The serde forms: the text, which decoding parses again, so a stored or sent name keeps its invariants.
macro_rules! text_serde {
    ($($t:ident => $parse:expr),*) => {$(
        impl From<$t> for String {
            fn from(v: $t) -> String {
                v.0.into()
            }
        }
        impl TryFrom<String> for $t {
            type Error = NameError;
            fn try_from(s: String) -> Result<$t, NameError> {
                let parse: fn(&str) -> Result<$t, NameError> = $parse;
                parse(&s)
            }
        }
    )*};
}
text_serde!(
    Login => Login::parse,
    RepoName => |s| RepoName::parse(s).and_then(|r| if r.as_str() == s { Ok(r) } else { Err(NameError::Repo) }),
    LoginKey => |s| Login::parse(s).map(|l| l.key()).and_then(|k| if k.as_str() == s { Ok(k) } else { Err(NameError::Login) }),
    RepoNameKey => |s| RepoName::parse(s).map(|r| r.key()).and_then(|k| if k.as_str() == s { Ok(k) } else { Err(NameError::Repo) })
);

/// The design's `NameKey`: what a `Names` shard is the uniqueness point of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NameKey {
    Login(LoginKey),
    Email(Digest),
    Shortcode(Shortcode),
    App(AppId),
    AppSlug(Slug),
    Client(ClientId),
    Installation(InstallationId),
    Repo(RepoId),
}

fn shard_of(tag: &[u8], body: &[u8]) -> NameShard {
    let h = Sha256::new().chain_update(tag).chain_update(body).finalize();
    // Four bytes, so the modulo bias is about 2^-22.
    NameShard((u32::from_be_bytes([h[0], h[1], h[2], h[3]]) % u32::from(NAME_SHARDS)) as u16)
}

/// Each key kind hashes under its own tag, so two kinds never collide by construction.
pub fn name_shard(key: &NameKey) -> NameShard {
    match key {
        NameKey::Login(k) => shard_of(b"graftyard/names/login/", k.0.as_bytes()),
        NameKey::Email(d) => shard_of(b"graftyard/names/email/", &d.0),
        NameKey::Shortcode(c) => shard_of(b"graftyard/names/shortcode/", c.as_str().as_bytes()),
        NameKey::App(a) => shard_of(b"graftyard/names/app/", &a.0.0.to_be_bytes()),
        NameKey::AppSlug(s) => shard_of(b"graftyard/names/app-slug/", s.as_str().as_bytes()),
        NameKey::Client(c) => shard_of(b"graftyard/names/client/", c.as_str().as_bytes()),
        NameKey::Installation(i) => shard_of(b"graftyard/names/installation/", &i.0.0.to_be_bytes()),
        NameKey::Repo(r) => shard_of(b"graftyard/names/repo/", &r.0.0.to_be_bytes()),
    }
}

/// The shard of an account's `Account` row, its owner `Placement` record, and its login (`AccountLogin`).
pub fn account_shard(id: AccountId) -> NameShard {
    shard_of(b"graftyard/names/account/", &id.0.0.to_be_bytes())
}

/// What a login key resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoginRow {
    Claimed {
        account: AccountId,
        kind: AccountKind,
    },
    /// An owner renamed away: the old login still points at the account until someone claims it.
    Former {
        account: AccountId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRow {
    pub suspended: bool,
    pub sessions_not_before: Timestamp,
}

/// A claimed App slug: the App id the claim assigned and the owner whose `Authz` chain holds, or will hold, its
/// `AppRecord`. Whether the registration finished is the owner's `Authz` fold's to say (`crate::finish`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlugRow {
    pub app: AppId,
    pub owner: AccountId,
}

/// Where a client id leads: the owner, and the App when the client is a GitHub App's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRoute {
    pub owner: AccountId,
    pub app: Option<AppId>,
}

/// The `NamesEvent` variants. Id blocks arrive as `EntryBody::IdBlock` entries (`Fold::apply_id_block`).
/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamesEvent {
    LoginClaimed {
        key: LoginKey,
        account: AccountId,
        kind: AccountKind,
    },
    Account {
        id: AccountId,
        suspended: bool,
        sessions_not_before: Timestamp,
    },
    FormerName {
        key: LoginKey,
        account: AccountId,
    },
    /// On `account_shard(owner)`.
    Placement {
        owner: AccountId,
        record: PlacementRecord,
    },
    /// On `name_shard(NameKey::Repo(repo))`.
    RepoOwner {
        repo: RepoId,
        owner: AccountId,
    },
    // Milestone 3. Serde's variant index is the declaration order, so these follow Milestone 1's five, whose stored
    // rows decode as before; the canonical tags below follow the design's order instead.
    /// On `name_shard(Login(owner))`: the pair is retired forever (SHARED, "Names", Retirement).
    Retired {
        owner: LoginKey,
        repo: RepoNameKey,
    },
    Shortcode {
        code: Shortcode,
        enterprise: AccountId,
    },
    /// `account: None` frees the email.
    EmailClaimed {
        email: Digest,
        account: Option<AccountId>,
    },
    /// On `name_shard(App(app))`.
    AppRegistered {
        app: AppId,
        owner: AccountId,
    },
    /// On `name_shard(AppSlug(slug))`: registration's commit point. `app: Some` claims the slug for the App id the
    /// claim assigned from the shard's id block; `None` releases it. The design's `AppSlug { slug, app }` has no
    /// owner, and the `Audit` finisher must know whose `Authz` chain to finish a claim on, so `owner` is added
    /// [inference].
    AppSlug {
        slug: Slug,
        app: Option<AppId>,
        owner: AccountId,
    },
    /// On `name_shard(Client(client))`.
    ClientRegistered {
        client: ClientId,
        owner: AccountId,
        app: Option<AppId>,
    },
    /// On `name_shard(Installation(id))`.
    InstallationRegistered {
        id: InstallationId,
        app: AppId,
        owner: AccountId,
    },
    /// On `account_shard(account)`: the account's current login, so the edge renders any account by id (F50; not in
    /// the design's `NamesEvent`). Written by `NamesOp::SetAccountLogin`, whose leader reads the login's own shard
    /// as a dependency and refuses unless the login is claimed by the account then, so a write that a rename
    /// overtook is refused rather than applied.
    AccountLogin {
        account: AccountId,
        login: Login,
    },
    /// The `Audit` consumer finished, or found nothing owed for, every entry of this shard through `through`
    /// (`crate::audit`). Not in the design's `NamesEvent`, as `TokensEvent::AuditCursor`.
    AuditCursor {
        through: Index,
    },
    /// On `name_shard(NameKey::Repo(repo))`: a fork's `RepoOwner` row with its network, never its own id (format
    /// 0x10.4). A root's row stays `RepoOwner`, whose network is its own id.
    ForkOwner {
        repo: RepoId,
        owner: AccountId,
        network: NetworkId,
    },
}

/// Tags follow the design's `NamesEvent` declaration order, so tags 0 to 11 are the design's 12 variants. Milestone 1
/// wrote 0, 1, 2, 10, and 11, and they keep their bytes. `AccountLogin`, which the design does not list, takes 12,
/// `AuditCursor` 13, and `ForkOwner` 14.
impl Record for NamesEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            NamesEvent::LoginClaimed { key, account, kind } => {
                e.u8(0);
                e.str(&key.0);
                enc::account(&mut e, *account);
                e.u8(*kind as u8);
            }
            NamesEvent::Account { id, suspended, sessions_not_before } => {
                e.u8(1);
                enc::account(&mut e, *id);
                e.u8(u8::from(*suspended));
                enc::at(&mut e, *sessions_not_before);
            }
            NamesEvent::FormerName { key, account } => {
                e.u8(2);
                e.str(&key.0);
                enc::account(&mut e, *account);
            }
            NamesEvent::Retired { owner, repo } => {
                e.u8(3);
                e.str(&owner.0);
                e.str(repo.as_str());
            }
            NamesEvent::Shortcode { code, enterprise } => {
                e.u8(4);
                e.str(code.as_str());
                enc::account(&mut e, *enterprise);
            }
            NamesEvent::EmailClaimed { email, account } => {
                e.u8(5);
                e.0.extend_from_slice(&email.0);
                e.option(account.as_ref(), |e, a| enc::account(e, *a));
            }
            NamesEvent::AppRegistered { app, owner } => {
                e.u8(6);
                apps::app_id(&mut e, *app);
                enc::account(&mut e, *owner);
            }
            NamesEvent::AppSlug { slug, app, owner } => {
                e.u8(7);
                e.str(slug.as_str());
                e.option(app.as_ref(), |e, a| apps::app_id(e, *a));
                enc::account(&mut e, *owner);
            }
            NamesEvent::ClientRegistered { client, owner, app } => {
                e.u8(8);
                apps::client(&mut e, client);
                enc::account(&mut e, *owner);
                e.option(app.as_ref(), |e, a| apps::app_id(e, *a));
            }
            NamesEvent::InstallationRegistered { id, app, owner } => {
                e.u8(9);
                enc::db(&mut e, id.0);
                apps::app_id(&mut e, *app);
                enc::account(&mut e, *owner);
            }
            NamesEvent::Placement { owner, record } => {
                e.u8(10);
                enc::account(&mut e, *owner);
                enc::placement_record(&mut e, record);
            }
            NamesEvent::RepoOwner { repo, owner } => {
                e.u8(11);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *owner);
            }
            NamesEvent::AccountLogin { account, login } => {
                e.u8(12);
                enc::account(&mut e, *account);
                e.str(login.as_str());
            }
            NamesEvent::AuditCursor { through } => {
                e.u8(13);
                e.u64(through.0);
            }
            NamesEvent::ForkOwner { repo, owner, network } => {
                e.u8(14);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *owner);
                enc::db(&mut e, network.0);
            }
        }
    }
}

/// Milestone 3's rows of a `Names` shard: the fold's trailing part (`crate::compat`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamesMore {
    pub account_logins: BTreeMap<AccountId, Login>,
    pub slugs: BTreeMap<Slug, SlugRow>,
    pub apps: BTreeMap<AppId, AccountId>,
    pub clients: BTreeMap<ClientId, ClientRoute>,
    pub installations: BTreeMap<InstallationId, (AppId, AccountId)>,
    pub shortcodes: BTreeMap<Shortcode, AccountId>,
    pub emails: BTreeMap<Digest, AccountId>,
    pub retired: BTreeSet<(LoginKey, RepoNameKey)>,
    /// The `Audit` consumer's cursor (`NamesEvent::AuditCursor`), then AM3-WP2a's fork networks as a trailing part, so
    /// a fold whose cursor never moved and that holds no fork network keeps the bytes of the release before either.
    pub more: CursorPart,
}

tail_serde!(NamesMore {
    account_logins: BTreeMap<AccountId, Login>,
    slugs: BTreeMap<Slug, SlugRow>,
    apps: BTreeMap<AppId, AccountId>,
    clients: BTreeMap<ClientId, ClientRoute>,
    installations: BTreeMap<InstallationId, (AppId, AccountId)>,
    shortcodes: BTreeMap<Shortcode, AccountId>,
    emails: BTreeMap<Digest, AccountId>,
    retired: BTreeSet<(LoginKey, RepoNameKey)>,
} more: CursorPart);

/// The `Audit` cursor, then AM3-WP2a's fork networks as a trailing part, so a fold with none keeps its cursor's bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CursorPart {
    pub cursor: Index,
    pub more: ForkRows,
}

impl serde::Serialize for CursorPart {
    /// A fold with no fork network writes the cursor alone, as the `Index` it was before AM3-WP2a.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        if self.more == ForkRows::default() {
            return self.cursor.serialize(s);
        }
        let mut t = s.serialize_tuple(3)?;
        t.serialize_element(&self.cursor)?;
        t.serialize_element(&1u8)?;
        t.serialize_element(&self.more)?;
        t.end()
    }
}

impl<'de> serde::Deserialize<'de> for CursorPart {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = CursorPart;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("CursorPart")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<CursorPart, A::Error> {
                let cursor: Index = seq.next_element()?.ok_or_else(|| serde::de::Error::missing_field("cursor"))?;
                let more = match seq.next_element::<u8>() {
                    Ok(Some(1)) => seq.next_element()?.ok_or_else(|| serde::de::Error::missing_field("more"))?,
                    Ok(Some(m)) => {
                        return Err(serde::de::Error::invalid_value(
                            serde::de::Unexpected::Unsigned(u64::from(m)),
                            &"1, the marker of a fold's trailing part",
                        ));
                    }
                    Ok(None) | Err(_) => ForkRows::default(),
                };
                Ok(CursorPart { cursor, more })
            }
        }
        d.deserialize_tuple(3, V)
    }
}

/// AM3-WP2a's fork networks, keyed by the fork's repository id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkRows {
    pub networks: BTreeMap<RepoId, NetworkId>,
}

/// One `Names` shard's fold. `shard` is the chain's own shard: a command whose key belongs to another shard is
/// refused, because the shard is the key's uniqueness point. `None`, the `Default` a member starts from before its
/// host sets the shard, checks no key placement. Its serde form is Milestone 1's fields, then `more` only when it
/// holds a row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamesFold {
    pub shard: Option<NameShard>,
    pub logins: BTreeMap<LoginKey, LoginRow>,
    pub accounts: BTreeMap<AccountId, AccountRow>,
    pub placements: BTreeMap<AccountId, PlacementRecord>,
    pub repo_owners: BTreeMap<RepoId, AccountId>,
    pub ids: IdCursor,
    pub more: NamesMore,
}

tail_serde!(NamesFold {
    shard: Option<NameShard>,
    logins: BTreeMap<LoginKey, LoginRow>,
    accounts: BTreeMap<AccountId, AccountRow>,
    placements: BTreeMap<AccountId, PlacementRecord>,
    repo_owners: BTreeMap<RepoId, AccountId>,
    ids: IdCursor,
} more: NamesMore);

impl NamesFold {
    pub fn new(shard: NameShard) -> NamesFold {
        NamesFold { shard: Some(shard), ..NamesFold::default() }
    }

    /// Every entry through this index is one the `Audit` consumer finished or found nothing owed for.
    pub fn audit_cursor(&self) -> Index {
        self.more.more.cursor
    }
}

/// Every read a `Names` shard serves, over the fold or a scratch view.
pub trait NamesRead {
    fn login(&self, key: &LoginKey) -> Option<LoginRow>;
    fn account(&self, id: AccountId) -> Option<AccountRow>;
    fn placement(&self, owner: AccountId) -> Option<&PlacementRecord>;
    fn repo_owner(&self, repo: RepoId) -> Option<AccountId>;
    /// The repository's network when its `RepoOwner` row names one: a fork's (format 0x10.4). `None` for a root,
    /// whose network is its own id, and for a repository with no row.
    fn repo_network(&self, repo: RepoId) -> Option<NetworkId>;
    /// The account's current login, on its `account_shard` (F50).
    fn account_login(&self, account: AccountId) -> Option<&Login>;
    fn slug(&self, slug: &Slug) -> Option<SlugRow>;
    fn app_owner(&self, app: AppId) -> Option<AccountId>;
    fn client(&self, client: &ClientId) -> Option<ClientRoute>;
    fn installation(&self, id: InstallationId) -> Option<(AppId, AccountId)>;
    fn shortcode(&self, code: &Shortcode) -> Option<AccountId>;
    fn email(&self, email: &Digest) -> Option<AccountId>;
    fn retired(&self, owner: &LoginKey, repo: &RepoNameKey) -> bool;
}

impl NamesRead for NamesFold {
    fn login(&self, key: &LoginKey) -> Option<LoginRow> {
        self.logins.get(key).copied()
    }
    fn account(&self, id: AccountId) -> Option<AccountRow> {
        self.accounts.get(&id).copied()
    }
    fn placement(&self, owner: AccountId) -> Option<&PlacementRecord> {
        self.placements.get(&owner)
    }
    fn repo_owner(&self, repo: RepoId) -> Option<AccountId> {
        self.repo_owners.get(&repo).copied()
    }
    fn repo_network(&self, repo: RepoId) -> Option<NetworkId> {
        self.more.more.more.networks.get(&repo).copied()
    }
    fn account_login(&self, account: AccountId) -> Option<&Login> {
        self.more.account_logins.get(&account)
    }
    fn slug(&self, slug: &Slug) -> Option<SlugRow> {
        self.more.slugs.get(slug).copied()
    }
    fn app_owner(&self, app: AppId) -> Option<AccountId> {
        self.more.apps.get(&app).copied()
    }
    fn client(&self, client: &ClientId) -> Option<ClientRoute> {
        self.more.clients.get(client).copied()
    }
    fn installation(&self, id: InstallationId) -> Option<(AppId, AccountId)> {
        self.more.installations.get(&id).copied()
    }
    fn shortcode(&self, code: &Shortcode) -> Option<AccountId> {
        self.more.shortcodes.get(code).copied()
    }
    fn email(&self, email: &Digest) -> Option<AccountId> {
        self.more.emails.get(email).copied()
    }
    fn retired(&self, owner: &LoginKey, repo: &RepoNameKey) -> bool {
        self.more.retired.contains(&(owner.clone(), repo.clone()))
    }
}

struct Tables<'t, L, A, P, R, M> {
    logins: &'t mut L,
    accounts: &'t mut A,
    placements: &'t mut P,
    repo_owners: &'t mut R,
    ids: &'t mut IdCursor,
    more: &'t mut M,
}

/// The tables of `NamesMore`, as the fold's maps or the scratch view's overlays.
trait MoreTables {
    fn account_logins(&mut self) -> &mut dyn Table<AccountId, Login>;
    fn slugs(&mut self) -> &mut dyn Table<Slug, SlugRow>;
    fn apps(&mut self) -> &mut dyn Table<AppId, AccountId>;
    fn clients(&mut self) -> &mut dyn Table<ClientId, ClientRoute>;
    fn installations(&mut self) -> &mut dyn Table<InstallationId, (AppId, AccountId)>;
    fn shortcodes(&mut self) -> &mut dyn Table<Shortcode, AccountId>;
    fn emails(&mut self) -> &mut dyn Table<Digest, AccountId>;
    fn retired(&mut self) -> &mut dyn Table<(LoginKey, RepoNameKey), ()>;
    fn networks(&mut self) -> &mut dyn Table<RepoId, NetworkId>;
}

impl MoreTables for NamesMore {
    fn account_logins(&mut self) -> &mut dyn Table<AccountId, Login> {
        &mut self.account_logins
    }
    fn slugs(&mut self) -> &mut dyn Table<Slug, SlugRow> {
        &mut self.slugs
    }
    fn apps(&mut self) -> &mut dyn Table<AppId, AccountId> {
        &mut self.apps
    }
    fn clients(&mut self) -> &mut dyn Table<ClientId, ClientRoute> {
        &mut self.clients
    }
    fn installations(&mut self) -> &mut dyn Table<InstallationId, (AppId, AccountId)> {
        &mut self.installations
    }
    fn shortcodes(&mut self) -> &mut dyn Table<Shortcode, AccountId> {
        &mut self.shortcodes
    }
    fn emails(&mut self) -> &mut dyn Table<Digest, AccountId> {
        &mut self.emails
    }
    fn retired(&mut self) -> &mut dyn Table<(LoginKey, RepoNameKey), ()> {
        &mut self.retired
    }
    fn networks(&mut self) -> &mut dyn Table<RepoId, NetworkId> {
        &mut self.more.more.networks
    }
}

/// The writes of one event, shared by `NamesFold::apply` and `NamesScratch::apply` so the two never diverge.
fn write<L, A, P, R, M>(ev: &NamesEvent, t: Tables<'_, L, A, P, R, M>)
where
    L: Table<LoginKey, LoginRow>,
    A: Table<AccountId, AccountRow>,
    P: Table<AccountId, PlacementRecord>,
    R: Table<RepoId, AccountId>,
    M: MoreTables,
{
    match ev {
        NamesEvent::LoginClaimed { key, account, kind } => {
            t.logins.put(key.clone(), LoginRow::Claimed { account: *account, kind: *kind });
            t.ids.apply_assigned(account.0);
        }
        NamesEvent::Account { id, suspended, sessions_not_before } => {
            t.accounts.put(*id, AccountRow { suspended: *suspended, sessions_not_before: *sessions_not_before });
        }
        NamesEvent::FormerName { key, account } => t.logins.put(key.clone(), LoginRow::Former { account: *account }),
        NamesEvent::Retired { owner, repo } => t.more.retired().put((owner.clone(), repo.clone()), ()),
        NamesEvent::Shortcode { code, enterprise } => t.more.shortcodes().put(*code, *enterprise),
        NamesEvent::EmailClaimed { email, account: Some(a) } => t.more.emails().put(*email, *a),
        NamesEvent::EmailClaimed { email, account: None } => t.more.emails().del(email),
        NamesEvent::AppRegistered { app, owner } => t.more.apps().put(*app, *owner),
        NamesEvent::AppSlug { slug, app: Some(app), owner } => {
            t.more.slugs().put(slug.clone(), SlugRow { app: *app, owner: *owner });
            t.ids.apply_assigned(app.0);
        }
        NamesEvent::AppSlug { slug, app: None, .. } => t.more.slugs().del(slug),
        NamesEvent::ClientRegistered { client, owner, app } => {
            t.more.clients().put(*client, ClientRoute { owner: *owner, app: *app })
        }
        NamesEvent::InstallationRegistered { id, app, owner } => t.more.installations().put(*id, (*app, *owner)),
        NamesEvent::Placement { owner, record } => t.placements.put(*owner, record.clone()),
        NamesEvent::RepoOwner { repo, owner } => t.repo_owners.put(*repo, *owner),
        NamesEvent::ForkOwner { repo, owner, network } => {
            t.repo_owners.put(*repo, *owner);
            t.more.networks().put(*repo, *network);
        }
        NamesEvent::AccountLogin { account, login } => t.more.account_logins().put(*account, login.clone()),
        // The fold's own field (`NamesFold::apply`); `decide` never reads it from a scratch view.
        NamesEvent::AuditCursor { .. } => {}
    }
}

/// Overlays of `NamesMore` for the scratch view.
struct MoreOverlay<'a> {
    account_logins: Overlay<'a, AccountId, Login>,
    slugs: Overlay<'a, Slug, SlugRow>,
    apps: Overlay<'a, AppId, AccountId>,
    clients: Overlay<'a, ClientId, ClientRoute>,
    installations: Overlay<'a, InstallationId, (AppId, AccountId)>,
    shortcodes: Overlay<'a, Shortcode, AccountId>,
    emails: Overlay<'a, Digest, AccountId>,
    retired: SetOverlay<'a, (LoginKey, RepoNameKey)>,
    networks: Overlay<'a, RepoId, NetworkId>,
}

impl MoreTables for MoreOverlay<'_> {
    fn account_logins(&mut self) -> &mut dyn Table<AccountId, Login> {
        &mut self.account_logins
    }
    fn slugs(&mut self) -> &mut dyn Table<Slug, SlugRow> {
        &mut self.slugs
    }
    fn apps(&mut self) -> &mut dyn Table<AppId, AccountId> {
        &mut self.apps
    }
    fn clients(&mut self) -> &mut dyn Table<ClientId, ClientRoute> {
        &mut self.clients
    }
    fn installations(&mut self) -> &mut dyn Table<InstallationId, (AppId, AccountId)> {
        &mut self.installations
    }
    fn shortcodes(&mut self) -> &mut dyn Table<Shortcode, AccountId> {
        &mut self.shortcodes
    }
    fn emails(&mut self) -> &mut dyn Table<Digest, AccountId> {
        &mut self.emails
    }
    fn retired(&mut self) -> &mut dyn Table<(LoginKey, RepoNameKey), ()> {
        &mut self.retired
    }
    fn networks(&mut self) -> &mut dyn Table<RepoId, NetworkId> {
        &mut self.networks
    }
}

pub struct NamesScratch<'a> {
    fold: &'a NamesFold,
    logins: Overlay<'a, LoginKey, LoginRow>,
    accounts: Overlay<'a, AccountId, AccountRow>,
    placements: Overlay<'a, AccountId, PlacementRecord>,
    repo_owners: Overlay<'a, RepoId, AccountId>,
    ids: IdCursor,
    more: MoreOverlay<'a>,
}

impl NamesScratch<'_> {
    pub fn shard(&self) -> Option<NameShard> {
        self.fold.shard
    }
    pub fn ids(&self) -> &IdCursor {
        &self.ids
    }
}

impl NamesRead for NamesScratch<'_> {
    fn login(&self, key: &LoginKey) -> Option<LoginRow> {
        self.logins.get(key).copied()
    }
    fn account(&self, id: AccountId) -> Option<AccountRow> {
        self.accounts.get(&id).copied()
    }
    fn placement(&self, owner: AccountId) -> Option<&PlacementRecord> {
        self.placements.get(&owner)
    }
    fn repo_owner(&self, repo: RepoId) -> Option<AccountId> {
        self.repo_owners.get(&repo).copied()
    }
    fn repo_network(&self, repo: RepoId) -> Option<NetworkId> {
        self.more.networks.get(&repo).copied()
    }
    fn account_login(&self, account: AccountId) -> Option<&Login> {
        self.more.account_logins.get(&account)
    }
    fn slug(&self, slug: &Slug) -> Option<SlugRow> {
        self.more.slugs.get(slug).copied()
    }
    fn app_owner(&self, app: AppId) -> Option<AccountId> {
        self.more.apps.get(&app).copied()
    }
    fn client(&self, client: &ClientId) -> Option<ClientRoute> {
        self.more.clients.get(client).copied()
    }
    fn installation(&self, id: InstallationId) -> Option<(AppId, AccountId)> {
        self.more.installations.get(&id).copied()
    }
    fn shortcode(&self, code: &Shortcode) -> Option<AccountId> {
        self.more.shortcodes.get(code).copied()
    }
    fn email(&self, email: &Digest) -> Option<AccountId> {
        self.more.emails.get(email).copied()
    }
    fn retired(&self, owner: &LoginKey, repo: &RepoNameKey) -> bool {
        self.more.retired.contains(&(owner.clone(), repo.clone()))
    }
}

/// Whether `next` may replace `prev` as an owner's record: a first record has no `moving_from`; a move bumps
/// `generation` by one and keeps the record it replaces, `moving_from` included, in `moving_from`; finishing a move
/// clears `moving_from` and keeps everything else.
pub(crate) fn follows(prev: Option<&PlacementRecord>, next: &PlacementRecord) -> bool {
    match (prev, next.moving_from.as_deref()) {
        (None, None) => true,
        (Some(p), Some(from)) => from == p && next.generation == p.generation + 1,
        (Some(p), None) => p.moving_from.is_some() && PlacementRecord { moving_from: None, ..p.clone() } == *next,
        (None, Some(_)) => false,
    }
}

/// A key taken by another owner, as a follower checks it: `Some(by)` when `owner` may not take a route `by` holds.
fn taken_by(held: Option<AccountId>, owner: AccountId) -> Option<AccountId> {
    held.filter(|by| *by != owner)
}

impl Scratch for NamesScratch<'_> {
    type Event = NamesEvent;
    fn apply(&mut self, ev: &NamesEvent) {
        write(
            ev,
            Tables {
                logins: &mut self.logins,
                accounts: &mut self.accounts,
                placements: &mut self.placements,
                repo_owners: &mut self.repo_owners,
                ids: &mut self.ids,
                more: &mut self.more,
            },
        );
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
    /// What a follower checks before it stores an entry: no claim of a reserved key or over another account's live
    /// claim, a placement that `follows` the record it replaces, and no route, slug, shortcode, or email taken from
    /// the owner that holds it. `verify` checks that an id block follows the group's last one.
    fn check(&self, ev: &NamesEvent) -> Result<(), Box<str>> {
        let held = |what: &str, by: AccountId| Err(format!("{what} is held by {by:?}").into());
        match ev {
            NamesEvent::LoginClaimed { key, account, .. } => {
                if key.is_reserved() {
                    return Err(format!("{key:?} is reserved").into());
                }
                match self.login(key) {
                    Some(LoginRow::Claimed { account: a, .. }) if a != *account => {
                        Err(format!("{key:?} is claimed by {a:?}").into())
                    }
                    _ => Ok(()),
                }
            }
            NamesEvent::Placement { owner, record } if !follows(self.placement(*owner), record) => {
                Err(format!("placement of {owner:?} does not follow {:?}", self.placement(*owner)).into())
            }
            NamesEvent::ForkOwner { repo, network, .. } if network.root() == *repo => {
                Err("a fork's network is not its own id".into())
            }
            NamesEvent::ForkOwner { repo, network, .. } => match self.repo_network(*repo) {
                Some(n) if n != *network => Err(format!("{repo:?} is already in network {n:?}").into()),
                _ => Ok(()),
            },
            NamesEvent::AppSlug { slug, app: Some(app), .. } => match self.slug(slug) {
                Some(r) if r.app != *app => Err(format!("{slug:?} is claimed for {:?} of {:?}", r.app, r.owner).into()),
                _ => Ok(()),
            },
            NamesEvent::AppRegistered { app, owner } => match taken_by(self.app_owner(*app), *owner) {
                Some(by) => held(&format!("{app:?}"), by),
                None => Ok(()),
            },
            NamesEvent::ClientRegistered { client, owner, .. } => {
                match taken_by(self.client(client).map(|r| r.owner), *owner) {
                    Some(by) => held(&format!("{client:?}"), by),
                    None => Ok(()),
                }
            }
            NamesEvent::InstallationRegistered { id, app, .. } => match self.installation(*id) {
                Some((a, _)) if a != *app => Err(format!("{id:?} belongs to {a:?}").into()),
                _ => Ok(()),
            },
            NamesEvent::Shortcode { code, enterprise } => match taken_by(self.shortcode(code), *enterprise) {
                Some(by) => held(&format!("{code:?}"), by),
                None => Ok(()),
            },
            NamesEvent::EmailClaimed { email, account: Some(a) } => match taken_by(self.email(email), *a) {
                Some(by) => held("the email", by),
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }
}

impl Fold for NamesFold {
    type Event = NamesEvent;
    type Scratch<'a> = NamesScratch<'a>;
    fn scratch(&self) -> NamesScratch<'_> {
        let m = &self.more;
        NamesScratch {
            fold: self,
            logins: Overlay::new(&self.logins),
            accounts: Overlay::new(&self.accounts),
            placements: Overlay::new(&self.placements),
            repo_owners: Overlay::new(&self.repo_owners),
            ids: self.ids,
            more: MoreOverlay {
                account_logins: Overlay::new(&m.account_logins),
                slugs: Overlay::new(&m.slugs),
                apps: Overlay::new(&m.apps),
                clients: Overlay::new(&m.clients),
                installations: Overlay::new(&m.installations),
                shortcodes: Overlay::new(&m.shortcodes),
                emails: Overlay::new(&m.emails),
                retired: SetOverlay::new(&m.retired),
                networks: Overlay::new(&m.more.more.networks),
            },
        }
    }
    fn apply(&mut self, ev: &NamesEvent) {
        write(
            ev,
            Tables {
                logins: &mut self.logins,
                accounts: &mut self.accounts,
                placements: &mut self.placements,
                repo_owners: &mut self.repo_owners,
                ids: &mut self.ids,
                more: &mut self.more,
            },
        );
        // Never back: two cursor commands in one entry leave the later of the two.
        if let NamesEvent::AuditCursor { through } = ev {
            self.more.more.cursor = self.more.more.cursor.max(*through);
        }
    }
    fn apply_id_block(&mut self, b: IdBlock) {
        self.ids.apply_block(b);
    }
}
// ---- Commands ----

/// One `Names` command and the credential it was submitted under (`CredentialShard::NONE` for `graft-ops`, the CLI,
/// and consumers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamesCmd {
    pub cred: CredentialShard,
    pub op: NamesOp,
}

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamesOp {
    /// Claim `login` on its shard. `account: None` is a signup: the account takes the next id of the group's block.
    /// `Some` is an existing account taking another login, the commit point of an owner rename.
    ClaimLogin {
        login: Login,
        kind: AccountKind,
        account: Option<AccountId>,
    },
    /// The account gives up `key`, which then points at it as a former name until someone claims it.
    ReleaseLogin {
        key: LoginKey,
        account: AccountId,
    },
    /// Writes the account's `Account` row on `account_shard(id)`: suspension and sign-out-everywhere.
    PutAccount {
        id: AccountId,
        suspended: bool,
        sessions_not_before: Timestamp,
    },
    /// A new owner's first record: a copy of the install account's record (`graftyard placement set default`).
    InitPlacement {
        owner: AccountId,
        record: PlacementRecord,
    },
    /// `graftyard placement set <owner>`, after `validate_record`: bumps `generation` and keeps the old record in
    /// `moving_from`.
    MovePlacement {
        owner: AccountId,
        name: PlacementName,
        sites: SmallVec<[SiteId; 3]>,
        home: SiteId,
        at: Timestamp,
    },
    /// The move Workflow's last step, once every group of the owner moved. Refused unless `generation` is the
    /// record's, so a stale Workflow cannot clear a later move's `moving_from`.
    FinishMove {
        owner: AccountId,
        generation: u32,
    },
    RepoOwner {
        repo: RepoId,
        owner: AccountId,
    },
    // ---- Milestone 3, format 2 ----
    /// Writes `account`'s login on its `account_shard` (F50). The leader reads the login's shard as a dependency and
    /// refuses unless the login is claimed by `account` then. A signup writes it after its claim commits, and a
    /// rename after the old login's `ReleaseLogin`, so a stale write of the old login is refused.
    SetAccountLogin {
        account: AccountId,
        login: Login,
    },
    /// An App registration's commit point, on `name_shard(AppSlug(slug))`: the slug is taken and the App takes the
    /// next id of the group's block. Then come `AuthzOp::RegisterApp` on the owner's chain, and `RegisterApp` and
    /// `RegisterClient` here (`crate::finish`).
    ClaimSlug {
        slug: Slug,
        owner: AccountId,
    },
    /// The abort path: frees a slug once its App's registration on the owner's `Authz` chain is deleted or was
    /// aborted, which the leader reads there as a dependency.
    ReleaseSlug {
        slug: Slug,
        app: AppId,
        owner: AccountId,
    },
    /// The route from an App id to its owner, on `name_shard(App(app))`.
    RegisterApp {
        app: AppId,
        owner: AccountId,
    },
    /// The route from a client id to its owner, on `name_shard(Client(client))`. For an App the client id is
    /// `ClientId::for_app`, so it is never taken; for an OAuth app it is the registration's commit point
    /// [inference: the design names no commit point for an OAuth app], and a taken id is redrawn by the edge.
    RegisterClient {
        client: ClientId,
        owner: AccountId,
        app: Option<AppId>,
    },
    /// The route from an installation id to its App and the account it is installed on.
    RegisterInstallation {
        id: InstallationId,
        app: AppId,
        owner: AccountId,
    },
    /// An enterprise's managed-user suffix. Only an operator submits it.
    ClaimShortcode {
        code: Shortcode,
        enterprise: AccountId,
    },
    /// A verified email for `account`: unique install-wide.
    ClaimEmail {
        email: Digest,
        account: AccountId,
    },
    ReleaseEmail {
        email: Digest,
        account: AccountId,
    },
    /// Retires a repository name under an owner's login, forever. Only an operator or a consumer submits it.
    Retire {
        owner: LoginKey,
        repo: RepoNameKey,
    },
    // ---- format 3 ----
    /// The `Audit` consumer's position on `shard`, which only the consumer, as the operator, submits. A cursor at or
    /// behind the shard's decides to no events.
    AuditCursor {
        shard: NameShard,
        through: Index,
    },
    // ---- format 4 ----
    /// `RepoOwner` with the repository's network: a fork's create sends it (AM3-WP2a). `decide` writes `RepoOwner`
    /// when `network` is the repository's own id, so a root's row keeps its bytes, and `ForkOwner` otherwise.
    RepoOwnerIn {
        repo: RepoId,
        owner: AccountId,
        network: NetworkId,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NamesOut {
    Account(AccountId),
    Placement(PlacementRecord),
    Done,
    App(AppId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamesErr {
    /// 401: the credential's row is missing, revoked, or suspended.
    Unauthorized(CredentialRefusal),
    /// 403: the actor may not submit this op.
    Forbidden,
    /// The key belongs to another shard: the edge routed it wrong.
    WrongShard {
        want: NameShard,
    },
    Reserved,
    /// 422 `already_exists`.
    Taken {
        by: AccountId,
    },
    /// The account does not hold the login it releases or names.
    NotHolder,
    /// The group has no free id: lease a block from `Ids`, commit it here (`Member::id_block`), and re-send.
    NeedIdBlock,
    NoRecord,
    RecordExists,
    /// The move names the record the owner already has.
    Unchanged,
    StaleGeneration {
        current: u32,
    },
    BadRecord(PlacementError),
    /// The slug is not claimed for this App.
    NoSlug,
    /// The App's registration on its owner's `Authz` chain is live, so its slug stays.
    Registered,
    /// The App has no record on its owner's `Authz` chain, so no route may lead to it yet.
    NotRegistered,
    /// An App's client id is `ClientId::for_app`, and an OAuth app's never has that form.
    BadClient,
}

impl Record for NamesCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        encode_cred(&mut e, &self.cred);
        match &self.op {
            NamesOp::ClaimLogin { login, kind, account } => {
                e.u8(0);
                e.str(&login.0);
                e.u8(*kind as u8);
                e.option(account.as_ref(), |e, a| enc::account(e, *a));
            }
            NamesOp::ReleaseLogin { key, account } => {
                e.u8(1);
                e.str(&key.0);
                enc::account(&mut e, *account);
            }
            NamesOp::PutAccount { id, suspended, sessions_not_before } => {
                e.u8(2);
                enc::account(&mut e, *id);
                e.u8(u8::from(*suspended));
                enc::at(&mut e, *sessions_not_before);
            }
            NamesOp::InitPlacement { owner, record } => {
                e.u8(3);
                enc::account(&mut e, *owner);
                enc::placement_record(&mut e, record);
            }
            NamesOp::MovePlacement { owner, name, sites, home, at } => {
                e.u8(4);
                enc::account(&mut e, *owner);
                let r = PlacementRecord {
                    name: *name,
                    sites: sites.clone(),
                    home: *home,
                    generation: 0,
                    moving_from: None,
                    at: *at,
                };
                enc::placement_record(&mut e, &r);
            }
            NamesOp::FinishMove { owner, generation } => {
                e.u8(5);
                enc::account(&mut e, *owner);
                e.u32(*generation);
            }
            NamesOp::RepoOwner { repo, owner } => {
                e.u8(6);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *owner);
            }
            NamesOp::SetAccountLogin { account, login } => {
                e.u8(7);
                enc::account(&mut e, *account);
                e.str(&login.0);
            }
            NamesOp::ClaimSlug { slug, owner } => {
                e.u8(8);
                e.str(slug.as_str());
                enc::account(&mut e, *owner);
            }
            NamesOp::ReleaseSlug { slug, app, owner } => {
                e.u8(9);
                e.str(slug.as_str());
                apps::app_id(&mut e, *app);
                enc::account(&mut e, *owner);
            }
            NamesOp::RegisterApp { app, owner } => {
                e.u8(10);
                apps::app_id(&mut e, *app);
                enc::account(&mut e, *owner);
            }
            NamesOp::RegisterClient { client, owner, app } => {
                e.u8(11);
                apps::client(&mut e, client);
                enc::account(&mut e, *owner);
                e.option(app.as_ref(), |e, a| apps::app_id(e, *a));
            }
            NamesOp::RegisterInstallation { id, app, owner } => {
                e.u8(12);
                enc::db(&mut e, id.0);
                apps::app_id(&mut e, *app);
                enc::account(&mut e, *owner);
            }
            NamesOp::ClaimShortcode { code, enterprise } => {
                e.u8(13);
                e.str(code.as_str());
                enc::account(&mut e, *enterprise);
            }
            NamesOp::ClaimEmail { email, account } => {
                e.u8(14);
                e.0.extend_from_slice(&email.0);
                enc::account(&mut e, *account);
            }
            NamesOp::ReleaseEmail { email, account } => {
                e.u8(15);
                e.0.extend_from_slice(&email.0);
                enc::account(&mut e, *account);
            }
            NamesOp::Retire { owner, repo } => {
                e.u8(16);
                e.str(&owner.0);
                e.str(repo.as_str());
            }
            NamesOp::AuditCursor { shard, through } => {
                e.u8(17);
                e.u16(shard.0);
                e.u64(through.0);
            }
            NamesOp::RepoOwnerIn { repo, owner, network } => {
                e.u8(18);
                enc::repo(&mut e, *repo);
                enc::account(&mut e, *owner);
                enc::db(&mut e, network.0);
            }
        }
    }
}

impl NamesCmd {
    /// The shard this command must run on, when it names a key.
    pub fn shard(&self) -> Option<NameShard> {
        let key = match &self.op {
            NamesOp::ClaimLogin { login, .. } => NameKey::Login(login.key()),
            NamesOp::ReleaseLogin { key, .. } => NameKey::Login(key.clone()),
            NamesOp::PutAccount { id: owner, .. }
            | NamesOp::InitPlacement { owner, .. }
            | NamesOp::MovePlacement { owner, .. }
            | NamesOp::FinishMove { owner, .. }
            | NamesOp::SetAccountLogin { account: owner, .. } => return Some(account_shard(*owner)),
            NamesOp::RepoOwner { repo, .. } | NamesOp::RepoOwnerIn { repo, .. } => NameKey::Repo(*repo),
            NamesOp::ClaimSlug { slug, .. } | NamesOp::ReleaseSlug { slug, .. } => NameKey::AppSlug(slug.clone()),
            NamesOp::RegisterApp { app, .. } => NameKey::App(*app),
            NamesOp::RegisterClient { client, .. } => NameKey::Client(*client),
            NamesOp::RegisterInstallation { id, .. } => NameKey::Installation(*id),
            NamesOp::ClaimShortcode { code, .. } => NameKey::Shortcode(*code),
            NamesOp::ClaimEmail { email, .. } | NamesOp::ReleaseEmail { email, .. } => NameKey::Email(*email),
            NamesOp::Retire { owner, .. } => NameKey::Login(owner.clone()),
            NamesOp::AuditCursor { shard, .. } => return Some(*shard),
        };
        Some(name_shard(&key))
    }

    /// The owner whose `Authz` fold the leader reads for this op: to check that a user submitting it administers
    /// the owner, and always for the steps that follow a record, since a route may lead only to a registered App and
    /// a slug is freed only once its registration is closed.
    fn owner_dep(&self) -> Option<AccountId> {
        match &self.op {
            NamesOp::ReleaseSlug { owner, .. }
            | NamesOp::RegisterApp { owner, .. }
            | NamesOp::RegisterClient { owner, app: Some(_), .. } => Some(*owner),
            NamesOp::ClaimSlug { owner, .. }
            | NamesOp::RegisterClient { owner, .. }
            | NamesOp::RegisterInstallation { owner, .. } => self.cred.account.map(|_| *owner),
            _ => None,
        }
    }

    /// The other `Names` shard the leader reads: the login's own shard, for `SetAccountLogin`.
    fn login_dep(&self) -> Option<ChainId> {
        match &self.op {
            NamesOp::SetAccountLogin { login, .. } => Some(ChainId::Names(name_shard(&NameKey::Login(login.key())))),
            _ => None,
        }
    }

    /// Who may submit the op. `graft-ops`, the CLI, and finishers name no account and may submit any. A user may
    /// sign up an org, take or give up a login or an email for itself, end its own sessions, record its own login,
    /// and register Apps and OAuth apps for an owner it administers; only an operator suspends, places, records a
    /// repository owner, claims a shortcode, or retires a name. The follow-ups of a multi-chain operation
    /// (`RepoOwner`, `SetAccountLogin` for an org) are submitted by the finisher with no account, so a credential
    /// revoked in between cannot strand them.
    fn permitted(&self, s: &NamesScratch<'_>, authz: Option<&dyn AuthzRead>) -> bool {
        let Some(actor) = self.cred.account else { return self.cred.is_operator() };
        let admin = |owner: AccountId| {
            authz.is_some_and(|a| a.owner() == Some(owner) && a.is_admin(crate::credential::UserId(actor)))
        };
        match &self.op {
            NamesOp::ClaimLogin { account, kind, .. } => match account {
                Some(a) => *a == actor,
                None => *kind == AccountKind::Org,
            },
            NamesOp::ReleaseLogin { account, .. }
            | NamesOp::SetAccountLogin { account, .. }
            | NamesOp::ClaimEmail { account, .. }
            | NamesOp::ReleaseEmail { account, .. } => *account == actor,
            NamesOp::PutAccount { id, suspended, .. } => {
                *id == actor && s.account(*id).is_some_and(|r| r.suspended == *suspended)
            }
            NamesOp::ClaimSlug { owner, .. }
            | NamesOp::ReleaseSlug { owner, .. }
            | NamesOp::RegisterApp { owner, .. }
            | NamesOp::RegisterClient { owner, .. }
            | NamesOp::RegisterInstallation { owner, .. } => admin(*owner),
            NamesOp::InitPlacement { .. }
            | NamesOp::MovePlacement { .. }
            | NamesOp::FinishMove { .. }
            | NamesOp::RepoOwner { .. }
            | NamesOp::RepoOwnerIn { .. }
            | NamesOp::ClaimShortcode { .. }
            | NamesOp::Retire { .. }
            | NamesOp::AuditCursor { .. } => false,
        }
    }

    fn decide_op(
        &self,
        s: &NamesScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[NamesEvent; 4]>, NamesOut), NamesErr> {
        let authz = match self.owner_dep() {
            Some(owner) => match dep_of(deps, ChainId::Authz(owner)) {
                Some(IdentityDep::Authz(f)) => Some(f as &dyn AuthzRead),
                _ => return Err(NamesErr::Unauthorized(CredentialRefusal::NotRead(ChainId::Authz(owner)))),
            },
            None => None,
        };
        if !self.permitted(s, authz) {
            return Err(NamesErr::Forbidden);
        }
        let placed = |owner: AccountId, record: PlacementRecord| {
            Ok((smallvec![NamesEvent::Placement { owner, record: record.clone() }], NamesOut::Placement(record)))
        };
        let one = |ev: NamesEvent, out: NamesOut| Ok((smallvec![ev], out));
        match &self.op {
            NamesOp::ClaimLogin { login, kind, account } => {
                let key = login.key();
                if key.is_reserved() {
                    return Err(NamesErr::Reserved);
                }
                let account = match (s.login(&key), account) {
                    (Some(LoginRow::Claimed { account: by, .. }), _) => return Err(NamesErr::Taken { by }),
                    (_, Some(a)) => *a,
                    (_, None) => AccountId(s.ids.peek().ok_or(NamesErr::NeedIdBlock)?),
                };
                one(NamesEvent::LoginClaimed { key, account, kind: *kind }, NamesOut::Account(account))
            }
            NamesOp::ReleaseLogin { key, account } => match s.login(key) {
                Some(LoginRow::Claimed { account: a, .. }) if a == *account => {
                    one(NamesEvent::FormerName { key: key.clone(), account: *account }, NamesOut::Account(*account))
                }
                _ => Err(NamesErr::NotHolder),
            },
            NamesOp::PutAccount { id, suspended, sessions_not_before } => one(
                NamesEvent::Account { id: *id, suspended: *suspended, sessions_not_before: *sessions_not_before },
                NamesOut::Done,
            ),
            NamesOp::InitPlacement { owner, record } => {
                if s.placement(*owner).is_some() {
                    return Err(NamesErr::RecordExists);
                }
                Placement::from_record(ChainId::Authz(*owner), record).map_err(NamesErr::BadRecord)?;
                placed(*owner, PlacementRecord { moving_from: None, ..record.clone() })
            }
            NamesOp::MovePlacement { owner, name, sites, home, at } => {
                // A move during a move nests: the new record keeps the whole chain, so a lookup still walks to
                // every record a group may sit in (`PlacementRecord::lookup_order`).
                let prev = s.placement(*owner).ok_or(NamesErr::NoRecord)?;
                if prev.name == *name && prev.sites == *sites && prev.home == *home {
                    return Err(NamesErr::Unchanged);
                }
                let record = PlacementRecord {
                    name: *name,
                    sites: sites.clone(),
                    home: *home,
                    generation: prev.generation + 1,
                    moving_from: Some(Box::new(prev.clone())),
                    at: *at,
                };
                Placement::from_record(ChainId::Authz(*owner), &record).map_err(NamesErr::BadRecord)?;
                placed(*owner, record)
            }
            NamesOp::FinishMove { owner, generation } => {
                let prev = s.placement(*owner).ok_or(NamesErr::NoRecord)?;
                if prev.generation != *generation || prev.moving_from.is_none() {
                    return Err(NamesErr::StaleGeneration { current: prev.generation });
                }
                placed(*owner, PlacementRecord { moving_from: None, ..prev.clone() })
            }
            NamesOp::RepoOwner { repo, owner } => {
                one(NamesEvent::RepoOwner { repo: *repo, owner: *owner }, NamesOut::Done)
            }
            NamesOp::RepoOwnerIn { repo, owner, network } => {
                if s.repo_network(*repo) == Some(*network) && s.repo_owner(*repo) == Some(*owner) {
                    return Ok((SmallVec::new(), NamesOut::Done));
                }
                if network.root() == *repo {
                    one(NamesEvent::RepoOwner { repo: *repo, owner: *owner }, NamesOut::Done)
                } else {
                    one(NamesEvent::ForkOwner { repo: *repo, owner: *owner, network: *network }, NamesOut::Done)
                }
            }
            NamesOp::SetAccountLogin { account, login } => {
                let chain = self.login_dep().expect("a login");
                let row = if s.fold.shard.map(ChainId::Names) == Some(chain) {
                    s.login(&login.key())
                } else {
                    match dep_of(deps, chain) {
                        Some(IdentityDep::Names(f)) => f.login(&login.key()),
                        _ => return Err(NamesErr::Unauthorized(CredentialRefusal::NotRead(chain))),
                    }
                };
                if !matches!(row, Some(LoginRow::Claimed { account: a, .. }) if a == *account) {
                    return Err(NamesErr::NotHolder);
                }
                if s.account_login(*account) == Some(login) {
                    return Ok((SmallVec::new(), NamesOut::Done));
                }
                one(NamesEvent::AccountLogin { account: *account, login: login.clone() }, NamesOut::Done)
            }
            NamesOp::ClaimSlug { slug, owner } => {
                if let Some(r) = s.slug(slug) {
                    return Err(NamesErr::Taken { by: r.owner });
                }
                let app = AppId(s.ids.peek().ok_or(NamesErr::NeedIdBlock)?);
                one(NamesEvent::AppSlug { slug: slug.clone(), app: Some(app), owner: *owner }, NamesOut::App(app))
            }
            NamesOp::ReleaseSlug { slug, app, owner } => {
                if s.slug(slug) != Some(SlugRow { app: *app, owner: *owner }) {
                    return Err(NamesErr::NoSlug);
                }
                if !authz.is_some_and(|a| a.app_closed(*app)) {
                    return Err(NamesErr::Registered);
                }
                one(NamesEvent::AppSlug { slug: slug.clone(), app: None, owner: *owner }, NamesOut::Done)
            }
            NamesOp::RegisterApp { app, .. } | NamesOp::RegisterClient { app: Some(app), .. }
                if !authz.is_some_and(|a| a.app(*app).is_some()) =>
            {
                Err(NamesErr::NotRegistered)
            }
            NamesOp::RegisterApp { app, owner } => match s.app_owner(*app) {
                Some(by) if by != *owner => Err(NamesErr::Taken { by }),
                Some(_) => Ok((SmallVec::new(), NamesOut::Done)),
                None => one(NamesEvent::AppRegistered { app: *app, owner: *owner }, NamesOut::Done),
            },
            NamesOp::RegisterClient { client, owner, app } => {
                if client.is_app_form() != app.is_some() || app.is_some_and(|a| ClientId::for_app(a) != *client) {
                    return Err(NamesErr::BadClient);
                }
                match s.client(client) {
                    Some(r) if r == (ClientRoute { owner: *owner, app: *app }) => Ok((SmallVec::new(), NamesOut::Done)),
                    Some(r) => Err(NamesErr::Taken { by: r.owner }),
                    None => {
                        one(NamesEvent::ClientRegistered { client: *client, owner: *owner, app: *app }, NamesOut::Done)
                    }
                }
            }
            // Ids `Ids` assigns are positive; a negative one is an owner's Actions installation, which has no route.
            NamesOp::RegisterInstallation { id, .. } if id.0.0 <= 0 => Err(NamesErr::Reserved),
            NamesOp::RegisterInstallation { id, app, owner } => match s.installation(*id) {
                Some(r) if r == (*app, *owner) => Ok((SmallVec::new(), NamesOut::Done)),
                Some((_, by)) => Err(NamesErr::Taken { by }),
                None => one(NamesEvent::InstallationRegistered { id: *id, app: *app, owner: *owner }, NamesOut::Done),
            },
            NamesOp::ClaimShortcode { code, enterprise } => match s.shortcode(code) {
                Some(by) => Err(NamesErr::Taken { by }),
                None => one(NamesEvent::Shortcode { code: *code, enterprise: *enterprise }, NamesOut::Done),
            },
            NamesOp::ClaimEmail { email, account } => match s.email(email) {
                Some(by) if by != *account => Err(NamesErr::Taken { by }),
                Some(_) => Ok((SmallVec::new(), NamesOut::Done)),
                None => one(NamesEvent::EmailClaimed { email: *email, account: Some(*account) }, NamesOut::Done),
            },
            NamesOp::ReleaseEmail { email, account } => match s.email(email) {
                Some(a) if a == *account => {
                    one(NamesEvent::EmailClaimed { email: *email, account: None }, NamesOut::Done)
                }
                _ => Err(NamesErr::NotHolder),
            },
            NamesOp::Retire { owner, repo } => {
                if s.retired(owner, repo) {
                    return Ok((SmallVec::new(), NamesOut::Done));
                }
                one(NamesEvent::Retired { owner: owner.clone(), repo: repo.clone() }, NamesOut::Done)
            }
            NamesOp::AuditCursor { through, .. } => {
                if *through <= s.fold.audit_cursor() {
                    return Ok((SmallVec::new(), NamesOut::Done));
                }
                one(NamesEvent::AuditCursor { through: *through }, NamesOut::Done)
            }
        }
    }
}

fn dep_of(deps: &[(ChainId, IdentityDep)], chain: ChainId) -> Option<&IdentityDep> {
    deps.iter().find(|(c, _)| *c == chain).map(|(_, d)| d)
}

impl Command for NamesCmd {
    type Fold = NamesFold;
    type Dep = IdentityDep;
    type Out = NamesOut;
    type Err = NamesErr;

    /// Milestone 1's ops keep format 1, so their entries are what an older release reads; the rest are format 2.
    fn format(&self) -> Format {
        match self.op {
            NamesOp::ClaimLogin { .. }
            | NamesOp::ReleaseLogin { .. }
            | NamesOp::PutAccount { .. }
            | NamesOp::InitPlacement { .. }
            | NamesOp::MovePlacement { .. }
            | NamesOp::FinishMove { .. }
            | NamesOp::RepoOwner { .. } => NAMES_FORMAT,
            NamesOp::AuditCursor { .. } => NAMES_FORMAT_V3,
            NamesOp::RepoOwnerIn { .. } => NAMES_FORMAT_V4,
            _ => NAMES_FORMAT_V2,
        }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, fold: &NamesFold) -> SmallVec<[ChainId; 3]> {
        let own = fold.shard.map(ChainId::Names);
        let mut d = credential_chains(&self.cred, own);
        for c in [self.owner_dep().map(ChainId::Authz), self.login_dep()].into_iter().flatten() {
            if Some(c) != own && !d.contains(&c) {
                d.push(c);
            }
        }
        d
    }
    fn decide(
        &self,
        s: &NamesScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, NamesOut), NamesErr> {
        let own = match s.fold.shard {
            Some(shard) => Own::Names(shard, s),
            None => Own::Other,
        };
        check_credential(&self.cred, own, deps).map_err(NamesErr::Unauthorized)?;
        if let (Some(want), Some(have)) = (self.shard(), s.fold.shard)
            && want != have
        {
            return Err(NamesErr::WrongShard { want });
        }
        self.decide_op(s, deps)
    }
    fn replay(&self, events: &[NamesEvent]) -> NamesOut {
        match events.first() {
            Some(NamesEvent::LoginClaimed { account, .. } | NamesEvent::FormerName { account, .. }) => {
                NamesOut::Account(*account)
            }
            Some(NamesEvent::Placement { record, .. }) => NamesOut::Placement(record.clone()),
            Some(NamesEvent::AppSlug { app: Some(app), .. }) => NamesOut::App(*app),
            _ => NamesOut::Done,
        }
    }
}
