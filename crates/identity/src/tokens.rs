//! Stored-token authentication (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Identity and tokens").
//!
//! A stored token is a prefix, 30 base62 secret characters, and the CRC32 of those 30 characters in 6 base62
//! characters. The edge checks the prefix and CRC offline, hashes the secret, and finds the row on
//! `token_shard(secret)`. A new token's secret is redrawn until its shard lands in the owner's home family.

use std::collections::BTreeMap;
use std::fmt;

use graftyard_raft::{Command, Enc, Ev, Fold, Format, Record, Scratch, TierSet};
use graftyard_types::{AccountId, ChainId, CredentialShard, Region, Timestamp, TokenShard};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use smallvec::{SmallVec, smallvec};

use crate::TOKENS_FORMAT;
use crate::credential::{CredentialRefusal, IdentityDep, Own, check_credential, credential_chains, encode_cred};
use crate::enc;
use crate::overlay::{Overlay, Table};

/// 1,024 shards per home family.
pub const SHARDS_PER_FAMILY: u16 = 1024;
const FAMILIES: u16 = 3;
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
        if Family::of(token_shard(&secret)) == family {
            return Ok((secret, draw));
        }
    }
    Err(RedrawExhausted { family, draws: MAX_DRAWS })
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

/// `Stored` and `Revoked`. The design's `Stored` also carries `sso_orgs`, `app`, `client`, `permissions`,
/// `resource_owner`, `repos`, `paired`, `grant`, and `write`, which belong to OAuth, Apps, and fine-grained PATs
/// (Milestone 3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokensEvent {
    Stored { hash: [u8; 32], kind: TokenKind, account: AccountId, scopes: ScopeSet, expires: Option<Timestamp> },
    Revoked { hash: [u8; 32] },
}

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
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokensFold {
    /// The chain's own shard. `None` checks no key placement.
    pub shard: Option<TokenShard>,
    pub tokens: BTreeMap<[u8; 32], TokenRow>,
}

impl TokensFold {
    pub fn new(shard: TokenShard) -> TokensFold {
        TokensFold { shard: Some(shard), ..TokensFold::default() }
    }
}

pub trait TokensRead {
    fn token(&self, hash: &[u8; 32]) -> Option<TokenRow>;
}

impl TokensRead for TokensFold {
    fn token(&self, hash: &[u8; 32]) -> Option<TokenRow> {
        self.tokens.get(hash).copied()
    }
}

fn write(ev: &TokensEvent, t: &mut impl Table<[u8; 32], TokenRow>) {
    match ev {
        TokensEvent::Stored { hash, kind, account, scopes, expires } => t.put(
            *hash,
            TokenRow { kind: *kind, account: *account, scopes: *scopes, expires: *expires, revoked: false },
        ),
        TokensEvent::Revoked { hash } => {
            if let Some(r) = t.get(hash).copied() {
                t.put(*hash, TokenRow { revoked: true, ..r });
            }
        }
    }
}

pub struct TokensScratch<'a> {
    fold: &'a TokensFold,
    tokens: Overlay<'a, [u8; 32], TokenRow>,
}

impl TokensRead for TokensScratch<'_> {
    fn token(&self, hash: &[u8; 32]) -> Option<TokenRow> {
        self.tokens.get(hash).copied()
    }
}

impl Scratch for TokensScratch<'_> {
    type Event = TokensEvent;
    fn apply(&mut self, ev: &TokensEvent) {
        write(ev, &mut self.tokens);
    }
    /// A hash is stored once, ever, and only on its own shard.
    fn check(&self, ev: &TokensEvent) -> Result<(), Box<str>> {
        match ev {
            TokensEvent::Stored { hash, .. } if self.token(hash).is_some() => Err("hash stored twice".into()),
            TokensEvent::Stored { hash, .. } if self.fold.shard.is_some_and(|s| s != shard_of_hash(hash)) => {
                Err("hash stored on another shard".into())
            }
            _ => Ok(()),
        }
    }
}

impl Fold for TokensFold {
    type Event = TokensEvent;
    type Scratch<'a> = TokensScratch<'a>;
    fn scratch(&self) -> TokensScratch<'_> {
        TokensScratch { fold: self, tokens: Overlay::new(&self.tokens) }
    }
    fn apply(&mut self, ev: &TokensEvent) {
        write(ev, &mut self.tokens);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokensCmd {
    pub cred: CredentialShard,
    pub op: TokensOp,
}

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
}

impl Record for TokensCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut e = Enc(out);
        encode_cred(&mut e, &self.cred);
        match &self.op {
            TokensOp::Store { hash, kind, account, scopes, expires } => {
                e.u8(0);
                e.0.extend_from_slice(hash);
                e.u8(*kind as u8);
                enc::account(&mut e, *account);
                e.u64(scopes.0);
                e.option(expires.as_ref(), |e, t| enc::at(e, *t));
            }
            TokensOp::Revoke { hash } => {
                e.u8(1);
                e.0.extend_from_slice(hash);
            }
        }
    }
}

impl TokensCmd {
    fn hash(&self) -> &[u8; 32] {
        match &self.op {
            TokensOp::Store { hash, .. } | TokensOp::Revoke { hash } => hash,
        }
    }

    /// A user acts only for itself; a command with no account (`graft-ops`, the CLI) acts for any.
    fn acts_for(&self, account: AccountId) -> bool {
        self.cred.account.is_none_or(|a| a == account)
    }
}

impl Command for TokensCmd {
    type Fold = TokensFold;
    type Dep = IdentityDep;
    type Out = ();
    type Err = TokensErr;

    fn format(&self) -> Format {
        TOKENS_FORMAT
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
        let want = shard_of_hash(self.hash());
        if s.fold.shard.is_some_and(|have| have != want) {
            return Err(TokensErr::WrongShard { want });
        }
        match &self.op {
            TokensOp::Store { hash, kind, account, scopes, expires } => {
                if !self.acts_for(*account) {
                    return Err(TokensErr::Forbidden);
                }
                if *kind == TokenKind::Installation {
                    return Err(TokensErr::NotStored);
                }
                if s.token(hash).is_some() {
                    return Err(TokensErr::Exists);
                }
                let ev = TokensEvent::Stored {
                    hash: *hash,
                    kind: *kind,
                    account: *account,
                    scopes: *scopes,
                    expires: *expires,
                };
                Ok((smallvec![ev], ()))
            }
            TokensOp::Revoke { hash } => {
                let row = s.token(hash).ok_or(TokensErr::NoToken)?;
                if !self.acts_for(row.account) {
                    return Err(TokensErr::Forbidden);
                }
                if row.revoked {
                    return Err(TokensErr::AlreadyRevoked);
                }
                Ok((smallvec![TokensEvent::Revoked { hash: *hash }], ()))
            }
        }
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
