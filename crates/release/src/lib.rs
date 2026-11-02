//! The `Release` group's signed entries and genesis acceptance (`design/DESIGN.md`, Shape, "Release group";
//! "Install operations", Genesis and Keys). Pure Rust with no clock, randomness, or Workers dependency, so the member
//! host runs the same acceptance on wasm32 that `graftyard sign` and `graftyard install` run on the operator's machine.
//!
//! A signed entry is stored, sent to `POST /api/graftyard/v1/release/entries`, and signed in one canonical form:
//!
//! ```text
//! b"graftyard/release/1" | Bound | ReleaseEntry | count:u32 | (key:[u8;32], sig:[u8;64])*
//! ```
//!
//! Each signature covers every byte before the count, so it covers the domain tag, `Bound`, and the body. The entry's
//! digest, which the next entry's `Bound.prev` names, is SHA-256 of the whole stored form, signatures included.

mod codec;
mod types;

use ed25519_dalek::{SigningKey, VerifyingKey};
use graftyard_types::{ChainId, Digest, MemberKind, Placement, PlacementName, SiteId, SystemChain};
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

pub use codec::Malformed;
pub use types::{
    Bound, Genesis, InstallId, IntentId, KeyId, Keys, ReleaseEntry, RestoreFrom, RestoreSource, Signature,
};

use codec::{Dec, Enc};

/// The domain tag every stored and signed `Release` entry starts with.
pub const DOMAIN: &[u8] = b"graftyard/release/1";

/// Most signatures one entry carries: `Keys` takes the current customer key and the new one.
const MAX_SIGS: usize = 4;

/// One `Release` entry as stored: `Bound`, body, and signatures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedEntry {
    pub bound: Bound,
    pub body: ReleaseEntry,
    pub sigs: SmallVec<[(KeyId, Signature); 2]>,
}

impl SignedEntry {
    /// An entry with no signatures yet: what `graftyard genesis` writes.
    pub fn unsigned(bound: Bound, body: ReleaseEntry) -> SignedEntry {
        SignedEntry { bound, body, sigs: SmallVec::new() }
    }

    /// The bytes every signature covers: the domain tag, `Bound`, and the body.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        let mut e = Enc(&mut out);
        e.raw(DOMAIN);
        types::bound(&mut e, &self.bound);
        types::entry(&mut e, &self.body);
        out
    }

    /// The stored form.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.signed_bytes();
        let mut e = Enc(&mut out);
        e.count(self.sigs.len());
        for (k, s) in &self.sigs {
            e.raw(&k.0);
            e.raw(&s.0);
        }
        out
    }

    /// Strict: the bytes must be exactly one entry's canonical form.
    pub fn decode(bytes: &[u8]) -> Result<SignedEntry, Malformed> {
        let mut d = Dec::new(bytes);
        if d.take(DOMAIN.len()).ok() != Some(DOMAIN) {
            return Err(Malformed("not a graftyard/release/1 entry".into()));
        }
        let bound = d.bound()?;
        let body = d.entry()?;
        let n = d.count(MAX_SIGS)?;
        let sigs = (0..n).map(|_| Ok((KeyId(d.array()?), Signature(d.array()?)))).collect::<Result<_, Malformed>>()?;
        d.finish()?;
        Ok(SignedEntry { bound, body, sigs })
    }

    /// SHA-256 of the stored form: what the next entry's `Bound.prev` names, and what 201 returns.
    pub fn digest(&self) -> Digest {
        Digest(Sha256::digest(self.encode()).into())
    }

    /// Adds the signature of the key whose 32-byte Ed25519 seed is `seed`, replacing an earlier one by the same key.
    pub fn sign(&mut self, seed: &[u8; 32]) -> KeyId {
        let sk = SigningKey::from_bytes(seed);
        let key = KeyId(sk.verifying_key().to_bytes());
        let sig = Signature(ed25519_dalek::Signer::sign(&sk, &self.signed_bytes()).to_bytes());
        self.sigs.retain(|(k, _)| *k != key);
        self.sigs.push((key, sig));
        key
    }

    /// Whether `key` signed this entry and the signature verifies (strict RFC 8032 verification, which refuses
    /// non-canonical and small-order encodings).
    pub fn verify(&self, key: &KeyId) -> Result<(), Refusal> {
        let Some((_, sig)) = self.sigs.iter().find(|(k, _)| k == key) else { return Err(Refusal::WrongKey) };
        let vk = VerifyingKey::from_bytes(&key.0).map_err(|_| Refusal::BadSignature)?;
        vk.verify_strict(&self.signed_bytes(), &ed25519_dalek::Signature::from_bytes(&sig.0))
            .map_err(|_| Refusal::BadSignature)
    }
}

/// The public half of an Ed25519 seed.
pub fn public_key(seed: &[u8; 32]) -> KeyId {
    KeyId(SigningKey::from_bytes(seed).verifying_key().to_bytes())
}

/// The trust anchor: the `GRAFTYARD_INSTALL` and `GRAFTYARD_CUSTOMER_KEY` variables that Wrangler set on every script
/// it created. It never changes after install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub install: InstallId,
    pub customer: KeyId,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnchorError {
    #[error("GRAFTYARD_INSTALL {0:?} is not a ULID")]
    Install(String),
    #[error("GRAFTYARD_CUSTOMER_KEY {0:?} is not 64 lower-case hex characters of an Ed25519 public key")]
    Key(String),
}

impl Anchor {
    /// Parses the two variables as a script's version holds them: the install id's ULID text, and the customer
    /// public key as 64 lower-case hex characters.
    pub fn from_vars(install: &str, customer_key: &str) -> Result<Anchor, AnchorError> {
        let install = install.parse().map_err(|_| AnchorError::Install(install.into()))?;
        let customer = KeyId::parse_hex(customer_key)
            .filter(|k| VerifyingKey::from_bytes(&k.0).is_ok())
            .ok_or_else(|| AnchorError::Key(customer_key.into()))?;
        Ok(Anchor { install, customer })
    }
}

/// Why the `Release` group refuses an entry. `variant()` is the `refusal` field of the endpoint's 422.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("malformed entry: {0}")]
    Malformed(Malformed),
    /// The empty group takes a genesis first, and only a genesis.
    #[error("the empty Release group accepts only a Genesis, got {0}")]
    NotGenesis(&'static str),
    #[error("a genesis already committed (digest {0:?}); the group accepts no second one")]
    AlreadyInstalled(Digest),
    #[error("bound to group {0}, not release")]
    WrongGroup(ChainId),
    #[error("bound to install {got}, this install is {want}")]
    WrongInstall { want: InstallId, got: InstallId },
    /// A genesis binds no `prev` and no `parent`, and its body names the install its `Bound` names.
    #[error("genesis bound wrongly: {0}")]
    BadBound(&'static str),
    #[error("not signed by the anchor's customer key, or signed by another key as well")]
    WrongKey,
    #[error("the customer key's signature does not verify")]
    BadSignature,
    #[error("genesis Keys name customer key {got:?}, the anchor is {want:?}")]
    KeysMismatch { want: KeyId, got: KeyId },
    #[error("genesis Keys are at generation {0}, not 0")]
    KeysGeneration(u64),
    #[error("genesis refused: {0}")]
    BadGenesis(Box<str>),
}

impl Refusal {
    pub fn variant(&self) -> &'static str {
        match self {
            Refusal::Malformed(_) => "Malformed",
            Refusal::NotGenesis(_) => "NotGenesis",
            Refusal::AlreadyInstalled(_) => "AlreadyInstalled",
            Refusal::WrongGroup(_) => "WrongGroup",
            Refusal::WrongInstall { .. } => "WrongInstall",
            Refusal::BadBound(_) => "BadBound",
            Refusal::WrongKey => "WrongKey",
            Refusal::BadSignature => "BadSignature",
            Refusal::KeysMismatch { .. } => "KeysMismatch",
            Refusal::KeysGeneration(_) => "KeysGeneration",
            Refusal::BadGenesis(_) => "BadGenesis",
        }
    }
}

fn bad_genesis(why: impl Into<String>) -> Refusal {
    Refusal::BadGenesis(why.into().into_boxed_str())
}

/// Whether `bytes` are a genesis this install accepts, judged against the anchor alone. Pure: the group's `decide`
/// adds only the "no genesis yet" rule (`ReleaseFold::decide`).
pub fn accept_genesis(anchor: &Anchor, bytes: &[u8]) -> Result<Genesis, Refusal> {
    let entry = SignedEntry::decode(bytes).map_err(Refusal::Malformed)?;
    check_genesis(anchor, &entry)?;
    match entry.body {
        ReleaseEntry::Genesis(g) => Ok(*g),
        ReleaseEntry::Keys(_) => unreachable!("check_genesis refused it"),
    }
}

/// The checks of `accept_genesis` on a decoded entry. The order is the order a caller can fix them in: what the
/// entry is, where it is bound, who signed it, then what it says.
pub fn check_genesis(anchor: &Anchor, entry: &SignedEntry) -> Result<(), Refusal> {
    let ReleaseEntry::Genesis(g) = &entry.body else { return Err(Refusal::NotGenesis(entry.body.kind())) };
    let b = &entry.bound;
    if b.group != ChainId::Release {
        return Err(Refusal::WrongGroup(b.group));
    }
    if b.install != anchor.install {
        return Err(Refusal::WrongInstall { want: anchor.install, got: b.install });
    }
    if g.install != b.install {
        return Err(Refusal::WrongInstall { want: anchor.install, got: g.install });
    }
    if b.prev.is_some() {
        return Err(Refusal::BadBound("a genesis binds no prev"));
    }
    if b.parent.is_some() {
        return Err(Refusal::BadBound("a genesis binds no parent intent"));
    }
    // Exactly one signature, by the anchor's key: an extra signer would be a key nobody registered.
    if entry.sigs.len() != 1 {
        return Err(Refusal::WrongKey);
    }
    entry.verify(&anchor.customer)?;
    if g.keys.customer != anchor.customer {
        return Err(Refusal::KeysMismatch { want: anchor.customer, got: g.keys.customer });
    }
    if g.keys.generation != 0 {
        return Err(Refusal::KeysGeneration(g.keys.generation));
    }
    check_body(g)
}

/// What a genesis must say for the groups it places to come up: distinct keys, a sane site table, and one record per
/// system chain that `Placement::from_record` builds, on active sites in distinct regions, `Solo` only under the
/// `evaluation` flag.
fn check_body(g: &Genesis) -> Result<(), Refusal> {
    let k = &g.keys;
    let mut all: Vec<KeyId> = [k.customer, k.release, k.drain, k.executor].into();
    all.extend(k.vendor);
    all.extend(k.agents_executor);
    all.extend(k.ring.iter().map(|(_, k)| *k));
    for (i, a) in all.iter().enumerate() {
        if VerifyingKey::from_bytes(&a.0).is_err() {
            return Err(bad_genesis(format!("key {a:?} is not an Ed25519 public key")));
        }
        if all[i + 1..].contains(a) {
            return Err(bad_genesis(format!("key {a:?} holds two roles")));
        }
    }
    if g.restore.is_some() {
        return Err(bad_genesis("a restore genesis is not built (Milestone 7)"));
    }
    for (i, s) in g.sites.iter().enumerate() {
        if s.id.0 >= 64 {
            return Err(bad_genesis(format!("site id {} is not below 64", s.id.0)));
        }
        if g.sites[i + 1..].iter().any(|t| t.id == s.id || t.script == s.script || t.bucket == s.bucket) {
            return Err(bad_genesis(format!("site {:?} ({}) is listed twice", s.id, s.script)));
        }
    }
    if !g.sites.iter().any(|s| !s.spare) {
        return Err(bad_genesis("no active site"));
    }
    for c in [SystemChain::Release, SystemChain::Names, SystemChain::Tokens, SystemChain::Ids] {
        let n = g.placements.iter().filter(|(p, _)| *p == c).count();
        if n != 1 {
            return Err(bad_genesis(format!("{c:?} has {n} placement records, not 1")));
        }
    }
    for (c, r) in &g.placements {
        if r.moving_from.is_some() {
            return Err(bad_genesis(format!("{c:?}'s record is moving")));
        }
        if r.name == PlacementName::Solo && !g.evaluation {
            return Err(bad_genesis(format!("{c:?} is Solo without the evaluation flag")));
        }
        // The chain only names the members; every `Names` and `Tokens` shard takes the same record.
        let p = Placement::from_record(ChainId::Release, r).map_err(|e| bad_genesis(format!("{c:?}'s record: {e}")))?;
        let site = |id: SiteId| g.sites.iter().find(|s| s.id == id);
        for (i, m) in p.voters().iter().enumerate() {
            let s = site(m.site).ok_or_else(|| bad_genesis(format!("{c:?} names unknown site {:?}", m.site)))?;
            if s.spare {
                return Err(bad_genesis(format!("{c:?} places a voter on spare site {:?}", m.site)));
            }
            for n in &p.voters()[i + 1..] {
                if site(n.site).is_some_and(|t| t.region == s.region) {
                    return Err(bad_genesis(format!("{c:?} puts {:?} and {:?} in one region", m.site, n.site)));
                }
            }
        }
        if r.name != PlacementName::Solo && p.voters().iter().filter(|m| m.kind == MemberKind::Full).count() < 2 {
            return Err(bad_genesis(format!("{c:?} has fewer than two Full voters")));
        }
    }
    Ok(())
}

/// What the `Release` group's fold holds for genesis acceptance: the accepted genesis, the latest committed `Keys`,
/// and the digest the next signed entry must bind as `prev`. `Noop` and `Config` entries never reach it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseFold {
    pub genesis: Option<Genesis>,
    pub keys: Option<Keys>,
    pub last: Option<Digest>,
}

/// The one event an accepted entry commits: the entry's stored bytes, so every member applies and digests the same
/// bytes. `Record::encode` for it is `bytes` itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub bytes: Box<[u8]>,
}

impl Accepted {
    pub fn entry(&self) -> SignedEntry {
        SignedEntry::decode(&self.bytes).expect("an accepted entry decodes")
    }

    pub fn digest(&self) -> Digest {
        Digest(Sha256::digest(&self.bytes).into())
    }
}

impl ReleaseFold {
    /// `decide` for a submitted entry. Pure: the fold and the anchor from the script's own version. Only a genesis is
    /// built, so an installed group refuses every entry: a second genesis, or a replay of the first, with
    /// `AlreadyInstalled`, and anything else with `NotGenesis` until Milestone 5 builds it.
    pub fn decide(&self, anchor: &Anchor, bytes: &[u8]) -> Result<Accepted, Refusal> {
        let entry = SignedEntry::decode(bytes).map_err(Refusal::Malformed)?;
        if let Some(d) = self.last {
            return Err(match entry.body {
                ReleaseEntry::Genesis(_) => Refusal::AlreadyInstalled(d),
                ref other => Refusal::NotGenesis(other.kind()),
            });
        }
        check_genesis(anchor, &entry)?;
        // Re-encode, so the stored bytes are canonical whatever the edge received; decode is strict, so they match.
        Ok(Accepted { bytes: entry.encode().into_boxed_slice() })
    }

    /// A follower's check of the event before it applies it: the same rules, on the fold it holds.
    pub fn check(&self, anchor: &Anchor, ev: &Accepted) -> Result<(), Refusal> {
        self.decide(anchor, &ev.bytes).map(|_| ())
    }

    pub fn apply(&mut self, ev: &Accepted) {
        let entry = ev.entry();
        if let ReleaseEntry::Genesis(g) = entry.body {
            self.keys = Some(g.keys.clone());
            self.genesis = Some(*g);
        }
        self.last = Some(ev.digest());
    }
}

#[cfg(test)]
mod tests;
