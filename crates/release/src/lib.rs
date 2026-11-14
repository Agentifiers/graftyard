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
mod group;
mod types;
pub mod view;

use ed25519_dalek::{SigningKey, VerifyingKey};
use graftyard_types::{
    ChainId, Digest, MemberKind, Placement, PlacementName, ReleaseId, SiteId, SystemChain, Timestamp,
};
use group::hex32;
use sha2::{Digest as _, Sha256};
use smallvec::SmallVec;

pub use codec::Malformed;
pub use group::{
    Accepted, Current, Drained, Pointers, ReleaseFold, STALE_MAX, Upload, ZERO_ACKS, formats_enable_message,
    presign_undo, roll_steps, undo_entry,
};
pub use types::{
    Bound, DayTarget, Format, FormatSet, Genesis, ImageRef, InstallId, Intent, IntentId, IntentState, KeyId, Keys,
    LeaveEvaluation, Mode, ReleaseEntry, ReleaseUploaded, RestoreFrom, RestoreSource, Scope, Script, SecretName,
    Signature, SignedIntent, SiteMode, VersionId,
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
    /// A genesis binds no `prev` and no `parent`; an undo binds a parent and no `prev`; nothing else binds a parent.
    #[error("bound wrongly: {0}")]
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
    /// 409: `prev` is not the digest of the last committed signed entry; the signer re-signs over this one.
    #[error("stale prev: the last committed entry is {0:?}")]
    StalePrev(Digest),
    #[error("signed by key {0:?}, which the latest Keys does not hold")]
    UnknownKey(KeyId),
    #[error("{0} is not signed by the key or keys its kind takes")]
    WrongSigner(&'static str),
    #[error("Keys generation {got}, not the latest plus one ({want})")]
    Generation { want: u64, got: u64 },
    #[error("the intent expired at {not_after:?}")]
    Expired { not_after: Timestamp },
    #[error("release {} has no ReleaseUploaded", hex32(&.0.0))]
    NoUpload(ReleaseId),
    #[error("release {} cannot read enabled format {format}", hex32(&release.0))]
    UnreadableFormat { release: ReleaseId, format: Format },
    #[error("intent {0:?} is current; only a ReplaceMember commits over it")]
    IntentCurrent(IntentId),
    #[error("intent {0:?} is not the current intent")]
    NotCurrent(IntentId),
    #[error("a {0} commits only as a pre-signed undo of the current intent, byte for byte")]
    NotAnUndo(&'static str),
    #[error("this {0} undo has already committed")]
    UndoUsed(&'static str),
    #[error("the drain key cannot drain a second site while site {} is drained", .0.0)]
    DrainGuard(SiteId),
    #[error("an EdgeRoll that drops version {} an undo still names", hex32(&.0.0))]
    DropsUndoVersion(ReleaseId),
    #[error("site {} is neither SiteLost nor drained ZERO_ACKS", .0.0)]
    NotLost(SiteId),
    #[error("IntentStep {got}, the next step is {want}")]
    WrongStep { want: u8, got: u8 },
    #[error("a {kind} has {steps} forward steps (ROLL_STEPS)")]
    PastLastStep { kind: &'static str, steps: u8 },
    #[error("{0}")]
    BadState(&'static str),
    #[error("bad release record: {0}")]
    BadRecord(Box<str>),
    #[error("{0}")]
    Rejected(Box<str>),
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
            Refusal::StalePrev(_) => "StalePrev",
            Refusal::UnknownKey(_) => "UnknownKey",
            Refusal::WrongSigner(_) => "WrongSigner",
            Refusal::Generation { .. } => "Generation",
            Refusal::Expired { .. } => "Expired",
            Refusal::NoUpload(_) => "NoUpload",
            Refusal::UnreadableFormat { .. } => "UnreadableFormat",
            Refusal::IntentCurrent(_) => "IntentCurrent",
            Refusal::NotCurrent(_) => "NotCurrent",
            Refusal::NotAnUndo(_) => "NotAnUndo",
            Refusal::UndoUsed(_) => "UndoUsed",
            Refusal::DrainGuard(_) => "DrainGuard",
            Refusal::DropsUndoVersion(_) => "DropsUndoVersion",
            Refusal::NotLost(_) => "NotLost",
            Refusal::WrongStep { .. } => "WrongStep",
            Refusal::PastLastStep { .. } => "PastLastStep",
            Refusal::BadState(_) => "BadState",
            Refusal::BadRecord(_) => "BadRecord",
            Refusal::Rejected(_) => "Rejected",
        }
    }

    /// The endpoint's status: 409 for a stale `prev` (a second genesis included), 422 for every other refusal.
    pub fn status(&self) -> u16 {
        match self {
            Refusal::StalePrev(_) | Refusal::AlreadyInstalled(_) => 409,
            _ => 422,
        }
    }

    /// The digest a 409 names: the last committed entry, or the held genesis for a second one.
    pub fn current(&self) -> Option<Digest> {
        match self {
            Refusal::StalePrev(d) | Refusal::AlreadyInstalled(d) => Some(*d),
            _ => None,
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
        _ => unreachable!("check_genesis refused it"),
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

/// Every key is an Ed25519 public key and holds one role, so a signature names its role.
pub(crate) fn check_keys(k: &Keys) -> Result<(), String> {
    let mut all: Vec<KeyId> = [k.customer, k.release, k.drain, k.executor].into();
    all.extend(k.vendor);
    all.extend(k.agents_executor);
    all.extend(k.ring.iter().map(|(_, k)| *k));
    for (i, a) in all.iter().enumerate() {
        if VerifyingKey::from_bytes(&a.0).is_err() {
            return Err(format!("key {a:?} is not an Ed25519 public key"));
        }
        if all[i + 1..].contains(a) {
            return Err(format!("key {a:?} holds two roles"));
        }
    }
    Ok(())
}

/// What a genesis must say for the groups it places to come up: distinct keys, a sane site table, and one record per
/// system chain that `Placement::from_record` builds, on active sites in distinct regions, `Solo` only under the
/// `evaluation` flag.
fn check_body(g: &Genesis) -> Result<(), Refusal> {
    check_keys(&g.keys).map_err(bad_genesis)?;
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

#[cfg(test)]
mod tests;
