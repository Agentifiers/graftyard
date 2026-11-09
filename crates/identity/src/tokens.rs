//! Stored-token authentication (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Identity and tokens").
//!
//! A stored token is a prefix, 30 base62 secret characters, and the CRC32 of those 30 characters in 6 base62
//! characters. The edge checks the prefix and CRC offline, hashes the secret, and finds the row on
//! `token_shard(secret)`. A new token's secret is redrawn until its shard lands in the owner's home family.
//!
//! Milestone 3 (M3-WP2) adds the rest of the shard's rows: the optional `Stored` fields, a `ghu_` and its `ghr_` on
//! one shard, SSH keys on `fingerprint_shard`, revoked `jti`s on `jti_shard`, verifying keys on `TokenShard(0)`,
//! lifeboat token hashes, and the short-lived OAuth and sign-in codes, which lapse at `expires` with no command.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{
    AccountId, ChainId, CredentialKey, CredentialShard, DbId, Digest, Index, Region, RepoId, Timestamp, TokenShard,
    Ulid,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use smallvec::{SmallVec, smallvec};

use crate::apps::{self, AppId, ClientId, PermissionSet, Url};
use crate::compat::tail_serde;
use crate::credential::{CredentialRefusal, IdentityDep, OrgId, Own, check_credential, credential_chains, encode_cred};
use crate::enc;
use crate::overlay::{Overlay, SetOverlay, Table};
use crate::{TOKENS_FORMAT, TOKENS_FORMAT_V2, TOKENS_FORMAT_V3};

/// 1,024 shards per home family.
pub const SHARDS_PER_FAMILY: u16 = 1024;
const FAMILIES: u16 = 3;
/// Every `Tokens` shard: `TokenShard(0 .. TOKEN_SHARDS)`. Install creates each one's group (R20).
pub const TOKEN_SHARDS: u16 = SHARDS_PER_FAMILY * FAMILIES;
/// A new secret is drawn at most this many times. Each draw lands in a given family with chance 1/3, so 64 draws
/// all miss with chance (2/3)^64, about 5e-12.
const MAX_DRAWS: u32 = 64;

/// A token's home family: `TokenShard(f * 1024 .. (f + 1) * 1024)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Family {
    Na,
    Eu,
    Apac,
}

impl Family {
    pub fn of(shard: TokenShard) -> Family {
        match shard.0 / SHARDS_PER_FAMILY {
            0 => Family::Na,
            1 => Family::Eu,
            _ => Family::Apac,
        }
    }
}

/// The family of a region: the owner's home site's region [inference: the design names the families na, eu, and
/// apac without mapping regions to them].
pub fn family_of(region: Region) -> Family {
    match region {
        Region::Enam | Region::Wnam => Family::Na,
        Region::Weur | Region::Eeur => Family::Eu,
        Region::Apac | Region::Oc => Family::Apac,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TokenKind {
    Pat,
    FineGrained,
    OAuth,
    UserToServer,
    Installation,
    Refresh,
}

impl TokenKind {
    pub const ALL: [TokenKind; 6] = [
        TokenKind::Pat,
        TokenKind::FineGrained,
        TokenKind::OAuth,
        TokenKind::UserToServer,
        TokenKind::Installation,
        TokenKind::Refresh,
    ];

    pub fn prefix(self) -> &'static str {
        match self {
            TokenKind::Pat => "ghp_",
            TokenKind::FineGrained => "github_pat_",
            TokenKind::OAuth => "gho_",
            TokenKind::UserToServer => "ghu_",
            TokenKind::Installation => "ghs_",
            TokenKind::Refresh => "ghr_",
        }
    }
}

/// Classic OAuth scopes as a bit set. Milestone 1 stores and returns them; which bit is which scope is Milestone 3's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeSet(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("no stored-token prefix")]
    Prefix,
    #[error("an installation token is a stateless JWS, not a stored token")]
    Stateless,
    #[error("a stored token is its prefix, 30 base62 characters, and a 6-character checksum")]
    Shape,
    #[error("checksum mismatch")]
    Checksum,
}

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn crc_text(secret: &[u8; 30]) -> [u8; 6] {
    let mut v = crc32(secret);
    let mut out = [b'0'; 6];
    for c in out.iter_mut().rev() {
        *c = BASE62[(v % 62) as usize];
        v /= 62;
    }
    out
}

/// A stored token: `ghp_`, `github_pat_`, `gho_`, `ghu_`, or `ghr_`, then 30 base62 characters (about 178 random
/// bits), then the CRC32 of those 30 characters in 6 base62 characters, zero padded. Never holds a `ghs_`.
#[derive(Clone, PartialEq, Eq)]
pub struct OpaqueToken {
    kind: TokenKind,
    secret: [u8; 30],
    crc: [u8; 6],
}

impl fmt::Debug for OpaqueToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OpaqueToken({}…)", self.kind.prefix())
    }
}

impl OpaqueToken {
    /// The edge's offline check: prefix, length, alphabet, and checksum. No store is read.
    pub fn parse(s: &str) -> Result<OpaqueToken, TokenError> {
        if s.starts_with(TokenKind::Installation.prefix()) {
            return Err(TokenError::Stateless);
        }
        let (kind, rest) = TokenKind::ALL
            .iter()
            .find_map(|k| s.strip_prefix(k.prefix()).map(|r| (*k, r)))
            .ok_or(TokenError::Prefix)?;
        let b = rest.as_bytes();
        if b.len() != 36 || !b.iter().all(u8::is_ascii_alphanumeric) {
            return Err(TokenError::Shape);
        }
        let secret: [u8; 30] = b[..30].try_into().expect("30");
        let crc: [u8; 6] = b[30..].try_into().expect("6");
        if crc_text(&secret) != crc {
            return Err(TokenError::Checksum);
        }
        Ok(OpaqueToken { kind, secret, crc })
    }

    /// A token from a secret `draw_secret` returned. `None` for `Installation`, which is never stored.
    pub fn new(kind: TokenKind, secret: [u8; 30]) -> Option<OpaqueToken> {
        let ok = kind != TokenKind::Installation && secret.iter().all(u8::is_ascii_alphanumeric);
        ok.then(|| OpaqueToken { kind, secret, crc: crc_text(&secret) })
    }

    pub fn kind(&self) -> TokenKind {
        self.kind
    }

    /// What the client is shown once.
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(47);
        s.push_str(self.kind.prefix());
        s.push_str(std::str::from_utf8(&self.secret).expect("base62"));
        s.push_str(std::str::from_utf8(&self.crc).expect("base62"));
        s
    }

    /// SHA-256 of the secret: the only form stored.
    pub fn hash(&self) -> [u8; 32] {
        hash_secret(&self.secret)
    }

    pub fn shard(&self) -> TokenShard {
        token_shard(&self.secret)
    }
}

fn hash_secret(secret: &[u8; 30]) -> [u8; 32] {
    Sha256::new().chain_update(b"graftyard/token/").chain_update(secret).finalize().into()
}

/// The shard of a stored hash, so a leader can check that a `CredentialKey::Token` names its hash's shard.
pub fn shard_of_hash(hash: &[u8; 32]) -> TokenShard {
    let v = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    TokenShard((v % u32::from(SHARDS_PER_FAMILY * FAMILIES)) as u16)
}

/// 1,024 shards per home family, by the secret's hash.
pub fn token_shard(secret: &[u8; 30]) -> TokenShard {
    shard_of_hash(&hash_secret(secret))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no secret landed in {family:?} in {draws} draws")]
pub struct RedrawExhausted {
    pub family: Family,
    pub draws: u32,
}

/// Draws a 30-character base62 secret from `rand`, again and again until its shard is in `family`, about 3 draws
/// on average. Returns the secret and how many draws it took. Characters are drawn by rejection from bytes, so each
/// of the 62 is equally likely.
pub fn draw_secret(mut rand: impl FnMut() -> u64, family: Family) -> Result<([u8; 30], u32), RedrawExhausted> {
    for draw in 1..=MAX_DRAWS {
        let secret = draw_one(&mut rand);
        if Family::of(token_shard(&secret)) == family {
            return Ok((secret, draw));
        }
    }
    Err(RedrawExhausted { family, draws: MAX_DRAWS })
}

/// One 30-character base62 draw, by rejection from bytes, so each of the 62 characters is equally likely.
fn draw_one(rand: &mut impl FnMut() -> u64) -> [u8; 30] {
    let mut secret = [0u8; 30];
    let mut i = 0;
    while i < 30 {
        for b in rand().to_le_bytes() {
            if i < 30 && b < 248 {
                secret[i] = BASE62[usize::from(b % 62)];
                i += 1;
            }
        }
    }
    secret
}

// ---- Routing every kind of credential row ----

/// SHA-256 of an SSH public key's wire blob, as `ssh-keygen -l` prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fingerprint(pub [u8; 32]);

/// A stateless installation token's id (SHARED's `InstallationClaims.jti`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Jti(pub Ulid);

/// An installation-token signing key's epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Kid(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ed25519Public(pub [u8; 32]);

/// The agent layer's grant behind a `ghu_` from `token/scoped` (`design/AGENT-LAYER.md`, "API reference").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GrantId(pub DbId);

/// `design/AGENT-LAYER.md`'s `WriteScope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WriteScope {
    Read,
    Write,
    WriteApproveMerge,
}

/// The repositories a fine-grained PAT reaches [inference: SHARED names `RepoScope` without its shape].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoScope {
    All,
    Selected(SmallVec<[RepoId; 4]>),
}

fn shard_of_digest(tag: &[u8], body: &[u8]) -> TokenShard {
    let h: [u8; 32] = Sha256::new().chain_update(tag).chain_update(body).finalize().into();
    shard_of_hash(&h)
}

/// The uniqueness point of an SSH fingerprint: its shard, over all three families.
pub fn fingerprint_shard(key: &Fingerprint) -> TokenShard {
    shard_of_digest(b"graftyard/fingerprint/", &key.0)
}

/// `DELETE /installation/token` commits the `jti` on the shard of a hash of it.
pub fn jti_shard(jti: &Jti) -> TokenShard {
    shard_of_digest(b"graftyard/jti/", &jti.0.0.to_be_bytes())
}

/// The hash a code row is keyed by: SHA-256 of the code's text under its own tag, never the code. Codes are not
/// tokens, so they hash under another tag than `OpaqueToken::hash`.
pub fn code_hash(code: &[u8]) -> [u8; 32] {
    Sha256::new().chain_update(b"graftyard/code/").chain_update(code).finalize().into()
}

/// Draws a 30-character base62 secret until its hash lands on `shard`, for a `ghr_` beside its `ghu_`: one shard in
/// 3,072, so about 3,072 draws, each one SHA-256. `None` after `max` draws.
pub fn draw_secret_on(mut rand: impl FnMut() -> u64, shard: TokenShard, max: u32) -> Option<([u8; 30], u32)> {
    for draw in 1..=max {
        let secret = draw_one(&mut rand);
        if token_shard(&secret) == shard {
            return Some((secret, draw));
        }
    }
    None
}

/// Draws a device code of `len` base62 characters until its `code_hash` lands on its user code's shard, so both rows
/// commit in one entry. `None` after `max` draws.
pub fn draw_device_code(mut rand: impl FnMut() -> u64, user_code: &[u8], max: u32) -> Option<(Vec<u8>, u32)> {
    let want = shard_of_hash(&code_hash(user_code));
    for draw in 1..=max {
        let s = draw_one(&mut rand).to_vec();
        if shard_of_hash(&code_hash(&s)) == want {
            return Some((s, draw));
        }
    }
    None
}

/// The time-to-live of each code kind. A row lapses at `expires` with no command, as a lease does.
pub const DEVICE_CODE_TTL: Duration = Duration::from_secs(900);
/// [unverified for GHES 3.22]
pub const AUTHORIZATION_CODE_TTL: Duration = Duration::from_secs(600);
/// [guess]
pub const SIGN_IN_LINK_TTL: Duration = Duration::from_secs(900);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeKind {
    /// A device code's row names the hash of its user code, and the user code's row the hash of its device code.
    Device {
        user_code: [u8; 32],
        interval: u8,
    },
    User {
        device_code: [u8; 32],
    },
    Authorization {
        redirect: Url,
        pkce: Option<Digest>,
    },
    SignInLink,
}

impl CodeKind {
    pub fn ttl(&self) -> Duration {
        match self {
            CodeKind::Device { .. } | CodeKind::User { .. } => DEVICE_CODE_TTL,
            CodeKind::Authorization { .. } => AUTHORIZATION_CODE_TTL,
            CodeKind::SignInLink => SIGN_IN_LINK_TTL,
        }
    }
}

// ---- The `Tokens` group ----

/// A stored token's row. A revoked row stays, so its hash is never stored again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRow {
    pub kind: TokenKind,
    pub account: AccountId,
    pub scopes: ScopeSet,
    pub expires: Option<Timestamp>,
    pub revoked: bool,
}

/// The optional `Stored` fields, which belong to OAuth, Apps, and fine-grained PATs. `paired` is the other token's
/// hash on a `ghu_` and its `ghr_`: both rows commit in one entry, and revoking either revokes both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenFields {
    pub sso_orgs: SmallVec<[OrgId; 2]>,
    pub app: Option<AppId>,
    pub client: Option<ClientId>,
    pub permissions: Option<PermissionSet>,
    pub resource_owner: Option<AccountId>,
    pub repos: Option<RepoScope>,
    pub paired: Option<[u8; 32]>,
    pub grant: Option<GrantId>,
    pub write: Option<WriteScope>,
}

impl TokenFields {
    fn is_empty(&self) -> bool {
        *self == TokenFields::default()
    }

    /// Which fields a kind may carry (SHARED, "Identity and tokens"; DESIGN, Shape, "Identity", `Stored`):
    /// `app` on a `ghu_` or `ghr_`, `client` on a `gho_`, `ghu_`, or `ghr_`, `resource_owner` and `repos` on a
    /// `github_pat_`, `permissions` on a `github_pat_` or `ghu_`, `paired` on a `ghu_` or `ghr_`, and `grant` and
    /// `write` only on a `ghu_`. A `github_pat_` names its resource owner.
    fn fits(&self, kind: TokenKind) -> bool {
        use TokenKind::*;
        let user = matches!(kind, UserToServer | Refresh);
        (self.app.is_none() || user)
            && (self.client.is_none() || matches!(kind, OAuth | UserToServer | Refresh))
            && (self.resource_owner.is_some() == (kind == FineGrained))
            && (self.repos.is_none() || kind == FineGrained)
            && (self.permissions.is_none() || matches!(kind, FineGrained | UserToServer))
            && (self.paired.is_none() || user)
            && (self.grant.is_none() && self.write.is_none() || kind == UserToServer)
            && (self.grant.is_some() == self.write.is_some())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshKeyRow {
    pub account: AccountId,
    /// A deploy key's repository.
    pub repo: Option<RepoId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyingKeyRow {
    pub key: Ed25519Public,
    pub retired: Option<Timestamp>,
}

/// A code row. `approved` is the account a user code was approved for; `used` is set once an exchange spent it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRow {
    pub kind: CodeKind,
    pub client: Option<ClientId>,
    pub scopes: ScopeSet,
    pub account: Option<AccountId>,
    pub expires: Timestamp,
    pub approved: Option<AccountId>,
    pub used: bool,
}

/// The design's `TokensEvent`. Serde's variant index is the declaration order, so Milestone 1's `Stored` and
/// `Revoked` stay first and decode from rows stored before Milestone 3.
/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokensEvent {
    /// A stored token with no optional field: Milestone 1's form, encoded as before.
    Stored {
        hash: [u8; 32],
        kind: TokenKind,
        account: AccountId,
        scopes: ScopeSet,
        expires: Option<Timestamp>,
    },
    Revoked {
        hash: [u8; 32],
    },
    /// `Stored` with its optional fields, which Milestone 1's `Stored` lacks. The design has one `Stored`; this is
    /// its encoding with the fields, so a row stored without them keeps its bytes.
    StoredWith {
        hash: [u8; 32],
        kind: TokenKind,
        account: AccountId,
        scopes: ScopeSet,
        expires: Option<Timestamp>,
        fields: Box<TokenFields>,
    },
    SshKeyRemoved {
        fingerprint: Fingerprint,
    },
    SshKey {
        fingerprint: Fingerprint,
        account: AccountId,
        repo: Option<RepoId>,
    },
    JtiRevoked {
        jti: Jti,
        exp: Timestamp,
    },
    LifeboatRead {
        hash: [u8; 32],
        account: AccountId,
    },
    VerifyingKey {
        kid: Kid,
        key: Ed25519Public,
        retired: Option<Timestamp>,
    },
    Code {
        hash: [u8; 32],
        kind: CodeKind,
        client: Option<ClientId>,
        scopes: ScopeSet,
        account: Option<AccountId>,
        expires: Timestamp,
    },
    CodeApproved {
        hash: [u8; 32],
        account: AccountId,
    },
    CodeUsed {
        hash: [u8; 32],
    },
    /// The `Audit` consumer finished, or found nothing owed for, every entry of this shard through `through`
    /// (`crate::audit`). Not in the design's `TokensEvent` [inference: SHARED, "Event fan-out", gives a committed
    /// cursor only to consumers that commit to the chain they read].
    AuditCursor {
        through: Index,
    },
}

fn token_fields(e: &mut Enc, f: &TokenFields) {
    e.count(f.sso_orgs.len());
    for o in &f.sso_orgs {
        enc::account(e, o.0);
    }
    e.option(f.app.as_ref(), |e, a| apps::app_id(e, *a));
    e.option(f.client.as_ref(), apps::client);
    e.option(f.permissions.as_ref(), apps::permissions);
    e.option(f.resource_owner.as_ref(), |e, a| enc::account(e, *a));
    e.option(f.repos.as_ref(), |e, r| match r {
        RepoScope::All => e.u8(0),
        RepoScope::Selected(rs) => {
            e.u8(1);
            e.count(rs.len());
            for r in rs {
                enc::repo(e, *r);
            }
        }
    });
    e.option(f.paired.as_ref(), |e, h| e.0.extend_from_slice(h));
    e.option(f.grant.as_ref(), |e, g| enc::db(e, g.0));
    e.option(f.write.as_ref(), |e, w| e.u8(*w as u8));
}

fn code_kind(e: &mut Enc, k: &CodeKind) {
    match k {
        CodeKind::Device { user_code, interval } => {
            e.u8(0);
            e.0.extend_from_slice(user_code);
            e.u8(*interval);
        }
        CodeKind::User { device_code } => {
            e.u8(1);
            e.0.extend_from_slice(device_code);
        }
        CodeKind::Authorization { redirect, pkce } => {
            e.u8(2);
            apps::url(e, redirect);
            e.option(pkce.as_ref(), |e, d| e.0.extend_from_slice(&d.0));
        }
        CodeKind::SignInLink => e.u8(3),
    }
}

/// Tags 0 and 1 are Milestone 1's and keep their bytes. The rest follow the design's declaration order from 2, with
/// `StoredWith`, the design's `Stored` with fields, after them at 11, and `AuditCursor` at 12. 10 is unused.
impl Record for TokensEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        match self {
            TokensEvent::Stored { hash, kind, account, scopes, expires } => {
                e.u8(0);
                e.0.extend_from_slice(hash);
                e.u8(*kind as u8);
                enc::account(&mut e, *account);
                e.u64(scopes.0);
                e.option(expires.as_ref(), |e, t| enc::at(e, *t));
            }
            TokensEvent::Revoked { hash } => {
                e.u8(1);
                e.0.extend_from_slice(hash);
            }
            TokensEvent::SshKeyRemoved { fingerprint } => {
                e.u8(2);
                e.0.extend_from_slice(&fingerprint.0);
            }
            TokensEvent::SshKey { fingerprint, account, repo } => {
                e.u8(3);
                e.0.extend_from_slice(&fingerprint.0);
                enc::account(&mut e, *account);
                e.option(repo.as_ref(), |e, r| enc::repo(e, *r));
            }
            TokensEvent::JtiRevoked { jti, exp } => {
                e.u8(4);
                e.u128(jti.0.0);
                enc::at(&mut e, *exp);
            }
            TokensEvent::LifeboatRead { hash, account } => {
                e.u8(5);
                e.0.extend_from_slice(hash);
                enc::account(&mut e, *account);
            }
            TokensEvent::VerifyingKey { kid, key, retired } => {
                e.u8(6);
                e.u32(kid.0);
                e.0.extend_from_slice(&key.0);
                e.option(retired.as_ref(), |e, t| enc::at(e, *t));
            }
            TokensEvent::Code { hash, kind, client, scopes, account, expires } => {
                e.u8(7);
                e.0.extend_from_slice(hash);
                code_kind(&mut e, kind);
                e.option(client.as_ref(), apps::client);
                e.u64(scopes.0);
                e.option(account.as_ref(), |e, a| enc::account(e, *a));
                enc::at(&mut e, *expires);
            }
            TokensEvent::CodeApproved { hash, account } => {
                e.u8(8);
                e.0.extend_from_slice(hash);
                enc::account(&mut e, *account);
            }
            TokensEvent::CodeUsed { hash } => {
                e.u8(9);
                e.0.extend_from_slice(hash);
            }
            TokensEvent::AuditCursor { through } => {
                e.u8(12);
                e.u64(through.0);
            }
            TokensEvent::StoredWith { hash, kind, account, scopes, expires, fields } => {
                e.u8(11);
                e.0.extend_from_slice(hash);
                e.u8(*kind as u8);
                enc::account(&mut e, *account);
                e.u64(scopes.0);
                e.option(expires.as_ref(), |e, t| enc::at(e, *t));
                token_fields(&mut e, fields);
            }
        }
    }
}

/// Milestone 3's rows of a `Tokens` shard: the fold's trailing part (`crate::compat`). A `jti` row and a code row are
/// kept until an entry's time passes their `exp` or `expires`; a lapsed row reads as absent at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokensMore {
    pub fields: BTreeMap<[u8; 32], TokenFields>,
    pub ssh_keys: BTreeMap<Fingerprint, SshKeyRow>,
    /// Fingerprints removed: a fingerprint is unique forever on its shard, as a token hash is [inference].
    pub ssh_removed: BTreeSet<Fingerprint>,
    pub jtis: BTreeMap<Jti, Timestamp>,
    pub lifeboat: BTreeMap<[u8; 32], AccountId>,
    pub verifying: BTreeMap<Kid, VerifyingKeyRow>,
    pub codes: BTreeMap<[u8; 32], CodeRow>,
    /// The `Audit` consumer's cursor (`TokensEvent::AuditCursor`), a trailing part of its own, so a fold whose
    /// cursor never moved keeps the bytes of the release before it.
    pub more: Index,
}

tail_serde!(TokensMore {
    fields: BTreeMap<[u8; 32], TokenFields>,
    ssh_keys: BTreeMap<Fingerprint, SshKeyRow>,
    ssh_removed: BTreeSet<Fingerprint>,
    jtis: BTreeMap<Jti, Timestamp>,
    lifeboat: BTreeMap<[u8; 32], AccountId>,
    verifying: BTreeMap<Kid, VerifyingKeyRow>,
    codes: BTreeMap<[u8; 32], CodeRow>,
} more: Index);

/// One `Tokens` shard's fold. Its serde form is Milestone 1's fields, then `more` only when it holds a row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokensFold {
    /// The chain's own shard. `None` checks no key placement.
    pub shard: Option<TokenShard>,
    pub tokens: BTreeMap<[u8; 32], TokenRow>,
    pub more: TokensMore,
}

tail_serde!(TokensFold {
    shard: Option<TokenShard>,
    tokens: BTreeMap<[u8; 32], TokenRow>,
} more: TokensMore);

impl TokensFold {
    pub fn new(shard: TokenShard) -> TokensFold {
        TokensFold { shard: Some(shard), ..TokensFold::default() }
    }

    /// Every entry through this index is one the `Audit` consumer finished or found nothing owed for.
    pub fn audit_cursor(&self) -> Index {
        self.more.more
    }
}

/// Every read a `Tokens` shard serves. A read that takes `now` treats a row whose `exp` or `expires` is not after
/// `now` as absent, so a row lapses with no command.
pub trait TokensRead {
    fn token(&self, hash: &[u8; 32]) -> Option<TokenRow>;
    fn fields(&self, hash: &[u8; 32]) -> Option<&TokenFields>;
    fn ssh_key(&self, fingerprint: &Fingerprint) -> Option<SshKeyRow>;
    fn ssh_removed(&self, fingerprint: &Fingerprint) -> bool;
    /// Whether the `jti` is revoked at `now`; a row past `exp` is gone, since the token itself has expired.
    fn jti_revoked(&self, jti: &Jti, now: Timestamp) -> bool;
    fn lifeboat(&self, hash: &[u8; 32]) -> Option<AccountId>;
    fn verifying_key(&self, kid: Kid) -> Option<VerifyingKeyRow>;
    /// The code's row, unless it lapsed by `now`.
    fn code(&self, hash: &[u8; 32], now: Timestamp) -> Option<&CodeRow>;
}

macro_rules! tokens_read {
    ($t:ty) => {
        impl TokensRead for $t {
            fn token(&self, hash: &[u8; 32]) -> Option<TokenRow> {
                self.tokens.get(hash).copied()
            }
            fn fields(&self, hash: &[u8; 32]) -> Option<&TokenFields> {
                let m = &self.more;
                m.fields.get(hash)
            }
            fn ssh_key(&self, fingerprint: &Fingerprint) -> Option<SshKeyRow> {
                let m = &self.more;
                m.ssh_keys.get(fingerprint).copied()
            }
            fn ssh_removed(&self, fingerprint: &Fingerprint) -> bool {
                let m = &self.more;
                m.ssh_removed.contains(fingerprint)
            }
            fn jti_revoked(&self, jti: &Jti, now: Timestamp) -> bool {
                let m = &self.more;
                m.jtis.get(jti).is_some_and(|exp| *exp > now)
            }
            fn lifeboat(&self, hash: &[u8; 32]) -> Option<AccountId> {
                let m = &self.more;
                m.lifeboat.get(hash).copied()
            }
            fn verifying_key(&self, kid: Kid) -> Option<VerifyingKeyRow> {
                let m = &self.more;
                m.verifying.get(&kid).copied()
            }
            fn code(&self, hash: &[u8; 32], now: Timestamp) -> Option<&CodeRow> {
                let m = &self.more;
                m.codes.get(hash).filter(|c| c.expires > now)
            }
        }
    };
}

tokens_read!(TokensFold);

/// The tables of `TokensMore`, as the fold's maps or the scratch view's overlays.
trait MoreTables {
    fn fields(&mut self) -> &mut dyn Table<[u8; 32], TokenFields>;
    fn ssh_keys(&mut self) -> &mut dyn Table<Fingerprint, SshKeyRow>;
    fn ssh_removed(&mut self) -> &mut dyn Table<Fingerprint, ()>;
    fn jtis(&mut self) -> &mut dyn Table<Jti, Timestamp>;
    fn lifeboat(&mut self) -> &mut dyn Table<[u8; 32], AccountId>;
    fn verifying(&mut self) -> &mut dyn Table<Kid, VerifyingKeyRow>;
    fn codes(&mut self) -> &mut dyn Table<[u8; 32], CodeRow>;
}

macro_rules! more_tables {
    ($t:ty) => {
        impl MoreTables for $t {
            fn fields(&mut self) -> &mut dyn Table<[u8; 32], TokenFields> {
                &mut self.fields
            }
            fn ssh_keys(&mut self) -> &mut dyn Table<Fingerprint, SshKeyRow> {
                &mut self.ssh_keys
            }
            fn ssh_removed(&mut self) -> &mut dyn Table<Fingerprint, ()> {
                &mut self.ssh_removed
            }
            fn jtis(&mut self) -> &mut dyn Table<Jti, Timestamp> {
                &mut self.jtis
            }
            fn lifeboat(&mut self) -> &mut dyn Table<[u8; 32], AccountId> {
                &mut self.lifeboat
            }
            fn verifying(&mut self) -> &mut dyn Table<Kid, VerifyingKeyRow> {
                &mut self.verifying
            }
            fn codes(&mut self) -> &mut dyn Table<[u8; 32], CodeRow> {
                &mut self.codes
            }
        }
    };
}

more_tables!(TokensMore);
more_tables!(MoreOverlay<'_>);

fn revoke(t: &mut impl Table<[u8; 32], TokenRow>, hash: &[u8; 32]) {
    if let Some(r) = t.get(hash).copied() {
        t.put(*hash, TokenRow { revoked: true, ..r });
    }
}

/// The writes of one event, shared by `TokensFold::apply` and `TokensScratch::apply` so the two never diverge.
fn write(ev: &TokensEvent, t: &mut impl Table<[u8; 32], TokenRow>, m: &mut impl MoreTables) {
    match ev {
        TokensEvent::Stored { hash, kind, account, scopes, expires } => t.put(
            *hash,
            TokenRow { kind: *kind, account: *account, scopes: *scopes, expires: *expires, revoked: false },
        ),
        TokensEvent::StoredWith { hash, kind, account, scopes, expires, fields } => {
            t.put(
                *hash,
                TokenRow { kind: *kind, account: *account, scopes: *scopes, expires: *expires, revoked: false },
            );
            m.fields().put(*hash, (**fields).clone());
        }
        TokensEvent::Revoked { hash } => {
            revoke(t, hash);
            // A paired `ghu_` and `ghr_` share one shard, so the other row is revoked in the same apply.
            if let Some(other) = m.fields().get(hash).and_then(|f| f.paired) {
                revoke(t, &other);
            }
        }
        TokensEvent::SshKey { fingerprint, account, repo } => {
            m.ssh_keys().put(*fingerprint, SshKeyRow { account: *account, repo: *repo })
        }
        TokensEvent::SshKeyRemoved { fingerprint } => {
            m.ssh_keys().del(fingerprint);
            m.ssh_removed().put(*fingerprint, ());
        }
        TokensEvent::JtiRevoked { jti, exp } => m.jtis().put(*jti, *exp),
        TokensEvent::LifeboatRead { hash, account } => m.lifeboat().put(*hash, *account),
        TokensEvent::VerifyingKey { kid, key, retired } => {
            m.verifying().put(*kid, VerifyingKeyRow { key: *key, retired: *retired })
        }
        TokensEvent::Code { hash, kind, client, scopes, account, expires } => m.codes().put(
            *hash,
            CodeRow {
                kind: kind.clone(),
                client: *client,
                scopes: *scopes,
                account: *account,
                expires: *expires,
                approved: None,
                used: false,
            },
        ),
        TokensEvent::CodeApproved { hash, account } => {
            if let Some(c) = m.codes().get(hash).cloned() {
                m.codes().put(*hash, CodeRow { approved: Some(*account), ..c });
            }
        }
        TokensEvent::CodeUsed { hash } => {
            if let Some(c) = m.codes().get(hash).cloned() {
                m.codes().put(*hash, CodeRow { used: true, ..c });
            }
        }
        // The fold's own field (`TokensFold::apply`); `decide` never reads it from a scratch view.
        TokensEvent::AuditCursor { .. } => {}
    }
}

struct MoreOverlay<'a> {
    fields: Overlay<'a, [u8; 32], TokenFields>,
    ssh_keys: Overlay<'a, Fingerprint, SshKeyRow>,
    ssh_removed: SetOverlay<'a, Fingerprint>,
    jtis: Overlay<'a, Jti, Timestamp>,
    lifeboat: Overlay<'a, [u8; 32], AccountId>,
    verifying: Overlay<'a, Kid, VerifyingKeyRow>,
    codes: Overlay<'a, [u8; 32], CodeRow>,
}

pub struct TokensScratch<'a> {
    fold: &'a TokensFold,
    /// The entry's time: what `decide` and `verify` compare a code's `expires` and a `jti`'s `exp` with.
    at: Timestamp,
    tokens: Overlay<'a, [u8; 32], TokenRow>,
    more: MoreOverlay<'a>,
}

impl TokensScratch<'_> {
    pub fn at(&self) -> Timestamp {
        self.at
    }
}

tokens_read!(TokensScratch<'_>);

/// The shard a key must live on, so a leader or follower refuses a row on another shard.
fn shard_of_event(ev: &TokensEvent) -> Option<TokenShard> {
    match ev {
        TokensEvent::Stored { hash, .. }
        | TokensEvent::StoredWith { hash, .. }
        | TokensEvent::LifeboatRead { hash, .. }
        | TokensEvent::Code { hash, .. } => Some(shard_of_hash(hash)),
        TokensEvent::SshKey { fingerprint, .. } | TokensEvent::SshKeyRemoved { fingerprint } => {
            Some(fingerprint_shard(fingerprint))
        }
        TokensEvent::JtiRevoked { jti, .. } => Some(jti_shard(jti)),
        TokensEvent::VerifyingKey { .. } => Some(TokenShard(0)),
        TokensEvent::Revoked { .. }
        | TokensEvent::CodeApproved { .. }
        | TokensEvent::CodeUsed { .. }
        | TokensEvent::AuditCursor { .. } => None,
    }
}

impl Scratch for TokensScratch<'_> {
    type Event = TokensEvent;
    fn apply(&mut self, ev: &TokensEvent) {
        write(ev, &mut self.tokens, &mut self.more);
    }
    /// A hash or fingerprint is stored once, ever, and only on its own shard; an SSH fingerprint has one owner; a
    /// code is used once, only before it lapses, and a user code is approved once.
    fn check(&self, ev: &TokensEvent) -> Result<(), Box<str>> {
        if let (Some(want), Some(have)) = (shard_of_event(ev), self.fold.shard)
            && want != have
        {
            return Err(format!("row of shard {want:?} on shard {have:?}").into());
        }
        let now = self.at;
        match ev {
            TokensEvent::Stored { hash, .. } | TokensEvent::StoredWith { hash, .. } if self.token(hash).is_some() => {
                Err("hash stored twice".into())
            }
            TokensEvent::SshKey { fingerprint, account, .. } => match self.ssh_key(fingerprint) {
                Some(r) if r.account != *account => Err(format!("{fingerprint:?} belongs to {:?}", r.account).into()),
                Some(_) => Err("fingerprint added twice".into()),
                None if self.ssh_removed(fingerprint) => Err("fingerprint removed before".into()),
                None => Ok(()),
            },
            TokensEvent::Code { hash, .. } if self.more.codes.get(hash).is_some() => Err("code stored twice".into()),
            TokensEvent::CodeApproved { hash, .. } => match self.code(hash, now) {
                Some(c) if c.approved.is_none() && !c.used => Ok(()),
                _ => Err("approval of a missing, lapsed, approved, or used code".into()),
            },
            TokensEvent::CodeUsed { hash } => match self.code(hash, now) {
                Some(c) if !c.used => Ok(()),
                _ => Err("use of a missing, lapsed, or used code".into()),
            },
            _ => Ok(()),
        }
    }
}

impl Fold for TokensFold {
    type Event = TokensEvent;
    type Scratch<'a> = TokensScratch<'a>;
    /// A scratch with no entry time sees every code as lapsed; the leader and `verify` use `scratch_at`.
    fn scratch(&self) -> TokensScratch<'_> {
        self.scratch_at(Timestamp(0), TierSet::CORE)
    }
    fn scratch_at(&self, at: Timestamp, _tiers: TierSet) -> TokensScratch<'_> {
        let m = &self.more;
        TokensScratch {
            fold: self,
            at,
            tokens: Overlay::new(&self.tokens),
            more: MoreOverlay {
                fields: Overlay::new(&m.fields),
                ssh_keys: Overlay::new(&m.ssh_keys),
                ssh_removed: SetOverlay::new(&m.ssh_removed),
                jtis: Overlay::new(&m.jtis),
                lifeboat: Overlay::new(&m.lifeboat),
                verifying: Overlay::new(&m.verifying),
                codes: Overlay::new(&m.codes),
            },
        }
    }
    fn apply(&mut self, ev: &TokensEvent) {
        write(ev, &mut self.tokens, &mut self.more);
        // Never back: two cursor commands in one entry leave the later of the two.
        if let TokensEvent::AuditCursor { through } = ev {
            self.more.more = self.more.more.max(*through);
        }
    }
    /// Applies the entry, then drops every `jti` and code row that lapsed by the entry's time, so every member
    /// prunes the same rows. A lapsed row already read as absent; pruning only bounds the fold.
    fn apply_entry(&mut self, e: &graftyard_raft::LogEntry<TokensEvent>) {
        if let graftyard_raft::EntryBody::Events(evs) = &e.body {
            for ev in evs {
                self.apply(ev);
            }
        }
        let now = e.at;
        self.more.jtis.retain(|_, exp| *exp > now);
        self.more.codes.retain(|_, c| c.expires > now);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokensCmd {
    pub cred: CredentialShard,
    pub op: TokensOp,
}

/// Wire tags: postcard stores and sends the variants by position, JSON by name (`crates/git/tests/wire_tags.golden`).
/// Add a variant only at the end: never insert, remove, reorder, or rename one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokensOp {
    /// Store a new token's hash. The commit point of a new PAT or OAuth token.
    Store {
        hash: [u8; 32],
        kind: TokenKind,
        account: AccountId,
        scopes: ScopeSet,
        expires: Option<Timestamp>,
    },
    Revoke {
        hash: [u8; 32],
    },
    // ---- Milestone 3, format 2 ----
    /// A stored token with the optional fields its kind may carry: a `github_pat_`, a `gho_`, or a `ghu_` with no
    /// refresh token.
    StoreWith {
        hash: [u8; 32],
        kind: TokenKind,
        account: AccountId,
        scopes: ScopeSet,
        expires: Option<Timestamp>,
        fields: TokenFields,
    },
    /// A `ghu_` and its `ghr_`, in one entry: the `ghr_` secret was redrawn onto the `ghu_`'s shard
    /// (`draw_secret_on`). Each row's `paired` names the other.
    StorePair {
        user: [u8; 32],
        refresh: [u8; 32],
        account: AccountId,
        scopes: ScopeSet,
        user_expires: Option<Timestamp>,
        refresh_expires: Option<Timestamp>,
        fields: TokenFields,
    },
    /// An SSH key or deploy key: the commit point, and where a fingerprint is unique.
    AddSshKey {
        fingerprint: Fingerprint,
        account: AccountId,
        repo: Option<RepoId>,
    },
    RemoveSshKey {
        fingerprint: Fingerprint,
    },
    /// `DELETE /installation/token`: the stateless token's `jti` until its `exp`. The edge submits it with no account
    /// once it has verified the token it revokes.
    RevokeJti {
        jti: Jti,
        exp: Timestamp,
    },
    /// An installation-token verifying key on `TokenShard(0)`. Only `graft-ops` submits it; a key may later only
    /// gain its `retired` time.
    PutVerifyingKey {
        kid: Kid,
        key: Ed25519Public,
        retired: Option<Timestamp>,
    },
    /// A `lifeboat:read` token's hash. Only `graftyard lifeboat enroll`, through the operator, submits it.
    StoreLifeboat {
        hash: [u8; 32],
        account: AccountId,
    },
    /// An authorization code or a sign-in link. The leader sets `expires` from the entry's time and the kind's TTL.
    PutCode {
        hash: [u8; 32],
        kind: CodeKind,
        client: Option<ClientId>,
        scopes: ScopeSet,
        account: Option<AccountId>,
    },
    /// A device flow's device code and user code, in one entry: the device code was redrawn onto the user code's
    /// shard (`draw_device_code`).
    PutDeviceCode {
        device: [u8; 32],
        user: [u8; 32],
        client: ClientId,
        scopes: ScopeSet,
        interval: u8,
    },
    /// The signed-in user approves a user code.
    ApproveCode {
        user: [u8; 32],
        account: AccountId,
    },
    /// An exchange's first commit: the code is spent, once. The edge then stores the new token for the account and
    /// scopes of the row it read, which stays readable until it lapses, and a failed second commit sends the client
    /// back to the start.
    UseCode {
        hash: [u8; 32],
        client: Option<ClientId>,
    },
    // ---- format 3 ----
    /// The `Audit` consumer's position on `shard`, which only the consumer, as the operator, submits. A cursor at or
    /// behind the shard's decides to no events.
    AuditCursor {
        shard: TokenShard,
        through: Index,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokensErr {
    Unauthorized(CredentialRefusal),
    /// The caller may not act for this account.
    Forbidden,
    WrongShard {
        want: TokenShard,
    },
    /// The hash is already stored, or was and is revoked: a secret is never reused.
    Exists,
    /// `Installation` tokens are stateless and never stored.
    NotStored,
    NoToken,
    AlreadyRevoked,
    /// The optional fields do not fit the token's kind.
    BadFields,
    /// The SSH fingerprint belongs to this account already, or to another.
    KeyTaken {
        by: AccountId,
    },
    NoKey,
    /// The verifying key differs from the one `kid` already names.
    KeyChanged,
    /// The code is missing or lapsed: `expired_token`.
    NoCode,
    /// The code was spent: a second exchange gets nothing.
    CodeUsed,
    /// The device code's user code is not approved yet: `authorization_pending`.
    Pending,
    /// The code belongs to another client, or is the wrong kind for the op.
    WrongCode,
    AlreadyApproved,
}

fn store_cmd(
    e: &mut Enc,
    hash: &[u8; 32],
    kind: TokenKind,
    account: AccountId,
    scopes: ScopeSet,
    x: Option<Timestamp>,
) {
    e.0.extend_from_slice(hash);
    e.u8(kind as u8);
    enc::account(e, account);
    e.u64(scopes.0);
    e.option(x.as_ref(), |e, t| enc::at(e, *t));
}

impl Record for TokensCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        encode_cred(&mut e, &self.cred);
        match &self.op {
            TokensOp::Store { hash, kind, account, scopes, expires } => {
                e.u8(0);
                store_cmd(&mut e, hash, *kind, *account, *scopes, *expires);
            }
            TokensOp::Revoke { hash } => {
                e.u8(1);
                e.0.extend_from_slice(hash);
            }
            TokensOp::StoreWith { hash, kind, account, scopes, expires, fields } => {
                e.u8(2);
                store_cmd(&mut e, hash, *kind, *account, *scopes, *expires);
                token_fields(&mut e, fields);
            }
            TokensOp::StorePair { user, refresh, account, scopes, user_expires, refresh_expires, fields } => {
                e.u8(3);
                e.0.extend_from_slice(user);
                e.0.extend_from_slice(refresh);
                enc::account(&mut e, *account);
                e.u64(scopes.0);
                e.option(user_expires.as_ref(), |e, t| enc::at(e, *t));
                e.option(refresh_expires.as_ref(), |e, t| enc::at(e, *t));
                token_fields(&mut e, fields);
            }
            TokensOp::AddSshKey { fingerprint, account, repo } => {
                e.u8(4);
                e.0.extend_from_slice(&fingerprint.0);
                enc::account(&mut e, *account);
                e.option(repo.as_ref(), |e, r| enc::repo(e, *r));
            }
            TokensOp::RemoveSshKey { fingerprint } => {
                e.u8(5);
                e.0.extend_from_slice(&fingerprint.0);
            }
            TokensOp::RevokeJti { jti, exp } => {
                e.u8(6);
                e.u128(jti.0.0);
                enc::at(&mut e, *exp);
            }
            TokensOp::PutVerifyingKey { kid, key, retired } => {
                e.u8(7);
                e.u32(kid.0);
                e.0.extend_from_slice(&key.0);
                e.option(retired.as_ref(), |e, t| enc::at(e, *t));
            }
            TokensOp::StoreLifeboat { hash, account } => {
                e.u8(8);
                e.0.extend_from_slice(hash);
                enc::account(&mut e, *account);
            }
            TokensOp::PutCode { hash, kind, client, scopes, account } => {
                e.u8(9);
                e.0.extend_from_slice(hash);
                code_kind(&mut e, kind);
                e.option(client.as_ref(), apps::client);
                e.u64(scopes.0);
                e.option(account.as_ref(), |e, a| enc::account(e, *a));
            }
            TokensOp::PutDeviceCode { device, user, client, scopes, interval } => {
                e.u8(10);
                e.0.extend_from_slice(device);
                e.0.extend_from_slice(user);
                apps::client(&mut e, client);
                e.u64(scopes.0);
                e.u8(*interval);
            }
            TokensOp::ApproveCode { user, account } => {
                e.u8(11);
                e.0.extend_from_slice(user);
                enc::account(&mut e, *account);
            }
            TokensOp::UseCode { hash, client } => {
                e.u8(12);
                e.0.extend_from_slice(hash);
                e.option(client.as_ref(), apps::client);
            }
            TokensOp::AuditCursor { shard, through } => {
                e.u8(13);
                e.u16(shard.0);
                e.u64(through.0);
            }
        }
    }
}

impl TokensCmd {
    /// The shard this command's key lives on: a stored hash's, a fingerprint's, a `jti`'s, `TokenShard(0)` for a
    /// verifying key, and a code hash's.
    pub fn shard(&self) -> TokenShard {
        match &self.op {
            TokensOp::Store { hash, .. }
            | TokensOp::Revoke { hash }
            | TokensOp::StoreWith { hash, .. }
            | TokensOp::StorePair { user: hash, .. }
            | TokensOp::StoreLifeboat { hash, .. }
            | TokensOp::PutCode { hash, .. }
            | TokensOp::PutDeviceCode { user: hash, .. }
            | TokensOp::ApproveCode { user: hash, .. }
            | TokensOp::UseCode { hash, .. } => shard_of_hash(hash),
            TokensOp::AddSshKey { fingerprint, .. } | TokensOp::RemoveSshKey { fingerprint } => {
                fingerprint_shard(fingerprint)
            }
            TokensOp::RevokeJti { jti, .. } => jti_shard(jti),
            TokensOp::PutVerifyingKey { .. } => TokenShard(0),
            TokensOp::AuditCursor { shard, .. } => *shard,
        }
    }

    /// A user acts only for itself; the operator (`graft-ops`, the CLI) acts for any; an installation token for none.
    fn acts_for(&self, account: AccountId) -> bool {
        match self.cred.account {
            Some(a) => a == account,
            None => self.cred.is_operator(),
        }
    }

    fn operator(&self) -> bool {
        self.cred.is_operator()
    }

    fn decide_op(&self, s: &TokensScratch<'_>) -> Result<(SmallVec<[TokensEvent; 4]>, ()), TokensErr> {
        let done = |evs: SmallVec<[TokensEvent; 4]>| Ok((evs, ()));
        let now = s.at;
        let fresh = |hash: &[u8; 32]| {
            if s.token(hash).is_some() { Err(TokensErr::Exists) } else { Ok(()) }
        };
        match &self.op {
            TokensOp::Store { hash, kind, account, scopes, expires } => {
                if !self.acts_for(*account) {
                    return Err(TokensErr::Forbidden);
                }
                if *kind == TokenKind::Installation {
                    return Err(TokensErr::NotStored);
                }
                fresh(hash)?;
                let ev = TokensEvent::Stored {
                    hash: *hash,
                    kind: *kind,
                    account: *account,
                    scopes: *scopes,
                    expires: *expires,
                };
                done(smallvec![ev])
            }
            TokensOp::Revoke { hash } => {
                let row = s.token(hash).ok_or(TokensErr::NoToken)?;
                if !self.acts_for(row.account) {
                    return Err(TokensErr::Forbidden);
                }
                if row.revoked {
                    return Err(TokensErr::AlreadyRevoked);
                }
                done(smallvec![TokensEvent::Revoked { hash: *hash }])
            }
            TokensOp::StoreWith { hash, kind, account, scopes, expires, fields } => {
                if !self.acts_for(*account) {
                    return Err(TokensErr::Forbidden);
                }
                if *kind == TokenKind::Installation {
                    return Err(TokensErr::NotStored);
                }
                // A paired row comes only from `StorePair`, and a `ghr_` never stands alone.
                if !fields.fits(*kind) || fields.paired.is_some() || *kind == TokenKind::Refresh {
                    return Err(TokensErr::BadFields);
                }
                fresh(hash)?;
                done(smallvec![stored(*hash, *kind, *account, *scopes, *expires, fields.clone())])
            }
            TokensOp::StorePair { user, refresh, account, scopes, user_expires, refresh_expires, fields } => {
                if !self.acts_for(*account) {
                    return Err(TokensErr::Forbidden);
                }
                let want = shard_of_hash(user);
                if shard_of_hash(refresh) != want {
                    return Err(TokensErr::WrongShard { want });
                }
                if user == refresh || fields.paired.is_some() || !fields.fits(TokenKind::UserToServer) {
                    return Err(TokensErr::BadFields);
                }
                fresh(user)?;
                fresh(refresh)?;
                // The `ghr_` carries the App and client, never the `ghu_`'s permissions, grant, or write scope.
                let r = TokenFields {
                    sso_orgs: fields.sso_orgs.clone(),
                    app: fields.app,
                    client: fields.client,
                    paired: Some(*user),
                    ..TokenFields::default()
                };
                let u = TokenFields { paired: Some(*refresh), ..fields.clone() };
                done(smallvec![
                    stored(*user, TokenKind::UserToServer, *account, *scopes, *user_expires, u),
                    stored(*refresh, TokenKind::Refresh, *account, *scopes, *refresh_expires, r),
                ])
            }
            TokensOp::AddSshKey { fingerprint, account, repo } => {
                if !self.acts_for(*account) {
                    return Err(TokensErr::Forbidden);
                }
                if let Some(r) = s.ssh_key(fingerprint) {
                    return Err(TokensErr::KeyTaken { by: r.account });
                }
                done(smallvec![TokensEvent::SshKey { fingerprint: *fingerprint, account: *account, repo: *repo }])
            }
            TokensOp::RemoveSshKey { fingerprint } => {
                let r = s.ssh_key(fingerprint).ok_or(TokensErr::NoKey)?;
                if !self.acts_for(r.account) {
                    return Err(TokensErr::Forbidden);
                }
                done(smallvec![TokensEvent::SshKeyRemoved { fingerprint: *fingerprint }])
            }
            TokensOp::RevokeJti { jti, exp } => {
                // `DELETE /installation/token` is sent under the token itself; `graft-ops` may revoke any.
                let own = self.cred.key == Some(CredentialKey::Jti { shard: jti_shard(jti), jti: jti.0 });
                if !self.operator() && !own {
                    return Err(TokensErr::Forbidden);
                }
                // A revoked or already expired token needs no row; `DELETE` answers 204 either way.
                if s.jti_revoked(jti, now) || *exp <= now {
                    return done(SmallVec::new());
                }
                done(smallvec![TokensEvent::JtiRevoked { jti: *jti, exp: *exp }])
            }
            TokensOp::PutVerifyingKey { kid, key, retired } => {
                if !self.operator() {
                    return Err(TokensErr::Forbidden);
                }
                if s.verifying_key(*kid).is_some_and(|r| r.key != *key) {
                    return Err(TokensErr::KeyChanged);
                }
                done(smallvec![TokensEvent::VerifyingKey { kid: *kid, key: *key, retired: *retired }])
            }
            TokensOp::StoreLifeboat { hash, account } => {
                if !self.operator() {
                    return Err(TokensErr::Forbidden);
                }
                if s.lifeboat(hash).is_some() || s.token(hash).is_some() {
                    return Err(TokensErr::Exists);
                }
                done(smallvec![TokensEvent::LifeboatRead { hash: *hash, account: *account }])
            }
            TokensOp::PutCode { hash, kind, client, scopes, account } => {
                // A device flow's pair comes only from `PutDeviceCode`; a sign-in link names its account.
                let ok = match kind {
                    CodeKind::Authorization { .. } => client.is_some() && account.is_some(),
                    CodeKind::SignInLink => client.is_none() && account.is_some(),
                    CodeKind::Device { .. } | CodeKind::User { .. } => false,
                };
                if !ok {
                    return Err(TokensErr::WrongCode);
                }
                if account.is_some_and(|a| !self.acts_for(a))
                    || matches!(kind, CodeKind::SignInLink) && !self.operator()
                {
                    return Err(TokensErr::Forbidden);
                }
                if s.more.codes.get(hash).is_some() {
                    return Err(TokensErr::Exists);
                }
                let expires = later(now, kind.ttl());
                done(smallvec![TokensEvent::Code {
                    hash: *hash,
                    kind: kind.clone(),
                    client: *client,
                    scopes: *scopes,
                    account: *account,
                    expires,
                }])
            }
            TokensOp::PutDeviceCode { device, user, client, scopes, interval } => {
                let want = shard_of_hash(user);
                if shard_of_hash(device) != want {
                    return Err(TokensErr::WrongShard { want });
                }
                if device == user || s.more.codes.get(device).is_some() || s.more.codes.get(user).is_some() {
                    return Err(TokensErr::Exists);
                }
                let expires = later(now, DEVICE_CODE_TTL);
                let code = |hash: [u8; 32], kind| TokensEvent::Code {
                    hash,
                    kind,
                    client: Some(*client),
                    scopes: *scopes,
                    account: None,
                    expires,
                };
                done(smallvec![
                    code(*device, CodeKind::Device { user_code: *user, interval: *interval }),
                    code(*user, CodeKind::User { device_code: *device }),
                ])
            }
            TokensOp::ApproveCode { user, account } => {
                if !self.acts_for(*account) {
                    return Err(TokensErr::Forbidden);
                }
                let c = s.code(user, now).ok_or(TokensErr::NoCode)?;
                if !matches!(c.kind, CodeKind::User { .. }) {
                    return Err(TokensErr::WrongCode);
                }
                if c.used {
                    return Err(TokensErr::CodeUsed);
                }
                if c.approved.is_some() {
                    return Err(TokensErr::AlreadyApproved);
                }
                done(smallvec![TokensEvent::CodeApproved { hash: *user, account: *account }])
            }
            TokensOp::UseCode { hash, client } => {
                let c = s.code(hash, now).ok_or(TokensErr::NoCode)?;
                if c.client != *client {
                    return Err(TokensErr::WrongCode);
                }
                if c.used {
                    return Err(TokensErr::CodeUsed);
                }
                match &c.kind {
                    CodeKind::User { .. } => return Err(TokensErr::WrongCode),
                    CodeKind::Device { user_code, .. } => {
                        if s.code(user_code, now).and_then(|u| u.approved).is_none() {
                            return Err(TokensErr::Pending);
                        }
                    }
                    CodeKind::Authorization { .. } | CodeKind::SignInLink => {}
                }
                done(smallvec![TokensEvent::CodeUsed { hash: *hash }])
            }
            TokensOp::AuditCursor { through, .. } => {
                if !self.operator() {
                    return Err(TokensErr::Forbidden);
                }
                if *through <= s.fold.audit_cursor() {
                    return done(SmallVec::new());
                }
                done(smallvec![TokensEvent::AuditCursor { through: *through }])
            }
        }
    }
}

fn later(t: Timestamp, d: Duration) -> Timestamp {
    Timestamp(t.0.saturating_add(d.as_millis() as u64))
}

/// Milestone 1's `Stored` when the fields are empty, so such a row keeps its bytes; `StoredWith` otherwise.
fn stored(
    hash: [u8; 32],
    kind: TokenKind,
    account: AccountId,
    scopes: ScopeSet,
    expires: Option<Timestamp>,
    fields: TokenFields,
) -> TokensEvent {
    if fields.is_empty() {
        TokensEvent::Stored { hash, kind, account, scopes, expires }
    } else {
        TokensEvent::StoredWith { hash, kind, account, scopes, expires, fields: Box::new(fields) }
    }
}

impl Command for TokensCmd {
    type Fold = TokensFold;
    type Dep = IdentityDep;
    type Out = ();
    type Err = TokensErr;

    /// Milestone 1's ops keep format 1, so their entries are what an older release reads; the rest are format 2.
    fn format(&self) -> Format {
        match self.op {
            TokensOp::Store { .. } | TokensOp::Revoke { .. } => TOKENS_FORMAT,
            TokensOp::AuditCursor { .. } => TOKENS_FORMAT_V3,
            _ => TOKENS_FORMAT_V2,
        }
    }
    fn tiers(&self) -> TierSet {
        TierSet::CORE
    }
    fn deps(&self, fold: &TokensFold) -> SmallVec<[ChainId; 3]> {
        credential_chains(&self.cred, fold.shard.map(ChainId::Tokens))
    }
    fn decide(
        &self,
        s: &TokensScratch<'_>,
        deps: &[(ChainId, IdentityDep)],
    ) -> Result<(SmallVec<[Ev<Self>; 4]>, ()), TokensErr> {
        let own = match s.fold.shard {
            Some(shard) => Own::Tokens(shard, s),
            None => Own::Other,
        };
        check_credential(&self.cred, own, deps).map_err(TokensErr::Unauthorized)?;
        let want = self.shard();
        if s.fold.shard.is_some_and(|have| have != want) {
            return Err(TokensErr::WrongShard { want });
        }
        self.decide_op(s)
    }
    fn replay(&self, _events: &[TokensEvent]) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_githubs_published_example_and_refuses_neighbours() {
        // From GitHub's 2021 format post, decoded by a public Kaitai spec: CRC32 9b121895 is "2q4HA5" in base62.
        let t = "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";
        let p = OpaqueToken::parse(t).unwrap();
        assert_eq!(p.kind(), TokenKind::Pat);
        assert_eq!(p.text(), t);
        assert_eq!(crc32(b"IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji4"), 0x9b12_1895);
        for (bad, why) in [
            ("ghp_IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji42q4HA6", TokenError::Checksum),
            ("ghp_JqIMNOZH6zOwIEB4T9A2g4EHMy8Ji42q4HA5", TokenError::Checksum),
            ("ghp_IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji42q4HA", TokenError::Shape),
            ("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x", TokenError::Shape),
            ("ghp_IqIMNOZH6zOwIEB4T9A2g4EHMy8J-42q4HA5", TokenError::Shape),
            ("ghx_IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji42q4HA5", TokenError::Prefix),
            ("GHP_IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji42q4HA5", TokenError::Prefix),
            ("ghs_eyJhbGciOi.eyJpc3Mi.c2ln", TokenError::Stateless),
        ] {
            assert_eq!(OpaqueToken::parse(bad), Err(why), "{bad}");
        }
        // Every stored prefix round-trips, with the same secret's checksum.
        for k in TokenKind::ALL.into_iter().filter(|k| *k != TokenKind::Installation) {
            let o = OpaqueToken::new(k, *b"IqIMNOZH6zOwIEB4T9A2g4EHMy8Ji4").unwrap();
            assert!(o.text().ends_with("2q4HA5"));
            assert_eq!(OpaqueToken::parse(&o.text()), Ok(o.clone()));
        }
        assert!(OpaqueToken::new(TokenKind::Installation, [b'a'; 30]).is_none());
    }

    #[test]
    fn redraw_lands_in_the_family_in_about_three_draws() {
        let mut x = 0x1234_5678_9abc_def0u64;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for family in [Family::Na, Family::Eu, Family::Apac] {
            let mut draws = 0u64;
            let mut shards = vec![0u32; SHARDS_PER_FAMILY as usize];
            let n = 6_000;
            for _ in 0..n {
                let (secret, d) = draw_secret(&mut rand, family).unwrap();
                let shard = token_shard(&secret);
                assert_eq!(Family::of(shard), family);
                assert_eq!(OpaqueToken::new(TokenKind::Pat, secret).unwrap().shard(), shard);
                shards[usize::from(shard.0 % SHARDS_PER_FAMILY)] += 1;
                draws += u64::from(d);
            }
            let mean = draws as f64 / f64::from(n);
            assert!((2.8..3.2).contains(&mean), "{family:?}: {mean} draws on average");
            // Uniform over the family's 1,024 shards: chi-square with 1,023 degrees of freedom has mean 1,023 and
            // standard deviation about 45, so 1,250 is five deviations out.
            let expect = f64::from(n) / f64::from(SHARDS_PER_FAMILY);
            let chi2: f64 = shards.iter().map(|&c| (f64::from(c) - expect).powi(2) / expect).sum();
            assert!(chi2 < 1_250.0, "{family:?}: chi-square {chi2}");
        }
        // Base62 characters come out evenly.
        let mut counts = [0u32; 62];
        for _ in 0..2_000 {
            let (s, _) = draw_secret(&mut rand, Family::Na).unwrap();
            for c in s {
                counts[BASE62.iter().position(|b| *b == c).unwrap()] += 1;
            }
        }
        assert!(counts.iter().all(|&c| (800..1_150).contains(&c)), "{counts:?}");
    }

    #[test]
    fn a_source_that_never_lands_is_reported() {
        // A constant source draws the same secret forever; pick a family it is not in.
        let (s, _) = draw_secret(|| 0, Family::Na)
            .or_else(|_| draw_secret(|| 0, Family::Eu))
            .or_else(|_| draw_secret(|| 0, Family::Apac))
            .unwrap();
        let home = Family::of(token_shard(&s));
        let other = [Family::Na, Family::Eu, Family::Apac].into_iter().find(|f| *f != home).unwrap();
        assert_eq!(draw_secret(|| 0, other), Err(RedrawExhausted { family: other, draws: MAX_DRAWS }));
    }
}
